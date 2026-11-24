/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The `TimeZone` element's value ([MS-ASDTYPE] 2.7.6): Windows'
//! TIME_ZONE_INFORMATION, 172 bytes, little-endian, sent as base64.
//!
//! ```text
//! Bias          LONG         4   UTC = local + Bias (minutes)
//! StandardName  WCHAR[32]   64
//! StandardDate  SYSTEMTIME  16   when standard time starts, in daylight time
//! StandardBias  LONG         4   added to Bias in standard time
//! DaylightName  WCHAR[32]   64
//! DaylightDate  SYSTEMTIME  16   when daylight time starts, in standard time
//! DaylightBias  LONG         4   added to Bias in daylight time
//! ```
//!
//! A SYSTEMTIME with `wYear` 0 is a yearly rule ([MS-DTYP] 2.3.13 and
//! TIME_ZONE_INFORMATION): `wDay` is the week of the month (5 = last) of
//! the weekday `wDayOfWeek` (0 = Sunday) in `wMonth`. `wMonth` 0 means the
//! zone has no daylight time.

use super::super::{ItemError, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Weekday};

/// The size of TIME_ZONE_INFORMATION.
pub const BLOB_LEN: usize = 172;

/// A SYSTEMTIME as a transition rule.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Transition {
    /// 0 for a rule that repeats every year, else the one year it applies
    /// to.
    pub year: u16,
    /// 1 to 12; 0 when there is no transition.
    pub month: u16,
    /// 0 = Sunday to 6 = Saturday.
    pub day_of_week: u16,
    /// With `year` 0, the week of the month (1 to 5, 5 = last); else the
    /// day of the month.
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
    pub milliseconds: u16,
}

impl Transition {
    /// Whether this describes a transition at all.
    pub fn is_set(&self) -> bool {
        self.month != 0
    }

    /// The local date and time of the transition in `year`, if the rule is
    /// valid. Week 5 is the last such weekday of the month, as is any week
    /// past the end of the month.
    pub fn in_year(&self, year: i32) -> Option<NaiveDateTime> {
        let month = u32::from(self.month);
        let time = NaiveTime::from_hms_opt(
            u32::from(self.hour),
            u32::from(self.minute),
            u32::from(self.second),
        )?;
        let date = if self.year != 0 {
            NaiveDate::from_ymd_opt(year, month, u32::from(self.day))?
        } else {
            let weekday = weekday_of(self.day_of_week)?;
            let week = u8::try_from(self.day.clamp(1, 5)).ok()?;
            NaiveDate::from_weekday_of_month_opt(year, month, weekday, week).or_else(|| {
                // Week 5 in a month with four: the last one.
                NaiveDate::from_weekday_of_month_opt(year, month, weekday, 4)
            })?
        };
        Some(date.and_time(time))
    }

    /// The yearly rule for a transition at `local`: the nth weekday of its
    /// month, where a date in the month's last seven days is "the last"
    /// (rules like "last Sunday of March" are far more common than "fourth
    /// Sunday", and from one year the two cannot be told apart).
    pub fn yearly(local: NaiveDateTime) -> Self {
        let date = local.date();
        let days_in_month = days_in_month(date.year(), date.month());
        let week = if date.day() + 7 > days_in_month {
            5
        } else {
            (date.day() - 1) / 7 + 1
        };
        Transition {
            year: 0,
            month: date.month() as u16,
            day_of_week: date.weekday().num_days_from_sunday() as u16,
            day: week as u16,
            hour: local.time().hour_u16(),
            minute: local.time().minute_u16(),
            second: 0,
            milliseconds: 0,
        }
    }

    fn read(bytes: &[u8]) -> Transition {
        let word = |index: usize| u16::from_le_bytes([bytes[index * 2], bytes[index * 2 + 1]]);
        Transition {
            year: word(0),
            month: word(1),
            day_of_week: word(2),
            day: word(3),
            hour: word(4),
            minute: word(5),
            second: word(6),
            milliseconds: word(7),
        }
    }

    fn write(&self, out: &mut Vec<u8>) {
        for word in [
            self.year,
            self.month,
            self.day_of_week,
            self.day,
            self.hour,
            self.minute,
            self.second,
            self.milliseconds,
        ] {
            out.extend_from_slice(&word.to_le_bytes());
        }
    }

    /// Whether two rules give the same transitions (milliseconds and
    /// seconds aside, which no zone uses).
    fn same_as(&self, other: &Transition) -> bool {
        (
            self.year,
            self.month,
            self.day_of_week,
            self.day,
            self.hour,
            self.minute,
        ) == (
            other.year,
            other.month,
            other.day_of_week,
            other.day,
            other.hour,
            other.minute,
        )
    }
}

/// Hours and minutes as the u16 SYSTEMTIME wants, without casts at every
/// use.
trait TimeWords {
    fn hour_u16(&self) -> u16;
    fn minute_u16(&self) -> u16;
}

impl TimeWords for NaiveTime {
    fn hour_u16(&self) -> u16 {
        chrono::Timelike::hour(self) as u16
    }

    fn minute_u16(&self) -> u16 {
        chrono::Timelike::minute(self) as u16
    }
}

/// SYSTEMTIME's weekday number.
fn weekday_of(day_of_week: u16) -> Option<Weekday> {
    Some(match day_of_week {
        0 => Weekday::Sun,
        1 => Weekday::Mon,
        2 => Weekday::Tue,
        3 => Weekday::Wed,
        4 => Weekday::Thu,
        5 => Weekday::Fri,
        6 => Weekday::Sat,
        _ => return None,
    })
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .and_then(|first| first.pred_opt())
        .map_or(31, |last| last.day())
}

/// A decoded `TimeZone` blob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeZoneRules {
    /// Minutes west of UTC; UTC = local + `bias` + `standard_bias` in
    /// standard time, local + `bias` + `daylight_bias` in daylight time.
    pub bias: i32,
    pub standard_name: String,
    pub standard_date: Transition,
    pub standard_bias: i32,
    pub daylight_name: String,
    pub daylight_date: Transition,
    pub daylight_bias: i32,
}

impl TimeZoneRules {
    /// A zone with a fixed offset from UTC, `offset` minutes east.
    pub fn fixed(offset_minutes: i32, name: &str) -> Self {
        TimeZoneRules {
            bias: -offset_minutes,
            standard_name: name.to_string(),
            standard_date: Transition::default(),
            standard_bias: 0,
            daylight_name: name.to_string(),
            daylight_date: Transition::default(),
            daylight_bias: 0,
        }
    }

    /// Decodes the 172 bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != BLOB_LEN {
            return Err(ItemError::Invalid {
                element: "Timezone",
                reason: "not 172 bytes",
            });
        }
        let long = |at: usize| {
            i32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        let rules = TimeZoneRules {
            bias: long(0),
            standard_name: read_name(&bytes[4..68]),
            standard_date: Transition::read(&bytes[68..84]),
            standard_bias: long(84),
            daylight_name: read_name(&bytes[88..152]),
            daylight_date: Transition::read(&bytes[152..168]),
            daylight_bias: long(168),
        };
        // Offsets beyond a day and dates beyond the calendar are not zones.
        let sane_bias = |minutes: i32| (-1440..=1440).contains(&minutes);
        let sane_date = |date: &Transition| {
            !date.is_set()
                || (date.month <= 12 && date.day <= 31 && date.hour < 24 && date.minute < 60)
        };
        if sane_bias(rules.bias)
            && sane_bias(rules.standard_bias)
            && sane_bias(rules.daylight_bias)
            && sane_date(&rules.standard_date)
            && sane_date(&rules.daylight_date)
        {
            Ok(rules)
        } else {
            Err(ItemError::Invalid {
                element: "Timezone",
                reason: "offsets or dates out of range",
            })
        }
    }

    /// Decodes the element's base64 text, which clients and the
    /// specification's examples break over lines.
    pub fn from_base64(text: &str) -> Result<Self> {
        let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        let bytes = STANDARD.decode(compact).map_err(|_| ItemError::Invalid {
            element: "Timezone",
            reason: "not base64",
        })?;
        Self::decode(&bytes)
    }

    /// The 172 bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(BLOB_LEN);
        out.extend_from_slice(&self.bias.to_le_bytes());
        write_name(&self.standard_name, &mut out);
        self.standard_date.write(&mut out);
        out.extend_from_slice(&self.standard_bias.to_le_bytes());
        write_name(&self.daylight_name, &mut out);
        self.daylight_date.write(&mut out);
        out.extend_from_slice(&self.daylight_bias.to_le_bytes());
        out
    }

    /// The element's text.
    pub fn to_base64(&self) -> String {
        STANDARD.encode(self.encode())
    }

    /// Whether the zone switches to daylight time at all.
    pub fn has_daylight(&self) -> bool {
        self.standard_date.is_set()
            && self.daylight_date.is_set()
            && self.daylight_bias != self.standard_bias
    }

    /// Minutes east of UTC in standard time.
    pub fn standard_offset(&self) -> i32 {
        -(self.bias + self.standard_bias)
    }

    /// Minutes east of UTC in daylight time.
    pub fn daylight_offset(&self) -> i32 {
        -(self.bias + self.daylight_bias)
    }

    /// Whether two blobs describe the same zone, names aside.
    pub fn same_rules(&self, other: &TimeZoneRules) -> bool {
        if self.standard_offset() != other.standard_offset()
            || self.has_daylight() != other.has_daylight()
        {
            return false;
        }
        !self.has_daylight()
            || (self.daylight_offset() == other.daylight_offset()
                && self.standard_date.same_as(&other.standard_date)
                && self.daylight_date.same_as(&other.daylight_date))
    }

    /// The UTC instants daylight time starts and ends in `year`.
    fn daylight_period(&self, year: i32) -> Option<(NaiveDateTime, NaiveDateTime)> {
        if !self.has_daylight() {
            return None;
        }
        // DaylightDate is in standard time, StandardDate in daylight time.
        let start = self.daylight_date.in_year(year)?
            - Duration::minutes(i64::from(self.standard_offset()));
        let end = self.standard_date.in_year(year)?
            - Duration::minutes(i64::from(self.daylight_offset()));
        Some((start, end))
    }

    /// Minutes east of UTC at the UTC instant `utc`.
    pub fn offset_at_utc(&self, utc: NaiveDateTime) -> i32 {
        match self.daylight_period(utc.year()) {
            // Northern hemisphere: daylight time within the year.
            Some((start, end)) if start <= end && utc >= start && utc < end => {
                self.daylight_offset()
            }
            // Southern hemisphere: daylight time across the new year.
            Some((start, end)) if start > end && (utc >= start || utc < end) => {
                self.daylight_offset()
            }
            _ => self.standard_offset(),
        }
    }

    /// The UTC instant of a local time. In the hour skipped in spring the
    /// standard offset is used; in the hour repeated in autumn, the first.
    pub fn local_to_utc(&self, local: NaiveDateTime) -> NaiveDateTime {
        let daylight = local - Duration::minutes(i64::from(self.daylight_offset()));
        if self.has_daylight() && self.offset_at_utc(daylight) == self.daylight_offset() {
            return daylight;
        }
        local - Duration::minutes(i64::from(self.standard_offset()))
    }

    /// The local time of a UTC instant.
    pub fn utc_to_local(&self, utc: NaiveDateTime) -> NaiveDateTime {
        utc + Duration::minutes(i64::from(self.offset_at_utc(utc)))
    }
}

/// A WCHAR[32] name, up to its first NUL; unpaired surrogates become
/// U+FFFD.
fn read_name(bytes: &[u8]) -> String {
    let units = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .take_while(|unit| *unit != 0);
    char::decode_utf16(units)
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

/// A name as WCHAR[32]: at most 32 UTF-16 units, NULs after a shorter one,
/// never cutting a surrogate pair. A name that fills all 32 has no NUL, as
/// in [MS-ASDTYPE] 3.6.6's own example.
fn write_name(name: &str, out: &mut Vec<u8>) {
    let mut written = 0;
    for c in name.chars() {
        let mut units = [0u16; 2];
        let units = c.encode_utf16(&mut units);
        if written + units.len() > 32 {
            break;
        }
        for unit in units.iter() {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        written += units.len();
    }
    out.resize(out.len() + (32 - written) * 2, 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").unwrap()
    }

    #[test]
    fn the_specification_example_decodes() {
        let rules = TimeZoneRules::from_base64(crate::items::fixtures::PACIFIC_BLOB).unwrap();
        assert_eq!(rules.bias, 480);
        assert_eq!(rules.standard_name, "(GMT-08:00) Pacific Time (US & C");
        assert_eq!(rules.standard_offset(), -480);
        assert_eq!(rules.daylight_offset(), -420);
        // First Sunday of November at 2:00, second Sunday of March at 2:00.
        assert_eq!(
            (
                rules.standard_date.month,
                rules.standard_date.day,
                rules.standard_date.hour
            ),
            (11, 1, 2)
        );
        assert_eq!(
            (
                rules.daylight_date.month,
                rules.daylight_date.day,
                rules.daylight_date.hour
            ),
            (3, 2, 2)
        );
        assert_eq!(TimeZoneRules::decode(&rules.encode()).unwrap(), rules);
    }

    #[test]
    fn offsets_follow_the_rules() {
        let pacific = TimeZoneRules::from_base64(crate::items::fixtures::PACIFIC_BLOB).unwrap();
        // 2026: DST from 8 March 10:00 UTC to 1 November 09:00 UTC.
        assert_eq!(pacific.offset_at_utc(at("2026-03-08 09:59")), -480);
        assert_eq!(pacific.offset_at_utc(at("2026-03-08 10:00")), -420);
        assert_eq!(pacific.offset_at_utc(at("2026-11-01 08:59")), -420);
        assert_eq!(pacific.offset_at_utc(at("2026-11-01 09:00")), -480);
        assert_eq!(
            pacific.local_to_utc(at("2026-07-01 10:00")),
            at("2026-07-01 17:00")
        );
        assert_eq!(
            pacific.utc_to_local(at("2026-01-01 18:00")),
            at("2026-01-01 10:00")
        );
    }

    #[test]
    fn names_are_cut_to_32_units() {
        let rules = TimeZoneRules::fixed(60, &"x".repeat(40));
        let decoded = TimeZoneRules::decode(&rules.encode()).unwrap();
        assert_eq!(decoded.standard_name.len(), 32);
        assert_eq!(rules.encode().len(), BLOB_LEN);
    }

    #[test]
    fn garbage_is_refused() {
        assert!(TimeZoneRules::from_base64("not base64!").is_err());
        assert!(TimeZoneRules::decode(&[0; 10]).is_err());
        let mut bytes = TimeZoneRules::fixed(0, "UTC").encode();
        bytes[0..4].copy_from_slice(&100_000i32.to_le_bytes());
        assert!(TimeZoneRules::decode(&bytes).is_err());
        let mut bytes = TimeZoneRules::fixed(0, "UTC").encode();
        bytes[70] = 99; // StandardDate.wMonth
        assert!(TimeZoneRules::decode(&bytes).is_err());
    }

    #[test]
    fn yearly_rules_pick_the_last_week() {
        let last_sunday_march = Transition::yearly(at("2026-03-29 02:00"));
        assert_eq!((last_sunday_march.month, last_sunday_march.day), (3, 5));
        let second_sunday = Transition::yearly(at("2026-03-08 02:00"));
        assert_eq!((second_sunday.month, second_sunday.day), (3, 2));
        assert_eq!(
            last_sunday_march.in_year(2027),
            Some(at("2027-03-28 02:00"))
        );
    }
}
