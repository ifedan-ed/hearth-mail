/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Recurrence patterns ([MS-ASCAL] 2.2.2.37, [MS-ASTASK] 2.2.2.20,
//! [MS-ASEMAIL] 2.2.2.60) and `RRULE` (RFC 5545 3.3.10, RFC 7529 for
//! `RSCALE`).
//!
//! ActiveSync has six pattern types; RRULE can say much more. Every pattern
//! has an exact RRULE. The other way, rules that fit a pattern become it,
//! and rules that do not (`BYHOUR`, several `BYMONTHDAY`s, `BYWEEKNO`...)
//! become the plainest pattern of the same frequency anchored on the start,
//! so a device shows the series rather than one event. Such a rule is not
//! lost: when the device sends back the pattern it was given, the stored
//! RRULE stays as it was ([`Pattern::same_as`]).

use super::{
    ItemError, Result, dates,
    element::{child_number, push_number},
};
use crate::wbxml::{
    Element, Tag,
    codepages::{calendar, email, email2, tasks},
};
use calcard::{
    common::{CalendarScale, PartialDateTime},
    icalendar::{
        ICalendarDay, ICalendarFrequency, ICalendarMonth, ICalendarRecurrenceRule, ICalendarWeekday,
    },
};
use chrono::{Datelike, NaiveDate, NaiveDateTime};

/// The tags of one namespace's recurrence elements, and how its `Until`
/// is written.
pub(crate) struct Tags {
    pub(crate) recurrence: Tag,
    kind: Tag,
    occurrences: Tag,
    interval: Tag,
    week_of_month: Tag,
    day_of_week: Tag,
    month_of_year: Tag,
    until: Tag,
    day_of_month: Tag,
    calendar_type: Tag,
    is_leap_month: Tag,
    first_day_of_week: Tag,
    /// `Until` in dateTime form (tasks) rather than Compact DateTime.
    until_iso: bool,
    /// The largest `Occurrences`: an unsignedShort up to 999 for
    /// calendars, an unsignedByte for tasks.
    max_occurrences: u32,
}

/// The Calendar class's `Recurrence`.
pub(crate) const CALENDAR: Tags = Tags {
    recurrence: calendar::RECURRENCE,
    kind: calendar::TYPE,
    occurrences: calendar::OCCURRENCES,
    interval: calendar::INTERVAL,
    week_of_month: calendar::WEEK_OF_MONTH,
    day_of_week: calendar::DAY_OF_WEEK,
    month_of_year: calendar::MONTH_OF_YEAR,
    until: calendar::UNTIL,
    day_of_month: calendar::DAY_OF_MONTH,
    calendar_type: calendar::CALENDAR_TYPE,
    is_leap_month: calendar::IS_LEAP_MONTH,
    first_day_of_week: calendar::FIRST_DAY_OF_WEEK,
    until_iso: false,
    max_occurrences: 999,
};

/// The Task class's `Recurrence` (its `Start`, `Regenerate` and
/// `DeadOccur` are the task module's).
pub(crate) const TASKS: Tags = Tags {
    recurrence: tasks::RECURRENCE,
    kind: tasks::TYPE,
    occurrences: tasks::OCCURRENCES,
    interval: tasks::INTERVAL,
    week_of_month: tasks::WEEK_OF_MONTH,
    day_of_week: tasks::DAY_OF_WEEK,
    month_of_year: tasks::MONTH_OF_YEAR,
    until: tasks::UNTIL,
    day_of_month: tasks::DAY_OF_MONTH,
    calendar_type: tasks::CALENDAR_TYPE,
    is_leap_month: tasks::IS_LEAP_MONTH,
    first_day_of_week: tasks::FIRST_DAY_OF_WEEK,
    until_iso: true,
    max_occurrences: 255,
};

/// The Email class's `Recurrence` in a `MeetingRequest`; its calendar
/// type, leap month and first day are in Email2.
pub(crate) const EMAIL: Tags = Tags {
    recurrence: email::RECURRENCE,
    kind: email::TYPE,
    occurrences: email::OCCURRENCES,
    interval: email::INTERVAL,
    week_of_month: email::WEEK_OF_MONTH,
    day_of_week: email::DAY_OF_WEEK,
    month_of_year: email::MONTH_OF_YEAR,
    until: email::UNTIL,
    day_of_month: email::DAY_OF_MONTH,
    calendar_type: email2::CALENDAR_TYPE,
    is_leap_month: email2::IS_LEAP_MONTH,
    first_day_of_week: email2::FIRST_DAY_OF_WEEK,
    until_iso: false,
    max_occurrences: 999,
};

/// `DayOfWeek` 127: with `WeekOfMonth`, "the nth day of the month" rather
/// than a weekday ([MS-ASCAL] 2.2.2.37.1).
const ANY_DAY: u8 = 127;

/// How a series ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum End {
    Never,
    /// `Occurrences`, `COUNT`.
    Count(u32),
    /// `Until`, `UNTIL`: the start of the last instance, UTC.
    Until(NaiveDateTime),
}

/// One recurrence pattern, as ActiveSync has it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Pattern {
    /// `Type`: 0 daily, 1 weekly, 2 monthly, 3 monthly on the nth day, 5
    /// yearly, 6 yearly on the nth day.
    pub(crate) kind: u8,
    pub(crate) interval: u16,
    /// `DayOfWeek`: bit 0 Sunday to bit 6 Saturday; 127 with
    /// `WeekOfMonth` is "day".
    pub(crate) day_of_week: u8,
    pub(crate) day_of_month: u8,
    /// 1 to 4, or 5 for the last.
    pub(crate) week_of_month: u8,
    pub(crate) month_of_year: u8,
    pub(crate) end: End,
    /// `CalendarType` ([MS-ASCAL] 2.2.2.10); 0 is the default (Gregorian).
    pub(crate) calendar_type: u8,
    pub(crate) leap_month: bool,
    /// `FirstDayOfWeek`: 0 Sunday to 6 Saturday.
    pub(crate) first_day_of_week: Option<u8>,
}

impl Pattern {
    /// Reads a `Recurrence` element, checking each value's range and which
    /// elements each type allows ([MS-ASCAL] 3.2.5.3). `Occurrences` wins
    /// over `Until` when both are there.
    pub(crate) fn read(element: &Element, tags: &Tags) -> Result<Pattern> {
        let kind = child_number(element, tags.kind, 6)?
            .ok_or_else(|| ItemError::invalid(tags.recurrence, "no Type"))?
            as u8;
        if kind == 4 {
            return Err(ItemError::invalid(tags.kind, "type 4 does not exist"));
        }
        let interval = child_number(element, tags.interval, 999)?
            .unwrap_or(1)
            .max(1) as u16;
        let day_of_week = child_number(element, tags.day_of_week, 127)?.unwrap_or(0) as u8;
        let day_of_month = child_number(element, tags.day_of_month, 31)?.unwrap_or(0) as u8;
        let week_of_month = child_number(element, tags.week_of_month, 5)?.unwrap_or(0) as u8;
        let month_of_year = child_number(element, tags.month_of_year, 12)?.unwrap_or(0) as u8;
        let calendar_type = child_number(element, tags.calendar_type, 23)?.unwrap_or(0) as u8;
        let leap_month = child_number(element, tags.is_leap_month, 1)? == Some(1);
        let first_day_of_week = child_number(element, tags.first_day_of_week, 6)?.map(|d| d as u8);
        let end = match (
            child_number(element, tags.occurrences, tags.max_occurrences)?,
            element
                .child(tags.until)
                .filter(|until| !until.as_bytes().is_empty()),
        ) {
            (Some(count), _) if count > 0 => End::Count(count),
            (_, Some(until)) => End::Until(dates::parse_element(until)?),
            _ => End::Never,
        };
        let needs = |ok: bool, tag: Tag, reason: &'static str| {
            if ok {
                Ok(())
            } else {
                Err(ItemError::invalid(tag, reason))
            }
        };
        // Which elements each type needs and allows ([MS-ASCAL] 2.2.2.37.1,
        // 3.2.5.3).
        needs(
            !matches!(kind, 2 | 5) || (1..=31).contains(&day_of_month),
            tags.day_of_month,
            "required for this type",
        )?;
        needs(
            matches!(kind, 2 | 5) || day_of_month == 0,
            tags.day_of_month,
            "not allowed for this type",
        )?;
        needs(
            !matches!(kind, 3 | 6) || (1..=5).contains(&week_of_month),
            tags.week_of_month,
            "required for this type",
        )?;
        needs(
            matches!(kind, 3 | 6) || week_of_month == 0,
            tags.week_of_month,
            "not allowed for this type",
        )?;
        needs(
            !matches!(kind, 5 | 6) || (1..=12).contains(&month_of_year),
            tags.month_of_year,
            "required for this type",
        )?;
        needs(
            matches!(kind, 5 | 6) || month_of_year == 0,
            tags.month_of_year,
            "not allowed for this type",
        )?;
        needs(
            !matches!(kind, 2 | 5) || day_of_week == 0,
            tags.day_of_week,
            "not allowed for this type",
        )?;
        needs(
            kind != 3 || day_of_week != 0,
            tags.day_of_week,
            "required for this type",
        )?;
        needs(
            !matches!(calendar_type, 13 | 16 | 17 | 18 | 19 | 21 | 22 | 23),
            tags.calendar_type,
            "reserved calendar type",
        )?;
        Ok(Pattern {
            kind,
            interval,
            day_of_week,
            day_of_month,
            week_of_month,
            month_of_year,
            end,
            calendar_type,
            leap_month,
            first_day_of_week,
        })
    }

    /// The `Recurrence` element. A weekly pattern always says its first
    /// day of the week, and the types that allow a calendar type say it
    /// ([MS-ASCAL] 2.2.2.10 and 3.2.5).
    pub(crate) fn element(&self, tags: &Tags) -> Element {
        let mut element = Element::new(tags.recurrence);
        push_number(&mut element, tags.kind, self.kind);
        match self.end {
            End::Count(count) => push_number(&mut element, tags.occurrences, count),
            End::Until(until) => element.push(Element::text(
                tags.until,
                if tags.until_iso {
                    dates::iso(until)
                } else {
                    dates::compact(until)
                },
            )),
            End::Never => {}
        }
        push_number(&mut element, tags.interval, self.interval);
        if matches!(self.kind, 3 | 6) {
            push_number(&mut element, tags.week_of_month, self.week_of_month);
        }
        if matches!(self.kind, 0 | 1 | 3 | 6) && self.day_of_week != 0 {
            push_number(&mut element, tags.day_of_week, self.day_of_week);
        }
        if matches!(self.kind, 5 | 6) {
            push_number(&mut element, tags.month_of_year, self.month_of_year);
        }
        if matches!(self.kind, 2 | 5) {
            push_number(&mut element, tags.day_of_month, self.day_of_month);
        }
        if matches!(self.kind, 2 | 3 | 5 | 6) {
            push_number(&mut element, tags.calendar_type, self.calendar_type);
            if self.leap_month {
                push_number(&mut element, tags.is_leap_month, 1u8);
            }
        }
        if self.kind == 1 || self.first_day_of_week.is_some() {
            push_number(
                &mut element,
                tags.first_day_of_week,
                self.first_day_of_week.unwrap_or(1),
            );
        }
        element
    }

    /// The RRULE; `until` is the `UNTIL` value, which the caller makes a
    /// date for all-day series and a UTC time otherwise (RFC 5545 3.3.10).
    pub(crate) fn rrule(&self, until: Option<PartialDateTime>) -> ICalendarRecurrenceRule {
        let mut rule = ICalendarRecurrenceRule {
            interval: (self.interval > 1).then_some(self.interval),
            wkst: self.first_day_of_week.map(weekday_of),
            rscale: calendar_scale(self.calendar_type),
            ..Default::default()
        };
        match self.end {
            End::Count(count) => rule.count = Some(count),
            End::Until(_) => rule.until = until,
            End::Never => {}
        }
        let month =
            || ICalendarMonth::new(self.month_of_year, self.leap_month && rule.rscale.is_some());
        match self.kind {
            0 if self.day_of_week == 0 => rule.freq = ICalendarFrequency::Daily,
            // Daily with days given is weekly ([MS-ASCAL] 2.2.2.37.1).
            0 | 1 => {
                rule.freq = ICalendarFrequency::Weekly;
                rule.byday = days_of(self.day_of_week, None);
            }
            2 => {
                rule.freq = ICalendarFrequency::Monthly;
                rule.bymonthday = vec![self.day_of_month as i8];
            }
            3 => {
                rule.freq = ICalendarFrequency::Monthly;
                self.nth(&mut rule);
            }
            5 => {
                rule.freq = ICalendarFrequency::Yearly;
                rule.bymonth = vec![month()];
                rule.bymonthday = vec![self.day_of_month as i8];
            }
            _ => {
                rule.freq = ICalendarFrequency::Yearly;
                rule.bymonth = vec![month()];
                self.nth(&mut rule);
            }
        }
        rule
    }

    /// "The nth day / weekday / weekend day / Tuesday" of types 3 and 6.
    fn nth(&self, rule: &mut ICalendarRecurrenceRule) {
        let n: i8 = if self.week_of_month >= 5 {
            -1
        } else {
            self.week_of_month as i8
        };
        let days = self.day_of_week & 0x7F;
        if days == ANY_DAY || days == 0 {
            rule.bymonthday = vec![n];
        } else if days.count_ones() == 1 {
            rule.byday = days_of(days, Some(i16::from(n)));
        } else {
            rule.byday = days_of(days, None);
            rule.bysetpos = vec![i32::from(n)];
        }
    }

    /// The pattern for an RRULE whose series starts on `start` (local),
    /// with `until` the rule's `UNTIL` as a UTC instant.
    pub(crate) fn from_rrule(
        rule: &ICalendarRecurrenceRule,
        start: NaiveDate,
        until: Option<NaiveDateTime>,
        max_occurrences: u32,
    ) -> Pattern {
        let mut pattern = Pattern {
            kind: 0,
            interval: rule.interval.unwrap_or(1).clamp(1, 999),
            day_of_week: 0,
            day_of_month: 0,
            week_of_month: 0,
            month_of_year: 0,
            end: match (rule.count, until) {
                (Some(count), _) => End::Count(count.clamp(1, max_occurrences)),
                (None, Some(until)) => End::Until(until),
                _ => End::Never,
            },
            calendar_type: match rule.rscale {
                Some(CalendarScale::Hebrew) => 8,
                Some(CalendarScale::IslamicCivil) => 6,
                Some(CalendarScale::Chinese) => 15,
                _ => 0,
            },
            leap_month: rule.bymonth.iter().any(ICalendarMonth::is_leap),
            first_day_of_week: rule.wkst.map(weekday_number),
        };
        let start_bit = 1u8 << start.weekday().num_days_from_sunday();
        let start_day = start.day() as u8;
        let start_month = start.month() as u8;
        let plain_days = || {
            rule.byday
                .iter()
                .filter(|day| day.ordwk.is_none())
                .fold(0u8, |mask, day| mask | 1 << weekday_number(day.weekday))
        };
        let fits = rule.bysecond.is_empty()
            && rule.byminute.is_empty()
            && rule.byhour.is_empty()
            && rule.byyearday.is_empty()
            && rule.byweekno.is_empty();
        let month = rule
            .bymonth
            .first()
            .map_or(start_month, |month| month.month());
        match rule.freq {
            ICalendarFrequency::Weekly => {
                pattern.kind = 1;
                pattern.day_of_week = if fits && rule.byday.iter().all(|d| d.ordwk.is_none()) {
                    plain_days()
                } else {
                    0
                };
                if pattern.day_of_week == 0 {
                    pattern.day_of_week = start_bit;
                }
            }
            ICalendarFrequency::Daily
                if fits
                    && !rule.byday.is_empty()
                    && pattern.interval == 1
                    && rule.bymonth.is_empty() =>
            {
                // Every weekday and the like: weekly on those days.
                pattern.kind = 1;
                pattern.day_of_week = plain_days().max(start_bit);
            }
            ICalendarFrequency::Monthly | ICalendarFrequency::Yearly => {
                let yearly = rule.freq == ICalendarFrequency::Yearly;
                pattern.month_of_year = if yearly { month } else { 0 };
                let nth = nth_of(rule).filter(|_| fits && rule.bymonth.len() <= 1);
                match nth {
                    Some((days, week)) => {
                        pattern.kind = if yearly { 6 } else { 3 };
                        pattern.day_of_week = days;
                        pattern.week_of_month = week;
                    }
                    None => {
                        pattern.kind = if yearly { 5 } else { 2 };
                        pattern.day_of_month = match rule.bymonthday.as_slice() {
                            [day] if *day > 0 && fits => *day as u8,
                            _ => start_day,
                        };
                    }
                }
            }
            // Daily, and anything more often than daily.
            _ => pattern.kind = 0,
        }
        if !matches!(pattern.kind, 2 | 3 | 5 | 6) {
            pattern.calendar_type = 0;
            pattern.leap_month = false;
        }
        pattern
    }

    /// Whether two patterns give the same series, the first day of the week
    /// counting only when both say it.
    pub(crate) fn same_as(&self, other: &Pattern) -> bool {
        let first_day = match (self.first_day_of_week, other.first_day_of_week) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        };
        let normal = |pattern: &Pattern| Pattern {
            first_day_of_week: None,
            // Daily with days is weekly.
            kind: if pattern.kind == 0 && pattern.day_of_week != 0 {
                1
            } else {
                pattern.kind
            },
            ..*pattern
        };
        first_day && normal(self) == normal(other)
    }
}

/// `(DayOfWeek, WeekOfMonth)` for the rules of types 3 and 6: `BYDAY=2TU`,
/// `BYMONTHDAY=-1`, `BYDAY=MO,TU,WE,TH,FR;BYSETPOS=1` and the like.
fn nth_of(rule: &ICalendarRecurrenceRule) -> Option<(u8, u8)> {
    let week = |n: i32| match n {
        1..=4 => Some(n as u8),
        -1 | 5 => Some(5),
        _ => None,
    };
    match (
        rule.byday.as_slice(),
        rule.bymonthday.as_slice(),
        rule.bysetpos.as_slice(),
    ) {
        ([day], [], []) => Some((
            1 << weekday_number(day.weekday),
            week(i32::from(day.ordwk?))?,
        )),
        ([], [-1], []) => Some((ANY_DAY, 5)),
        (days, [], [n]) if !days.is_empty() && days.iter().all(|d| d.ordwk.is_none()) => Some((
            days.iter()
                .fold(0u8, |mask, day| mask | 1 << weekday_number(day.weekday)),
            week(*n)?,
        )),
        _ => None,
    }
}

/// `BYDAY` entries for a day mask.
fn days_of(mask: u8, ordwk: Option<i16>) -> Vec<ICalendarDay> {
    (0..7u8)
        .filter(|bit| mask & (1 << bit) != 0)
        .map(|bit| ICalendarDay {
            ordwk,
            weekday: weekday_of(bit),
        })
        .collect()
}

/// 0 = Sunday, as `DayOfWeek` bits and `FirstDayOfWeek` count.
fn weekday_of(number: u8) -> ICalendarWeekday {
    match number {
        0 => ICalendarWeekday::Sunday,
        1 => ICalendarWeekday::Monday,
        2 => ICalendarWeekday::Tuesday,
        3 => ICalendarWeekday::Wednesday,
        4 => ICalendarWeekday::Thursday,
        5 => ICalendarWeekday::Friday,
        _ => ICalendarWeekday::Saturday,
    }
}

fn weekday_number(weekday: ICalendarWeekday) -> u8 {
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

/// `RSCALE` for a `CalendarType`: the lunar calendars RFC 7529 knows; the
/// eras of the Gregorian calendar (Japanese, Taiwan, Korean, Thai) count
/// months and days as Gregorian does.
fn calendar_scale(calendar_type: u8) -> Option<CalendarScale> {
    match calendar_type {
        6 => Some(CalendarScale::IslamicCivil),
        8 => Some(CalendarScale::Hebrew),
        14 | 15 | 20 => Some(CalendarScale::Chinese),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wbxml::from_xml;

    fn pattern(xml: &str) -> Pattern {
        let xml = format!("<Recurrence xmlns=\"Calendar:\">{xml}</Recurrence>");
        Pattern::read(&from_xml(&xml).unwrap(), &CALENDAR).unwrap()
    }

    fn rule_text(pattern: &Pattern) -> String {
        pattern.rrule(None).to_string()
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// Every pattern of [MS-ASCAL] 4.4 and 4.5, its RRULE, and the pattern
    /// read back from the RRULE.
    #[test]
    fn the_specification_patterns() {
        let cases = [
            // 4.4: every day; every other day.
            (
                "<Type>0</Type><Interval>1</Interval>",
                "FREQ=DAILY",
                date(2026, 11, 23),
            ),
            (
                "<Type>0</Type><Interval>2</Interval>",
                "FREQ=DAILY;INTERVAL=2",
                date(2026, 11, 23),
            ),
            // Every weekday; every Saturday.
            (
                "<Type>1</Type><Interval>1</Interval><DayOfWeek>62</DayOfWeek>",
                "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR",
                date(2026, 11, 23),
            ),
            (
                "<Type>1</Type><Interval>1</Interval><DayOfWeek>64</DayOfWeek>",
                "FREQ=WEEKLY;BYDAY=SA",
                date(2026, 11, 28),
            ),
            // The first day of every month; the last day.
            (
                "<Type>2</Type><Interval>1</Interval><DayOfMonth>1</DayOfMonth>",
                "FREQ=MONTHLY;BYMONTHDAY=1",
                date(2026, 12, 1),
            ),
            (
                "<Type>3</Type><Interval>1</Interval><WeekOfMonth>5</WeekOfMonth><DayOfWeek>127</DayOfWeek>",
                "FREQ=MONTHLY;BYMONTHDAY=-1",
                date(2026, 11, 30),
            ),
            // The first Saturday, weekday, weekend day of every month.
            (
                "<Type>3</Type><Interval>1</Interval><WeekOfMonth>1</WeekOfMonth><DayOfWeek>64</DayOfWeek>",
                "FREQ=MONTHLY;BYDAY=1SA",
                date(2026, 12, 5),
            ),
            (
                "<Type>3</Type><Interval>1</Interval><WeekOfMonth>1</WeekOfMonth><DayOfWeek>62</DayOfWeek>",
                "FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=1",
                date(2026, 12, 1),
            ),
            (
                "<Type>3</Type><Interval>1</Interval><WeekOfMonth>1</WeekOfMonth><DayOfWeek>65</DayOfWeek>",
                "FREQ=MONTHLY;BYDAY=SU,SA;BYSETPOS=1",
                date(2026, 12, 5),
            ),
            // The first of June; the first Saturday of June; the last day of
            // June.
            (
                "<Type>5</Type><Interval>1</Interval><DayOfMonth>1</DayOfMonth><MonthOfYear>6</MonthOfYear>",
                "FREQ=YEARLY;BYMONTHDAY=1;BYMONTH=6",
                date(2027, 6, 1),
            ),
            (
                "<Type>6</Type><Interval>1</Interval><WeekOfMonth>1</WeekOfMonth><DayOfWeek>64</DayOfWeek><MonthOfYear>6</MonthOfYear>",
                "FREQ=YEARLY;BYDAY=1SA;BYMONTH=6",
                date(2027, 6, 5),
            ),
            (
                "<Type>6</Type><Interval>1</Interval><WeekOfMonth>5</WeekOfMonth><DayOfWeek>127</DayOfWeek><MonthOfYear>6</MonthOfYear>",
                "FREQ=YEARLY;BYMONTHDAY=-1;BYMONTH=6",
                date(2027, 6, 30),
            ),
            // 4.5: the second day of the month, twice; Saturdays, twice.
            (
                "<Type>2</Type><Interval>1</Interval><DayOfMonth>2</DayOfMonth><CalendarType>1</CalendarType>",
                "FREQ=MONTHLY;BYMONTHDAY=2",
                date(2026, 12, 2),
            ),
            (
                "<Type>3</Type><Interval>1</Interval><WeekOfMonth>2</WeekOfMonth><DayOfWeek>127</DayOfWeek><CalendarType>1</CalendarType>",
                "FREQ=MONTHLY;BYMONTHDAY=2",
                date(2026, 12, 2),
            ),
            (
                "<Type>0</Type><Interval>1</Interval><DayOfWeek>64</DayOfWeek>",
                "FREQ=WEEKLY;BYDAY=SA",
                date(2026, 11, 28),
            ),
            (
                "<Type>1</Type><Interval>1</Interval><DayOfWeek>64</DayOfWeek>",
                "FREQ=WEEKLY;BYDAY=SA",
                date(2026, 11, 28),
            ),
        ];
        for (xml, rrule, start) in cases {
            let read = pattern(xml);
            let rule = read.rrule(None);
            assert_eq!(rule_text(&read), rrule, "{xml}");
            let back = Pattern::from_rrule(&rule, start, None, 999);
            // Patterns that say the same thing two ways come back in the
            // RRULE's way; the RRULE is what must not change.
            assert_eq!(back.rrule(None).to_string(), rrule, "{xml}");
            let element = back.element(&CALENDAR);
            let again = Pattern::read(&element, &CALENDAR).unwrap();
            assert!(again.same_as(&back), "{xml}");
        }
    }

    #[test]
    fn ends_and_weeks_are_kept() {
        let counted = pattern(
            "<Type>1</Type><Interval>1</Interval><Occurrences>3</Occurrences><DayOfWeek>32</DayOfWeek>",
        );
        assert_eq!(counted.end, End::Count(3));
        assert_eq!(rule_text(&counted), "FREQ=WEEKLY;COUNT=3;BYDAY=FR");
        // [MS-ASCAL] 4.1's monthly meeting: the second Monday until July.
        let until = pattern(
            "<Type>3</Type><Interval>1</Interval><Until>20090713T190000Z</Until>\
             <WeekOfMonth>2</WeekOfMonth><DayOfWeek>2</DayOfWeek>",
        );
        assert_eq!(
            until.end,
            End::Until(dates::parse("20090713T190000Z").unwrap())
        );
        let element = until.element(&CALENDAR);
        assert_eq!(element.child_str(calendar::UNTIL), Some("20090713T190000Z"));
        assert_eq!(element.child_u64(calendar::CALENDAR_TYPE), Some(0));
        // Both: Occurrences wins ([MS-ASCAL] 3.2.5.3).
        let both =
            pattern("<Type>0</Type><Occurrences>5</Occurrences><Until>20090713T190000Z</Until>");
        assert_eq!(both.end, End::Count(5));
        let first_day =
            pattern("<Type>1</Type><DayOfWeek>2</DayOfWeek><FirstDayOfWeek>0</FirstDayOfWeek>");
        assert_eq!(rule_text(&first_day), "FREQ=WEEKLY;BYDAY=MO;WKST=SU");
        // Weekly patterns always say their first day.
        let weekly = pattern("<Type>1</Type><DayOfWeek>2</DayOfWeek>").element(&CALENDAR);
        assert_eq!(weekly.child_u64(calendar::FIRST_DAY_OF_WEEK), Some(1));
    }

    #[test]
    fn lunar_calendars_use_rscale() {
        let hebrew = pattern(
            "<Type>5</Type><DayOfMonth>15</DayOfMonth><MonthOfYear>6</MonthOfYear>\
             <CalendarType>8</CalendarType><IsLeapMonth>1</IsLeapMonth>",
        );
        let rule = hebrew.rrule(None);
        assert_eq!(rule.rscale, Some(CalendarScale::Hebrew));
        assert!(rule.bymonth[0].is_leap());
        let back = Pattern::from_rrule(&rule, date(2027, 3, 1), None, 999);
        assert_eq!((back.calendar_type, back.leap_month), (8, true));
    }

    #[test]
    fn rules_without_a_pattern_are_approximated() {
        let rule = ICalendarRecurrenceRule {
            freq: ICalendarFrequency::Monthly,
            bymonthday: vec![1, 15],
            ..Default::default()
        };
        let approximated = Pattern::from_rrule(&rule, date(2026, 11, 1), None, 999);
        assert_eq!((approximated.kind, approximated.day_of_month), (2, 1));
        let hourly = ICalendarRecurrenceRule {
            freq: ICalendarFrequency::Hourly,
            count: Some(5000),
            ..Default::default()
        };
        let approximated = Pattern::from_rrule(&hourly, date(2026, 11, 1), None, 999);
        assert_eq!((approximated.kind, approximated.end), (0, End::Count(999)));
        let weekdays_daily = ICalendarRecurrenceRule {
            freq: ICalendarFrequency::Daily,
            byday: days_of(62, None),
            ..Default::default()
        };
        let weekly = Pattern::from_rrule(&weekdays_daily, date(2026, 11, 23), None, 999);
        assert_eq!((weekly.kind, weekly.day_of_week), (1, 62));
    }

    #[test]
    fn invalid_patterns_are_refused() {
        for xml in [
            "<Interval>1</Interval>",
            "<Type>4</Type>",
            "<Type>9</Type>",
            "<Type>2</Type>",
            "<Type>2</Type><DayOfMonth>1</DayOfMonth><DayOfWeek>2</DayOfWeek>",
            "<Type>0</Type><DayOfMonth>1</DayOfMonth>",
            "<Type>3</Type><DayOfWeek>2</DayOfWeek>",
            "<Type>5</Type><DayOfMonth>1</DayOfMonth>",
            "<Type>1</Type><DayOfWeek>200</DayOfWeek>",
            "<Type>2</Type><DayOfMonth>32</DayOfMonth>",
            "<Type>0</Type><Interval>1000</Interval>",
            "<Type>0</Type><Occurrences>1000</Occurrences>",
            "<Type>0</Type><Until>soon</Until>",
            "<Type>2</Type><DayOfMonth>1</DayOfMonth><CalendarType>13</CalendarType>",
            "<Type>1</Type><DayOfWeek>2</DayOfWeek><FirstDayOfWeek>7</FirstDayOfWeek>",
        ] {
            let xml = format!("<Recurrence xmlns=\"Calendar:\">{xml}</Recurrence>");
            assert!(
                Pattern::read(&from_xml(&xml).unwrap(), &CALENDAR).is_err(),
                "{xml}"
            );
        }
    }
}
