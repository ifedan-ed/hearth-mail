/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Exceptions to a series ([MS-ASCAL] 2.2.2.21 to 2.2.2.23) and
//! iCalendar's overrides (a VEVENT with the series' `UID` and a
//! `RECURRENCE-ID`) and `EXDATE`s (RFC 5545 3.8.5.1).
//!
//! An instance is named by its original start as ActiveSync sends it
//! (`ExceptionStartTime` in 14.1, `airsyncbase:InstanceId` in 16.1). A
//! deleted instance (`Deleted` 1) is an `EXDATE`; a changed one an
//! override, made by copying the series and then applying the exception's
//! elements, so what the exception leaves out is inherited.

use super::event::{self, Times, View};
use crate::items::{
    Context, ItemError, MAX_ITEMS, Omitted, Result,
    dates::{self, IcalTime},
    element::{flag, push_number},
    ical::{self, Calendar},
    timezone::Zone,
};
use crate::wbxml::{
    Element,
    codepages::{airsyncbase, calendar},
};
use calcard::icalendar::{
    ICalendarComponentType, ICalendarProperty, ICalendarStatus, ICalendarValue,
};
use chrono::NaiveDateTime;

/// The overrides of the series and the original start of each, as sent.
fn overrides(calendar: &Calendar, ctx: &Context<'_>) -> Vec<(u32, NaiveDateTime)> {
    calendar
        .overrides(&ICalendarComponentType::VEvent)
        .into_iter()
        .filter_map(|id| {
            let (time, tzid) =
                ical::time(calendar.component(id), &ICalendarProperty::RecurrenceId)?;
            let zone = Zone::of_time(calendar, time, tzid, ctx.zone);
            Some((id, event::as_sent(&zone, time, ctx)))
        })
        .collect()
}

/// The `EXDATE`s of VEVENT `id`, as sent.
fn exdates(calendar: &Calendar, id: u32, ctx: &Context<'_>) -> Vec<NaiveDateTime> {
    let event = calendar.component(id);
    event
        .entries
        .iter()
        .filter(|entry| entry.name == ICalendarProperty::Exdate)
        .flat_map(|entry| {
            let tzid = entry.tz_id();
            entry.values.iter().filter_map(move |value| {
                let time = IcalTime::read(value.as_partial_date_time()?)?;
                let zone = Zone::of_time(calendar, time, tzid, ctx.zone);
                Some(event::as_sent(&zone, time, ctx))
            })
        })
        .take(MAX_ITEMS)
        .collect()
}

/// The `Exceptions` element of a series; `None` without exceptions.
pub(super) fn element(
    calendar: &Calendar,
    master: u32,
    series: &View,
    ctx: &Context<'_>,
) -> Result<Option<Element>> {
    let mut found: Vec<(NaiveDateTime, Element)> = Vec::new();
    for (id, original) in overrides(calendar, ctx).into_iter().take(MAX_ITEMS) {
        let mut exception = Element::new(calendar::EXCEPTION);
        let instance = calendar.component(id);
        if matches!(instance.status(), Some(ICalendarStatus::Cancelled)) {
            push_number(&mut exception, calendar::DELETED, 1u8);
            push_instance_id(&mut exception, original, ctx);
        } else {
            push_instance_id(&mut exception, original, ctx);
            View::read(calendar, id, ctx)?.push(Some(series), instance, ctx, &mut exception);
        }
        found.push((original, exception));
    }
    for original in exdates(calendar, master, ctx) {
        if found.iter().any(|(at, _)| *at == original) || found.len() >= MAX_ITEMS {
            continue;
        }
        let mut exception = Element::new(calendar::EXCEPTION);
        push_number(&mut exception, calendar::DELETED, 1u8);
        push_instance_id(&mut exception, original, ctx);
        found.push((original, exception));
    }
    if found.is_empty() {
        return Ok(None);
    }
    found.sort_by_key(|(original, _)| *original);
    let mut list = Element::new(calendar::EXCEPTIONS);
    for (_, exception) in found {
        list.push(exception);
    }
    Ok(Some(list))
}

/// `ExceptionStartTime` in 14.1, `airsyncbase:InstanceId` in 16.1
/// ([MS-ASCAL] 2.2.2.21).
fn push_instance_id(exception: &mut Element, original: NaiveDateTime, ctx: &Context<'_>) {
    let tag = if ctx.version.is_16() {
        airsyncbase::INSTANCE_ID
    } else {
        calendar::EXCEPTION_START_TIME
    };
    exception.push(Element::text(tag, dates::compact(original)));
}

/// Applies a 14.1 `Exceptions` element: exceptions it lists are deleted
/// or changed, the others stay ([MS-ASCAL] 3.2.5.3.1).
pub(super) fn apply(
    calendar: &mut Calendar,
    master: u32,
    element: &Element,
    ctx: &Context<'_>,
) -> Result<()> {
    let exceptions: Vec<&Element> = element.children_of(calendar::EXCEPTION).collect();
    if exceptions.len() > MAX_ITEMS {
        return Err(ItemError::TooLarge("too many exceptions"));
    }
    for exception in exceptions {
        let original = exception
            .child(calendar::EXCEPTION_START_TIME)
            .or_else(|| exception.child(airsyncbase::INSTANCE_ID))
            .ok_or_else(|| ItemError::invalid(calendar::EXCEPTION, "no ExceptionStartTime"))
            .and_then(dates::parse_element)?;
        let deleted = exception
            .child(calendar::DELETED)
            .filter(|deleted| !deleted.as_bytes().is_empty())
            .map(flag)
            .transpose()?
            .unwrap_or(false);
        if deleted {
            delete(calendar, master, original, ctx)?;
        } else {
            change(calendar, master, original, exception, ctx)?;
        }
    }
    Ok(())
}

/// Deletes one instance: its override goes and an `EXDATE` is added.
pub(super) fn delete(
    calendar: &mut Calendar,
    master: u32,
    original: NaiveDateTime,
    ctx: &Context<'_>,
) -> Result<()> {
    let found: Vec<u32> = overrides(calendar, ctx)
        .into_iter()
        .filter(|(_, at)| *at == original)
        .map(|(id, _)| id)
        .collect();
    calendar.remove(&found);
    let master = calendar
        .master(&ICalendarComponentType::VEvent)
        .filter(|_| !found.is_empty())
        .unwrap_or(master);
    if exdates(calendar, master, ctx).contains(&original) {
        return Ok(());
    }
    let times = Times::read(calendar, master, ctx)?;
    let entry = ical::time_entry(
        ICalendarProperty::Exdate,
        times.ical(original, ctx),
        times.zone.tzid(),
    );
    calendar.component_mut(master).entries.push(entry);
    Ok(())
}

/// Changes one instance: its override, made from the series if there is
/// none, gets the exception's elements; an `EXDATE` for it goes.
pub(super) fn change(
    calendar: &mut Calendar,
    master: u32,
    original: NaiveDateTime,
    exception: &Element,
    ctx: &Context<'_>,
) -> Result<()> {
    let series = Times::read(calendar, master, ctx)?;
    remove_exdate(calendar, master, original, ctx);
    let id = match overrides(calendar, ctx)
        .into_iter()
        .find(|(_, at)| *at == original)
    {
        Some((id, _)) => id,
        None => {
            let id = calendar.duplicate(master);
            let instance = calendar.component_mut(id);
            for name in [
                ICalendarProperty::Rrule,
                ICalendarProperty::Rdate,
                ICalendarProperty::Exdate,
                ICalendarProperty::Exrule,
            ] {
                ical::remove(instance, &name);
            }
            let tzid = series.zone.tzid();
            ical::replace(
                instance,
                ical::time_entry(
                    ICalendarProperty::RecurrenceId,
                    series.ical(original, ctx),
                    tzid,
                ),
            );
            event::set_time(
                calendar,
                id,
                ICalendarProperty::Dtstart,
                &series,
                original,
                ctx,
            );
            let end = original + (series.end - series.start);
            event::set_time(calendar, id, ICalendarProperty::Dtend, &series, end, ctx);
            id
        }
    };
    // An exception's day or time can move; it stays all-day or not as the
    // series is ([MS-ASCAL] 2.2.2.1).
    let times = Times {
        start: original,
        end: original,
        ..series
    };
    if let Some(start) = event::sent_time(exception, calendar::START_TIME)? {
        event::set_time(calendar, id, ICalendarProperty::Dtstart, &times, start, ctx);
    }
    if let Some(end) = event::sent_time(exception, calendar::END_TIME)? {
        event::set_time(calendar, id, ICalendarProperty::Dtend, &times, end, ctx);
    }
    event::apply(calendar, id, exception, Omitted::Kept, ctx)?;
    let stamp = crate::items::dates::IcalTime::Utc(ctx.now.naive_utc());
    ical::replace(
        calendar.component_mut(id),
        ical::time_entry(ICalendarProperty::Dtstamp, stamp, None),
    );
    Ok(())
}

/// The override of the instance that originally started at `original`.
pub(super) fn find(calendar: &Calendar, original: NaiveDateTime, ctx: &Context<'_>) -> Option<u32> {
    overrides(calendar, ctx)
        .into_iter()
        .find(|(_, at)| *at == original)
        .map(|(id, _)| id)
}

/// Removes the `EXDATE` for an instance being brought back.
fn remove_exdate(calendar: &mut Calendar, master: u32, original: NaiveDateTime, ctx: &Context<'_>) {
    if !exdates(calendar, master, ctx).contains(&original) {
        return;
    }
    let zones: Vec<Option<Zone>> = calendar
        .component(master)
        .entries
        .iter()
        .map(|entry| {
            (entry.name == ICalendarProperty::Exdate).then(|| {
                let first = entry
                    .values
                    .first()
                    .and_then(ICalendarValue::as_partial_date_time)
                    .and_then(IcalTime::read)
                    .unwrap_or(IcalTime::Utc(original));
                Zone::of_time(calendar, first, entry.tz_id(), ctx.zone)
            })
        })
        .collect();
    let event = calendar.component_mut(master);
    for (entry, zone) in event.entries.iter_mut().zip(&zones) {
        if let Some(zone) = zone {
            entry.values.retain(|value| {
                value
                    .as_partial_date_time()
                    .and_then(IcalTime::read)
                    .is_none_or(|time| event::as_sent(zone, time, ctx) != original)
            });
        }
    }
    event
        .entries
        .retain(|entry| entry.name != ICalendarProperty::Exdate || !entry.values.is_empty());
}

/// Removes every exception of the series: its overrides and `EXDATE`s.
/// 16.1 does this when a series' pattern or times change ([MS-ASCAL]
/// 2.2.2.22).
pub(super) fn clear(calendar: &mut Calendar, master: u32) {
    ical::remove(calendar.component_mut(master), &ICalendarProperty::Exdate);
    let overrides = calendar.overrides(&ICalendarComponentType::VEvent);
    calendar.remove(&overrides);
}
