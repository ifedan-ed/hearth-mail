/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The Notes class ([MS-ASNOTE]) and VJOURNAL (RFC 5545 3.6.3), the form
//! notes take in Nextcloud's calendars and in Hearth's CalDAV.
//!
//! | ActiveSync | iCalendar |
//! |---|---|
//! | `Subject` | `SUMMARY` |
//! | `airsyncbase:Body` | `DESCRIPTION` (and `X-ALT-DESC` for HTML) |
//! | `Categories` | `CATEGORIES` |
//! | `LastModifiedDate` | `LAST-MODIFIED` (set by the server, never by the device) |
//! | `MessageClass` | `X-ACTIVESYNC-MESSAGECLASS` when not plain `IPM.StickyNote` |

use super::{
    Context, ItemError, Omitted, Result,
    body::{self, NewBody, StoredBody},
    calendar::{ALT_DESC, set_alt_desc},
    dates::{self, IcalTime},
    element::{Field, texts, trimmed},
    ical::{self, Calendar},
    timezone::Zone,
};
use crate::wbxml::{
    Element,
    codepages::{airsync, notes},
};
use calcard::icalendar::{ICalendarComponent, ICalendarComponentType, ICalendarProperty};

/// The class every note has ([MS-ASNOTE] 2.2.2.5).
const STICKY_NOTE: &str = "IPM.StickyNote";
/// Where another `IPM.StickyNote.*` class is kept.
const MESSAGE_CLASS: &str = "X-ACTIVESYNC-MESSAGECLASS";

/// Writes a Notes class `ApplicationData` into the note's iCalendar text
/// (`existing`, or a new object for an `Add`) and returns the new text.
/// A `MessageClass` other than `IPM.StickyNote` or `IPM.StickyNote.*` is
/// refused ([MS-ASNOTE] 3.2.5.3.2); `LastModifiedDate` is ignored
/// (3.2.5.3.1).
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
    let id = match calendar.master(&ICalendarComponentType::VJournal) {
        Some(id) => id,
        None if existing.is_some() => return Err(ItemError::Unreadable("no VJOURNAL")),
        None => calendar.add(0, ICalendarComponent::new(ICalendarComponentType::VJournal)),
    };
    let field = |tag| Field::of(data, tag, omitted);
    let journal = calendar.component_mut(id);

    match field(notes::SUBJECT) {
        Field::Set(subject) => {
            ical::set_text(journal, ICalendarProperty::Summary, subject.as_str())
        }
        Field::Clear => ical::remove(journal, &ICalendarProperty::Summary),
        Field::Keep => {}
    }
    match NewBody::read(data)? {
        NewBody::Set { text, html } => {
            ical::set_text(journal, ICalendarProperty::Description, Some(&text));
            set_alt_desc(journal, html.as_deref());
        }
        NewBody::Clear => {
            ical::remove(journal, &ICalendarProperty::Description);
            set_alt_desc(journal, None);
        }
        NewBody::Keep => {}
    }
    match field(notes::CATEGORIES) {
        Field::Set(element) => ical::set_categories(journal, &texts(element, notes::CATEGORY)?),
        Field::Clear => ical::remove(journal, &ICalendarProperty::Categories),
        Field::Keep => {}
    }
    if let Field::Set(element) = field(notes::MESSAGE_CLASS) {
        let class = trimmed(element)?;
        let is_note = class == STICKY_NOTE
            || class
                .strip_prefix(STICKY_NOTE)
                .is_some_and(|rest| rest.starts_with('.') && rest.len() > 1);
        if !is_note {
            return Err(ItemError::invalid(
                notes::MESSAGE_CLASS,
                "not IPM.StickyNote",
            ));
        }
        let name = ical::x_name(MESSAGE_CLASS);
        if class == STICKY_NOTE {
            ical::remove(journal, &name);
        } else {
            ical::replace(journal, ical::text_entry(name, class));
        }
    }

    if journal.uid().is_none() {
        ical::set_text(
            journal,
            ICalendarProperty::Uid,
            Some(&ical::new_uid(ctx.now)),
        );
    }
    let now = IcalTime::Utc(ctx.now.naive_utc());
    // A journal entry is dated; a new note is dated now.
    if !journal.has_property(&ICalendarProperty::Dtstart) {
        ical::replace(
            journal,
            ical::time_entry(ICalendarProperty::Dtstart, now, None),
        );
    }
    ical::replace(
        journal,
        ical::time_entry(ICalendarProperty::Dtstamp, now, None),
    );
    ical::replace(
        journal,
        ical::time_entry(ICalendarProperty::LastModified, now, None),
    );
    Ok(calendar.write())
}

/// The Notes class `ApplicationData` for a journal's iCalendar text.
pub fn from_ical(text: &str, ctx: &Context<'_>) -> Result<Element> {
    let calendar = Calendar::parse(text)?;
    let id = calendar
        .master(&ICalendarComponentType::VJournal)
        .ok_or(ItemError::Unreadable("no VJOURNAL"))?;
    let journal = calendar.component(id);
    let mut data = Element::new(airsync::APPLICATION_DATA);

    let stored = ical::text(journal, &ICalendarProperty::Description)
        .filter(|text| !text.is_empty())
        .map(|text| StoredBody {
            text,
            html: ical::x_text(journal, ALT_DESC),
        });
    data.push(body::element(stored, &ctx.body));
    if let Some(subject) = ical::text(journal, &ICalendarProperty::Summary) {
        data.push(Element::text(notes::SUBJECT, subject));
    }
    data.push(Element::text(
        notes::MESSAGE_CLASS,
        ical::x_text(journal, MESSAGE_CLASS).unwrap_or(STICKY_NOTE),
    ));
    // Required in responses ([MS-ASNOTE] 3.2.5.3.1): the last change, or
    // failing that the entry's stamp or date.
    let modified = [
        ICalendarProperty::LastModified,
        ICalendarProperty::Dtstamp,
        ICalendarProperty::Dtstart,
    ]
    .iter()
    .find_map(|name| ical::time(journal, name))
    .map(|(time, tzid)| Zone::of_time(&calendar, time, tzid, ctx.zone).utc_of(time))
    .unwrap_or_else(|| ctx.now.naive_utc());
    data.push(Element::text(
        notes::LAST_MODIFIED_DATE,
        dates::compact(modified),
    ));
    if let Some(list) = super::element::list(
        notes::CATEGORIES,
        notes::CATEGORY,
        ical::categories(journal),
    ) {
        data.push(list);
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::{
        BodyPreference, BodyType,
        fixtures::{application_data, context},
    };
    use crate::request::Version;
    use crate::wbxml::codepages::airsyncbase;

    /// [MS-ASNOTE] 4: the note the device adds, with an HTML body.
    const ADD: &str = r#"
        <airsyncbase:Body>
          <airsyncbase:Type>2</airsyncbase:Type>
          <airsyncbase:Data>A new note I just created.</airsyncbase:Data>
        </airsyncbase:Body>
        <notes:Categories>
          <notes:Category>Business</notes:Category>
        </notes:Categories>
        <notes:Subject>New note</notes:Subject>
        <notes:MessageClass>IPM.StickyNote</notes:MessageClass>"#;

    /// [MS-ASNOTE] 4: the change, its HTML broken over two lines.
    const CHANGE: &str = r#"
        <airsyncbase:Body>
          <airsyncbase:Type>2</airsyncbase:Type>
          <airsyncbase:Data>&lt;strong&gt;This is my second
    note.&lt;/strong&gt;</airsyncbase:Data>
        </airsyncbase:Body>
        <notes:Categories>
          <notes:Category>Business</notes:Category>
        </notes:Categories>
        <notes:Subject>Second Note</notes:Subject>
        <notes:MessageClass>IPM.StickyNote</notes:MessageClass>"#;

    #[test]
    fn spec_4_notes_both_ways() {
        let ctx = context(Version::V14_1);
        let text = to_ical(&application_data(ADD), None, Omitted::Cleared, &ctx).unwrap();
        for line in [
            "BEGIN:VJOURNAL",
            "SUMMARY:New note",
            "DESCRIPTION:A new note I just created.",
            "X-ALT-DESC;FMTTYPE=text/html:A new note I just created.",
            "CATEGORIES:Business",
            "LAST-MODIFIED:20261123T180300Z",
            "DTSTART:20261123T180300Z",
        ] {
            assert!(text.contains(line), "{line} missing from\n{text}");
        }
        assert!(!text.contains(MESSAGE_CLASS));
        let back = from_ical(&text, &ctx).unwrap();
        assert_eq!(back.child_str(notes::SUBJECT), Some("New note"));
        assert_eq!(back.child_str(notes::MESSAGE_CLASS), Some(STICKY_NOTE));
        assert_eq!(
            back.child_str(notes::LAST_MODIFIED_DATE),
            Some("20261123T180300Z")
        );
        let category = back
            .child(notes::CATEGORIES)
            .unwrap()
            .child_str(notes::CATEGORY);
        assert_eq!(category, Some("Business"));
        let body = back.child(airsyncbase::BODY).unwrap();
        assert_eq!(body.child_str(airsyncbase::TYPE), Some("1"));
        assert_eq!(
            body.child_str(airsyncbase::DATA),
            Some("A new note I just created.")
        );

        // The change, onto a journal Nextcloud wrote, keeps what it had.
        let stored = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VJOURNAL\r\nUID:n1\r\n\
            DTSTART;VALUE=DATE:20261101\r\nSUMMARY:Old\r\nX-NC-KEEP:yes\r\nEND:VJOURNAL\r\nEND:VCALENDAR\r\n";
        let text = to_ical(
            &application_data(CHANGE),
            Some(stored),
            Omitted::Cleared,
            &ctx,
        )
        .unwrap();
        assert!(text.contains("SUMMARY:Second Note"));
        assert!(
            text.contains("DESCRIPTION:This is my second\\n    note."),
            "{text}"
        );
        assert!(text.contains("X-NC-KEEP:yes"));
        assert!(text.contains("DTSTART;VALUE=DATE:20261101"));
        assert!(text.contains("UID:n1"));
        let html = Context {
            body: BodyPreference {
                kind: BodyType::Html,
                truncation: Some(5120),
            },
            ..ctx
        };
        let back = from_ical(&text, &html).unwrap();
        let body = back.child(airsyncbase::BODY).unwrap();
        assert_eq!(
            body.child_str(airsyncbase::DATA),
            Some("<strong>This is my second\n    note.</strong>")
        );
    }

    #[test]
    fn message_classes_are_checked() {
        let ctx = context(Version::V16_1);
        let custom =
            application_data("<notes:MessageClass>IPM.StickyNote.Blue</notes:MessageClass>");
        let text = to_ical(&custom, None, Omitted::Cleared, &ctx).unwrap();
        assert!(text.contains("X-ACTIVESYNC-MESSAGECLASS:IPM.StickyNote.Blue"));
        let back = from_ical(&text, &ctx).unwrap();
        assert_eq!(
            back.child_str(notes::MESSAGE_CLASS),
            Some("IPM.StickyNote.Blue")
        );
        for class in ["IPM.Note", "IPM.StickyNote.", "IPM.StickyNoteX"] {
            let data =
                application_data(&format!("<notes:MessageClass>{class}</notes:MessageClass>"));
            assert!(
                to_ical(&data, None, Omitted::Cleared, &ctx).is_err(),
                "{class}"
            );
        }
    }

    #[test]
    fn garbage_is_refused() {
        let ctx = context(Version::V14_1);
        for text in [
            "",
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        ] {
            assert!(from_ical(text, &ctx).is_err());
            assert!(to_ical(&application_data(ADD), Some(text), Omitted::Cleared, &ctx).is_err());
        }
    }
}
