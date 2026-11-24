/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The Calendar class ([MS-ASCAL]) and VEVENT (RFC 5545).
//!
//! [`to_ical`] writes what a device sent into the event's iCalendar
//! object, [`from_ical`] makes the `ApplicationData` a device gets, and
//! [`change_instance`] and [`delete_instance`] carry out 16.1's changes to
//! one instance of a series (a `Change` or `Delete` with
//! `airsyncbase:InstanceId`). The shared properties are in [`event`],
//! attendees in [`attendees`], exceptions in [`exceptions`]; the
//! recurrence is [`crate::items::recurrence`] and the time zone
//! [`crate::items::timezone`].
//!
//! 14.1 and 16.1 differ here ([MS-ASCAL] 2.2.2): 16.1 devices send no
//! `UID` (the server makes one; `ClientUid` is kept as
//! `X-ACTIVESYNC-CLIENTUID` to recognise a repeated `Add`), no organizer
//! (the account is), no `AttendeeStatus` and no `Exceptions`; their
//! `Location` is `airsyncbase:Location`; their all-day events have dates
//! and no `Timezone`; and changing a series' times or pattern removes its
//! exceptions.

mod attendees;
mod event;
mod exceptions;

pub(crate) use attendees::{address_of, line_address, organizer, set_param};
pub(crate) use event::{ALT_DESC, Times, View, as_sent, set_alt_desc, set_class};

use super::{
    Context, ItemError, Omitted, Result,
    dates::{self, IcalTime},
    element::{Field, flag, push_number, trimmed},
    ical::{self, Calendar},
    recurrence::{self, End, Pattern},
    timezone::{Zone, finish_timezones},
};
use crate::wbxml::{
    Element,
    codepages::{airsync, airsyncbase, calendar},
};
use calcard::icalendar::{
    ICalendarComponent, ICalendarComponentType, ICalendarEntry, ICalendarProperty, ICalendarValue,
};
use chrono::{Duration, NaiveDateTime, Timelike};

/// Where a 16.1 `ClientUid` is kept ([MS-ASCAL] 2.2.2.13).
pub const CLIENT_UID: &str = "X-ACTIVESYNC-CLIENTUID";
/// Outlook's "attendees may not propose a new time" flag
/// ([MS-ASCAL] 2.2.2.17).
const DISALLOW_COUNTER: &str = "X-MICROSOFT-DISALLOW-COUNTER";

/// Writes a Calendar class `ApplicationData` from a device into the
/// event's iCalendar text (`existing`, or a new object for an `Add`) and
/// returns the new text. `omitted` says what elements left out of a
/// `Change` mean.
pub fn to_ical(
    data: &Element,
    existing: Option<&str>,
    omitted: Omitted<'_>,
    ctx: &Context<'_>,
) -> Result<String> {
    let mut calendar = match existing {
        Some(text) => Calendar::parse(text)?,
        None => Calendar::new(),
    };
    let master = match calendar.master(&ICalendarComponentType::VEvent) {
        Some(id) => id,
        None if existing.is_some() => return Err(ItemError::Unreadable("no VEVENT")),
        None => calendar.add(0, ICalendarComponent::new(ICalendarComponentType::VEvent)),
    };
    let is_new = existing.is_none();
    let old = (!is_new)
        .then(|| Times::read(&calendar, master, ctx))
        .transpose()?;
    let old_rule = rrule_entry(calendar.component(master)).cloned();

    let times = new_times(&calendar, master, data, omitted, old.as_ref(), ctx)?;
    let mut moved = event::set_time(
        &mut calendar,
        master,
        ICalendarProperty::Dtstart,
        &times,
        times.start,
        ctx,
    );
    moved |= event::set_time(
        &mut calendar,
        master,
        ICalendarProperty::Dtend,
        &times,
        times.end,
        ctx,
    );

    write_identity(&mut calendar, master, data, is_new, ctx)?;
    event::apply(&mut calendar, master, data, omitted, ctx)?;
    write_organizer(calendar.component_mut(master), data, omitted, ctx)?;
    match Field::of(data, calendar::DISALLOW_NEW_TIME_PROPOSAL, omitted) {
        Field::Set(element) => {
            let disallow = flag(element)?;
            let event = calendar.component_mut(master);
            if disallow {
                ical::replace(
                    event,
                    ical::text_entry(ical::x_name(DISALLOW_COUNTER), "TRUE"),
                );
            } else {
                ical::remove(event, &ical::x_name(DISALLOW_COUNTER));
            }
        }
        Field::Clear => ical::remove(
            calendar.component_mut(master),
            &ical::x_name(DISALLOW_COUNTER),
        ),
        Field::Keep => {}
    }

    write_recurrence(&mut calendar, master, data, omitted, &times, ctx)?;
    let rule_changed = rrule_entry(calendar.component(master)) != old_rule.as_ref();
    if !is_new && ctx.version.is_16() && (moved || rule_changed) && old_rule.is_some() {
        exceptions::clear(&mut calendar, master);
    }
    // Exceptions left out stay, whatever `Supported` says ([MS-ASCAL]
    // 3.2.5.3.1); 16.1 devices change instances one by one instead.
    if !ctx.version.is_16()
        && let Some(list) = data.child(calendar::EXCEPTIONS)
    {
        exceptions::apply(&mut calendar, master, list, ctx)?;
    }

    let master = calendar
        .master(&ICalendarComponentType::VEvent)
        .ok_or(ItemError::Unreadable("no VEVENT"))?;
    stamp(
        &mut calendar,
        master,
        data,
        !is_new && (moved || rule_changed),
        ctx,
    )?;
    finish_timezones(&mut calendar);
    Ok(calendar.write())
}

/// The event's zone, all-day flag, start and end after this request.
fn new_times(
    calendar: &Calendar,
    master: u32,
    data: &Element,
    omitted: Omitted<'_>,
    old: Option<&Times>,
    ctx: &Context<'_>,
) -> Result<Times> {
    let all_day = match Field::of(data, calendar::ALL_DAY_EVENT, omitted) {
        Field::Set(element) => flag(element)?,
        Field::Clear => false,
        Field::Keep => old.is_some_and(|old| old.all_day),
    };
    let start = event::sent_time(data, calendar::START_TIME)?;
    let end = event::sent_time(data, calendar::END_TIME)?;
    let (start, end) = match (start, end, old) {
        (Some(start), Some(end), _) => (start, end.max(start)),
        (Some(start), None, Some(old)) => (start, start + (old.end - old.start)),
        (None, Some(end), Some(old)) => (old.start, end.max(old.start)),
        (None, None, Some(old)) => (old.start, old.end),
        (start, end, None) => default_times(start, end, all_day, ctx)?,
    };
    let zone = match (
        all_day && ctx.version.is_16(),
        Field::of(data, calendar::TIMEZONE, omitted),
    ) {
        (false, Field::Set(element)) => event::zone_of_blob(element, start)?,
        (_, _) => match old {
            Some(old) => old.zone.clone(),
            None => {
                let (time, tzid) =
                    ical::time(calendar.component(master), &ICalendarProperty::Dtstart)
                        .unwrap_or((IcalTime::Utc(start), None));
                Zone::of_time(calendar, time, tzid, ctx.zone)
            }
        },
    };
    Ok(Times {
        zone,
        all_day,
        start,
        end,
    })
}

/// [MS-ASCAL] 3.2.4.4: the times of an event added without a start or an
/// end. "Now" is rounded up to the next half hour; an event lasts half an
/// hour.
fn default_times(
    start: Option<NaiveDateTime>,
    end: Option<NaiveDateTime>,
    all_day: bool,
    ctx: &Context<'_>,
) -> Result<(NaiveDateTime, NaiveDateTime)> {
    let now = ctx.now.naive_utc();
    let past_half_hour = i64::from(now.minute() % 30) * 60 + i64::from(now.second());
    let rounded = if past_half_hour == 0 {
        now
    } else {
        now + Duration::seconds(30 * 60 - past_half_hour)
    };
    let half_hour = Duration::minutes(30);
    let too_early = || {
        ItemError::invalid(
            calendar::START_TIME,
            "missing, and the times given cannot be completed",
        )
    };
    match (start, end) {
        (Some(start), Some(end)) => Ok((start, end.max(start))),
        (None, None) => Ok((rounded, rounded + half_hour)),
        (None, Some(end)) if end > rounded => Ok((rounded, end)),
        (None, Some(_)) => Err(too_early()),
        (Some(start), None) if all_day => Ok((start, start + Duration::days(1))),
        (Some(start), None) if start <= now => Ok((start, rounded + half_hour)),
        (Some(_), None) => Err(ItemError::invalid(
            calendar::END_TIME,
            "missing for an event in the future",
        )),
    }
}

/// `UID` (14.1 sends one; a new event without gets a new one) and 16.1's
/// `ClientUid`. An existing event's `UID` never changes.
fn write_identity(
    calendar: &mut Calendar,
    master: u32,
    data: &Element,
    is_new: bool,
    ctx: &Context<'_>,
) -> Result<()> {
    let event = calendar.component_mut(master);
    if event.uid().is_none() || is_new {
        let sent = data
            .child(calendar::UID)
            .filter(|_| !ctx.version.is_16())
            .map(trimmed)
            .transpose()?
            .filter(|uid| !uid.is_empty() && uid.len() <= 300);
        let uid = match (sent, event.uid()) {
            (Some(uid), _) => uid.to_string(),
            (None, Some(uid)) => uid.to_string(),
            (None, None) => ical::new_uid(ctx.now),
        };
        ical::set_text(event, ICalendarProperty::Uid, Some(&uid));
    }
    if is_new
        && let Some(client_uid) = data
            .child(calendar::CLIENT_UID)
            .map(trimmed)
            .transpose()?
            .filter(|uid| !uid.is_empty() && uid.len() <= 300)
    {
        ical::replace(
            event,
            ical::text_entry(ical::x_name(CLIENT_UID), client_uid),
        );
    }
    Ok(())
}

/// The organizer: 14.1's `OrganizerEmail`/`OrganizerName`, or the account
/// for a meeting that has none (16.1 never sends one, [MS-ASCAL]
/// 2.2.2.35).
fn write_organizer(
    event: &mut ICalendarComponent,
    data: &Element,
    omitted: Omitted<'_>,
    ctx: &Context<'_>,
) -> Result<()> {
    if !ctx.version.is_16()
        && let Field::Set(element) = Field::of(data, calendar::ORGANIZER_EMAIL, omitted)
    {
        let email = address_of(trimmed(element)?);
        let name = data
            .child(calendar::ORGANIZER_NAME)
            .and_then(|name| name.as_str())
            .map(str::trim)
            .filter(|name| !name.is_empty());
        if !email.is_empty() {
            attendees::set_organizer(event, email, name);
        }
        return Ok(());
    }
    let has_attendees = event
        .entries
        .iter()
        .any(|entry| entry.name == ICalendarProperty::Attendee);
    if has_attendees
        && attendees::organizer(event).is_none()
        && let Some(me) = ctx.addresses.first()
    {
        attendees::set_organizer(event, me, ctx.name);
    }
    Ok(())
}

/// The first `RRULE` line.
fn rrule_entry(event: &ICalendarComponent) -> Option<&ICalendarEntry> {
    ical::first(event, &ICalendarProperty::Rrule)
}

/// The series' pattern, read from its `RRULE`.
pub(crate) fn stored_pattern(
    event: &ICalendarComponent,
    times: &Times,
    ctx: &Context<'_>,
) -> Option<Pattern> {
    let rule = match rrule_entry(event)?.values.first()? {
        ICalendarValue::RecurrenceRule(rule) => rule,
        _ => return None,
    };
    let until = rule
        .until
        .as_ref()
        .and_then(IcalTime::read)
        .map(|until| match until {
            // A date `UNTIL` is the day of the last instance: its start.
            IcalTime::Date(date) => {
                let local = date.and_time(times.zone.to_local(times.start).time());
                if times.all_day && ctx.version.is_16() {
                    date.and_time(Default::default())
                } else if times.all_day {
                    times.zone.utc_of(IcalTime::Date(date))
                } else {
                    times.zone.to_utc(local)
                }
            }
            time => times.zone.utc_of(time),
        });
    let start = match times.ical(times.start, ctx) {
        IcalTime::Date(date) => date,
        IcalTime::Local(time) | IcalTime::Utc(time) => time.date(),
    };
    Some(Pattern::from_rrule(rule, start, until, 999))
}

/// `RRULE` from `Recurrence`. A pattern equal to the stored rule's keeps
/// the rule as it is; an empty `Recurrence` (16.1) or one cleared ends the
/// series, with its exceptions.
fn write_recurrence(
    calendar: &mut Calendar,
    master: u32,
    data: &Element,
    omitted: Omitted<'_>,
    times: &Times,
    ctx: &Context<'_>,
) -> Result<()> {
    match Field::of(data, calendar::RECURRENCE, omitted) {
        Field::Set(element) => {
            let pattern = Pattern::read(element, &recurrence::CALENDAR)?;
            let stored = stored_pattern(calendar.component(master), times, ctx);
            if stored.is_some_and(|stored| stored.same_as(&pattern)) {
                return Ok(());
            }
            // RFC 5545 3.3.10: `UNTIL` is a date for all-day series and UTC
            // otherwise.
            let until = match pattern.end {
                End::Until(until) if times.all_day => {
                    Some(IcalTime::Date(match times.ical(until, ctx) {
                        IcalTime::Date(date) => date,
                        IcalTime::Local(time) | IcalTime::Utc(time) => time.date(),
                    }))
                }
                End::Until(until) => Some(IcalTime::Utc(until)),
                _ => None,
            }
            .map(IcalTime::write);
            let rule = pattern.rrule(until);
            ical::replace(
                calendar.component_mut(master),
                ICalendarEntry::new(ICalendarProperty::Rrule)
                    .with_value(ICalendarValue::RecurrenceRule(Box::new(rule))),
            );
        }
        Field::Clear => {
            if rrule_entry(calendar.component(master)).is_some() {
                let event = calendar.component_mut(master);
                ical::remove(event, &ICalendarProperty::Rrule);
                ical::remove(event, &ICalendarProperty::Rdate);
                exceptions::clear(calendar, master);
            }
        }
        Field::Keep => {}
    }
    Ok(())
}

/// `DTSTAMP` (the device's `DtStamp` in 14.1, else now), `LAST-MODIFIED`,
/// and `SEQUENCE` one up when the times or the pattern changed (RFC 5546
/// 2.1.4: attendees' copies are updated by sequence).
fn stamp(
    calendar: &mut Calendar,
    master: u32,
    data: &Element,
    rescheduled: bool,
    ctx: &Context<'_>,
) -> Result<()> {
    let now = ctx.now.naive_utc();
    let sent = data
        .child(calendar::DT_STAMP)
        .filter(|_| !ctx.version.is_16())
        .filter(|stamp| !stamp.as_bytes().is_empty())
        .map(dates::parse_element)
        .transpose()?;
    let event = calendar.component_mut(master);
    ical::replace(
        event,
        ical::time_entry(
            ICalendarProperty::Dtstamp,
            IcalTime::Utc(sent.unwrap_or(now)),
            None,
        ),
    );
    ical::replace(
        event,
        ical::time_entry(ICalendarProperty::LastModified, IcalTime::Utc(now), None),
    );
    if rescheduled {
        let sequence = ical::integer(event, &ICalendarProperty::Sequence).unwrap_or(0);
        ical::replace(
            event,
            ICalendarEntry::new(ICalendarProperty::Sequence)
                .with_value(ICalendarValue::Integer(sequence.saturating_add(1))),
        );
    }
    Ok(())
}

/// The Calendar class `ApplicationData` for an event's iCalendar text.
pub fn from_ical(text: &str, ctx: &Context<'_>) -> Result<Element> {
    let calendar = Calendar::parse(text)?;
    let master = calendar
        .master(&ICalendarComponentType::VEvent)
        .ok_or(ItemError::Unreadable("no VEVENT"))?;
    let view = View::read(&calendar, master, ctx)?;
    let event = calendar.component(master);
    let mut data = Element::new(airsync::APPLICATION_DATA);

    // 16.1 all-day events have no zone ([MS-ASCAL] 2.2.2.1).
    if !(view.times.all_day && ctx.version.is_16()) {
        let at = view
            .times
            .zone
            .utc_of(view.times.ical(view.times.start, ctx));
        data.push(Element::text(
            calendar::TIMEZONE,
            view.times.zone.rules(at).to_base64(),
        ));
    }
    view.push(None, event, ctx, &mut data);
    if let Some(uid) = event.uid() {
        data.push(Element::text(calendar::UID, uid));
    }
    attendees::organizer_elements(event, &mut data);
    if let Some(pattern) = stored_pattern(event, &view.times, ctx) {
        data.push(pattern.element(&recurrence::CALENDAR));
        if let Some(list) = exceptions::element(&calendar, master, &view, ctx)? {
            data.push(list);
        }
    }
    if !view.attendees.is_empty() {
        push_number(
            &mut data,
            calendar::RESPONSE_REQUESTED,
            u8::from(attendees::response_requested(event)),
        );
        push_number(
            &mut data,
            calendar::RESPONSE_TYPE,
            attendees::response_type(event, ctx),
        );
    }
    let disallow = ical::x_text(event, DISALLOW_COUNTER)
        .is_some_and(|value| value.eq_ignore_ascii_case("TRUE"));
    push_number(
        &mut data,
        calendar::DISALLOW_NEW_TIME_PROPOSAL,
        u8::from(disallow),
    );
    data.push(super::body::native_type(view.body.as_ref().map(
        |(text, html)| super::body::StoredBody {
            text,
            html: html.as_deref(),
        },
    )));
    Ok(data)
}

/// 16.1: changes one instance of a series (a `Change` with
/// `airsyncbase:InstanceId`, [MS-ASCMD] 2.2.3.24) and returns the new
/// text. What the request leaves out stays as the instance has it.
pub fn change_instance(
    existing: &str,
    instance_id: &str,
    data: &Element,
    ctx: &Context<'_>,
) -> Result<String> {
    let mut calendar = Calendar::parse(existing)?;
    let master = series(&calendar)?;
    let original = instance(instance_id)?;
    exceptions::change(&mut calendar, master, original, data, ctx)?;
    finish_timezones(&mut calendar);
    Ok(calendar.write())
}

/// 16.1: deletes one instance of a series (a `Delete` with
/// `airsyncbase:InstanceId`, [MS-ASCMD] 2.2.3.42.2).
pub fn delete_instance(existing: &str, instance_id: &str, ctx: &Context<'_>) -> Result<String> {
    let mut calendar = Calendar::parse(existing)?;
    let master = series(&calendar)?;
    exceptions::delete(&mut calendar, master, instance(instance_id)?, ctx)?;
    finish_timezones(&mut calendar);
    Ok(calendar.write())
}

/// The VEVENT one instance of a series is (its override, made from the
/// series when there is none), or the series' own without `instance`. An
/// invitation to one instance only has no series: its VEVENT is used.
pub(crate) fn event_for(
    calendar: &mut Calendar,
    instance_id: Option<&str>,
    ctx: &Context<'_>,
) -> Result<u32> {
    let master = calendar
        .master(&ICalendarComponentType::VEvent)
        .ok_or(ItemError::Unreadable("no VEVENT"))?;
    let Some(instance_id) = instance_id else {
        return Ok(master);
    };
    if rrule_entry(calendar.component(master)).is_none() {
        return Ok(master);
    }
    let original = instance(instance_id)?;
    exceptions::change(
        calendar,
        master,
        original,
        &Element::new(airsync::APPLICATION_DATA),
        ctx,
    )?;
    exceptions::find(calendar, original, ctx).ok_or(ItemError::Unreadable("no such instance"))
}

/// The VEVENT of a recurring series.
fn series(calendar: &Calendar) -> Result<u32> {
    calendar
        .master(&ICalendarComponentType::VEvent)
        .filter(|id| rrule_entry(calendar.component(*id)).is_some())
        .ok_or(ItemError::Unreadable("not a recurring event"))
}

/// An `InstanceId` value: Compact DateTime, or dateTime as in
/// MeetingResponse ([MS-ASCMD] 2.2.3.92.1).
fn instance(instance_id: &str) -> Result<NaiveDateTime> {
    dates::parse(instance_id).ok_or(ItemError::Invalid {
        element: airsyncbase::INSTANCE_ID.name(),
        reason: "not a date and time",
    })
}

/// The `UID` of an iCalendar object's first event, task or journal; the
/// DAV bridge names new resources by it.
pub fn uid(text: &str) -> Result<Option<String>> {
    let calendar = Calendar::parse(text)?;
    Ok(calendar
        .ical
        .components
        .iter()
        .find_map(|component| component.uid())
        .map(str::to_string))
}

/// The `ClientUid` a 16.1 device added the event with, to answer a
/// repeated `Add` with the event already made ([MS-ASCAL] 2.2.2.13).
pub fn client_uid(text: &str) -> Result<Option<String>> {
    let calendar = Calendar::parse(text)?;
    Ok(calendar
        .master(&ICalendarComponentType::VEvent)
        .and_then(|id| ical::x_text(calendar.component(id), CLIENT_UID))
        .map(str::to_string))
}

#[cfg(test)]
mod tests;
