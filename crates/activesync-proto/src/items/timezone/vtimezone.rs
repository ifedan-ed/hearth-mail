/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! VTIMEZONE components for zones that are not IANA zones, both ways: a
//! blob from a device whose rules match no IANA zone is written as a
//! VTIMEZONE with yearly rules, and a VTIMEZONE whose `TZID` names no IANA
//! zone is read into a blob.

use super::blob::{TimeZoneRules, Transition};
use crate::items::ical::{self, Calendar};
use calcard::{
    common::PartialDateTime,
    icalendar::{
        ICalendarComponent, ICalendarComponentType, ICalendarDay, ICalendarEntry,
        ICalendarFrequency, ICalendarMonth, ICalendarProperty, ICalendarRecurrenceRule,
        ICalendarValue, ICalendarWeekday,
    },
};
use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};

/// Adds a VTIMEZONE named `tzid` with the blob's rules under the
/// VCALENDAR, before the first other child (RFC 5545 puts zones first).
pub(crate) fn add(calendar: &mut Calendar, tzid: &str, rules: &TimeZoneRules) {
    let mut zone = ICalendarComponent::new(ICalendarComponentType::VTimezone);
    zone.entries
        .push(ical::text_entry(ICalendarProperty::Tzid, tzid));
    let zone_id = calendar.add(0, zone);
    // Keep the zone ahead of the events in the output.
    let root = calendar.component_mut(0);
    root.component_ids.retain(|id| *id != zone_id);
    root.component_ids.insert(0, zone_id);

    if rules.has_daylight() {
        let standard = observance(
            ICalendarComponentType::Standard,
            &rules.standard_date,
            rules.daylight_offset(),
            rules.standard_offset(),
        );
        let daylight = observance(
            ICalendarComponentType::Daylight,
            &rules.daylight_date,
            rules.standard_offset(),
            rules.daylight_offset(),
        );
        calendar.add(zone_id, standard);
        calendar.add(zone_id, daylight);
    } else {
        let offset = rules.standard_offset();
        let mut standard = ICalendarComponent::new(ICalendarComponentType::Standard);
        standard.entries = vec![
            start_entry(0, 0),
            offset_entry(ICalendarProperty::Tzoffsetfrom, offset),
            offset_entry(ICalendarProperty::Tzoffsetto, offset),
        ];
        calendar.add(zone_id, standard);
    }
}

/// A STANDARD or DAYLIGHT observance starting at `transition` every year.
fn observance(
    kind: ICalendarComponentType,
    transition: &Transition,
    from: i32,
    to: i32,
) -> ICalendarComponent {
    let mut component = ICalendarComponent::new(kind);
    let weekday = [
        ICalendarWeekday::Sunday,
        ICalendarWeekday::Monday,
        ICalendarWeekday::Tuesday,
        ICalendarWeekday::Wednesday,
        ICalendarWeekday::Thursday,
        ICalendarWeekday::Friday,
        ICalendarWeekday::Saturday,
    ]
    .get(usize::from(transition.day_of_week))
    .copied()
    .unwrap_or(ICalendarWeekday::Sunday);
    let week = if transition.day >= 5 {
        -1
    } else {
        i16::try_from(transition.day.max(1)).unwrap_or(1)
    };
    let rule = ICalendarRecurrenceRule {
        freq: ICalendarFrequency::Yearly,
        bymonth: vec![ICalendarMonth::new(transition.month.min(12) as u8, false)],
        byday: vec![ICalendarDay {
            ordwk: Some(week),
            weekday,
        }],
        ..Default::default()
    };
    component.entries = vec![
        start_entry(transition.hour, transition.minute),
        offset_entry(ICalendarProperty::Tzoffsetfrom, from),
        offset_entry(ICalendarProperty::Tzoffsetto, to),
        ICalendarEntry::new(ICalendarProperty::Rrule)
            .with_value(ICalendarValue::RecurrenceRule(Box::new(rule))),
    ];
    component
}

/// `DTSTART:16010101Thhmm00`, the start Outlook gives yearly rules.
fn start_entry(hour: u16, minute: u16) -> ICalendarEntry {
    let start = NaiveDate::from_ymd_opt(1601, 1, 1)
        .and_then(|date| date.and_hms_opt(u32::from(hour.min(23)), u32::from(minute.min(59)), 0))
        .unwrap_or_default();
    ical::time_entry(
        ICalendarProperty::Dtstart,
        crate::items::dates::IcalTime::Local(start),
        None,
    )
}

/// A `TZOFFSETFROM` or `TZOFFSETTO` of `minutes` east of UTC.
fn offset_entry(name: ICalendarProperty, minutes: i32) -> ICalendarEntry {
    let magnitude = minutes.unsigned_abs();
    ICalendarEntry::new(name).with_value(ICalendarValue::PartialDateTime(Box::new(
        PartialDateTime {
            tz_hour: Some((magnitude / 60).min(23) as u8),
            tz_minute: Some((magnitude % 60) as u8),
            tz_minus: minutes < 0,
            ..Default::default()
        },
    )))
}

/// The blob for the VTIMEZONE `zone_id` in `year`: for each of STANDARD
/// and DAYLIGHT, the observance that started last before the end of that
/// year. `None` when the zone has no usable STANDARD.
pub(crate) fn rules(calendar: &Calendar, zone_id: u32, year: i32) -> Option<TimeZoneRules> {
    let tzid =
        ical::text(calendar.component(zone_id), &ICalendarProperty::Tzid).unwrap_or_default();
    let standard = latest(calendar, zone_id, &ICalendarComponentType::Standard, year)?;
    let Some(daylight) = latest(calendar, zone_id, &ICalendarComponentType::Daylight, year) else {
        return Some(TimeZoneRules::fixed(standard.offset, tzid));
    };
    if daylight.offset == standard.offset {
        return Some(TimeZoneRules::fixed(standard.offset, tzid));
    }
    Some(TimeZoneRules {
        bias: -standard.offset,
        standard_name: tzid.to_string(),
        standard_date: standard.transition,
        standard_bias: 0,
        daylight_name: tzid.to_string(),
        daylight_date: daylight.transition,
        daylight_bias: standard.offset - daylight.offset,
    })
}

/// One observance as the blob needs it.
struct Observance {
    /// `TZOFFSETTO`, minutes east of UTC.
    offset: i32,
    transition: Transition,
    /// `DTSTART`, to pick the latest.
    start: NaiveDateTime,
}

/// The observance of `kind` in force in `year`.
fn latest(
    calendar: &Calendar,
    zone_id: u32,
    kind: &ICalendarComponentType,
    year: i32,
) -> Option<Observance> {
    calendar
        .children(zone_id, kind)
        .into_iter()
        .filter_map(|id| read_observance(calendar.component(id)))
        .filter(|observance| observance.start.year() <= year)
        .max_by_key(|observance| observance.start)
}

fn read_observance(component: &ICalendarComponent) -> Option<Observance> {
    let start = match ical::time(component, &ICalendarProperty::Dtstart)?.0 {
        crate::items::dates::IcalTime::Local(time) | crate::items::dates::IcalTime::Utc(time) => {
            time
        }
        crate::items::dates::IcalTime::Date(date) => date.and_time(Default::default()),
    };
    let offset = ical::first(component, &ICalendarProperty::Tzoffsetto)?
        .values
        .first()?
        .as_partial_date_time()?;
    let minutes = i32::from(offset.tz_hour?) * 60 + i32::from(offset.tz_minute.unwrap_or(0));
    let offset = if offset.tz_minus { -minutes } else { minutes };
    let rule = ical::first(component, &ICalendarProperty::Rrule)
        .and_then(|entry| entry.values.first())
        .and_then(|value| match value {
            ICalendarValue::RecurrenceRule(rule) => Some(rule),
            _ => None,
        });
    let transition = match rule {
        Some(rule) => {
            let month = rule
                .bymonth
                .first()
                .map_or(start.month(), |m| u32::from(m.month()));
            let day = rule.byday.first()?;
            let week = match day.ordwk {
                Some(n) if n < 0 => 5,
                Some(n) => n.clamp(1, 5) as u16,
                None => 1,
            };
            Transition {
                year: 0,
                month: month as u16,
                day_of_week: weekday_number(day.weekday),
                day: week,
                hour: start.hour() as u16,
                minute: start.minute() as u16,
                ..Default::default()
            }
        }
        None => Transition::yearly(start),
    };
    Some(Observance {
        offset,
        transition,
        start,
    })
}

fn weekday_number(weekday: ICalendarWeekday) -> u16 {
    match weekday {
        ICalendarWeekday::Sunday => 0,
        ICalendarWeekday::Monday => 1,
        ICalendarWeekday::Tuesday => 2,
        ICalendarWeekday::Wednesday => 3,
        ICalendarWeekday::Thursday => 4,
        ICalendarWeekday::Friday => 5,
        ICalendarWeekday::Saturday => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::timezone::iana::rules_for_zone;

    #[test]
    fn rules_survive_a_vtimezone() {
        let at = NaiveDate::from_ymd_opt(2026, 6, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        for zone in [
            chrono_tz::Europe::Berlin,
            chrono_tz::Australia::Sydney,
            chrono_tz::Africa::Lagos,
            chrono_tz::America::New_York,
        ] {
            let expected = rules_for_zone(zone, at);
            let mut calendar = Calendar::new();
            add(&mut calendar, "Custom", &expected);
            let text = calendar.write();
            let calendar = Calendar::parse(&text).unwrap();
            let zone_id = calendar.top_level(&ICalendarComponentType::VTimezone)[0];
            let read = rules(&calendar, zone_id, 2026).unwrap();
            assert!(read.same_rules(&expected), "{zone}: {text}");
        }
    }

    /// A VTIMEZONE as Outlook writes them for a zone with an older rule:
    /// the observance in force for the year is the one used.
    #[test]
    fn the_latest_observance_wins() {
        let text = "BEGIN:VCALENDAR\r\nBEGIN:VTIMEZONE\r\nTZID:Old and New\r\n\
            BEGIN:STANDARD\r\nDTSTART:16011104T020000\r\nRRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10\r\n\
            TZOFFSETFROM:-0400\r\nTZOFFSETTO:-0500\r\nEND:STANDARD\r\n\
            BEGIN:DAYLIGHT\r\nDTSTART:16010401T020000\r\nRRULE:FREQ=YEARLY;BYDAY=1SU;BYMONTH=4\r\n\
            TZOFFSETFROM:-0500\r\nTZOFFSETTO:-0400\r\nEND:DAYLIGHT\r\n\
            BEGIN:STANDARD\r\nDTSTART:20071104T020000\r\nRRULE:FREQ=YEARLY;BYDAY=1SU;BYMONTH=11\r\n\
            TZOFFSETFROM:-0400\r\nTZOFFSETTO:-0500\r\nEND:STANDARD\r\n\
            BEGIN:DAYLIGHT\r\nDTSTART:20070311T020000\r\nRRULE:FREQ=YEARLY;BYDAY=2SU;BYMONTH=3\r\n\
            TZOFFSETFROM:-0500\r\nTZOFFSETTO:-0400\r\nEND:DAYLIGHT\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n";
        let calendar = Calendar::parse(text).unwrap();
        let zone_id = calendar.top_level(&ICalendarComponentType::VTimezone)[0];
        let now = rules(&calendar, zone_id, 2026).unwrap();
        assert_eq!(
            (now.bias, now.daylight_date.month, now.daylight_date.day),
            (300, 3, 2)
        );
        assert_eq!((now.standard_date.month, now.standard_date.day), (11, 1));
        let then = rules(&calendar, zone_id, 2005).unwrap();
        assert_eq!(
            (then.daylight_date.month, then.standard_date.month),
            (4, 10)
        );
    }
}
