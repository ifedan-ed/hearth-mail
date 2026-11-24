/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Reminders as VALARM components.
//!
//! ActiveSync knows one reminder per item: minutes before the start for
//! events ([MS-ASCAL] 2.2.2.38), a time for tasks ([MS-ASTASK] 2.2.2.23).
//! The first VALARM of the item is that reminder. Changing it changes only
//! that alarm's `TRIGGER`, so its other properties and any further alarms
//! set in Nextcloud stay; removing the reminder removes every alarm, since
//! the device said the item has none.

use super::{
    dates::IcalTime,
    ical::{self, Calendar},
};
use calcard::icalendar::{
    ICalendarAction, ICalendarComponent, ICalendarComponentType, ICalendarDuration, ICalendarEntry,
    ICalendarParameter, ICalendarParameterName, ICalendarParameterValue, ICalendarProperty,
    ICalendarRelated, ICalendarValue,
};
use chrono::{Duration, NaiveDateTime};

/// When a reminder goes off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Trigger {
    /// Minutes before the start.
    Before(u32),
    /// At this UTC time.
    At(NaiveDateTime),
}

impl Trigger {
    /// The UTC time it goes off for an item starting at `start`.
    fn time(self, start: Option<NaiveDateTime>) -> Option<NaiveDateTime> {
        match self {
            Trigger::At(time) => Some(time),
            Trigger::Before(minutes) => {
                start.map(|start| start - Duration::minutes(i64::from(minutes)))
            }
        }
    }
}

/// The first alarm's trigger. A relative trigger counts from the start, or
/// from the end (`DTEND`, `DUE`) with `RELATED=END`; one after the start
/// is not a reminder ActiveSync can say and reads as none.
pub(crate) fn read(calendar: &Calendar, item: u32, end: Option<NaiveDateTime>) -> Option<Trigger> {
    let alarm = *calendar
        .children(item, &ICalendarComponentType::VAlarm)
        .first()?;
    let entry = ical::first(calendar.component(alarm), &ICalendarProperty::Trigger)?;
    match entry.values.first()? {
        ICalendarValue::Duration(duration) => {
            let seconds = duration.as_seconds();
            let from_end = matches!(
                entry.parameter(&ICalendarParameterName::Related),
                Some(ICalendarParameterValue::Related(ICalendarRelated::End))
            );
            if from_end {
                end.map(|end| Trigger::At(end + Duration::seconds(seconds)))
            } else if seconds <= 0 {
                u32::try_from(-seconds / 60).ok().map(Trigger::Before)
            } else {
                None
            }
        }
        ICalendarValue::PartialDateTime(time) => match IcalTime::read(time)? {
            IcalTime::Utc(time) => Some(Trigger::At(time)),
            // RFC 5545 wants absolute triggers in UTC; read others as UTC.
            IcalTime::Local(time) => Some(Trigger::At(time)),
            IcalTime::Date(date) => Some(Trigger::At(date.and_time(Default::default()))),
        },
        _ => None,
    }
}

/// Sets the reminder: `None` removes every alarm; otherwise the first
/// alarm gets the trigger (unless it already goes off then), or a display
/// alarm is added.
pub(crate) fn write(
    calendar: &mut Calendar,
    item: u32,
    trigger: Option<Trigger>,
    start: Option<NaiveDateTime>,
    end: Option<NaiveDateTime>,
) {
    let alarms = calendar.children(item, &ICalendarComponentType::VAlarm);
    let Some(trigger) = trigger else {
        calendar.remove(&alarms);
        return;
    };
    if let Some(current) = read(calendar, item, end)
        && (current == trigger || current.time(start) == trigger.time(start))
    {
        return;
    }
    let entry = trigger_entry(trigger);
    match alarms.first() {
        Some(alarm) => ical::replace(calendar.component_mut(*alarm), entry),
        None => {
            let mut alarm = ICalendarComponent::new(ICalendarComponentType::VAlarm);
            alarm.entries = vec![
                ICalendarEntry::new(ICalendarProperty::Action)
                    .with_value(ICalendarValue::Action(ICalendarAction::Display)),
                ical::text_entry(ICalendarProperty::Description, "Reminder"),
                entry,
            ];
            calendar.add(item, alarm);
        }
    }
}

fn trigger_entry(trigger: Trigger) -> ICalendarEntry {
    match trigger {
        Trigger::Before(minutes) => ICalendarEntry::new(ICalendarProperty::Trigger).with_value(
            ICalendarValue::Duration(ICalendarDuration::from_seconds(-i64::from(minutes) * 60)),
        ),
        Trigger::At(time) => {
            ical::time_entry(ICalendarProperty::Trigger, IcalTime::Utc(time), None).with_param(
                ICalendarParameter::value(ICalendarParameterValue::Value(
                    calcard::icalendar::ICalendarValueType::DateTime,
                )),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVENT: &str = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a\r\n\
        BEGIN:VALARM\r\nACTION:AUDIO\r\nTRIGGER:-PT10M\r\nX-KEEP:1\r\nEND:VALARM\r\n\
        BEGIN:VALARM\r\nACTION:EMAIL\r\nTRIGGER;RELATED=END:PT0S\r\nEND:VALARM\r\n\
        END:VEVENT\r\nEND:VCALENDAR\r\n";

    fn at(text: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").unwrap()
    }

    #[test]
    fn the_first_alarm_is_the_reminder() {
        let mut calendar = Calendar::parse(EVENT).unwrap();
        let event = calendar.master(&ICalendarComponentType::VEvent).unwrap();
        let start = Some(at("2026-11-24 10:00"));
        assert_eq!(read(&calendar, event, None), Some(Trigger::Before(10)));
        // The same time said another way changes nothing.
        write(
            &mut calendar,
            event,
            Some(Trigger::At(at("2026-11-24 09:50"))),
            start,
            None,
        );
        assert!(calendar.write().contains("TRIGGER:-PT10M"));
        write(&mut calendar, event, Some(Trigger::Before(30)), start, None);
        let text = calendar.write();
        assert!(text.contains("TRIGGER:-PT30M"));
        assert!(text.contains("X-KEEP:1"));
        assert!(text.contains("ACTION:EMAIL"));
        write(&mut calendar, event, None, start, None);
        assert!(!calendar.write().contains("VALARM"));
        write(
            &mut calendar,
            event,
            Some(Trigger::At(at("2026-11-24 09:00"))),
            start,
            None,
        );
        let text = calendar.write();
        assert!(
            text.contains("TRIGGER;VALUE=DATE-TIME:20261124T090000Z"),
            "{text}"
        );
        assert!(text.contains("ACTION:DISPLAY"));
        let event = Calendar::parse(&text).unwrap();
        assert_eq!(
            read(&event, 1, None),
            Some(Trigger::At(at("2026-11-24 09:00")))
        );
    }
}
