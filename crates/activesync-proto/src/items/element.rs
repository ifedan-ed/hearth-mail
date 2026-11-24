/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Reading the elements of an `ApplicationData` the way every class needs:
//! whether a property is set, cleared or left alone ([`Field`]), and its
//! value as a number, a flag or a list, checked against the
//! specification's ranges.

use super::{ItemError, MAX_ITEMS, Omitted, Result};
use crate::wbxml::{Content, Element, Tag};

/// What a request says about one property.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Field<'x> {
    /// Not in the request and ghosted: the stored value stays.
    Keep,
    /// An empty element, or not in the request and not ghosted: the stored
    /// value goes ([MS-ASCAL] 3.1.5.3.1, [MS-ASCMD] 2.2.3.24).
    Clear,
    /// The element, with a value.
    Set(&'x Element),
}

impl<'x> Field<'x> {
    /// What `data` says about `tag`, `omitted` deciding for an absent one.
    pub(crate) fn of(data: &'x Element, tag: Tag, omitted: Omitted<'_>) -> Self {
        match data.child(tag) {
            Some(element) if has_value(element) => Field::Set(element),
            Some(_) => Field::Clear,
            None if omitted.keeps(tag) => Field::Keep,
            None => Field::Clear,
        }
    }

    /// As [`Field::of`], for the properties a `Change` never clears by
    /// leaving them out: bodies and pictures ([MS-ASCMD] 2.2.3.24).
    pub(crate) fn kept_when_absent(data: &'x Element, tag: Tag) -> Self {
        Self::of(data, tag, Omitted::Kept)
    }
}

/// Whether an element holds anything: an empty tag and an empty string both
/// mean "no value".
fn has_value(element: &Element) -> bool {
    match &element.content {
        Content::Empty => false,
        Content::Text(text) => !text.is_empty(),
        Content::Opaque(bytes) => !bytes.is_empty(),
        Content::Children(_) => true,
    }
}

/// The trimmed text of an element; clients pad values with line breaks in
/// the specifications' own examples.
pub(crate) fn trimmed(element: &Element) -> Result<&str> {
    element
        .as_str()
        .map(str::trim)
        .ok_or_else(|| ItemError::invalid(element.tag, "not text"))
}

/// An unsigned number no larger than `max`.
pub(crate) fn number(element: &Element, max: u32) -> Result<u32> {
    trimmed(element)?
        .parse::<u32>()
        .ok()
        .filter(|value| *value <= max)
        .ok_or_else(|| ItemError::invalid(element.tag, "not a number in range"))
}

/// A number from `allowed`.
pub(crate) fn one_of(element: &Element, allowed: &[u32]) -> Result<u32> {
    let value = number(element, u32::MAX)?;
    if allowed.contains(&value) {
        Ok(value)
    } else {
        Err(ItemError::invalid(element.tag, "value not allowed"))
    }
}

/// An ActiveSync boolean: `0` or `1` ([MS-ASDTYPE] 2.1).
pub(crate) fn flag(element: &Element) -> Result<bool> {
    Ok(one_of(element, &[0, 1])? == 1)
}

/// The child `tag` of `parent` as a number no larger than `max`, if there.
pub(crate) fn child_number(parent: &Element, tag: Tag, max: u32) -> Result<Option<u32>> {
    parent
        .child(tag)
        .filter(|element| has_value(element))
        .map(|element| number(element, max))
        .transpose()
}

/// The texts of the `item` children of a container (`Categories`,
/// `Children`), at most [`MAX_ITEMS`] of them, empty ones skipped.
pub(crate) fn texts(container: &Element, item: Tag) -> Result<Vec<&str>> {
    let mut values = Vec::new();
    for child in container.children_of(item) {
        if values.len() == MAX_ITEMS {
            return Err(ItemError::TooLarge("too many values in a list"));
        }
        if let Some(text) = child.as_str().map(str::trim).filter(|t| !t.is_empty()) {
            values.push(text);
        }
    }
    Ok(values)
}

/// A container of `item` elements, one per value; `None` for no values.
pub(crate) fn list<'v>(
    container: Tag,
    item: Tag,
    values: impl IntoIterator<Item = &'v str>,
) -> Option<Element> {
    let mut element = Element::new(container);
    for value in values {
        element.push(Element::text(item, value));
    }
    (!element.children().is_empty()).then_some(element)
}

/// Adds `<tag>value</tag>` to `parent` when there is a value.
pub(crate) fn push_text(parent: &mut Element, tag: Tag, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        parent.push(Element::text(tag, value));
    }
}

/// Adds `<tag>number</tag>` to `parent`.
pub(crate) fn push_number(parent: &mut Element, tag: Tag, value: impl Into<u64>) {
    parent.push(Element::text(tag, value.into().to_string()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wbxml::codepages::calendar;

    fn data() -> Element {
        Element::new(crate::wbxml::codepages::airsync::APPLICATION_DATA)
            .with(Element::text(calendar::SUBJECT, "Lunch"))
            .with(Element::new(calendar::LOCATION))
            .with(Element::text(calendar::BUSY_STATUS, " 2\n"))
    }

    #[test]
    fn fields_follow_the_ghosting_rules() {
        let data = data();
        assert!(matches!(
            Field::of(&data, calendar::SUBJECT, Omitted::Cleared),
            Field::Set(_)
        ));
        assert!(matches!(
            Field::of(&data, calendar::LOCATION, Omitted::Kept),
            Field::Clear
        ));
        assert!(matches!(
            Field::of(&data, calendar::REMINDER, Omitted::Cleared),
            Field::Clear
        ));
        assert!(matches!(
            Field::of(&data, calendar::REMINDER, Omitted::Kept),
            Field::Keep
        ));
        let managed = [calendar::REMINDER];
        assert!(matches!(
            Field::of(&data, calendar::REMINDER, Omitted::Supported(&managed)),
            Field::Clear
        ));
        assert!(matches!(
            Field::of(&data, calendar::SENSITIVITY, Omitted::Supported(&managed)),
            Field::Keep
        ));
    }

    #[test]
    fn numbers_are_checked() {
        let data = data();
        let busy = data.child(calendar::BUSY_STATUS).unwrap();
        assert_eq!(number(busy, 4), Ok(2));
        assert!(number(busy, 1).is_err());
        assert!(flag(busy).is_err());
        assert!(number(data.child(calendar::SUBJECT).unwrap(), 9).is_err());
    }
}
