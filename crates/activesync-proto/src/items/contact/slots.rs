/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The fixed slots a device has for repeated vCard properties: three
//! emails, three IM addresses, thirteen kinds of phone number and three
//! addresses.
//!
//! Each `TEL` line takes the first free slot its `TYPE`s allow ([`phones`]):
//!
//! | `TYPE` | slot |
//! |---|---|
//! | `FAX` with `WORK`, `FAX` | `BusinessFaxNumber`, `HomeFaxNumber` |
//! | `PAGER`, `CELL`, `CAR` | `PagerNumber`, `MobilePhoneNumber`, `CarPhoneNumber` |
//! | `X-RADIO`, `X-ASSISTANT`, `X-MMS` | `RadioPhoneNumber`, `AssistantPhoneNumber`, `contacts2:MMS` |
//! | `MAIN` (`MAIN-NUMBER`) | `contacts2:CompanyMainPhone` |
//! | `WORK` | `BusinessPhoneNumber`, then `Business2PhoneNumber` |
//! | `HOME` | `HomePhoneNumber`, then `Home2PhoneNumber` |
//! | none of these | the first free of mobile, home, home 2, business, business 2 |
//!
//! Writing changes the line in a slot in place, removes it when the slot
//! is cleared, and adds a line with the slot's types when the slot was
//! empty; lines in no slot stay as they are.

use super::card::{Card, components, first_text, type_param, types};
use crate::items::{
    Omitted, Result,
    element::{Field, push_text, trimmed},
};
use crate::wbxml::{
    Element, Tag,
    codepages::{contacts, contacts2},
};
use calcard::vcard::{VCardEntry, VCardProperty, VCardValue};

const BUSINESS: Tag = contacts::BUSINESS_PHONE_NUMBER;
const BUSINESS2: Tag = contacts::BUSINESS2_PHONE_NUMBER;
const HOME: Tag = contacts::HOME_PHONE_NUMBER;
const HOME2: Tag = contacts::HOME2_PHONE_NUMBER;
const MOBILE: Tag = contacts::MOBILE_PHONE_NUMBER;
const BUSINESS_FAX: Tag = contacts::BUSINESS_FAX_NUMBER;
const HOME_FAX: Tag = contacts::HOME_FAX_NUMBER;

/// Every phone slot, and the `TYPE`s a new line for it gets.
const PHONES: &[(Tag, &[&str])] = &[
    (BUSINESS, &["WORK", "VOICE"]),
    (BUSINESS2, &["WORK", "VOICE"]),
    (HOME, &["HOME", "VOICE"]),
    (HOME2, &["HOME", "VOICE"]),
    (MOBILE, &["CELL"]),
    (BUSINESS_FAX, &["WORK", "FAX"]),
    (HOME_FAX, &["HOME", "FAX"]),
    (contacts::PAGER_NUMBER, &["PAGER"]),
    (contacts::CAR_PHONE_NUMBER, &["CAR"]),
    (contacts::RADIO_PHONE_NUMBER, &["X-RADIO"]),
    (contacts::ASSISTANT_PHONE_NUMBER, &["X-ASSISTANT"]),
    (contacts2::COMPANY_MAIN_PHONE, &["WORK", "MAIN"]),
    (contacts2::MMS, &["X-MMS"]),
];

/// The slots a `TEL` line with these types may take, best first.
fn candidates(types: &[String]) -> &'static [Tag] {
    let has = |wanted: &[&str]| types.iter().any(|t| wanted.contains(&t.as_str()));
    if has(&["FAX"]) {
        if has(&["WORK"]) {
            &[BUSINESS_FAX, HOME_FAX]
        } else {
            &[HOME_FAX, BUSINESS_FAX]
        }
    } else if has(&["PAGER"]) {
        &[contacts::PAGER_NUMBER]
    } else if has(&["CELL", "MOBILE", "IPHONE"]) {
        &[MOBILE]
    } else if has(&["CAR"]) {
        &[contacts::CAR_PHONE_NUMBER]
    } else if has(&["X-RADIO", "RADIO"]) {
        &[contacts::RADIO_PHONE_NUMBER]
    } else if has(&["X-ASSISTANT", "ASSISTANT"]) {
        &[contacts::ASSISTANT_PHONE_NUMBER]
    } else if has(&["MAIN", "MAIN-NUMBER", "X-MAIN"]) {
        &[contacts2::COMPANY_MAIN_PHONE]
    } else if has(&["X-MMS", "MMS"]) {
        &[contacts2::MMS]
    } else if has(&["WORK"]) {
        &[BUSINESS, BUSINESS2]
    } else if has(&["HOME"]) {
        &[HOME, HOME2]
    } else {
        UNTYPED
    }
}

/// The slots of a line with no telling type.
const UNTYPED: &[Tag] = &[MOBILE, HOME, HOME2, BUSINESS, BUSINESS2];

/// Which `TEL` line (by entry index) is in which slot, in the order of the
/// lines. Typed lines are placed first, so a plain `TEL` never takes the
/// mobile slot from a `TEL;TYPE=CELL` further down.
pub(super) fn phones(card: &Card) -> Vec<(Tag, usize)> {
    let lines: Vec<(usize, &'static [Tag])> = card
        .indexes(&VCardProperty::Tel)
        .into_iter()
        .map(|index| (index, candidates(&types(&card.vcard.entries[index]))))
        .collect();
    let mut taken: Vec<(Tag, usize)> = Vec::new();
    for untyped in [false, true] {
        for (index, slots) in &lines {
            if (*slots == UNTYPED) != untyped {
                continue;
            }
            if let Some(tag) = slots
                .iter()
                .find(|tag| !taken.iter().any(|(taken, _)| taken == *tag))
            {
                taken.push((*tag, *index));
            }
        }
    }
    taken.sort_by_key(|(_, index)| *index);
    taken
}

/// Writes the phone slots. `field_of` gives each slot's field and its
/// text.
pub(super) fn write_phones<'x>(
    card: &mut Card,
    field_of: impl Fn(Tag) -> Result<(Field<'x>, Option<&'x str>)>,
) -> Result<()> {
    let assigned = phones(card);
    let mut gone = Vec::new();
    let mut added = Vec::new();
    for (tag, new_types) in PHONES {
        let slot = assigned
            .iter()
            .find(|(taken, _)| taken == tag)
            .map(|(_, index)| *index);
        match (field_of(*tag)?, slot) {
            ((Field::Set(_), Some(number)), Some(index)) => {
                set_value(&mut card.vcard.entries[index], number)
            }
            ((Field::Set(_), Some(number)), None) => {
                let mut entry = VCardEntry::new(VCardProperty::Tel);
                for kind in *new_types {
                    entry.add_param(type_param(kind));
                }
                added.push(entry.with_value(VCardValue::Text(number.to_string())));
            }
            ((Field::Clear, _), Some(index)) => gone.push(index),
            _ => {}
        }
    }
    remove_entries(card, &gone);
    card.vcard.entries.extend(added);
    Ok(())
}

/// Sets an entry's one value, unless it already has it.
fn set_value(entry: &mut VCardEntry, value: &str) {
    if first_text(entry) != Some(value) {
        entry.values = vec![VCardValue::Text(value.to_string())];
    }
}

/// Removes the entries at these indexes.
fn remove_entries(card: &mut Card, indexes: &[usize]) {
    if indexes.is_empty() {
        return;
    }
    let mut index = 0;
    card.vcard.entries.retain(|_| {
        let keep = !indexes.contains(&index);
        index += 1;
        keep
    });
}

/// Writes ordered slots (the three `EMAIL`s or `IMPP`s): slot n is the
/// nth line. `shown` is how a stored value looks to the device, so an
/// unchanged value keeps its line; `stored` is how a new one is written.
pub(super) fn write_ordered(
    card: &mut Card,
    name: VCardProperty,
    fields: &[(Field<'_>, Option<&str>)],
    shown: fn(&str) -> &str,
    stored: fn(&str) -> String,
) {
    let lines = card.indexes(&name);
    let mut gone = Vec::new();
    for (slot, (field, value)) in fields.iter().enumerate() {
        match (field, value, lines.get(slot)) {
            (Field::Set(_), Some(value), Some(index)) => {
                let entry = &mut card.vcard.entries[*index];
                if first_text(entry).map(shown) != Some(*value) {
                    entry.values = vec![VCardValue::Text(stored(value))];
                }
            }
            (Field::Set(_), Some(value), None) => card
                .vcard
                .entries
                .push(VCardEntry::new(name.clone()).with_value(VCardValue::Text(stored(value)))),
            (Field::Clear, _, Some(index)) => gone.push(*index),
            _ => {}
        }
    }
    remove_entries(card, &gone);
}

/// An `IMPP` value as a device shows it: without its scheme
/// (`xmpp:bob@example.com` is `bob@example.com`).
pub(super) fn im_address(uri: &str) -> &str {
    match uri.split_once(':') {
        Some((scheme, rest))
            if !scheme.is_empty()
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) =>
        {
            rest.trim_start_matches('/')
        }
        _ => uri,
    }
}

/// An IM address from a device as an `IMPP` URI: kept when it has a
/// scheme, else `im:` (RFC 3860).
pub(super) fn im_uri(address: &str) -> String {
    if im_address(address) != address {
        address.to_string()
    } else {
        format!("im:{address}")
    }
}

/// The three addresses a contact has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AddressKind {
    Business,
    Home,
    Other,
}

impl AddressKind {
    /// Street, city, state, postal code and country.
    fn tags(self) -> [Tag; 5] {
        match self {
            AddressKind::Business => [
                contacts::BUSINESS_ADDRESS_STREET,
                contacts::BUSINESS_ADDRESS_CITY,
                contacts::BUSINESS_ADDRESS_STATE,
                contacts::BUSINESS_ADDRESS_POSTAL_CODE,
                contacts::BUSINESS_ADDRESS_COUNTRY,
            ],
            AddressKind::Home => [
                contacts::HOME_ADDRESS_STREET,
                contacts::HOME_ADDRESS_CITY,
                contacts::HOME_ADDRESS_STATE,
                contacts::HOME_ADDRESS_POSTAL_CODE,
                contacts::HOME_ADDRESS_COUNTRY,
            ],
            AddressKind::Other => [
                contacts::OTHER_ADDRESS_STREET,
                contacts::OTHER_ADDRESS_CITY,
                contacts::OTHER_ADDRESS_STATE,
                contacts::OTHER_ADDRESS_POSTAL_CODE,
                contacts::OTHER_ADDRESS_COUNTRY,
            ],
        }
    }

    /// The `ADR` line of this kind: the first `WORK`, the first `HOME`, or
    /// the first with neither.
    fn index(self, card: &Card) -> Option<usize> {
        card.indexes(&VCardProperty::Adr).into_iter().find(|index| {
            let kinds = types(&card.vcard.entries[*index]);
            let work = kinds.iter().any(|t| t == "WORK");
            let home = kinds.iter().any(|t| t == "HOME");
            match self {
                AddressKind::Business => work,
                AddressKind::Home => home && !work,
                AddressKind::Other => !work && !home,
            }
        })
    }
}

/// `ADR`'s seven components: PO box, extended, street, locality, region,
/// postal code, country. ActiveSync has the last five.
const ADR_PARTS: usize = 7;
const FIRST_SENT: usize = 2;

/// One address as stored.
pub(super) struct Address {
    parts: Vec<String>,
}

impl Address {
    pub(super) fn read(card: &Card, kind: AddressKind) -> Option<Address> {
        let entry = &card.vcard.entries[kind.index(card)?];
        Some(Address {
            parts: components(entry, ADR_PARTS),
        })
    }

    /// Adds the address's elements to `out`.
    pub(super) fn push(&self, kind: AddressKind, out: &mut Element) {
        for (tag, part) in kind.tags().iter().zip(&self.parts[FIRST_SENT..]) {
            push_text(out, *tag, Some(part));
        }
    }
}

/// Writes one address from its five fields: set parts change, cleared
/// ones empty, kept ones stay; the PO box and extended parts stay; an
/// address left with nothing goes.
pub(super) fn write_address(
    card: &mut Card,
    kind: AddressKind,
    data: &Element,
    omitted: Omitted<'_>,
) -> Result<()> {
    let fields = kind.tags().map(|tag| Field::of(data, tag, omitted));
    if fields.iter().all(|field| matches!(field, Field::Keep)) {
        return Ok(());
    }
    let index = kind.index(card);
    let mut parts = index
        .map(|index| components(&card.vcard.entries[index], ADR_PARTS))
        .unwrap_or_else(|| vec![String::new(); ADR_PARTS]);
    let before = parts.clone();
    for (offset, field) in fields.iter().enumerate() {
        match field {
            Field::Set(element) => parts[FIRST_SENT + offset] = trimmed(element)?.to_string(),
            Field::Clear => parts[FIRST_SENT + offset].clear(),
            Field::Keep => {}
        }
    }
    if parts == before {
        return Ok(());
    }
    let values: Vec<VCardValue> = parts.iter().cloned().map(VCardValue::Text).collect();
    match index {
        Some(index) if parts.iter().all(String::is_empty) => remove_entries(card, &[index]),
        Some(index) => card.vcard.entries[index].values = values,
        None if parts.iter().all(String::is_empty) => {}
        None => {
            let mut entry = VCardEntry::new(VCardProperty::Adr).with_values(values);
            match kind {
                AddressKind::Business => entry.add_param(type_param("WORK")),
                AddressKind::Home => entry.add_param(type_param("HOME")),
                AddressKind::Other => {}
            }
            card.vcard.entries.push(entry);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARD: &str = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:A\r\n\
        TEL;TYPE=WORK,VOICE:1\r\nTEL;TYPE=WORK:2\r\nTEL;TYPE=WORK:3\r\nTEL:4\r\n\
        TEL;TYPE=WORK,FAX:5\r\nTEL;TYPE=CELL:6\r\nitem1.TEL:7\r\nitem1.X-ABLabel:Other\r\nEND:VCARD\r\n";

    #[test]
    fn lines_take_slots_in_order() {
        let card = Card::parse(CARD).unwrap();
        let slots: Vec<(Tag, String)> = phones(&card)
            .into_iter()
            .map(|(tag, index)| {
                (
                    tag,
                    first_text(&card.vcard.entries[index]).unwrap().to_string(),
                )
            })
            .collect();
        assert_eq!(
            slots,
            [
                (BUSINESS, "1".to_string()),
                (BUSINESS2, "2".to_string()),
                (HOME, "4".to_string()),
                (BUSINESS_FAX, "5".to_string()),
                (MOBILE, "6".to_string()),
                (HOME2, "7".to_string()),
            ]
        );
    }

    #[test]
    fn im_addresses_lose_their_scheme() {
        assert_eq!(im_address("xmpp:bob@example.com"), "bob@example.com");
        assert_eq!(im_address("bob@example.com"), "bob@example.com");
        assert_eq!(im_uri("bob@example.com"), "im:bob@example.com");
        assert_eq!(im_uri("sip:bob@example.com"), "sip:bob@example.com");
    }
}
