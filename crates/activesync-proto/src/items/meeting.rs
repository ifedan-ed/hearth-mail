/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Meetings that arrive and leave as mail.
//!
//! - [`meeting_request`]: the Email class `MeetingRequest` element
//!   ([MS-ASEMAIL] 2.2.2.48) for an invitation (an iTIP REQUEST or CANCEL,
//!   RFC 5546) in a message, so the device shows Accept / Tentative /
//!   Decline; [`message_class`] is the message's `MessageClass`.
//! - [`reply`]: the iTIP REPLY a `MeetingResponse` ([MS-ASCMD] 2.2.1.11)
//!   sends the organizer: the same `UID`, `SEQUENCE` and `RECURRENCE-ID`,
//!   and the account's `ATTENDEE` with its new `PARTSTAT`.
//! - [`respond`]: the account's copy of the event with that answer, for
//!   the calendar.

use super::{
    Context, ItemError, Result,
    calendar::{self, View, line_address, organizer, set_param},
    dates::{self, IcalTime},
    element::push_number,
    ical::{self, Calendar},
    recurrence,
    timezone::finish_timezones,
};
use crate::wbxml::{
    Element,
    codepages::{airsyncbase, calendar as calendar_tags, email, email2},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use calcard::icalendar::{
    ICalendarComponent, ICalendarComponentType, ICalendarEntry, ICalendarMethod,
    ICalendarParameter, ICalendarParameterName, ICalendarParameterValue,
    ICalendarParticipationStatus, ICalendarProperty, ICalendarValue, Uri,
};
use chrono::{Datelike, NaiveDateTime};

/// An answer to an invitation: `UserResponse` ([MS-ASCMD] 2.2.3.194).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Response {
    Accepted,
    Tentative,
    Declined,
}

impl Response {
    /// The `UserResponse` value: 1, 2 or 3.
    pub fn from_user_response(value: u8) -> Option<Response> {
        match value {
            1 => Some(Response::Accepted),
            2 => Some(Response::Tentative),
            3 => Some(Response::Declined),
            _ => None,
        }
    }

    fn partstat(self) -> ICalendarParticipationStatus {
        match self {
            Response::Accepted => ICalendarParticipationStatus::Accepted,
            Response::Tentative => ICalendarParticipationStatus::Tentative,
            Response::Declined => ICalendarParticipationStatus::Declined,
        }
    }
}

/// The `MessageClass` of a message carrying this iTIP object
/// ([MS-ASEMAIL] 2.2.2.49): a request, a cancellation, or an answer.
pub fn message_class(itip: &str) -> Result<&'static str> {
    let calendar = Calendar::parse(itip)?;
    let method = ical::first(calendar.component(0), &ICalendarProperty::Method)
        .and_then(|entry| entry.values.first())
        .and_then(|value| match value {
            ICalendarValue::Method(method) => Some(method.clone()),
            _ => None,
        });
    Ok(match method {
        Some(ICalendarMethod::Cancel) => "IPM.Schedule.Meeting.Canceled",
        Some(ICalendarMethod::Reply) => {
            let event = calendar
                .master(&ICalendarComponentType::VEvent)
                .map(|id| calendar.component(id));
            let answer = event
                .and_then(|event| ical::first(event, &ICalendarProperty::Attendee))
                .map(calendar_status);
            match answer {
                Some(Some(ICalendarParticipationStatus::Accepted)) => {
                    "IPM.Schedule.Meeting.Resp.Pos"
                }
                Some(Some(ICalendarParticipationStatus::Declined)) => {
                    "IPM.Schedule.Meeting.Resp.Neg"
                }
                _ => "IPM.Schedule.Meeting.Resp.Tent",
            }
        }
        _ => "IPM.Schedule.Meeting.Request",
    })
}

fn calendar_status(entry: &ICalendarEntry) -> Option<ICalendarParticipationStatus> {
    match entry.parameter(&ICalendarParameterName::Partstat) {
        Some(ICalendarParameterValue::Partstat(status)) => Some(status.clone()),
        _ => None,
    }
}

/// The `email:MeetingRequest` element for an invitation, in the order of
/// [MS-ASEMAIL] 4.1.5's examples. Times are dateTime values; 16.1 gets
/// `calendar:UID` and `airsyncbase:Location` where 14.1 gets
/// `GlobalObjId` and `email:Location`.
pub fn meeting_request(itip: &str, ctx: &Context<'_>) -> Result<Element> {
    let calendar = Calendar::parse(itip)?;
    let id = calendar
        .master(&ICalendarComponentType::VEvent)
        .ok_or(ItemError::Unreadable("no VEVENT"))?;
    let event = calendar.component(id);
    let view = View::read(&calendar, id, ctx)?;
    let times = &view.times;
    let mut request = Element::new(email::MEETING_REQUEST);

    push_number(&mut request, email::ALL_DAY_EVENT, u8::from(times.all_day));
    request.push(Element::text(email::START_TIME, dates::iso(times.start)));
    let stamp = view.stamp.unwrap_or_else(|| ctx.now.naive_utc());
    request.push(Element::text(email::DT_STAMP, dates::iso(stamp)));
    request.push(Element::text(email::END_TIME, dates::iso(times.end)));
    let instance = recurrence_id(&calendar, event, ctx);
    let pattern = calendar::stored_pattern(event, times, ctx);
    let instance_type = match (instance, pattern) {
        (Some(_), _) => 3u8,
        (None, Some(_)) => 1,
        (None, None) => 0,
    };
    push_number(&mut request, email::INSTANCE_TYPE, instance_type);
    if let Some(location) = &view.location {
        if ctx.version.is_16() {
            request.push(
                Element::new(airsyncbase::LOCATION)
                    .with(Element::text(airsyncbase::DISPLAY_NAME, location.as_str())),
            );
        } else {
            request.push(Element::text(email::LOCATION, location.as_str()));
        }
    }
    if let Some((address, name)) = organizer(event) {
        let organizer = match name {
            Some(name) => format!("\"{}\" <{address}>", name.replace('"', "'")),
            None => address.to_string(),
        };
        request.push(Element::text(email::ORGANIZER, organizer));
    }
    if let Some(original) = instance {
        request.push(Element::text(email::RECURRENCE_ID, dates::iso(original)));
    }
    if let Some(minutes) = view.reminder {
        push_number(&mut request, email::REMINDER, u64::from(minutes) * 60);
    }
    push_number(
        &mut request,
        email::RESPONSE_REQUESTED,
        u8::from(rsvp_requested(event, ctx)),
    );
    if let Some(pattern) = pattern {
        request.push(Element::new(email::RECURRENCES).with(pattern.element(&recurrence::EMAIL)));
    }
    push_number(&mut request, email::SENSITIVITY, view.sensitivity);
    push_number(
        &mut request,
        email::BUSY_STATUS,
        view.busy.min(if ctx.version.is_16() { 4 } else { 3 }),
    );
    let at = times.zone.utc_of(times.ical(times.start, ctx));
    request.push(Element::text(
        email::TIME_ZONE,
        times.zone.rules(at).to_base64(),
    ));
    let uid = event
        .uid()
        .ok_or(ItemError::Unreadable("an invitation without UID"))?;
    if ctx.version.is_16() {
        request.push(Element::text(calendar_tags::UID, uid));
    } else {
        request.push(Element::text(
            email::GLOBAL_OBJ_ID,
            STANDARD.encode(global_object_id(uid, instance.map(|at| at.date()))),
        ));
    }
    let disallow = ical::x_text(event, "X-MICROSOFT-DISALLOW-COUNTER")
        .is_some_and(|value| value.eq_ignore_ascii_case("TRUE"));
    push_number(
        &mut request,
        email::DISALLOW_NEW_TIME_PROPOSAL,
        u8::from(disallow),
    );
    // A first invitation, or an update of one ([MS-ASEMAIL] 2.2.2.47).
    let sequence = ical::integer(event, &ICalendarProperty::Sequence).unwrap_or(0);
    push_number(
        &mut request,
        email2::MEETING_MESSAGE_TYPE,
        if sequence > 0 { 2u8 } else { 1u8 },
    );
    Ok(request)
}

/// The original start of the instance an invitation is about, as sent.
fn recurrence_id(
    calendar: &Calendar,
    event: &ICalendarComponent,
    ctx: &Context<'_>,
) -> Option<NaiveDateTime> {
    let (time, tzid) = ical::time(event, &ICalendarProperty::RecurrenceId)?;
    let zone = crate::items::timezone::Zone::of_time(calendar, time, tzid, ctx.zone);
    Some(calendar::as_sent(&zone, time, ctx))
}

/// Whether the account is asked to answer: its own line's `RSVP`, or any
/// line's when the account is not listed by name.
fn rsvp_requested(event: &ICalendarComponent, ctx: &Context<'_>) -> bool {
    let rsvp = |entry: &ICalendarEntry| {
        matches!(
            entry.parameter(&ICalendarParameterName::Rsvp),
            Some(ICalendarParameterValue::Bool(true))
        )
    };
    match own_line(event, ctx) {
        Some(index) => rsvp(&event.entries[index]),
        None => event
            .entries
            .iter()
            .filter(|entry| entry.name == ICalendarProperty::Attendee)
            .any(rsvp),
    }
}

/// The index of the account's `ATTENDEE` line.
fn own_line(event: &ICalendarComponent, ctx: &Context<'_>) -> Option<usize> {
    event.entries.iter().position(|entry| {
        entry.name == ICalendarProperty::Attendee
            && line_address(entry).is_some_and(|address| ctx.is_me(address))
    })
}

/// `GlobalObjId` ([MS-ASEMAIL] 2.2.2.37): an Outlook id (a `UID` that is
/// one, in hex) as it is, any other `UID` wrapped as a vCal id; with the
/// instance's date for one instance of a series.
pub fn global_object_id(uid: &str, instance: Option<chrono::NaiveDate>) -> Vec<u8> {
    const CLASS_ID: [u8; 16] = [
        0x04, 0x00, 0x00, 0x00, 0x82, 0x00, 0xE0, 0x00, 0x74, 0xC5, 0xB7, 0x10, 0x1A, 0x82, 0xE0,
        0x08,
    ];
    let outlook = (uid.len() >= 80 && uid.len().is_multiple_of(2))
        .then(|| {
            (0..uid.len())
                .step_by(2)
                .map(|at| u8::from_str_radix(uid.get(at..at + 2)?, 16).ok())
                .collect::<Option<Vec<u8>>>()
        })
        .flatten()
        .filter(|bytes| bytes.starts_with(&CLASS_ID));
    let mut id = match outlook {
        Some(bytes) => bytes,
        None => {
            let mut id = Vec::with_capacity(52 + uid.len());
            id.extend_from_slice(&CLASS_ID);
            id.extend_from_slice(&[0; 4 + 8 + 8]);
            let data_len = 8 + 4 + uid.len() + 1;
            id.extend_from_slice(&(data_len as u32).to_le_bytes());
            id.extend_from_slice(b"vCal-Uid");
            id.extend_from_slice(&[1, 0, 0, 0]);
            id.extend_from_slice(uid.as_bytes());
            id.push(0);
            id
        }
    };
    if let Some(date) = instance {
        let year = date.year().clamp(0, 0xFFFF) as u16;
        id[16..20].copy_from_slice(&[
            (year >> 8) as u8,
            (year & 0xFF) as u8,
            date.month() as u8,
            date.day() as u8,
        ]);
    }
    id
}

/// The iTIP REPLY (RFC 5546 3.2.3) answering `invitation` for the
/// account: `UID`, `SEQUENCE`, `ORGANIZER`, times and `RECURRENCE-ID` as
/// the invitation has them (the instance's, with `instance_id`), and the
/// account's `ATTENDEE` with its answer; `comment` goes in `COMMENT`.
pub fn reply(
    invitation: &str,
    response: Response,
    instance_id: Option<&str>,
    comment: Option<&str>,
    ctx: &Context<'_>,
) -> Result<String> {
    let mut source = Calendar::parse(invitation)?;
    let id = calendar::event_for(&mut source, instance_id, ctx)?;
    let event = source.component(id);
    let organizer = ical::first(event, &ICalendarProperty::Organizer)
        .cloned()
        .ok_or(ItemError::Unreadable("an invitation without ORGANIZER"))?;
    let uid = event
        .uid()
        .ok_or(ItemError::Unreadable("an invitation without UID"))?;

    let mut answer = ICalendarComponent::new(ICalendarComponentType::VEvent);
    answer
        .entries
        .push(ical::text_entry(ICalendarProperty::Uid, uid));
    for name in [
        ICalendarProperty::Sequence,
        ICalendarProperty::RecurrenceId,
        ICalendarProperty::Dtstart,
        ICalendarProperty::Dtend,
        ICalendarProperty::Duration,
        ICalendarProperty::Summary,
    ] {
        if let Some(entry) = ical::first(event, &name) {
            answer.entries.push(entry.clone());
        }
    }
    answer.entries.push(ical::time_entry(
        ICalendarProperty::Dtstamp,
        IcalTime::Utc(ctx.now.naive_utc()),
        None,
    ));
    answer.entries.push(organizer);
    answer.entries.push(own_attendee(event, response, ctx)?);
    if let Some(comment) = comment.map(str::trim).filter(|comment| !comment.is_empty()) {
        answer
            .entries
            .push(ical::text_entry(ICalendarProperty::Comment, comment));
    }

    let mut out = Calendar::new();
    out.component_mut(0).entries.push(
        ICalendarEntry::new(ICalendarProperty::Method)
            .with_value(ICalendarValue::Method(ICalendarMethod::Reply)),
    );
    // The zones the copied times name come along.
    for zone in source.top_level(&ICalendarComponentType::VTimezone) {
        copy_tree(&source, zone, &mut out, 0);
    }
    out.add(0, answer);
    finish_timezones(&mut out);
    Ok(out.write())
}

/// The account's `ATTENDEE` line with its answer: its own line from the
/// invitation, parameters kept, `RSVP` dropped; or a new one for an
/// account invited through a list or another address.
fn own_attendee(
    event: &ICalendarComponent,
    response: Response,
    ctx: &Context<'_>,
) -> Result<ICalendarEntry> {
    let mut entry = match own_line(event, ctx) {
        Some(index) => event.entries[index].clone(),
        None => {
            let me = ctx
                .addresses
                .first()
                .ok_or(ItemError::Unreadable("the account has no address"))?;
            let mut entry = ICalendarEntry::new(ICalendarProperty::Attendee)
                .with_value(ICalendarValue::Uri(Uri::Location(format!("mailto:{me}"))));
            if let Some(name) = ctx.name {
                entry.add_param(ICalendarParameter::cn(name.to_string()));
            }
            entry
        }
    };
    entry
        .params
        .retain(|param| param.name != ICalendarParameterName::Rsvp);
    set_param(
        &mut entry,
        ICalendarParameter::partstat(ICalendarParameterValue::Partstat(response.partstat())),
    );
    Ok(entry)
}

/// Copies component `id` of `from`, with its children, under `parent` of
/// `to`.
fn copy_tree(from: &Calendar, id: u32, to: &mut Calendar, parent: u32) {
    let mut copy = from.component(id).clone();
    let children = std::mem::take(&mut copy.component_ids);
    let new = to.add(parent, copy);
    for child in children {
        copy_tree(from, child, to, new);
    }
}

/// The account's copy of the event with its answer, for its calendar:
/// `stored` is the calendar's copy, or the invitation for a first answer
/// (its `METHOD` goes: CalDAV objects have none). With `instance_id` only
/// that instance is answered.
pub fn respond(
    stored: &str,
    response: Response,
    instance_id: Option<&str>,
    ctx: &Context<'_>,
) -> Result<String> {
    let mut calendar = Calendar::parse(stored)?;
    ical::remove(calendar.component_mut(0), &ICalendarProperty::Method);
    let id = calendar::event_for(&mut calendar, instance_id, ctx)?;
    let line = own_attendee(calendar.component(id), response, ctx)?;
    let event = calendar.component_mut(id);
    match own_line(event, ctx) {
        Some(index) => event.entries[index] = line,
        None => event.entries.push(line),
    }
    ical::replace(
        event,
        ical::time_entry(
            ICalendarProperty::Dtstamp,
            IcalTime::Utc(ctx.now.naive_utc()),
            None,
        ),
    );
    finish_timezones(&mut calendar);
    Ok(calendar.write())
}

#[cfg(test)]
mod tests;
