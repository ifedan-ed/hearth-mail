/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The properties a series and its exceptions share, for one VEVENT, both
//! ways: times, subject, location, body, sensitivity, busy status,
//! categories, reminder, meeting status and attendees.
//!
//! | ActiveSync | iCalendar |
//! |---|---|
//! | `Subject` | `SUMMARY` |
//! | `Location` (14.1), `airsyncbase:Location` (16.1) | `LOCATION`, `GEO` |
//! | `airsyncbase:Body` | `DESCRIPTION` (and `X-ALT-DESC` for HTML) |
//! | `Sensitivity` 0, 1/2, 3 | `CLASS` PUBLIC, PRIVATE, CONFIDENTIAL |
//! | `BusyStatus` | `TRANSP`, `X-MICROSOFT-CDO-BUSYSTATUS` |
//! | `Categories` | `CATEGORIES` |
//! | `Reminder` | the first `VALARM` |
//! | `MeetingStatus` (cancelled bit) | `STATUS:CANCELLED` |
//!
//! Reading builds a [`View`], the item as ActiveSync sees it; an exception
//! is sent as the fields where its view differs from the series'.

use super::attendees::{self, Attendee};
use crate::items::{
    Context, ItemError, Omitted, Result,
    alarm::{self, Trigger},
    body::{self, NewBody, StoredBody},
    dates::{self, IcalTime},
    element::{Field, flag, number, push_number, texts, trimmed},
    ical::{self, Calendar},
    timezone::Zone,
};
use crate::wbxml::{
    Element,
    codepages::{airsyncbase, calendar},
};
use calcard::icalendar::{
    ICalendarClassification, ICalendarComponent, ICalendarEntry, ICalendarParameter,
    ICalendarProperty, ICalendarStatus, ICalendarTransparency, ICalendarValue,
};
use chrono::{Duration, NaiveDateTime};

/// `X-MICROSOFT-CDO-BUSYSTATUS`, Outlook's busy status in iCalendar.
const CDO_BUSY: &str = "X-MICROSOFT-CDO-BUSYSTATUS";
/// `X-ALT-DESC`, Outlook's HTML description.
pub(crate) const ALT_DESC: &str = "X-ALT-DESC";

/// When an event happens, as ActiveSync sends it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Times {
    pub(crate) zone: Zone,
    pub(crate) all_day: bool,
    /// `StartTime` and `EndTime`: UTC, except that a 16.1 all-day event's
    /// are its dates at midnight with no zone ([MS-ASCAL] 2.2.2.1).
    pub(crate) start: NaiveDateTime,
    pub(crate) end: NaiveDateTime,
}

impl Times {
    /// The times of VEVENT `id`. An event without `DTEND` lasts its
    /// `DURATION`, or a day when all-day, or no time.
    pub(crate) fn read(calendar: &Calendar, id: u32, ctx: &Context<'_>) -> Result<Times> {
        let event = calendar.component(id);
        let (start, tzid) = ical::time(event, &ICalendarProperty::Dtstart)
            .ok_or(ItemError::Unreadable("an event without DTSTART"))?;
        let zone = Zone::of_time(calendar, start, tzid, ctx.zone);
        let all_day = matches!(start, IcalTime::Date(_));
        let start = as_sent(&zone, start, ctx);
        let end = match ical::time(event, &ICalendarProperty::Dtend) {
            Some((end, end_tzid)) => {
                as_sent(&Zone::of_time(calendar, end, end_tzid, ctx.zone), end, ctx)
            }
            None => {
                let duration = ical::first(event, &ICalendarProperty::Duration)
                    .and_then(|entry| entry.values.first())
                    .and_then(|value| match value {
                        ICalendarValue::Duration(duration) => duration.to_time_delta(),
                        _ => None,
                    });
                match duration {
                    Some(duration) => start + duration,
                    None if all_day => start + Duration::days(1),
                    None => start,
                }
            }
        };
        Ok(Times {
            zone,
            all_day,
            start,
            end: end.max(start),
        })
    }

    /// The iCalendar time for a time as sent.
    pub(crate) fn ical(&self, sent: NaiveDateTime, ctx: &Context<'_>) -> IcalTime {
        to_ical(&self.zone, self.all_day, sent, ctx)
    }
}

/// A calendar time as ActiveSync sends it.
pub(crate) fn as_sent(zone: &Zone, time: IcalTime, ctx: &Context<'_>) -> NaiveDateTime {
    match time {
        IcalTime::Date(date) if ctx.version.is_16() => date.and_time(Default::default()),
        time => zone.utc_of(time),
    }
}

/// The iCalendar time for a time as sent: a date for all-day events (the
/// local date in 14.1, the date as given in 16.1), else local in the zone.
pub(crate) fn to_ical(
    zone: &Zone,
    all_day: bool,
    sent: NaiveDateTime,
    ctx: &Context<'_>,
) -> IcalTime {
    if !all_day {
        zone.time_of(sent)
    } else if ctx.version.is_16() {
        IcalTime::Date(sent.date())
    } else {
        IcalTime::Date(zone.to_local(sent).date())
    }
}

/// One event as ActiveSync sees it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct View {
    pub(crate) times: Times,
    pub(crate) subject: Option<String>,
    pub(crate) location: Option<String>,
    /// `GEO`'s latitude and longitude, as written.
    pub(crate) geo: Option<(String, String)>,
    pub(crate) body: Option<(String, Option<String>)>,
    pub(crate) sensitivity: u8,
    pub(crate) busy: u8,
    pub(crate) reminder: Option<u32>,
    pub(crate) categories: Vec<String>,
    pub(crate) cancelled: bool,
    pub(crate) attendees: Vec<Attendee>,
    pub(crate) stamp: Option<NaiveDateTime>,
}

impl View {
    /// Reads VEVENT `id`.
    pub(crate) fn read(calendar: &Calendar, id: u32, ctx: &Context<'_>) -> Result<View> {
        let times = Times::read(calendar, id, ctx)?;
        let event = calendar.component(id);
        let start_utc = times.zone.utc_of(times.ical(times.start, ctx));
        let end_utc = times.zone.utc_of(times.ical(times.end, ctx));
        let reminder = match alarm::read(calendar, id, Some(end_utc)) {
            Some(Trigger::Before(minutes)) => Some(minutes),
            Some(Trigger::At(time)) if time <= start_utc => {
                u32::try_from((start_utc - time).num_minutes()).ok()
            }
            _ => None,
        };
        let text = |name: ICalendarProperty| {
            ical::text(event, &name)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        };
        Ok(View {
            subject: text(ICalendarProperty::Summary),
            location: text(ICalendarProperty::Location),
            geo: geo(event),
            body: text(ICalendarProperty::Description)
                .map(|text| (text, ical::x_text(event, ALT_DESC).map(str::to_string))),
            sensitivity: match ical::first(event, &ICalendarProperty::Class)
                .and_then(|entry| entry.values.first())
            {
                Some(ICalendarValue::Classification(ICalendarClassification::Private)) => 2,
                Some(ICalendarValue::Classification(ICalendarClassification::Confidential)) => 3,
                _ => 0,
            },
            busy: busy_status(event),
            reminder,
            categories: ical::categories(event)
                .into_iter()
                .map(str::to_string)
                .collect(),
            cancelled: matches!(event.status(), Some(ICalendarStatus::Cancelled)),
            attendees: attendees::read(event),
            stamp: ical::time(event, &ICalendarProperty::Dtstamp)
                .or_else(|| ical::time(event, &ICalendarProperty::LastModified))
                .map(|(time, _)| times.zone.utc_of(time)),
            times,
        })
    }

    /// `MeetingStatus` ([MS-ASCAL] 2.2.2.28): 0 without attendees; 1 or 3
    /// for the organizer or not; 4 added when cancelled.
    pub(crate) fn meeting_status(&self, event: &ICalendarComponent, ctx: &Context<'_>) -> u8 {
        if self.attendees.is_empty() {
            return 0;
        }
        let received = attendees::organizer(event).is_some_and(|(address, _)| !ctx.is_me(address));
        1 | if received { 2 } else { 0 } | if self.cancelled { 4 } else { 0 }
    }

    /// Adds the elements of this view to `out`. With `base`, the series'
    /// view, only what differs is added, and a property the series has and
    /// this exception lacks is sent empty ([MS-ASCAL] 3.2.5.3.2).
    pub(crate) fn push(
        &self,
        base: Option<&View>,
        event: &ICalendarComponent,
        ctx: &Context<'_>,
        out: &mut Element,
    ) {
        let differs = |same: bool| base.is_none() || !same;
        let emptied = |stored: bool, inherited: bool| base.is_some() && !stored && inherited;
        if let Some(stamp) = self.stamp.filter(|_| base.is_none()) {
            out.push(Element::text(calendar::DT_STAMP, dates::compact(stamp)));
        }
        out.push(Element::text(
            calendar::START_TIME,
            dates::compact(self.times.start),
        ));
        if differs(base.is_some_and(|base| base.subject == self.subject)) {
            push_or_empty(
                out,
                calendar::SUBJECT,
                self.subject.as_deref(),
                emptied(
                    self.subject.is_some(),
                    base.is_some_and(|b| b.subject.is_some()),
                ),
            );
        }
        if differs(base.is_some_and(|base| base.location == self.location && base.geo == self.geo))
        {
            let inherited = base.is_some_and(|b| b.location.is_some());
            if ctx.version.is_16() {
                if let Some(location) = self.location_element() {
                    out.push(location);
                } else if emptied(false, inherited) {
                    out.push(Element::new(airsyncbase::LOCATION));
                }
            } else {
                push_or_empty(
                    out,
                    calendar::LOCATION,
                    self.location.as_deref(),
                    emptied(self.location.is_some(), inherited),
                );
            }
        }
        out.push(Element::text(
            calendar::END_TIME,
            dates::compact(self.times.end),
        ));
        if differs(base.is_some_and(|base| base.body == self.body))
            && (base.is_none() || self.body.is_some())
        {
            let stored = self.body.as_ref().map(|(text, html)| StoredBody {
                text,
                html: html.as_deref(),
            });
            out.push(body::element(stored, &ctx.body));
        }
        if differs(base.is_some_and(|base| base.sensitivity == self.sensitivity)) {
            push_number(out, calendar::SENSITIVITY, self.sensitivity);
        }
        if differs(base.is_some_and(|base| base.busy == self.busy)) {
            // "Working elsewhere" is 16.x only ([MS-ASCAL] 2.2.2.9).
            let busy = if self.busy == 4 && !ctx.version.is_16() {
                2
            } else {
                self.busy
            };
            push_number(out, calendar::BUSY_STATUS, busy);
        }
        if differs(base.is_some_and(|base| base.times.all_day == self.times.all_day)) {
            push_number(out, calendar::ALL_DAY_EVENT, u8::from(self.times.all_day));
        }
        if differs(base.is_some_and(|base| base.reminder == self.reminder)) {
            match self.reminder {
                Some(minutes) => push_number(out, calendar::REMINDER, minutes),
                None if base.is_some() => out.push(Element::new(calendar::REMINDER)),
                None => {}
            }
        }
        if differs(base.is_some_and(|base| base.categories == self.categories)) {
            match crate::items::element::list(
                calendar::CATEGORIES,
                calendar::CATEGORY,
                self.categories.iter().map(String::as_str),
            ) {
                Some(list) => out.push(list),
                None if emptied(false, base.is_some_and(|b| !b.categories.is_empty())) => {
                    out.push(Element::new(calendar::CATEGORIES))
                }
                None => {}
            }
        }
        if differs(base.is_some_and(|base| base.attendees == self.attendees)) {
            match attendees::element(&self.attendees) {
                Some(list) => out.push(list),
                None if emptied(false, base.is_some_and(|b| !b.attendees.is_empty())) => {
                    out.push(Element::new(calendar::ATTENDEES))
                }
                None => {}
            }
        }
        if base.is_none() || base.is_some_and(|base| base.cancelled != self.cancelled) {
            push_number(
                out,
                calendar::MEETING_STATUS,
                self.meeting_status(event, ctx),
            );
        }
    }

    /// 16.1's `airsyncbase:Location`.
    fn location_element(&self) -> Option<Element> {
        let name = self.location.as_deref()?;
        let mut location = Element::new(airsyncbase::LOCATION)
            .with(Element::text(airsyncbase::DISPLAY_NAME, name));
        if let Some((latitude, longitude)) = &self.geo {
            location.push(Element::text(airsyncbase::LATITUDE, latitude.as_str()));
            location.push(Element::text(airsyncbase::LONGITUDE, longitude.as_str()));
        }
        Some(location)
    }
}

/// Adds `<tag>value</tag>`, or `<tag/>` when `empty` says the value was
/// removed from an exception.
fn push_or_empty(out: &mut Element, tag: crate::wbxml::Tag, value: Option<&str>, empty: bool) {
    match value {
        Some(value) => out.push(Element::text(tag, value)),
        None if empty => out.push(Element::new(tag)),
        None => {}
    }
}

/// `GEO` as two numbers, written as they are stored.
fn geo(event: &ICalendarComponent) -> Option<(String, String)> {
    let entry = ical::first(event, &ICalendarProperty::Geo)?;
    let number = |value: &ICalendarValue| match value {
        ICalendarValue::Float(float) => Some(float.to_string()),
        ICalendarValue::Text(text) => text.trim().parse::<f64>().ok().map(|f| f.to_string()),
        _ => None,
    };
    match entry.values.as_slice() {
        [latitude, longitude] => Some((number(latitude)?, number(longitude)?)),
        _ => None,
    }
}

/// `BusyStatus` from the CDO status, else from `TRANSP`.
fn busy_status(event: &ICalendarComponent) -> u8 {
    match ical::x_text(event, CDO_BUSY)
        .map(str::to_ascii_uppercase)
        .as_deref()
    {
        Some("FREE") => 0,
        Some("TENTATIVE") => 1,
        Some("BUSY") => 2,
        Some("OOF") => 3,
        Some("WORKINGELSEWHERE") => 4,
        _ => match event.transparency() {
            Some(ICalendarTransparency::Transparent) => 0,
            _ => 2,
        },
    }
}

/// Applies the shared properties of `data` to VEVENT `id`. `omitted` says
/// what a property left out means: for an exception, `Kept` (it inherits
/// from the series, which the exception's copy already holds).
pub(crate) fn apply(
    calendar: &mut Calendar,
    id: u32,
    data: &Element,
    omitted: Omitted<'_>,
    ctx: &Context<'_>,
) -> Result<()> {
    let field = |tag| Field::of(data, tag, omitted);
    let current = View::read(calendar, id, ctx)?;
    let event = calendar.component_mut(id);

    match field(calendar::SUBJECT) {
        Field::Set(subject) => ical::set_text(event, ICalendarProperty::Summary, subject.as_str()),
        Field::Clear => ical::remove(event, &ICalendarProperty::Summary),
        Field::Keep => {}
    }
    apply_location(event, data, omitted, ctx)?;
    match NewBody::read(data)? {
        NewBody::Set { text, html } => {
            if current.body.as_ref().map(|(text, _)| text.as_str()) != Some(text.as_str())
                || html.is_some()
            {
                ical::set_text(event, ICalendarProperty::Description, Some(&text));
                set_alt_desc(event, html.as_deref());
            }
        }
        NewBody::Clear => {
            ical::remove(event, &ICalendarProperty::Description);
            set_alt_desc(event, None);
        }
        NewBody::Keep => {}
    }
    match field(calendar::SENSITIVITY) {
        Field::Set(element) => {
            let sensitivity = number(element, 3)? as u8;
            let same = sensitivity == current.sensitivity
                || (sensitivity == 1 && current.sensitivity == 2);
            if !same {
                set_class(event, sensitivity);
            }
        }
        Field::Clear => ical::remove(event, &ICalendarProperty::Class),
        Field::Keep => {}
    }
    match field(calendar::BUSY_STATUS) {
        Field::Set(element) => {
            let busy = number(element, 4)? as u8;
            if busy != current.busy {
                set_busy_status(event, busy);
            }
        }
        Field::Clear => {
            ical::remove(event, &ICalendarProperty::Transp);
            ical::remove(event, &ical::x_name(CDO_BUSY));
        }
        Field::Keep => {}
    }
    match field(calendar::CATEGORIES) {
        Field::Set(element) => ical::set_categories(event, &texts(element, calendar::CATEGORY)?),
        Field::Clear => ical::remove(event, &ICalendarProperty::Categories),
        Field::Keep => {}
    }
    match field(calendar::MEETING_STATUS) {
        Field::Set(element) => {
            let status = crate::items::element::one_of(element, &[0, 1, 3, 5, 7, 9, 11, 13, 15])?;
            let cancelled = status & 4 != 0;
            if cancelled != current.cancelled {
                if cancelled {
                    ical::replace(
                        event,
                        ICalendarEntry::new(ICalendarProperty::Status)
                            .with_value(ICalendarValue::Status(ICalendarStatus::Cancelled)),
                    );
                } else {
                    ical::remove(event, &ICalendarProperty::Status);
                }
            }
        }
        Field::Clear | Field::Keep => {}
    }
    let rsvp = match field(calendar::RESPONSE_REQUESTED) {
        Field::Set(element) => Some(flag(element)?),
        _ => None,
    };
    match field(calendar::ATTENDEES) {
        Field::Set(element) => attendees::write(event, &attendees::parse(element)?, rsvp),
        Field::Clear => attendees::write(event, &[], None),
        Field::Keep => {
            if rsvp.is_some() {
                attendees::write(event, &current.attendees, rsvp);
            }
        }
    }

    // The reminder counts from the start as the event now has it.
    let times = Times::read(calendar, id, ctx)?;
    let start = times.zone.utc_of(times.ical(times.start, ctx));
    match field(calendar::REMINDER) {
        Field::Set(element) => {
            let minutes = number(element, u32::MAX)?;
            alarm::write(
                calendar,
                id,
                Some(Trigger::Before(minutes)),
                Some(start),
                None,
            );
        }
        Field::Clear => alarm::write(calendar, id, None, Some(start), None),
        Field::Keep => {}
    }
    Ok(())
}

/// `LOCATION` and `GEO` from 14.1's text `Location` or 16.1's
/// `airsyncbase:Location` (its `DisplayName`, `Latitude`, `Longitude`).
fn apply_location(
    event: &mut ICalendarComponent,
    data: &Element,
    omitted: Omitted<'_>,
    ctx: &Context<'_>,
) -> Result<()> {
    let tag = if ctx.version.is_16() && data.child(calendar::LOCATION).is_none() {
        airsyncbase::LOCATION
    } else {
        calendar::LOCATION
    };
    match Field::of(data, tag, omitted) {
        Field::Set(element) if tag == calendar::LOCATION => {
            ical::set_text(event, ICalendarProperty::Location, Some(trimmed(element)?));
        }
        Field::Set(element) => {
            let name = element
                .child_str(airsyncbase::DISPLAY_NAME)
                .map(str::trim)
                .filter(|name| !name.is_empty());
            ical::set_text(event, ICalendarProperty::Location, name);
            let coordinate = |tag| {
                element
                    .child_str(tag)
                    .and_then(|value| value.trim().parse::<f64>().ok())
            };
            match (
                coordinate(airsyncbase::LATITUDE),
                coordinate(airsyncbase::LONGITUDE),
            ) {
                (Some(latitude), Some(longitude))
                    if (-90.0..=90.0).contains(&latitude)
                        && (-180.0..=180.0).contains(&longitude) =>
                {
                    ical::replace(
                        event,
                        ICalendarEntry::new(ICalendarProperty::Geo).with_values(vec![
                            ICalendarValue::Float(latitude),
                            ICalendarValue::Float(longitude),
                        ]),
                    );
                }
                _ => ical::remove(event, &ICalendarProperty::Geo),
            }
        }
        Field::Clear => {
            ical::remove(event, &ICalendarProperty::Location);
            ical::remove(event, &ICalendarProperty::Geo);
        }
        Field::Keep => {}
    }
    Ok(())
}

/// `X-ALT-DESC;FMTTYPE=text/html`, or none.
pub(crate) fn set_alt_desc(component: &mut ICalendarComponent, html: Option<&str>) {
    let name = ical::x_name(ALT_DESC);
    match html {
        Some(html) => ical::replace(
            component,
            ical::text_entry(name, html)
                .with_param(ICalendarParameter::fmttype("text/html".to_string())),
        ),
        None => ical::remove(component, &name),
    }
}

/// `CLASS` for a `Sensitivity`: personal is private, normal is the
/// default and needs no line.
pub(crate) fn set_class(component: &mut ICalendarComponent, sensitivity: u8) {
    let class = match sensitivity {
        1 | 2 => ICalendarClassification::Private,
        3 => ICalendarClassification::Confidential,
        _ => {
            ical::remove(component, &ICalendarProperty::Class);
            return;
        }
    };
    ical::replace(
        component,
        ICalendarEntry::new(ICalendarProperty::Class)
            .with_value(ICalendarValue::Classification(class)),
    );
}

/// `TRANSP` for every status, and the CDO status for the ones `TRANSP`
/// cannot say (or when the event already had one, so it stays right).
fn set_busy_status(event: &mut ICalendarComponent, busy: u8) {
    let transparency = if busy == 0 {
        ICalendarTransparency::Transparent
    } else {
        ICalendarTransparency::Opaque
    };
    ical::replace(
        event,
        ICalendarEntry::new(ICalendarProperty::Transp)
            .with_value(ICalendarValue::Transparency(transparency)),
    );
    let name = ["FREE", "TENTATIVE", "BUSY", "OOF", "WORKINGELSEWHERE"][usize::from(busy.min(4))];
    if matches!(busy, 1 | 3 | 4) || ical::x_text(event, CDO_BUSY).is_some() {
        ical::replace(event, ical::text_entry(ical::x_name(CDO_BUSY), name));
    }
}

/// Sets `DTSTART` or `DTEND` to a time as sent, unless the stored one is
/// the same instant in the same form (then its line, `TZID` and all,
/// stays). Returns whether it changed.
pub(crate) fn set_time(
    calendar: &mut Calendar,
    id: u32,
    name: ICalendarProperty,
    times: &Times,
    sent: NaiveDateTime,
    ctx: &Context<'_>,
) -> bool {
    let new = times.ical(sent, ctx);
    let stored = ical::time(calendar.component(id), &name)
        .map(|(time, tzid)| (time, Zone::of_time(calendar, time, tzid, ctx.zone)));
    if let Some((time, zone)) = stored
        && matches!(time, IcalTime::Date(_)) == times.all_day
        && (times.all_day || zone == times.zone)
        && as_sent(&zone, time, ctx) == sent
    {
        return false;
    }
    let tzid = times.zone.tzid().map(str::to_string);
    let event = calendar.component_mut(id);
    if name == ICalendarProperty::Dtend {
        ical::remove(event, &ICalendarProperty::Duration);
    }
    ical::replace(event, ical::time_entry(name, new, tzid.as_deref()));
    times.zone.define(calendar);
    true
}

/// The `Timezone` element's zone for an event starting at `start` (UTC),
/// checked.
pub(crate) fn zone_of_blob(element: &Element, start: NaiveDateTime) -> Result<Zone> {
    let text = element
        .as_str()
        .ok_or_else(|| ItemError::invalid(calendar::TIMEZONE, "not text"))?;
    let rules = crate::items::timezone::TimeZoneRules::from_base64(text)?;
    Ok(Zone::from_rules(rules, chrono::Datelike::year(&start)))
}

/// `StartTime`/`EndTime` of an element, if set.
pub(crate) fn sent_time(data: &Element, tag: crate::wbxml::Tag) -> Result<Option<NaiveDateTime>> {
    data.child(tag)
        .filter(|element| !element.as_bytes().is_empty())
        .map(dates::parse_element)
        .transpose()
}
