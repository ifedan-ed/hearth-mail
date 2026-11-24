/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The Contact class ([MS-ASCNTC], with Contacts2) and vCard (RFC 2426 and
//! RFC 6350). Cards are read in 3.0 and 4.0 and written back in the
//! version they came in; new cards are 3.0, which Nextcloud keeps.
//!
//! | ActiveSync | vCard |
//! |---|---|
//! | `LastName`, `FirstName`, `MiddleName`, `Title`, `Suffix` | `N` (and `FN` made from it) |
//! | `CompanyName`, `Department` | `ORG` |
//! | `JobTitle` | `TITLE` |
//! | `contacts2:NickName` | `NICKNAME` |
//! | `Email1Address` to `Email3Address` | the first three `EMAIL`s |
//! | the phone numbers | `TEL` by type (see [`slots`]) |
//! | `Business`/`Home`/`OtherAddress*` | `ADR;TYPE=WORK`, `HOME`, and another |
//! | `Birthday`, `Anniversary` | `BDAY`, `ANNIVERSARY` (`X-ANNIVERSARY` in 3.0) |
//! | `WebPage` | `URL` |
//! | `airsyncbase:Body` | `NOTE` |
//! | `Categories` | `CATEGORIES` |
//! | `contacts2:IMAddress` to `IMAddress3` | the first three `IMPP`s |
//! | `Picture` | `PHOTO` |
//! | `Children`, `Spouse`, `AssistantName` | `RELATED;TYPE=child`, `spouse`, `agent` |
//! | `FileAs`, `OfficeLocation`, `contacts2:ManagerName`, `CustomerId`, `GovernmentId`, `AccountName` | `X-ACTIVESYNC-` properties |
//! | `YomiFirstName`, `YomiLastName`, `YomiCompanyName` | Apple's `X-PHONETIC-` properties |
//!
//! A device has one slot for each of these; a card can have more (a fourth
//! email, a third work phone). Lines a slot holds are updated in place,
//! keeping their groups and parameters; lines no slot holds are never
//! touched.

mod card;
mod slots;

use super::{
    Context, ItemError, Omitted, Result,
    body::{self, NewBody, StoredBody},
    dates,
    element::{Field, push_text, texts, trimmed},
};
use crate::wbxml::{
    Element, Tag,
    codepages::{airsync, airsyncbase, contacts, contacts2},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use calcard::{
    common::{Data, IanaParse, PartialDateTime},
    vcard::{VCardEntry, VCardParameter, VCardProperty, VCardValue, VCardValueType},
};
use card::{Card, components, first_text, type_param, types};
use chrono::{Datelike, Duration, NaiveDate};
use slots::{Address, AddressKind};

/// The largest `Picture` accepted, in base64 characters ([MS-ASCNTC]
/// 2.2.2.58: 48 KB).
const MAX_PICTURE: usize = 48 * 1024;

/// The simple text fields that are one vCard property each.
const TEXTS: &[(Tag, &str)] = &[
    (contacts::JOB_TITLE, "TITLE"),
    (contacts2::NICK_NAME, "NICKNAME"),
    (contacts::WEB_PAGE, "URL"),
    (contacts::FILE_AS, "X-ACTIVESYNC-FILEAS"),
    (contacts::OFFICE_LOCATION, "X-ACTIVESYNC-OFFICELOCATION"),
    (contacts2::MANAGER_NAME, "X-ACTIVESYNC-MANAGERNAME"),
    (contacts2::CUSTOMER_ID, "X-ACTIVESYNC-CUSTOMERID"),
    (contacts2::GOVERNMENT_ID, "X-ACTIVESYNC-GOVERNMENTID"),
    (contacts2::ACCOUNT_NAME, "X-ACTIVESYNC-ACCOUNTNAME"),
    (contacts::YOMI_FIRST_NAME, "X-PHONETIC-FIRST-NAME"),
    (contacts::YOMI_LAST_NAME, "X-PHONETIC-LAST-NAME"),
    (contacts::YOMI_COMPANY_NAME, "X-PHONETIC-ORG"),
];

/// `N`'s components ActiveSync has: family, given, additional, prefix,
/// suffix.
const NAME: [Tag; 5] = [
    contacts::LAST_NAME,
    contacts::FIRST_NAME,
    contacts::MIDDLE_NAME,
    contacts::TITLE,
    contacts::SUFFIX,
];

/// `ORG`'s components: the organization and its unit.
const ORG: [Tag; 2] = [contacts::COMPANY_NAME, contacts::DEPARTMENT];

/// The email slots, in order.
const EMAILS: [Tag; 3] = [
    contacts::EMAIL1_ADDRESS,
    contacts::EMAIL2_ADDRESS,
    contacts::EMAIL3_ADDRESS,
];

/// The instant-messaging slots, in order.
const IMS: [Tag; 3] = [
    contacts2::IM_ADDRESS,
    contacts2::IM_ADDRESS2,
    contacts2::IM_ADDRESS3,
];

/// Writes a Contact class `ApplicationData` into the contact's vCard
/// (`existing`, or a new card for an `Add`) and returns the new text.
pub fn to_vcard(
    data: &Element,
    existing: Option<&str>,
    omitted: Omitted<'_>,
    ctx: &Context<'_>,
) -> Result<String> {
    let mut card = match existing {
        Some(text) => Card::parse(text)?,
        None => Card::new(),
    };
    let field = |tag| Field::of(data, tag, omitted);

    let names_changed = write_structured(&mut card, VCardProperty::N, &NAME, data, omitted)?;
    write_structured(&mut card, VCardProperty::Org, &ORG, data, omitted)?;
    for (tag, name) in TEXTS {
        let property = property_named(name);
        match field(*tag) {
            // The filing name devices make themselves ("Last, First") needs
            // no line of its own.
            Field::Set(element)
                if *tag == contacts::FILE_AS
                    && card.text(&property).is_none()
                    && trimmed(element)? == file_as(&card) => {}
            Field::Set(element) => card.set_text(property, Some(trimmed(element)?)),
            Field::Clear => card.set_text(property, None),
            Field::Keep => {}
        }
    }
    let emails = EMAILS
        .iter()
        .map(|tag| {
            Ok((
                field(*tag),
                sent_text(field(*tag))?.map(super::calendar::address_of),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    slots::write_ordered(
        &mut card,
        VCardProperty::Email,
        &emails,
        |stored| stored,
        str::to_string,
    );
    let ims = IMS
        .iter()
        .map(|tag| Ok((field(*tag), sent_text(field(*tag))?)))
        .collect::<Result<Vec<_>>>()?;
    slots::write_ordered(
        &mut card,
        VCardProperty::Impp,
        &ims,
        slots::im_address,
        slots::im_uri,
    );
    slots::write_phones(&mut card, |tag| Ok((field(tag), sent_text(field(tag))?)))?;
    for kind in [AddressKind::Business, AddressKind::Home, AddressKind::Other] {
        slots::write_address(&mut card, kind, data, omitted)?;
    }
    write_date(&mut card, VCardProperty::Bday, field(contacts::BIRTHDAY))?;
    let anniversary = if card.is_v4() {
        VCardProperty::Anniversary
    } else {
        property_named("X-ANNIVERSARY")
    };
    write_date(&mut card, anniversary, field(contacts::ANNIVERSARY))?;
    write_related(&mut card, "SPOUSE", field(contacts::SPOUSE))?;
    write_related(&mut card, "AGENT", field(contacts::ASSISTANT_NAME))?;
    write_children(&mut card, field(contacts::CHILDREN))?;
    match field(contacts::CATEGORIES) {
        Field::Set(element) => {
            let values = texts(element, contacts::CATEGORY)?;
            let current: Vec<&str> = card
                .first(&VCardProperty::Categories)
                .map(|entry| {
                    entry
                        .values
                        .iter()
                        .filter_map(VCardValue::as_text)
                        .collect()
                })
                .unwrap_or_default();
            if current != values {
                card.set(
                    VCardProperty::Categories,
                    Some(
                        values
                            .iter()
                            .map(|value| VCardValue::Text(value.to_string()))
                            .collect(),
                    ),
                );
            }
        }
        Field::Clear => card.set(VCardProperty::Categories, None),
        Field::Keep => {}
    }
    match NewBody::read(data)? {
        NewBody::Set { text, .. } => card.set_text(VCardProperty::Note, Some(&text)),
        NewBody::Clear => card.set_text(VCardProperty::Note, None),
        NewBody::Keep => {}
    }
    write_picture(&mut card, Field::kept_when_absent(data, contacts::PICTURE))?;

    if names_changed || card.text(&VCardProperty::Fn).is_none_or(str::is_empty) {
        let display = display_name(&card);
        card.set_text(VCardProperty::Fn, Some(&display));
    }
    if card.vcard.uid().is_none() {
        let uid = super::ical::new_uid(ctx.now);
        card.set_text(VCardProperty::Uid, Some(&uid));
    }
    card.set_text(
        VCardProperty::Rev,
        Some(&ctx.now.format("%Y%m%dT%H%M%SZ").to_string()),
    );
    Ok(card.write())
}

/// The text of a set field, trimmed; `None` otherwise.
fn sent_text(field: Field<'_>) -> Result<Option<&str>> {
    match field {
        Field::Set(element) => trimmed(element).map(Some),
        _ => Ok(None),
    }
}

/// A property by name, `X-` ones included.
fn property_named(name: &str) -> VCardProperty {
    <VCardProperty as IanaParse>::parse(name.as_bytes())
        .unwrap_or_else(|| VCardProperty::Other(name.to_string()))
}

/// Writes a structured property's components from their fields: set ones
/// change, cleared ones empty, kept ones stay; components ActiveSync does
/// not know (`N`'s second given names beyond these, `ADR`'s PO box) stay.
/// Returns whether anything changed.
fn write_structured(
    card: &mut Card,
    name: VCardProperty,
    tags: &[Tag],
    data: &Element,
    omitted: Omitted<'_>,
) -> Result<bool> {
    let fields: Vec<Field<'_>> = tags
        .iter()
        .map(|tag| Field::of(data, *tag, omitted))
        .collect();
    if fields.iter().all(|field| matches!(field, Field::Keep)) {
        return Ok(false);
    }
    let mut parts = card
        .first(&name)
        .map(|entry| components(entry, tags.len()))
        .unwrap_or_else(|| vec![String::new(); tags.len()]);
    let before = parts.clone();
    for (index, field) in fields.iter().enumerate() {
        match field {
            Field::Set(element) => parts[index] = trimmed(element)?.to_string(),
            Field::Clear => parts[index].clear(),
            Field::Keep => {}
        }
    }
    if parts == before {
        return Ok(false);
    }
    if parts.iter().all(String::is_empty) {
        card.set(name, None);
    } else {
        card.set(
            name,
            Some(parts.into_iter().map(VCardValue::Text).collect()),
        );
    }
    Ok(true)
}

/// `FN` from the name, else the company, else the first address: vCard
/// requires one, and Nextcloud shows it.
fn display_name(card: &Card) -> String {
    let name = card
        .first(&VCardProperty::N)
        .map(|entry| {
            let parts = components(entry, 5);
            [&parts[3], &parts[1], &parts[2], &parts[0], &parts[4]]
                .iter()
                .filter(|part| !part.is_empty())
                .map(|part| part.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    if !name.is_empty() {
        return name;
    }
    card.first(&VCardProperty::Org)
        .map(|entry| components(entry, 1).swap_remove(0))
        .filter(|org| !org.is_empty())
        .or_else(|| card.text(&VCardProperty::Email).map(str::to_string))
        .unwrap_or_default()
}

/// A `dateTime` birthday or anniversary as a date. The time "might be
/// 11:59 and SHOULD be ignored" ([MS-ASCNTC] 2.2.2.6); phones send local
/// midnight in UTC instead, so the nearest midnight is the date.
fn date_of_sent(element: &Element) -> Result<NaiveDate> {
    let time = dates::parse_element(element)?;
    Ok((time + Duration::hours(12)).date())
}

fn write_date(card: &mut Card, name: VCardProperty, field: Field<'_>) -> Result<()> {
    match field {
        Field::Set(element) => {
            let date = date_of_sent(element)?;
            if read_date(card, &name) == Some(date) {
                return Ok(());
            }
            let value = if card.is_v4() {
                VCardValue::PartialDateTime(PartialDateTime {
                    year: u16::try_from(date.year()).ok(),
                    month: u8::try_from(date.month()).ok(),
                    day: u8::try_from(date.day()).ok(),
                    ..Default::default()
                })
            } else {
                VCardValue::Text(date.format("%Y-%m-%d").to_string())
            };
            card.vcard.entries.retain(|entry| entry.name != name);
            let mut entry = VCardEntry::new(name).with_value(value);
            if card.is_v4() {
                entry.add_param(VCardParameter::value(VCardValueType::Date));
            }
            card.vcard.entries.push(entry);
        }
        Field::Clear => card.set(name, None),
        Field::Keep => {}
    }
    Ok(())
}

/// A date property, if it has a full date.
fn read_date(card: &Card, name: &VCardProperty) -> Option<NaiveDate> {
    match card.first(name)?.values.first()? {
        VCardValue::PartialDateTime(date) => NaiveDate::from_ymd_opt(
            i32::from(date.year?),
            u32::from(date.month?),
            u32::from(date.day?),
        ),
        VCardValue::Text(text) => {
            let text = text.trim();
            let date = text.get(..10).unwrap_or(text);
            NaiveDate::parse_from_str(date, "%Y-%m-%d")
                .or_else(|_| NaiveDate::parse_from_str(text.get(..8).unwrap_or(text), "%Y%m%d"))
                .ok()
        }
        _ => None,
    }
}

/// The `RELATED` entries of a type (`SPOUSE`, `AGENT`, `CHILD`).
fn related(card: &Card, kind: &str) -> Vec<usize> {
    card.indexes(&VCardProperty::Related)
        .into_iter()
        .filter(|index| types(&card.vcard.entries[*index]).iter().any(|t| t == kind))
        .collect()
}

fn related_entry(kind: &str, name: &str) -> VCardEntry {
    VCardEntry::new(VCardProperty::Related)
        .with_param(VCardParameter::value(VCardValueType::Text))
        .with_param(type_param(&kind.to_ascii_lowercase()))
        .with_value(VCardValue::Text(name.to_string()))
}

/// `Spouse` and `AssistantName`: the first `RELATED` of the type.
fn write_related(card: &mut Card, kind: &str, field: Field<'_>) -> Result<()> {
    let first = related(card, kind).first().copied();
    match (field, first) {
        (Field::Set(element), Some(index)) => {
            let name = trimmed(element)?;
            let entry = &mut card.vcard.entries[index];
            if first_text(entry) != Some(name) {
                entry.values = vec![VCardValue::Text(name.to_string())];
            }
        }
        (Field::Set(element), None) => {
            let entry = related_entry(kind, trimmed(element)?);
            card.vcard.entries.push(entry);
        }
        (Field::Clear, Some(index)) => {
            card.vcard.entries.remove(index);
        }
        _ => {}
    }
    Ok(())
}

/// `Children`: every `RELATED;TYPE=child`.
fn write_children(card: &mut Card, field: Field<'_>) -> Result<()> {
    let names = match field {
        Field::Set(element) => texts(element, contacts::CHILD)?,
        Field::Clear => Vec::new(),
        Field::Keep => return Ok(()),
    };
    let current: Vec<usize> = related(card, "CHILD");
    let same = current.len() == names.len()
        && current
            .iter()
            .zip(&names)
            .all(|(index, name)| first_text(&card.vcard.entries[*index]) == Some(*name));
    if same {
        return Ok(());
    }
    let mut index = 0;
    card.vcard.entries.retain(|_| {
        let keep = !current.contains(&index);
        index += 1;
        keep
    });
    for name in names {
        card.vcard.entries.push(related_entry("CHILD", name));
    }
    Ok(())
}

/// `Picture`: base64 image data, at most 48 KB; left out it stays, empty
/// it goes ([MS-ASCMD] 2.2.3.24).
fn write_picture(card: &mut Card, field: Field<'_>) -> Result<()> {
    match field {
        Field::Set(element) => {
            let text: String = element
                .as_str()
                .unwrap_or_default()
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            if text.len() > MAX_PICTURE {
                return Err(ItemError::invalid(contacts::PICTURE, "larger than 48 KB"));
            }
            let bytes = STANDARD
                .decode(&text)
                .map_err(|_| ItemError::invalid(contacts::PICTURE, "not base64"))?;
            let current = card
                .first(&VCardProperty::Photo)
                .and_then(|entry| entry.values.first())
                .and_then(|value| value.as_binary().map(|data| data.data.as_slice()));
            if current == Some(bytes.as_slice()) {
                return Ok(());
            }
            let content_type = image_type(&bytes);
            card.vcard
                .entries
                .retain(|entry| entry.name != VCardProperty::Photo);
            card.vcard
                .entries
                .push(
                    VCardEntry::new(VCardProperty::Photo).with_value(VCardValue::Binary(Data {
                        content_type: Some(content_type.to_string()),
                        data: bytes,
                    })),
                );
        }
        Field::Clear => card.set(VCardProperty::Photo, None),
        Field::Keep => {}
    }
    Ok(())
}

/// The media type of an image from its first bytes.
fn image_type(bytes: &[u8]) -> &'static str {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [b'G', b'I', b'F', ..] => "image/gif",
        _ => "image/jpeg",
    }
}

/// The Contact class `ApplicationData` for a vCard.
pub fn from_vcard(text: &str, ctx: &Context<'_>) -> Result<Element> {
    let card = Card::parse(text)?;
    let mut data = Element::new(airsync::APPLICATION_DATA);

    let note = card
        .text(&VCardProperty::Note)
        .filter(|note| !note.is_empty());
    data.push(body::element(
        note.map(|text| StoredBody { text, html: None }),
        &ctx.body,
    ));
    for (property, tags) in [
        (VCardProperty::N, &NAME[..]),
        (VCardProperty::Org, &ORG[..]),
    ] {
        if let Some(entry) = card.first(&property) {
            for (tag, part) in tags.iter().zip(components(entry, tags.len())) {
                push_text(&mut data, *tag, Some(&part));
            }
        }
    }
    for (tag, name) in TEXTS {
        if *tag == contacts::FILE_AS {
            continue;
        }
        push_text(&mut data, *tag, card.text(&property_named(name)));
    }
    push_text(&mut data, contacts::FILE_AS, Some(&file_as(&card)));
    for (tag, index) in EMAILS.iter().zip(card.indexes(&VCardProperty::Email)) {
        push_text(&mut data, *tag, first_text(&card.vcard.entries[index]));
    }
    for (tag, index) in IMS.iter().zip(card.indexes(&VCardProperty::Impp)) {
        push_text(
            &mut data,
            *tag,
            first_text(&card.vcard.entries[index]).map(slots::im_address),
        );
    }
    for (tag, index) in slots::phones(&card) {
        push_text(&mut data, tag, first_text(&card.vcard.entries[index]));
    }
    for kind in [AddressKind::Business, AddressKind::Home, AddressKind::Other] {
        if let Some(address) = Address::read(&card, kind) {
            address.push(kind, &mut data);
        }
    }
    let anniversary = if card.first(&VCardProperty::Anniversary).is_some() {
        VCardProperty::Anniversary
    } else {
        property_named("X-ANNIVERSARY")
    };
    for (tag, name) in [
        (contacts::BIRTHDAY, VCardProperty::Bday),
        (contacts::ANNIVERSARY, anniversary),
    ] {
        if let Some(date) = read_date(&card, &name) {
            data.push(Element::text(
                tag,
                dates::iso(date.and_hms_opt(11, 59, 0).unwrap_or_default()),
            ));
        }
    }
    for (tag, kind) in [
        (contacts::SPOUSE, "SPOUSE"),
        (contacts::ASSISTANT_NAME, "AGENT"),
    ] {
        if let Some(index) = related(&card, kind).first() {
            push_text(&mut data, tag, first_text(&card.vcard.entries[*index]));
        }
    }
    let children: Vec<&str> = related(&card, "CHILD")
        .into_iter()
        .filter_map(|index| first_text(&card.vcard.entries[index]))
        .collect();
    if let Some(list) = super::element::list(contacts::CHILDREN, contacts::CHILD, children) {
        data.push(list);
    }
    let categories: Vec<&str> = card
        .first(&VCardProperty::Categories)
        .map(|entry| {
            entry
                .values
                .iter()
                .filter_map(VCardValue::as_text)
                .collect()
        })
        .unwrap_or_default();
    if let Some(list) = super::element::list(contacts::CATEGORIES, contacts::CATEGORY, categories) {
        data.push(list);
    }
    if let Some(picture) = picture(&card) {
        data.push(Element::text(contacts::PICTURE, picture));
    }
    data.push(Element::text(airsyncbase::NATIVE_BODY_TYPE, "1"));
    Ok(data)
}

/// `FileAs`: the stored one, else "Last, First" as Outlook files people,
/// else the display name.
fn file_as(card: &Card) -> String {
    if let Some(file_as) = card.text(&property_named("X-ACTIVESYNC-FILEAS")) {
        return file_as.to_string();
    }
    if let Some(entry) = card.first(&VCardProperty::N) {
        let parts = components(entry, 2);
        match (parts[0].is_empty(), parts[1].is_empty()) {
            (false, false) => return format!("{}, {}", parts[0], parts[1]),
            (false, true) => return parts[0].clone(),
            (true, false) => return parts[1].clone(),
            (true, true) => {}
        }
    }
    card.text(&VCardProperty::Fn)
        .map(str::to_string)
        .unwrap_or_else(|| display_name(card))
}

/// `PHOTO` as base64, when it is image data no larger than a device takes
/// (a photo by URL, or a larger one, is not sent).
fn picture(card: &Card) -> Option<String> {
    let entry = card.first(&VCardProperty::Photo)?;
    let bytes = match entry.values.first()? {
        VCardValue::Binary(data) => data.data.as_slice(),
        _ => return None,
    };
    (bytes.len().div_ceil(3) * 4 <= MAX_PICTURE).then(|| STANDARD.encode(bytes))
}

/// The `UID` of a vCard; the DAV bridge names new resources by it.
pub fn uid(text: &str) -> Result<Option<String>> {
    Ok(Card::parse(text)?.vcard.uid().map(str::to_string))
}

#[cfg(test)]
mod tests;
