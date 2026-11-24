/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Time zones between ActiveSync and iCalendar.
//!
//! ActiveSync sends every calendar time in UTC with the item's zone as a
//! `TimeZone` blob ([`TimeZoneRules`], [MS-ASDTYPE] 2.7.6); iCalendar keeps
//! local times with a `TZID`. [`Zone`] is the zone of one item, resolved
//! from either side: a blob becomes the IANA zone with the same rules where
//! there is one ([`zone_for_rules`]) and a VTIMEZONE of its own where there
//! is not, and a `TZID` becomes a blob for the year of the event
//! ([`rules_for_zone`]).

mod blob;
mod iana;
mod vtimezone;

pub use blob::{BLOB_LEN, TimeZoneRules, Transition};
pub use iana::{rules_for_zone, zone_for_rules};

use super::{dates::IcalTime, ical::Calendar};
use calcard::{
    common::timezone::Tz,
    icalendar::{ICalendarComponentType, ICalendarParameterName, ICalendarProperty},
};
use chrono::{Datelike, Duration, NaiveDateTime, Offset, TimeZone};

/// The zone of one calendar item.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Zone {
    /// Times are written with `Z`.
    Utc,
    /// Times are written with the zone's name as `TZID`.
    Iana(chrono_tz::Tz),
    /// A zone no IANA zone matches, written as its own VTIMEZONE.
    Custom { tzid: String, rules: TimeZoneRules },
}

impl Zone {
    /// The zone a calendar time is in: UTC for a `Z` time; the zone its
    /// `TZID` names, through the calendar's VTIMEZONE where the name alone
    /// is not an IANA zone; `floating` for a local time without `TZID` and
    /// for a `TZID` that is neither.
    pub(crate) fn of_time(
        calendar: &Calendar,
        time: IcalTime,
        tzid: Option<&str>,
        floating: chrono_tz::Tz,
    ) -> Zone {
        let (IcalTime::Local(_) | IcalTime::Date(_), Some(tzid)) = (time, tzid) else {
            return match time {
                IcalTime::Utc(_) => Zone::Utc,
                _ => Zone::iana(floating),
            };
        };
        match calendar.ical.build_tz_resolver().resolve(tzid) {
            Some(Tz::Tz(zone)) => return Zone::iana(zone),
            Some(Tz::Fixed(offset)) => {
                return Zone::Custom {
                    tzid: tzid.to_string(),
                    rules: TimeZoneRules::fixed(offset.local_minus_utc() / 60, tzid),
                };
            }
            _ => {}
        }
        let year = match time {
            IcalTime::Local(local) => local.year(),
            IcalTime::Date(date) => date.year(),
            IcalTime::Utc(utc) => utc.year(),
        };
        calendar
            .top_level(&ICalendarComponentType::VTimezone)
            .into_iter()
            .find(|id| {
                super::ical::text(calendar.component(*id), &ICalendarProperty::Tzid) == Some(tzid)
            })
            .and_then(|id| vtimezone::rules(calendar, id, year))
            .map(|rules| Zone::Custom {
                tzid: tzid.to_string(),
                rules,
            })
            .unwrap_or_else(|| Zone::iana(floating))
    }

    /// The zone of a blob from a device, for an item in `year`.
    pub(crate) fn from_rules(rules: TimeZoneRules, year: i32) -> Zone {
        if let Some(zone) = zone_for_rules(&rules, year) {
            return Zone::iana(zone);
        }
        let tzid = if rules.standard_name.trim().is_empty() {
            let offset = rules.standard_offset();
            format!(
                "GMT{}{:02}:{:02}",
                if offset < 0 { '-' } else { '+' },
                offset.unsigned_abs() / 60,
                offset.unsigned_abs() % 60
            )
        } else {
            rules.standard_name.trim().to_string()
        };
        Zone::Custom { tzid, rules }
    }

    /// An IANA zone, UTC and its aliases as [`Zone::Utc`].
    fn iana(zone: chrono_tz::Tz) -> Zone {
        if matches!(
            zone,
            chrono_tz::UTC | chrono_tz::Etc::UTC | chrono_tz::Etc::GMT | chrono_tz::Etc::Zulu
        ) {
            Zone::Utc
        } else {
            Zone::Iana(zone)
        }
    }

    /// The `TZID` times in this zone are written with.
    pub(crate) fn tzid(&self) -> Option<&str> {
        match self {
            Zone::Utc => None,
            Zone::Iana(zone) => Some(zone.name()),
            Zone::Custom { tzid, .. } => Some(tzid),
        }
    }

    /// The UTC instant of a local time in this zone. In a gap (the hour
    /// skipped in spring) the offset before the gap is used.
    pub(crate) fn to_utc(&self, local: NaiveDateTime) -> NaiveDateTime {
        match self {
            Zone::Utc => local,
            Zone::Iana(zone) => match zone.from_local_datetime(&local).earliest() {
                Some(time) => time.naive_utc(),
                None => {
                    let before = zone
                        .offset_from_utc_datetime(&(local - Duration::days(1)))
                        .fix()
                        .local_minus_utc();
                    local - Duration::seconds(i64::from(before))
                }
            },
            Zone::Custom { rules, .. } => rules.local_to_utc(local),
        }
    }

    /// The local time of a UTC instant in this zone.
    pub(crate) fn to_local(&self, utc: NaiveDateTime) -> NaiveDateTime {
        match self {
            Zone::Utc => utc,
            Zone::Iana(zone) => zone.from_utc_datetime(&utc).naive_local(),
            Zone::Custom { rules, .. } => rules.utc_to_local(utc),
        }
    }

    /// The UTC instant of a calendar time read in this zone; a date is its
    /// midnight.
    pub(crate) fn utc_of(&self, time: IcalTime) -> NaiveDateTime {
        match time {
            IcalTime::Utc(utc) => utc,
            IcalTime::Local(local) => self.to_utc(local),
            IcalTime::Date(date) => self.to_utc(date.and_time(Default::default())),
        }
    }

    /// The calendar time for a UTC instant: `Z` in UTC, local otherwise.
    pub(crate) fn time_of(&self, utc: NaiveDateTime) -> IcalTime {
        match self {
            Zone::Utc => IcalTime::Utc(utc),
            _ => IcalTime::Local(self.to_local(utc)),
        }
    }

    /// The blob for this zone around the UTC instant `at`.
    pub(crate) fn rules(&self, at: NaiveDateTime) -> TimeZoneRules {
        match self {
            Zone::Utc => TimeZoneRules::fixed(0, "UTC"),
            Zone::Iana(zone) => rules_for_zone(*zone, at),
            Zone::Custom { rules, .. } => rules.clone(),
        }
    }

    /// Adds this zone's VTIMEZONE when the calendar has none by its name.
    /// IANA zones are added by [`finish_timezones`] for the years the
    /// calendar's times need.
    pub(crate) fn define(&self, calendar: &mut Calendar) {
        if let Zone::Custom { tzid, rules } = self
            && !has_timezone(calendar, tzid)
        {
            vtimezone::add(calendar, tzid, rules);
        }
    }
}

fn has_timezone(calendar: &Calendar, tzid: &str) -> bool {
    calendar
        .top_level(&ICalendarComponentType::VTimezone)
        .into_iter()
        .any(|id| super::ical::text(calendar.component(id), &ICalendarProperty::Tzid) == Some(tzid))
}

/// After an edit: drops VTIMEZONEs no time refers to any more, and adds
/// those of the IANA zones that are referred to but not defined (RFC 5545
/// wants one per `TZID`).
pub(crate) fn finish_timezones(calendar: &mut Calendar) {
    let mut referenced: Vec<&str> = Vec::new();
    for component in &calendar.ical.components {
        if matches!(
            component.component_type,
            ICalendarComponentType::VTimezone
                | ICalendarComponentType::Standard
                | ICalendarComponentType::Daylight
        ) {
            continue;
        }
        for entry in &component.entries {
            for param in &entry.params {
                if param.name == ICalendarParameterName::Tzid
                    && let Some(tzid) = param.value.as_text()
                    && !referenced.contains(&tzid)
                {
                    referenced.push(tzid);
                }
            }
        }
    }
    let unused: Vec<u32> = calendar
        .top_level(&ICalendarComponentType::VTimezone)
        .into_iter()
        .filter(|id| {
            super::ical::text(calendar.component(*id), &ICalendarProperty::Tzid)
                .is_none_or(|tzid| !referenced.contains(&tzid))
        })
        .collect();
    calendar.remove(&unused);
    calendar.ical.add_missing_timezones();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::fixtures::PACIFIC_BLOB;

    fn at(text: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").unwrap()
    }

    #[test]
    fn blobs_become_iana_zones_or_their_own() {
        let pacific = TimeZoneRules::from_base64(PACIFIC_BLOB).unwrap();
        assert_eq!(
            Zone::from_rules(pacific, 2009),
            Zone::Iana(chrono_tz::America::Los_Angeles)
        );
        assert_eq!(
            Zone::from_rules(TimeZoneRules::fixed(0, ""), 2026),
            Zone::Utc
        );
        let odd = TimeZoneRules::fixed(17, "");
        let zone = Zone::from_rules(odd.clone(), 2026);
        assert_eq!(zone.tzid(), Some("GMT+00:17"));
        assert_eq!(zone.to_utc(at("2026-01-01 10:17")), at("2026-01-01 10:00"));
        let mut calendar = Calendar::new();
        zone.define(&mut calendar);
        zone.define(&mut calendar);
        assert_eq!(
            calendar.top_level(&ICalendarComponentType::VTimezone).len(),
            1
        );
    }

    #[test]
    fn local_times_convert_both_ways() {
        let berlin = Zone::Iana(chrono_tz::Europe::Berlin);
        assert_eq!(
            berlin.to_utc(at("2026-07-01 12:00")),
            at("2026-07-01 10:00")
        );
        assert_eq!(
            berlin.to_local(at("2026-01-01 10:00")),
            at("2026-01-01 11:00")
        );
        // 02:30 on 29 March 2026 does not exist in Berlin.
        assert_eq!(
            berlin.to_utc(at("2026-03-29 02:30")),
            at("2026-03-29 01:30")
        );
    }

    #[test]
    fn unknown_tzids_are_read_through_their_vtimezone() {
        let text = "BEGIN:VCALENDAR\r\nBEGIN:VTIMEZONE\r\nTZID:My Zone\r\n\
            BEGIN:STANDARD\r\nDTSTART:16010101T000000\r\nTZOFFSETFROM:+0530\r\n\
            TZOFFSETTO:+0530\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n";
        let calendar = Calendar::parse(text).unwrap();
        let time = IcalTime::Local(at("2026-01-01 10:00"));
        let zone = Zone::of_time(&calendar, time, Some("My Zone"), chrono_tz::UTC);
        assert_eq!(zone.utc_of(time), at("2026-01-01 04:30"));
        let windows = Zone::of_time(
            &calendar,
            time,
            Some("W. Europe Standard Time"),
            chrono_tz::UTC,
        );
        assert_eq!(windows, Zone::Iana(chrono_tz::Europe::Berlin));
        let floating = Zone::of_time(&calendar, time, None, chrono_tz::Africa::Lagos);
        assert_eq!(floating, Zone::Iana(chrono_tz::Africa::Lagos));
    }
}
