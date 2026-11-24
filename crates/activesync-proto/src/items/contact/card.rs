/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! A thin layer over calcard's vCard model: reading a card (3.0 or 4.0)
//! with a size cap, writing it back in the version it came in (3.0 for new
//! cards, which is what Nextcloud keeps), and the small helpers the
//! contact mapping needs for types and structured values.

use crate::items::{ItemError, MAX_TEXT, Result};
use calcard::vcard::{
    VCard, VCardEntry, VCardParameter, VCardParameterName, VCardProperty, VCardValue, VCardVersion,
};

/// One vCard being read or edited.
#[derive(Debug, Clone)]
pub(super) struct Card {
    pub(super) vcard: VCard,
    /// The version written back: 4.0 only when the card was 4.0.
    pub(super) version: VCardVersion,
}

impl Card {
    /// Reads a stored card no larger than [`MAX_TEXT`].
    pub(super) fn parse(text: &str) -> Result<Card> {
        if text.len() > MAX_TEXT {
            return Err(ItemError::TooLarge("vCard text"));
        }
        let vcard = VCard::parse(text).map_err(|_| ItemError::Unreadable("not a vCard"))?;
        let version = match vcard.version() {
            Some(VCardVersion::V4_0) => VCardVersion::V4_0,
            _ => VCardVersion::V3_0,
        };
        Ok(Card { vcard, version })
    }

    /// A new, empty 3.0 card.
    pub(super) fn new() -> Card {
        Card {
            vcard: VCard::default(),
            version: VCardVersion::V3_0,
        }
    }

    pub(super) fn is_v4(&self) -> bool {
        self.version == VCardVersion::V4_0
    }

    /// The text, in the card's version.
    pub(super) fn write(&self) -> String {
        let mut out = String::with_capacity(512);
        // Writing into a String cannot fail.
        let _ = self.vcard.write_to(&mut out, self.version);
        out
    }

    /// The indexes of the entries named `name`, in order.
    pub(super) fn indexes(&self, name: &VCardProperty) -> Vec<usize> {
        self.vcard
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| &entry.name == name)
            .map(|(index, _)| index)
            .collect()
    }

    /// The first entry named `name`.
    pub(super) fn first(&self, name: &VCardProperty) -> Option<&VCardEntry> {
        self.vcard.entries.iter().find(|entry| &entry.name == name)
    }

    /// Replaces the first entry named `name` (keeping its group and
    /// parameters) with these values, or adds one; `None` removes them
    /// all.
    pub(super) fn set(&mut self, name: VCardProperty, values: Option<Vec<VCardValue>>) {
        let Some(values) = values else {
            self.vcard.entries.retain(|entry| entry.name != name);
            return;
        };
        match self
            .vcard
            .entries
            .iter_mut()
            .find(|entry| entry.name == name)
        {
            Some(entry) if entry.values == values => {}
            Some(entry) => entry.values = values,
            None => self
                .vcard
                .entries
                .push(VCardEntry::new(name).with_values(values)),
        }
    }

    /// The text of the first entry named `name`.
    pub(super) fn text(&self, name: &VCardProperty) -> Option<&str> {
        self.first(name).and_then(first_text)
    }

    /// Sets a one-value text property, or removes it.
    pub(super) fn set_text(&mut self, name: VCardProperty, value: Option<&str>) {
        self.set(
            name,
            value.map(|value| vec![VCardValue::Text(value.to_string())]),
        );
    }
}

/// The first value of an entry as text; a URI value counts.
pub(super) fn first_text(entry: &VCardEntry) -> Option<&str> {
    entry.values.first().and_then(VCardValue::as_text)
}

/// The components of a structured value (`N`, `ADR`, `ORG`), each as
/// written, `n` of them at least.
pub(super) fn components(entry: &VCardEntry, n: usize) -> Vec<String> {
    let mut parts: Vec<String> = entry
        .values
        .iter()
        .map(|value| match value {
            VCardValue::Component(list) => list.join(","),
            value => value.as_text().unwrap_or_default().to_string(),
        })
        .collect();
    if parts.len() < n {
        parts.resize(n, String::new());
    }
    parts
}

/// The `TYPE` values of an entry, upper case. vCard 3 cards from phones
/// write some as bare parameters (`TEL;CELL:`), which calcard keeps as
/// parameters without a value: those count too.
pub(super) fn types(entry: &VCardEntry) -> Vec<String> {
    entry
        .params
        .iter()
        .filter_map(|param| match &param.name {
            VCardParameterName::Type => param.value.as_text().map(str::to_ascii_uppercase),
            VCardParameterName::Other(name) if param.value.as_text().is_none_or(str::is_empty) => {
                Some(name.to_ascii_uppercase())
            }
            _ => None,
        })
        .flat_map(|value| {
            value
                .split(',')
                .map(|part| part.trim().to_string())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// A `TYPE` parameter.
pub(super) fn type_param(value: &str) -> VCardParameter {
    VCardParameter::typ(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cards_keep_their_version() {
        let three = Card::parse("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:A\r\nTEL;TYPE=WORK,VOICE:1\r\nTEL;CELL:2\r\nEND:VCARD\r\n").unwrap();
        assert!(!three.is_v4());
        assert!(three.write().contains("VERSION:3.0"));
        let tels = three.indexes(&VCardProperty::Tel);
        assert_eq!(types(&three.vcard.entries[tels[0]]), ["WORK", "VOICE"]);
        assert_eq!(types(&three.vcard.entries[tels[1]]), ["CELL"]);
        let four = Card::parse("BEGIN:VCARD\r\nVERSION:4.0\r\nFN:A\r\nEND:VCARD\r\n").unwrap();
        assert!(four.write().contains("VERSION:4.0"));
        assert!(Card::new().write().contains("VERSION:3.0"));
        assert!(Card::parse("BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n").is_err());
        assert!(Card::parse("nothing").is_err());
    }
}
