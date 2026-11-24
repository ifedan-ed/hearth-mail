/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! A thin layer over calcard's iCalendar model for editing one item in
//! place.
//!
//! calcard keeps a calendar as a flat list of components, the VCALENDAR
//! first, each listing its children by index. [`Calendar`] adds and removes
//! components without breaking those indexes (calcard's writer trusts
//! them), and the free functions read and replace properties so that a
//! property whose value did not change keeps its line as it was.

use super::{ItemError, MAX_TEXT, Result, dates::IcalTime};
use calcard::{
    Entry, Parser,
    icalendar::{
        ICalendar, ICalendarComponent, ICalendarComponentType, ICalendarEntry,
        ICalendarParameterName, ICalendarParameterValue, ICalendarProperty, ICalendarValue,
        ICalendarValueType,
    },
};

/// What new calendars say made them.
const PRODID: &str = "-//Hearth Mail//ActiveSync//EN";

/// One iCalendar object being read or edited.
#[derive(Debug, Clone)]
pub(crate) struct Calendar {
    pub(crate) ical: ICalendar,
}

impl Calendar {
    /// Reads a stored object. It must be a VCALENDAR no larger than
    /// [`MAX_TEXT`].
    pub(crate) fn parse(text: &str) -> Result<Calendar> {
        if text.len() > MAX_TEXT {
            return Err(ItemError::TooLarge("iCalendar text"));
        }
        match Parser::new(text).entry() {
            Entry::ICalendar(ical)
                if ical.components.first().is_some_and(|root| {
                    root.component_type == ICalendarComponentType::VCalendar
                }) =>
            {
                Ok(Calendar { ical })
            }
            _ => Err(ItemError::Unreadable("not an iCalendar object")),
        }
    }

    /// An empty VCALENDAR.
    pub(crate) fn new() -> Calendar {
        let mut root = ICalendarComponent::new(ICalendarComponentType::VCalendar);
        root.entries
            .push(text_entry(ICalendarProperty::Version, "2.0"));
        root.entries
            .push(text_entry(ICalendarProperty::Prodid, PRODID));
        Calendar {
            ical: ICalendar {
                components: vec![root],
            },
        }
    }

    /// The text, CRLF line endings, folded.
    pub(crate) fn write(&self) -> String {
        self.ical.to_string()
    }

    pub(crate) fn component(&self, id: u32) -> &ICalendarComponent {
        &self.ical.components[id as usize]
    }

    pub(crate) fn component_mut(&mut self, id: u32) -> &mut ICalendarComponent {
        &mut self.ical.components[id as usize]
    }

    /// The ids of the VCALENDAR's children of `kind`, in order.
    pub(crate) fn top_level(&self, kind: &ICalendarComponentType) -> Vec<u32> {
        self.children(0, kind)
    }

    /// The ids of `parent`'s children of `kind`, in order.
    pub(crate) fn children(&self, parent: u32, kind: &ICalendarComponentType) -> Vec<u32> {
        self.ical
            .components
            .get(parent as usize)
            .map(|component| {
                component
                    .component_ids
                    .iter()
                    .copied()
                    .filter(|id| {
                        self.ical
                            .components
                            .get(*id as usize)
                            .is_some_and(|child| &child.component_type == kind)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The main component of an item: the first of `kind` without a
    /// `RECURRENCE-ID`, or failing that the first of `kind` (an invitation
    /// to one instance of a series).
    pub(crate) fn master(&self, kind: &ICalendarComponentType) -> Option<u32> {
        let ids = self.top_level(kind);
        ids.iter()
            .copied()
            .find(|id| {
                !self
                    .component(*id)
                    .has_property(&ICalendarProperty::RecurrenceId)
            })
            .or_else(|| ids.first().copied())
    }

    /// The components of `kind` that override one instance of the series
    /// (they have a `RECURRENCE-ID`).
    pub(crate) fn overrides(&self, kind: &ICalendarComponentType) -> Vec<u32> {
        self.top_level(kind)
            .into_iter()
            .filter(|id| {
                self.component(*id)
                    .has_property(&ICalendarProperty::RecurrenceId)
            })
            .collect()
    }

    /// Adds `component` as the last child of `parent` and returns its id.
    pub(crate) fn add(&mut self, parent: u32, component: ICalendarComponent) -> u32 {
        let id = self.ical.components.len() as u32;
        self.ical.components.push(component);
        self.ical.components[parent as usize].component_ids.push(id);
        id
    }

    /// Adds a copy of component `source` (with its children, such as
    /// alarms) under the VCALENDAR and returns the copy's id.
    pub(crate) fn duplicate(&mut self, source: u32) -> u32 {
        let mut copy = self.component(source).clone();
        let children = std::mem::take(&mut copy.component_ids);
        let id = self.add(0, copy);
        for child in children {
            self.duplicate_under(child, id);
        }
        id
    }

    fn duplicate_under(&mut self, source: u32, parent: u32) {
        let mut copy = self.component(source).clone();
        let children = std::mem::take(&mut copy.component_ids);
        let id = self.add(parent, copy);
        for child in children {
            self.duplicate_under(child, id);
        }
    }

    /// Removes components and their children. Ids of the others change.
    pub(crate) fn remove(&mut self, ids: &[u32]) {
        if !ids.is_empty() {
            self.ical.remove_component_ids(ids);
        }
    }
}

/// A new, unique `UID` for an item a device added without one (16.1
/// devices never send one, [MS-ASCAL] 2.2.2.46): 128 bits from the
/// process's random hash keys, a counter and the time, as a UUID-shaped
/// string. Not a v4 UUID, but as unique, without a random-number crate.
pub(crate) fn new_uid(now: chrono::DateTime<chrono::Utc>) -> String {
    use std::{
        hash::{BuildHasher, Hasher},
        sync::atomic::{AtomicU64, Ordering},
    };
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    static KEYS: std::sync::OnceLock<std::collections::hash_map::RandomState> =
        std::sync::OnceLock::new();
    let keys = KEYS.get_or_init(Default::default);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let half = |salt: u64| {
        let mut hasher = keys.build_hasher();
        hasher.write_u64(salt);
        hasher.write_u64(count);
        hasher.write_i64(now.timestamp_nanos_opt().unwrap_or_default());
        hasher.finish()
    };
    let (high, low) = (half(1), half(2));
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        high >> 32,
        (high >> 16) & 0xffff,
        high & 0xffff,
        low >> 48,
        low & 0xffff_ffff_ffff
    )
}

/// An entry with one text value.
pub(crate) fn text_entry(name: ICalendarProperty, value: &str) -> ICalendarEntry {
    ICalendarEntry::new(name).with_value(value.to_string())
}

/// The first entry named `name`.
pub(crate) fn first<'x>(
    component: &'x ICalendarComponent,
    name: &ICalendarProperty,
) -> Option<&'x ICalendarEntry> {
    component.entries.iter().find(|entry| &entry.name == name)
}

/// The first text value of the first entry named `name`.
pub(crate) fn text<'x>(
    component: &'x ICalendarComponent,
    name: &ICalendarProperty,
) -> Option<&'x str> {
    first(component, name)
        .and_then(|entry| entry.values.first())
        .and_then(ICalendarValue::as_text)
}

/// The text of an `X-` property.
pub(crate) fn x_text<'x>(component: &'x ICalendarComponent, name: &str) -> Option<&'x str> {
    component
        .entries
        .iter()
        .find(|entry| matches!(&entry.name, ICalendarProperty::Other(other) if other.eq_ignore_ascii_case(name)))
        .and_then(|entry| entry.values.first())
        .and_then(ICalendarValue::as_text)
}

/// An `X-` property's name as calcard keeps it.
pub(crate) fn x_name(name: &str) -> ICalendarProperty {
    ICalendarProperty::Other(name.to_string())
}

/// Replaces every entry named like `new` with `new`, at the place of the
/// first, unless the first already has `new`'s values: then that one stays
/// as it is, parameters included, and only the others go.
pub(crate) fn replace(component: &mut ICalendarComponent, new: ICalendarEntry) {
    let name = new.name.clone();
    let position = component
        .entries
        .iter()
        .position(|entry| entry.name == name);
    match position {
        Some(at) if component.entries[at].values == new.values => {
            let mut index = 0;
            component.entries.retain(|entry| {
                let keep = entry.name != name || index == at;
                index += 1;
                keep
            });
        }
        Some(at) => {
            component.entries[at] = new;
            let mut index = 0;
            component.entries.retain(|entry| {
                let keep = entry.name != name || index == at;
                index += 1;
                keep
            });
        }
        None => component.entries.push(new),
    }
}

/// Removes every entry named `name`.
pub(crate) fn remove(component: &mut ICalendarComponent, name: &ICalendarProperty) {
    component.entries.retain(|entry| &entry.name != name);
}

/// Sets a text property, or removes it for `None`. A changed value keeps
/// the old line's parameters (`LANGUAGE`, say) except `ALTREP`, which
/// points at the old text.
pub(crate) fn set_text(
    component: &mut ICalendarComponent,
    name: ICalendarProperty,
    value: Option<&str>,
) {
    let Some(value) = value else {
        remove(component, &name);
        return;
    };
    let params = first(component, &name)
        .map(|entry| {
            entry
                .params
                .iter()
                .filter(|param| param.name != ICalendarParameterName::Altrep)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    replace(component, text_entry(name, value).with_params(params));
}

/// Every value of every `CATEGORIES` line.
pub(crate) fn categories(component: &ICalendarComponent) -> Vec<&str> {
    component
        .entries
        .iter()
        .filter(|entry| entry.name == ICalendarProperty::Categories)
        .flat_map(|entry| entry.values.iter().filter_map(ICalendarValue::as_text))
        .filter(|value| !value.is_empty())
        .collect()
}

/// Sets the categories, as one line; leaves the lines alone when they
/// already hold these values in this order.
pub(crate) fn set_categories(component: &mut ICalendarComponent, values: &[&str]) {
    if categories(component) == values {
        return;
    }
    if values.is_empty() {
        remove(component, &ICalendarProperty::Categories);
    } else {
        replace(
            component,
            ICalendarEntry::new(ICalendarProperty::Categories).with_values(
                values
                    .iter()
                    .map(|value| ICalendarValue::Text(value.to_string()))
                    .collect(),
            ),
        );
    }
}

/// A date or date-time property: its time and its `TZID`, if any.
pub(crate) fn time<'x>(
    component: &'x ICalendarComponent,
    name: &ICalendarProperty,
) -> Option<(IcalTime, Option<&'x str>)> {
    let entry = first(component, name)?;
    let value = entry.values.first()?.as_partial_date_time()?;
    Some((IcalTime::read(value)?, entry.tz_id()))
}

/// A date or date-time entry, with `VALUE=DATE` for dates and `TZID` for
/// local times in a named zone.
pub(crate) fn time_entry(
    name: ICalendarProperty,
    time: IcalTime,
    tzid: Option<&str>,
) -> ICalendarEntry {
    let mut entry = ICalendarEntry::new(name);
    match time {
        IcalTime::Date(_) => entry.add_param(calcard::icalendar::ICalendarParameter::value(
            ICalendarParameterValue::Value(ICalendarValueType::Date),
        )),
        IcalTime::Local(_) => {
            if let Some(tzid) = tzid {
                entry.add_param(calcard::icalendar::ICalendarParameter::tzid(
                    tzid.to_string(),
                ));
            }
        }
        IcalTime::Utc(_) => {}
    }
    entry.with_value(ICalendarValue::PartialDateTime(Box::new(time.write())))
}

/// An integer property's value.
pub(crate) fn integer(component: &ICalendarComponent, name: &ICalendarProperty) -> Option<i64> {
    first(component, name)
        .and_then(|entry| entry.values.first())
        .and_then(ICalendarValue::as_integer)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVENT: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\n\
        BEGIN:VEVENT\r\nUID:a\r\nSUMMARY;LANGUAGE=de:Mittag\r\nX-KEEP:1\r\n\
        BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT5M\r\nEND:VALARM\r\nEND:VEVENT\r\n\
        BEGIN:VEVENT\r\nUID:a\r\nRECURRENCE-ID:20261124T100000Z\r\nEND:VEVENT\r\n\
        END:VCALENDAR\r\n";

    #[test]
    fn components_are_found_added_and_removed() {
        let mut calendar = Calendar::parse(EVENT).unwrap();
        let master = calendar.master(&ICalendarComponentType::VEvent).unwrap();
        assert_eq!(calendar.overrides(&ICalendarComponentType::VEvent).len(), 1);
        let copy = calendar.duplicate(master);
        assert_eq!(
            calendar
                .children(copy, &ICalendarComponentType::VAlarm)
                .len(),
            1
        );
        assert_eq!(calendar.top_level(&ICalendarComponentType::VEvent).len(), 3);
        calendar.remove(&calendar.overrides(&ICalendarComponentType::VEvent));
        let text = calendar.write();
        assert_eq!(text.matches("BEGIN:VEVENT").count(), 2);
        assert_eq!(text.matches("BEGIN:VALARM").count(), 2);
        assert!(!text.contains("RECURRENCE-ID"));
    }

    #[test]
    fn unchanged_values_keep_their_lines() {
        let mut calendar = Calendar::parse(EVENT).unwrap();
        let master = calendar.master(&ICalendarComponentType::VEvent).unwrap();
        let event = calendar.component_mut(master);
        set_text(event, ICalendarProperty::Summary, Some("Mittag"));
        assert!(calendar.write().contains("SUMMARY;LANGUAGE=de:Mittag"));
        let event = calendar.component_mut(master);
        set_text(event, ICalendarProperty::Summary, Some("Lunch"));
        assert!(calendar.write().contains("SUMMARY;LANGUAGE=de:Lunch"));
        let event = calendar.component_mut(master);
        set_text(event, ICalendarProperty::Summary, None);
        let text = calendar.write();
        assert!(!text.contains("SUMMARY"));
        assert!(text.contains("X-KEEP:1"));
    }

    #[test]
    fn uids_are_unique() {
        let now = chrono::Utc::now();
        let first = new_uid(now);
        assert_eq!(first.len(), 36);
        assert_ne!(first, new_uid(now));
    }

    #[test]
    fn garbage_is_refused() {
        assert!(Calendar::parse("hello").is_err());
        assert!(Calendar::parse("BEGIN:VCARD\r\nEND:VCARD\r\n").is_err());
        assert!(Calendar::parse(&"X".repeat(MAX_TEXT + 1)).is_err());
    }
}
