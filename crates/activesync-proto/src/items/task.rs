/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The Task class ([MS-ASTASK]) and VTODO (RFC 5545 3.6.2).
//!
//! | ActiveSync | iCalendar |
//! |---|---|
//! | `Subject` | `SUMMARY` |
//! | `airsyncbase:Body` | `DESCRIPTION` (and `X-ALT-DESC` for HTML) |
//! | `UtcStartDate`, `StartDate` | `DTSTART` |
//! | `UtcDueDate`, `DueDate` | `DUE` |
//! | `Complete`, `DateCompleted` | `STATUS:COMPLETED`, `COMPLETED`, `PERCENT-COMPLETE:100` |
//! | `Importance` 0, 1, 2 | `PRIORITY` 9, 5, 1 |
//! | `Sensitivity` | `CLASS` |
//! | `ReminderSet`, `ReminderTime` | the first `VALARM`, at an absolute time |
//! | `Recurrence` | `RRULE`; `Regenerate` and `DeadOccur` as `X-ACTIVESYNC-` properties |
//! | `Categories` | `CATEGORIES` |
//!
//! Tasks carry each date twice: UTC, and the device's local time written
//! as if it were UTC ([MS-ASTASK] 4.1: `DueDate` 13:00, `UtcDueDate`
//! 20:00). A local time of midnight is a date (`DUE;VALUE=DATE`), which is
//! what phones mean by a task due on a day; any other is the UTC time.

use super::{
    Context, ItemError, Omitted, Result,
    alarm::{self, Trigger},
    body::{self, NewBody, StoredBody},
    calendar::{ALT_DESC, set_alt_desc, set_class},
    dates::{self, IcalTime},
    element::{Field, flag, number, push_number, texts},
    ical::{self, Calendar},
    recurrence::{self, End, Pattern},
    timezone::{Zone, finish_timezones},
};
use crate::wbxml::{
    Element, Tag,
    codepages::{airsync, tasks},
};
use calcard::icalendar::{
    ICalendarComponent, ICalendarComponentType, ICalendarEntry, ICalendarProperty, ICalendarStatus,
    ICalendarValue,
};
use chrono::{NaiveDate, NaiveDateTime, NaiveTime};

/// Where `Regenerate` and `DeadOccur` are kept: iCalendar has neither.
const REGENERATE: &str = "X-ACTIVESYNC-REGENERATE";
const DEAD_OCCUR: &str = "X-ACTIVESYNC-DEADOCCUR";

/// A task date: its property and its two elements.
struct DateTags {
    name: ICalendarProperty,
    utc: Tag,
    local: Tag,
}

const START: DateTags = DateTags {
    name: ICalendarProperty::Dtstart,
    utc: tasks::UTC_START_DATE,
    local: tasks::START_DATE,
};

const DUE: DateTags = DateTags {
    name: ICalendarProperty::Due,
    utc: tasks::UTC_DUE_DATE,
    local: tasks::DUE_DATE,
};

/// Writes a Task class `ApplicationData` into the task's iCalendar text
/// (`existing`, or a new object for an `Add`) and returns the new text.
/// `Importance` and `ReminderSet` left out of a `Change` always keep their
/// value ([MS-ASTASK] 3.2.5.3.1 and 3.2.5.3.2).
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
    let id = match calendar.master(&ICalendarComponentType::VTodo) {
        Some(id) => id,
        None if existing.is_some() => return Err(ItemError::Unreadable("no VTODO")),
        None => calendar.add(0, ICalendarComponent::new(ICalendarComponentType::VTodo)),
    };
    let field = |tag| Field::of(data, tag, omitted);
    let current = View::read(&calendar, id, ctx);
    let todo = calendar.component_mut(id);

    match field(tasks::SUBJECT) {
        Field::Set(subject) => ical::set_text(todo, ICalendarProperty::Summary, subject.as_str()),
        Field::Clear => ical::remove(todo, &ICalendarProperty::Summary),
        Field::Keep => {}
    }
    match NewBody::read(data)? {
        NewBody::Set { text, html } => {
            if current.body.as_deref() != Some(text.as_str()) || html.is_some() {
                ical::set_text(todo, ICalendarProperty::Description, Some(&text));
                set_alt_desc(todo, html.as_deref());
            }
        }
        NewBody::Clear => {
            ical::remove(todo, &ICalendarProperty::Description);
            set_alt_desc(todo, None);
        }
        NewBody::Keep => {}
    }
    if let Field::Set(element) = Field::of(data, tasks::IMPORTANCE, Omitted::Kept) {
        // Values past 2 are accepted ([MS-ASTASK] 2.2.2.14) and kept as high.
        let importance = number(element, u32::MAX)?.min(2) as u8;
        if importance != current.importance {
            let priority = [9, 5, 1][usize::from(importance)];
            ical::replace(
                todo,
                ICalendarEntry::new(ICalendarProperty::Priority)
                    .with_value(ICalendarValue::Integer(priority)),
            );
        }
    }
    match field(tasks::SENSITIVITY) {
        Field::Set(element) => {
            let sensitivity = number(element, 3)? as u8;
            if sensitivity != current.sensitivity && !(sensitivity == 1 && current.sensitivity == 2)
            {
                set_class(todo, sensitivity);
            }
        }
        Field::Clear => ical::remove(todo, &ICalendarProperty::Class),
        Field::Keep => {}
    }
    match field(tasks::CATEGORIES) {
        Field::Set(element) => ical::set_categories(todo, &texts(element, tasks::CATEGORY)?),
        Field::Clear => ical::remove(todo, &ICalendarProperty::Categories),
        Field::Keep => {}
    }
    write_completion(todo, data, ctx)?;
    write_date(&mut calendar, id, data, omitted, ctx, &START)?;
    write_date(&mut calendar, id, data, omitted, ctx, &DUE)?;
    write_recurrence(&mut calendar, id, data, omitted, ctx)?;
    write_reminder(&mut calendar, id, data, ctx)?;

    let todo = calendar.component_mut(id);
    if todo.uid().is_none() {
        ical::set_text(todo, ICalendarProperty::Uid, Some(&ical::new_uid(ctx.now)));
    }
    let now = IcalTime::Utc(ctx.now.naive_utc());
    ical::replace(
        todo,
        ical::time_entry(ICalendarProperty::Dtstamp, now, None),
    );
    ical::replace(
        todo,
        ical::time_entry(ICalendarProperty::LastModified, now, None),
    );
    finish_timezones(&mut calendar);
    Ok(calendar.write())
}

/// `Complete` and `DateCompleted`. A task marked done gets `COMPLETED`
/// (the device's time, else now) and 100 percent; one marked not done
/// loses them and goes back to needing action. Left out, nothing changes:
/// `Complete` is required in every request, so its absence is not a "no".
fn write_completion(
    todo: &mut ICalendarComponent,
    data: &Element,
    ctx: &Context<'_>,
) -> Result<()> {
    let Some(complete) = data
        .child(tasks::COMPLETE)
        .filter(|c| !c.as_bytes().is_empty())
    else {
        return Ok(());
    };
    let done = flag(complete)?;
    let was_done = is_done(todo);
    if done {
        let at = data
            .child(tasks::DATE_COMPLETED)
            .filter(|at| !at.as_bytes().is_empty())
            .map(dates::parse_element)
            .transpose()?;
        let stored = ical::time(todo, &ICalendarProperty::Completed).map(|(time, _)| time);
        if !was_done || at.is_some_and(|at| stored != Some(IcalTime::Utc(at))) {
            let at = at.unwrap_or_else(|| ctx.now.naive_utc());
            ical::replace(
                todo,
                ICalendarEntry::new(ICalendarProperty::Status)
                    .with_value(ICalendarValue::Status(ICalendarStatus::Completed)),
            );
            ical::replace(
                todo,
                ical::time_entry(ICalendarProperty::Completed, IcalTime::Utc(at), None),
            );
            ical::replace(
                todo,
                ICalendarEntry::new(ICalendarProperty::PercentComplete)
                    .with_value(ICalendarValue::Integer(100)),
            );
        }
    } else if was_done {
        ical::replace(
            todo,
            ICalendarEntry::new(ICalendarProperty::Status)
                .with_value(ICalendarValue::Status(ICalendarStatus::NeedsAction)),
        );
        ical::remove(todo, &ICalendarProperty::Completed);
        if ical::integer(todo, &ICalendarProperty::PercentComplete) == Some(100) {
            ical::remove(todo, &ICalendarProperty::PercentComplete);
        }
    }
    Ok(())
}

fn is_done(todo: &ICalendarComponent) -> bool {
    matches!(todo.status(), Some(ICalendarStatus::Completed))
        || todo.has_property(&ICalendarProperty::Completed)
}

/// A start or due date from its UTC and local elements.
fn sent_date(data: &Element, tags: &DateTags, ctx: &Context<'_>) -> Result<Option<IcalTime>> {
    let read = |tag| {
        data.child(tag)
            .filter(|element| !element.as_bytes().is_empty())
            .map(dates::parse_element)
            .transpose()
    };
    Ok(match (read(tags.utc)?, read(tags.local)?) {
        (_, Some(local)) if local.time() == NaiveTime::MIN => Some(IcalTime::Date(local.date())),
        (Some(utc), _) => Some(IcalTime::Utc(utc)),
        (None, Some(local)) => Some(IcalTime::Utc(Zone::Iana(ctx.zone).to_utc(local))),
        (None, None) => None,
    })
}

/// Writes `DTSTART` or `DUE`, keeping the stored line when it is the same
/// date or the same instant.
fn write_date(
    calendar: &mut Calendar,
    id: u32,
    data: &Element,
    omitted: Omitted<'_>,
    ctx: &Context<'_>,
    tags: &DateTags,
) -> Result<()> {
    let absent = data.child(tags.utc).is_none() && data.child(tags.local).is_none();
    let new = match sent_date(data, tags, ctx)? {
        Some(time) => time,
        None if absent && omitted.keeps(tags.utc) => return Ok(()),
        None => {
            ical::remove(calendar.component_mut(id), &tags.name);
            return Ok(());
        }
    };
    if let Some((stored, tzid)) = ical::time(calendar.component(id), &tags.name) {
        let zone = Zone::of_time(calendar, stored, tzid, ctx.zone);
        let same = match (stored, new) {
            (IcalTime::Date(a), IcalTime::Date(b)) => a == b,
            (IcalTime::Date(_), _) | (_, IcalTime::Date(_)) => false,
            (stored, new) => zone.utc_of(stored) == zone.utc_of(new),
        };
        if same {
            return Ok(());
        }
    }
    ical::replace(
        calendar.component_mut(id),
        ical::time_entry(tags.name.clone(), new, None),
    );
    Ok(())
}

/// `Recurrence` with its required `Start`, `Regenerate` and `DeadOccur`.
fn write_recurrence(
    calendar: &mut Calendar,
    id: u32,
    data: &Element,
    omitted: Omitted<'_>,
    ctx: &Context<'_>,
) -> Result<()> {
    let element = match Field::of(data, tasks::RECURRENCE, omitted) {
        Field::Set(element) => element,
        Field::Clear => {
            let todo = calendar.component_mut(id);
            ical::remove(todo, &ICalendarProperty::Rrule);
            ical::remove(todo, &ical::x_name(REGENERATE));
            ical::remove(todo, &ical::x_name(DEAD_OCCUR));
            return Ok(());
        }
        Field::Keep => return Ok(()),
    };
    let pattern = Pattern::read(element, &recurrence::TASKS)?;
    let start = element
        .child(tasks::START)
        .filter(|start| !start.as_bytes().is_empty())
        .ok_or_else(|| ItemError::invalid(tasks::RECURRENCE, "no Start"))
        .and_then(dates::parse_element)?;
    // The series counts from DTSTART; a task without one starts with the
    // recurrence ([MS-ASTASK] 2.2.2.25).
    if ical::time(calendar.component(id), &ICalendarProperty::Dtstart).is_none() {
        ical::replace(
            calendar.component_mut(id),
            ical::time_entry(
                ICalendarProperty::Dtstart,
                IcalTime::Date(start.date()),
                None,
            ),
        );
    }
    let view = View::read(calendar, id, ctx);
    let todo = calendar.component_mut(id);
    if !view.pattern.is_some_and(|stored| stored.same_as(&pattern)) {
        // RFC 5545 3.3.10: a date `UNTIL` for a date start, else UTC.
        let until = match pattern.end {
            End::Until(until) if matches!(view.anchor, Some(IcalTime::Date(_)) | None) => {
                Some(IcalTime::Date(until.date()))
            }
            End::Until(until) => Some(IcalTime::Utc(until)),
            _ => None,
        };
        ical::replace(
            todo,
            ICalendarEntry::new(ICalendarProperty::Rrule).with_value(
                ICalendarValue::RecurrenceRule(Box::new(pattern.rrule(until.map(IcalTime::write)))),
            ),
        );
    }
    for (tag, name) in [
        (tasks::REGENERATE, REGENERATE),
        (tasks::DEAD_OCCUR, DEAD_OCCUR),
    ] {
        let value = element
            .child(tag)
            .filter(|value| !value.as_bytes().is_empty())
            .map(|value| number(value, 255))
            .transpose()?
            .unwrap_or(0);
        if value == 0 {
            ical::remove(todo, &ical::x_name(name));
        } else {
            ical::replace(
                todo,
                ical::text_entry(ical::x_name(name), &value.to_string()),
            );
        }
    }
    Ok(())
}

/// `ReminderSet` and `ReminderTime`: an alarm at that time, or none.
fn write_reminder(
    calendar: &mut Calendar,
    id: u32,
    data: &Element,
    ctx: &Context<'_>,
) -> Result<()> {
    let Field::Set(set) = Field::of(data, tasks::REMINDER_SET, Omitted::Kept) else {
        return Ok(());
    };
    let view = View::read(calendar, id, ctx);
    if !flag(set)? {
        alarm::write(calendar, id, None, view.start_utc, view.due_utc);
        return Ok(());
    }
    if let Some(time) = data
        .child(tasks::REMINDER_TIME)
        .filter(|time| !time.as_bytes().is_empty())
        .map(dates::parse_element)
        .transpose()?
    {
        alarm::write(
            calendar,
            id,
            Some(Trigger::At(time)),
            view.start_utc,
            view.due_utc,
        );
    }
    Ok(())
}

/// A task as ActiveSync sees it.
struct View {
    body: Option<String>,
    importance: u8,
    sensitivity: u8,
    /// `DTSTART`, or `DUE` without one: what a recurrence counts from.
    anchor: Option<IcalTime>,
    start_utc: Option<NaiveDateTime>,
    due_utc: Option<NaiveDateTime>,
    pattern: Option<Pattern>,
}

impl View {
    fn read(calendar: &Calendar, id: u32, ctx: &Context<'_>) -> View {
        let todo = calendar.component(id);
        let utc = |name: &ICalendarProperty| {
            ical::time(todo, name)
                .map(|(time, tzid)| Zone::of_time(calendar, time, tzid, ctx.zone).utc_of(time))
        };
        let anchor = ical::time(todo, &ICalendarProperty::Dtstart)
            .or_else(|| ical::time(todo, &ICalendarProperty::Due))
            .map(|(time, _)| time);
        let pattern = ical::first(todo, &ICalendarProperty::Rrule)
            .and_then(|entry| entry.values.first())
            .and_then(|value| match value {
                ICalendarValue::RecurrenceRule(rule) => Some(rule),
                _ => None,
            })
            .map(|rule| {
                let start = anchor.map_or(ctx.now.date_naive(), date_of);
                let until = rule
                    .until
                    .as_ref()
                    .and_then(IcalTime::read)
                    .map(|until| match until {
                        IcalTime::Date(date) => date.and_time(NaiveTime::MIN),
                        IcalTime::Utc(time) | IcalTime::Local(time) => time,
                    });
                Pattern::from_rrule(rule, start, until, 255)
            });
        View {
            body: ical::text(todo, &ICalendarProperty::Description).map(str::to_string),
            importance: match ical::integer(todo, &ICalendarProperty::Priority) {
                Some(1..=4) => 2,
                Some(6..=9) => 0,
                _ => 1,
            },
            sensitivity: match ical::text(todo, &ICalendarProperty::Class) {
                Some(class) if class.eq_ignore_ascii_case("PRIVATE") => 2,
                Some(class) if class.eq_ignore_ascii_case("CONFIDENTIAL") => 3,
                _ => 0,
            },
            anchor,
            start_utc: utc(&ICalendarProperty::Dtstart),
            due_utc: utc(&ICalendarProperty::Due),
            pattern,
        }
    }
}

fn date_of(time: IcalTime) -> NaiveDate {
    match time {
        IcalTime::Date(date) => date,
        IcalTime::Utc(time) | IcalTime::Local(time) => time.date(),
    }
}

/// The Task class `ApplicationData` for a task's iCalendar text, in the
/// order of [MS-ASTASK] 4.2's example.
pub fn from_ical(text: &str, ctx: &Context<'_>) -> Result<Element> {
    let calendar = Calendar::parse(text)?;
    let id = calendar
        .master(&ICalendarComponentType::VTodo)
        .ok_or(ItemError::Unreadable("no VTODO"))?;
    let todo = calendar.component(id);
    let view = View::read(&calendar, id, ctx);
    let mut data = Element::new(airsync::APPLICATION_DATA);

    let stored = ical::text(todo, &ICalendarProperty::Description)
        .filter(|text| !text.is_empty())
        .map(|text| StoredBody {
            text,
            html: ical::x_text(todo, ALT_DESC),
        });
    data.push(body::element(stored, &ctx.body));
    if let Some(subject) = ical::text(todo, &ICalendarProperty::Summary) {
        data.push(Element::text(tasks::SUBJECT, subject));
    }
    push_number(&mut data, tasks::IMPORTANCE, view.importance);
    for tags in [&START, &DUE] {
        if let Some((time, tzid)) = ical::time(todo, &tags.name) {
            let zone = Zone::of_time(&calendar, time, tzid, ctx.zone);
            let local = match time {
                IcalTime::Date(date) => date.and_time(NaiveTime::MIN),
                IcalTime::Local(local) => local,
                IcalTime::Utc(utc) => Zone::Iana(ctx.zone).to_local(utc),
            };
            data.push(Element::text(tags.utc, dates::iso(zone.utc_of(time))));
            data.push(Element::text(tags.local, dates::iso(local)));
        }
    }
    if let Some(list) =
        super::element::list(tasks::CATEGORIES, tasks::CATEGORY, ical::categories(todo))
    {
        data.push(list);
    }
    if let Some(pattern) = view.pattern {
        let mut element = pattern.element(&recurrence::TASKS);
        let start = view.anchor.map_or(ctx.now.date_naive(), date_of);
        element.push(Element::text(
            tasks::START,
            dates::iso(start.and_time(NaiveTime::MIN)),
        ));
        for (tag, name) in [
            (tasks::REGENERATE, REGENERATE),
            (tasks::DEAD_OCCUR, DEAD_OCCUR),
        ] {
            if let Some(value) =
                ical::x_text(todo, name).and_then(|value| value.trim().parse::<u8>().ok())
            {
                push_number(&mut element, tag, value);
            }
        }
        data.push(element);
    }
    let done = is_done(todo);
    push_number(&mut data, tasks::COMPLETE, u8::from(done));
    if done {
        let at = ical::time(todo, &ICalendarProperty::Completed)
            .or_else(|| ical::time(todo, &ICalendarProperty::LastModified))
            .map(|(time, _)| Zone::Utc.utc_of(time))
            .unwrap_or_else(|| ctx.now.naive_utc());
        data.push(Element::text(tasks::DATE_COMPLETED, dates::iso(at)));
    }
    push_number(&mut data, tasks::SENSITIVITY, view.sensitivity);
    let reminder = alarm::read(&calendar, id, view.due_utc).and_then(|trigger| match trigger {
        Trigger::At(time) => Some(time),
        Trigger::Before(minutes) => view
            .start_utc
            .or(view.due_utc)
            .map(|anchor| anchor - chrono::Duration::minutes(i64::from(minutes))),
    });
    if let Some(time) = reminder {
        data.push(Element::text(tasks::REMINDER_TIME, dates::iso(time)));
    }
    push_number(&mut data, tasks::REMINDER_SET, u8::from(reminder.is_some()));
    Ok(data)
}

#[cfg(test)]
mod tests;
