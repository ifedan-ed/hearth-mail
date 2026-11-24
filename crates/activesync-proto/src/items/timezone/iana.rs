/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! IANA zones and the `TimeZone` blob.
//!
//! A blob holds one year's rules, so a zone becomes a blob for the year of
//! the event it goes with: its transitions are found by sampling the
//! zone's offset day by day and narrowing each change to the minute (about
//! 400 lookups, a few microseconds). The other way, a blob names its zone
//! only loosely (`(GMT-08:00) Pacific Time (US & C`, `Eastern Standard
//! Time`, or nothing), so a name is trusted only when that zone's rules for
//! the year match the blob's; otherwise the zones are searched for one with
//! the same rules, the well-known ones first so that `Europe/Berlin` wins
//! over `Arctic/Longyearbyen`.

use super::blob::{TimeZoneRules, Transition};
use calcard::common::timezone::Tz;
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, Offset, TimeZone};
use std::str::FromStr;

/// The blob for `zone` in the year of `at` (UTC). A zone with exactly one
/// daylight period in that year gets yearly rules; any other (no daylight
/// time, or a zone that changed its offset for good that year) gets the
/// fixed offset in force at `at`.
pub fn rules_for_zone(zone: chrono_tz::Tz, at: NaiveDateTime) -> TimeZoneRules {
    let name = zone.name();
    let transitions = transitions_in_year(zone, at.date().year());
    if let [first, second] = transitions.as_slice() {
        let (to_daylight, to_standard) = if first.after > first.before {
            (first, second)
        } else {
            (second, first)
        };
        let standard = to_standard.after;
        let daylight = to_daylight.after;
        if to_daylight.before == standard && to_standard.before == daylight && daylight > standard {
            return TimeZoneRules {
                bias: -standard,
                standard_name: name.to_string(),
                // Each transition is given in the local time in force
                // before it.
                standard_date: Transition::yearly(
                    to_standard.at + Duration::minutes(i64::from(daylight)),
                ),
                standard_bias: 0,
                daylight_name: name.to_string(),
                daylight_date: Transition::yearly(
                    to_daylight.at + Duration::minutes(i64::from(standard)),
                ),
                daylight_bias: standard - daylight,
            };
        }
    }
    TimeZoneRules::fixed(offset_minutes(zone, at), name)
}

/// The IANA zone a blob stands for in `year`, if there is one with the same
/// rules: the zone its names give, else the first match among the
/// well-known zones, else among all.
pub fn zone_for_rules(rules: &TimeZoneRules, year: i32) -> Option<chrono_tz::Tz> {
    let mid_year = NaiveDate::from_ymd_opt(year, 7, 1)?.and_time(Default::default());
    let matches = |zone: chrono_tz::Tz| rules.same_rules(&rules_for_zone(zone, mid_year));
    let named = [rules.standard_name.as_str(), rules.daylight_name.as_str()]
        .into_iter()
        .filter_map(named_zone)
        .find(|zone| matches(*zone));
    if named.is_some() {
        return named;
    }
    // Two cheap lookups rule out most zones before the year is scanned.
    let january = mid_year - Duration::days(181);
    let offsets = |zone: chrono_tz::Tz| {
        (
            offset_minutes(zone, january),
            offset_minutes(zone, mid_year),
        )
    };
    let wanted = (rules.offset_at_utc(january), rules.offset_at_utc(mid_year));
    let candidate = |zone: &chrono_tz::Tz| offsets(*zone) == wanted && matches(*zone);
    WELL_KNOWN.iter().copied().find(candidate).or_else(|| {
        chrono_tz::TZ_VARIANTS
            .iter()
            .filter(|zone| is_canonical(zone.name()))
            .copied()
            .find(candidate)
    })
}

/// The zone a Windows, IANA or "(GMT+01:00) City" name stands for.
fn named_zone(name: &str) -> Option<chrono_tz::Tz> {
    if name.is_empty() {
        return None;
    }
    match Tz::from_str(name) {
        Ok(Tz::Tz(zone)) => Some(zone),
        _ => None,
    }
}

/// Region zones only: no `Etc/GMT+1`, `US/Eastern` or `EST5EDT` aliases,
/// which would read badly in a calendar.
fn is_canonical(name: &str) -> bool {
    const REGIONS: [&str; 9] = [
        "Africa/",
        "America/",
        "Antarctica/",
        "Asia/",
        "Atlantic/",
        "Australia/",
        "Europe/",
        "Indian/",
        "Pacific/",
    ];
    REGIONS.iter().any(|region| name.starts_with(region))
}

/// The zones tried first, in order: the ones Windows names map to and the
/// largest cities of each offset, so that a blob without a usable name
/// gets the zone people expect.
const WELL_KNOWN: &[chrono_tz::Tz] = {
    use chrono_tz::Tz as Z;
    &[
        Z::UTC,
        Z::Europe__London,
        Z::Europe__Berlin,
        Z::Europe__Paris,
        Z::Europe__Athens,
        Z::Europe__Helsinki,
        Z::Europe__Moscow,
        Z::Europe__Istanbul,
        Z::Africa__Lagos,
        Z::Africa__Johannesburg,
        Z::Africa__Cairo,
        Z::Africa__Nairobi,
        Z::Africa__Casablanca,
        Z::America__New_York,
        Z::America__Chicago,
        Z::America__Denver,
        Z::America__Phoenix,
        Z::America__Los_Angeles,
        Z::America__Anchorage,
        Z::Pacific__Honolulu,
        Z::America__Halifax,
        Z::America__St_Johns,
        Z::America__Mexico_City,
        Z::America__Bogota,
        Z::America__Sao_Paulo,
        Z::America__Argentina__Buenos_Aires,
        Z::America__Santiago,
        Z::Asia__Dubai,
        Z::Asia__Tehran,
        Z::Asia__Karachi,
        Z::Asia__Kolkata,
        Z::Asia__Kathmandu,
        Z::Asia__Dhaka,
        Z::Asia__Bangkok,
        Z::Asia__Shanghai,
        Z::Asia__Singapore,
        Z::Asia__Tokyo,
        Z::Asia__Seoul,
        Z::Australia__Perth,
        Z::Australia__Darwin,
        Z::Australia__Adelaide,
        Z::Australia__Brisbane,
        Z::Australia__Sydney,
        Z::Pacific__Auckland,
    ]
};

/// One change of a zone's offset.
#[derive(Debug, Clone, Copy)]
struct Change {
    /// The UTC instant of the change.
    at: NaiveDateTime,
    /// Minutes east of UTC before and after.
    before: i32,
    after: i32,
}

/// The zone's offset changes in `year`, found by sampling each day at
/// midnight UTC and narrowing each change to the minute. Bounded: 366
/// samples and 11 halvings per change.
fn transitions_in_year(zone: chrono_tz::Tz, year: i32) -> Vec<Change> {
    let Some(start) = NaiveDate::from_ymd_opt(year, 1, 1).map(|d| d.and_time(Default::default()))
    else {
        return Vec::new();
    };
    let mut changes = Vec::new();
    let mut previous = (start, offset_minutes(zone, start));
    for day in 1..=366 {
        let sample = start + Duration::days(day);
        if sample.date().year() != year && day > 365 {
            break;
        }
        let offset = offset_minutes(zone, sample);
        if offset != previous.1 {
            // The change lies in (previous.0, sample]: halve to the minute.
            let (mut low, mut high) = (previous.0, sample);
            while high - low > Duration::minutes(1) {
                let middle = low + (high - low) / 2;
                if offset_minutes(zone, middle) == previous.1 {
                    low = middle;
                } else {
                    high = middle;
                }
            }
            changes.push(Change {
                at: high,
                before: previous.1,
                after: offset,
            });
        }
        previous = (sample, offset);
    }
    changes
}

/// Minutes east of UTC in `zone` at the UTC instant `utc`.
fn offset_minutes(zone: chrono_tz::Tz, utc: NaiveDateTime) -> i32 {
    zone.offset_from_utc_datetime(&utc).fix().local_minus_utc() / 60
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Tz as Z;

    fn at(text: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").unwrap()
    }

    /// Each zone's blob for 2026 maps back to the zone, through its name
    /// and, with the names blanked, through its rules alone.
    #[test]
    fn zones_round_trip_through_the_blob() {
        for zone in [
            Z::UTC,
            Z::Europe__Berlin,
            Z::America__New_York,
            Z::Africa__Lagos,
            Z::Australia__Sydney,
        ] {
            let rules = rules_for_zone(zone, at("2026-06-15 12:00"));
            assert_eq!(zone_for_rules(&rules, 2026), Some(zone), "{zone}");
            let blob = TimeZoneRules::decode(&rules.encode()).unwrap();
            assert_eq!(blob, rules);
            let anonymous = TimeZoneRules {
                standard_name: String::new(),
                daylight_name: String::new(),
                ..rules
            };
            assert_eq!(zone_for_rules(&anonymous, 2026), Some(zone), "{zone}");
        }
    }

    #[test]
    fn rules_match_the_zone_data() {
        let berlin = rules_for_zone(Z::Europe__Berlin, at("2026-01-10 12:00"));
        assert_eq!((berlin.bias, berlin.daylight_bias), (-60, -60));
        // Last Sunday of March at 2:00 standard, of October at 3:00 daylight.
        let daylight = berlin.daylight_date;
        assert_eq!(
            (
                daylight.month,
                daylight.day,
                daylight.day_of_week,
                daylight.hour
            ),
            (3, 5, 0, 2)
        );
        let standard = berlin.standard_date;
        assert_eq!((standard.month, standard.day, standard.hour), (10, 5, 3));

        let new_york = rules_for_zone(Z::America__New_York, at("2026-01-10 12:00"));
        assert_eq!(new_york.bias, 300);
        let daylight = new_york.daylight_date;
        assert_eq!((daylight.month, daylight.day, daylight.hour), (3, 2, 2));
        let standard = new_york.standard_date;
        assert_eq!((standard.month, standard.day, standard.hour), (11, 1, 2));

        // Daylight time across the new year.
        let sydney = rules_for_zone(Z::Australia__Sydney, at("2026-01-10 12:00"));
        assert_eq!((sydney.bias, sydney.daylight_bias), (-600, -60));
        assert_eq!(sydney.daylight_date.month, 10);
        assert_eq!(sydney.standard_date.month, 4);
        assert_eq!(sydney.offset_at_utc(at("2026-01-10 12:00")), 660);
        assert_eq!(sydney.offset_at_utc(at("2026-07-10 12:00")), 600);

        let lagos = rules_for_zone(Z::Africa__Lagos, at("2026-01-10 12:00"));
        assert_eq!(lagos.bias, -60);
        assert!(!lagos.has_daylight());

        let utc = rules_for_zone(Z::UTC, at("2026-01-10 12:00"));
        assert_eq!(utc, TimeZoneRules::fixed(0, "UTC"));
    }

    /// Every instant of 2026 gets the zone's own offset from the blob.
    #[test]
    fn blob_offsets_agree_with_the_zone() {
        for zone in [
            Z::Europe__Berlin,
            Z::America__New_York,
            Z::Australia__Sydney,
        ] {
            let rules = rules_for_zone(zone, at("2026-06-15 12:00"));
            let mut instant = at("2026-01-01 00:30");
            while instant < at("2027-01-01 00:00") {
                assert_eq!(
                    rules.offset_at_utc(instant),
                    offset_minutes(zone, instant),
                    "{zone} {instant}"
                );
                instant += Duration::hours(7);
            }
        }
    }

    /// The Pacific blob of [MS-ASDTYPE] 3.6.6 names no zone calcard knows
    /// ("Pacific Time (US & C" is cut short), so its rules find it.
    #[test]
    fn the_specification_blob_finds_los_angeles() {
        let rules = TimeZoneRules::from_base64(crate::items::fixtures::PACIFIC_BLOB).unwrap();
        assert_eq!(zone_for_rules(&rules, 2009), Some(Z::America__Los_Angeles));
    }

    #[test]
    fn unknown_rules_find_nothing() {
        let rules = TimeZoneRules::fixed(17, "Nowhere");
        assert_eq!(zone_for_rules(&rules, 2026), None);
    }
}
