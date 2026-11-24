/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The two date forms ActiveSync uses and their iCalendar counterparts.
//!
//! - Compact DateTime ([MS-ASDTYPE] 2.7.2), `20091212T000000Z`: calendar
//!   times, `DtStamp`, `Until`, notes' `LastModifiedDate`.
//! - dateTime ([MS-ASDTYPE] 2.3), `2009-11-12T00:45:06.000Z`: tasks,
//!   contacts' birthdays, meeting requests in mail.
//!
//! Both are UTC. Clients are known to send one where the other belongs, so
//! reading accepts either; writing uses the form the element is defined
//! with.

use super::{ItemError, Result};
use crate::wbxml::Element;
use calcard::common::PartialDateTime;
use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime, Timelike};

/// A UTC time in Compact DateTime form.
pub(crate) fn compact(time: NaiveDateTime) -> String {
    time.format("%Y%m%dT%H%M%SZ").to_string()
}

/// A UTC time in dateTime form, with milliseconds as Exchange sends them.
pub(crate) fn iso(time: NaiveDateTime) -> String {
    time.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// A UTC time in either form, with or without the `Z` and the
/// milliseconds.
pub(crate) fn parse(text: &str) -> Option<NaiveDateTime> {
    let text = text.trim();
    let text = text.strip_suffix('Z').unwrap_or(text);
    NaiveDateTime::parse_from_str(text, "%Y%m%dT%H%M%S")
        .or_else(|_| NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f"))
        .or_else(|_| NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M"))
        .ok()
        .or_else(|| {
            NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .or_else(|_| NaiveDate::parse_from_str(text, "%Y%m%d"))
                .ok()
                .map(|date| date.and_time(NaiveTime::MIN))
        })
}

/// An element holding a UTC time in either form.
pub(crate) fn parse_element(element: &Element) -> Result<NaiveDateTime> {
    element
        .as_str()
        .and_then(parse)
        .ok_or_else(|| ItemError::invalid(element.tag, "not a date and time"))
}

/// A time as iCalendar keeps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IcalTime {
    /// A `DATE` value: all-day.
    Date(NaiveDate),
    /// A UTC time (`Z`, or an explicit offset already applied).
    Utc(NaiveDateTime),
    /// A local time: floating, or in the zone its `TZID` names.
    Local(NaiveDateTime),
}

impl IcalTime {
    /// Reads a parsed iCalendar or vCard date; `None` for partial dates
    /// (no year) and impossible ones.
    pub(crate) fn read(value: &PartialDateTime) -> Option<IcalTime> {
        let date = NaiveDate::from_ymd_opt(
            i32::from(value.year?),
            u32::from(value.month?),
            u32::from(value.day?),
        )?;
        let Some(hour) = value.hour else {
            return Some(IcalTime::Date(date));
        };
        let time = NaiveTime::from_hms_opt(
            u32::from(hour),
            u32::from(value.minute.unwrap_or(0)),
            u32::from(value.second.unwrap_or(0).min(59)),
        )?;
        let local = date.and_time(time);
        match (value.tz_hour, value.tz_minute) {
            (Some(hours), minutes) => {
                let offset = i64::from(hours) * 3600 + i64::from(minutes.unwrap_or(0)) * 60;
                let offset = if value.tz_minus { -offset } else { offset };
                local
                    .checked_sub_signed(chrono::TimeDelta::seconds(offset))
                    .map(IcalTime::Utc)
            }
            (None, _) => Some(IcalTime::Local(local)),
        }
    }

    /// The value calcard writes for this time.
    pub(crate) fn write(self) -> PartialDateTime {
        match self {
            IcalTime::Date(date) => date_value(date),
            IcalTime::Utc(time) => PartialDateTime {
                tz_hour: Some(0),
                tz_minute: Some(0),
                ..time_value(time)
            },
            IcalTime::Local(time) => time_value(time),
        }
    }
}

/// A date with no time.
fn date_value(date: NaiveDate) -> PartialDateTime {
    PartialDateTime {
        year: u16::try_from(date.year()).ok(),
        month: u8::try_from(date.month()).ok(),
        day: u8::try_from(date.day()).ok(),
        ..Default::default()
    }
}

/// A date and time with no zone.
fn time_value(time: NaiveDateTime) -> PartialDateTime {
    PartialDateTime {
        hour: u8::try_from(time.hour()).ok(),
        minute: u8::try_from(time.minute()).ok(),
        second: u8::try_from(time.second()).ok(),
        ..date_value(time.date())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S").unwrap()
    }

    #[test]
    fn both_forms_are_read_and_written() {
        // [MS-ASDTYPE] 3.6.2 and 3.3.
        let time = at("2013-07-22 09:00:00");
        assert_eq!(parse("20130722T090000Z"), Some(time));
        assert_eq!(compact(time), "20130722T090000Z");
        assert_eq!(
            parse("2000-12-25T08:35:00.000Z"),
            Some(at("2000-12-25 08:35:00"))
        );
        assert_eq!(iso(at("2000-12-25 08:35:00")), "2000-12-25T08:35:00.000Z");
        assert_eq!(parse("2009-09-03"), Some(at("2009-09-03 00:00:00")));
        assert_eq!(parse("not a date"), None);
        assert_eq!(parse("20131322T090000Z"), None);
    }

    #[test]
    fn ical_times_round_trip() {
        for time in [
            IcalTime::Date(NaiveDate::from_ymd_opt(2026, 11, 23).unwrap()),
            IcalTime::Utc(at("2026-11-23 10:30:00")),
            IcalTime::Local(at("2026-11-23 10:30:00")),
        ] {
            assert_eq!(IcalTime::read(&time.write()), Some(time));
        }
        let offset = PartialDateTime {
            tz_hour: Some(1),
            tz_minute: Some(0),
            ..IcalTime::Local(at("2026-11-23 10:30:00")).write()
        };
        assert_eq!(
            IcalTime::read(&offset),
            Some(IcalTime::Utc(at("2026-11-23 09:30:00")))
        );
        assert_eq!(IcalTime::read(&PartialDateTime::default()), None);
    }
}
