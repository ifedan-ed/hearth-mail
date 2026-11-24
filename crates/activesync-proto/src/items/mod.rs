/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Conversions between ActiveSync items and the iCalendar and vCard objects
//! the DAV bridge keeps them as (docs/activesync.md, "Calendars, contacts,
//! tasks, notes: the DAV bridge"):
//!
//! - [`calendar`]: the Calendar class ([MS-ASCAL]) and VEVENT,
//! - [`contact`]: the Contact class ([MS-ASCNTC], Contacts2) and vCard,
//! - [`task`]: the Task class ([MS-ASTASK]) and VTODO,
//! - [`note`]: the Notes class ([MS-ASNOTE]) and VJOURNAL,
//! - [`meeting`]: the Email class `MeetingRequest` built from an iTIP
//!   invitation, and the iTIP REPLY a `MeetingResponse` sends,
//! - [`timezone`]: the `TimeZone` blob ([MS-ASDTYPE] 2.7.6) and its
//!   mapping to IANA zones and VTIMEZONE.
//!
//! Each class has two directions. `to_*` takes the `ApplicationData`
//! element a device sent and the item's current text, if it has one, and
//! returns the new text; `from_*` takes the text and returns the
//! `ApplicationData` element for the device. Writing back replaces only the
//! properties ActiveSync carries and keeps everything else (X- properties,
//! other components, unknown parameters), so a field set in Nextcloud
//! survives an edit on a phone; a property whose value did not change keeps
//! its original line, parameters and all.
//!
//! Nothing here panics on input: text that is not iCalendar or vCard, and
//! elements with values outside the specification's ranges, give an
//! [`ItemError`]. Inputs are capped ([`MAX_TEXT`], [`MAX_ITEMS`]) and no
//! conversion expands a recurrence, so the work per item is bounded by its
//! size.

pub mod calendar;
pub mod contact;
pub mod meeting;
pub mod note;
pub mod task;
pub mod timezone;

mod alarm;
mod body;
mod dates;
mod element;
mod ical;
mod recurrence;

#[cfg(test)]
mod fixtures;

pub use body::{BodyPreference, BodyType};

use crate::{request::Version, wbxml::Tag};
use chrono::{DateTime, Utc};
use std::fmt;

/// The largest iCalendar or vCard text accepted, in bytes. A contact with a
/// photo is the largest item there is (ActiveSync itself caps photos at
/// 48 KB of base64; Nextcloud keeps larger ones, which are then not sent).
pub const MAX_TEXT: usize = 4 * 1024 * 1024;

/// The most repeated children of one kind accepted in an element
/// (attendees, exceptions, categories, children). [MS-ASCAL] allows up to
/// 256 exceptions and 300 categories, and no meeting has more attendees
/// than this.
pub const MAX_ITEMS: usize = 1000;

/// What the conversions need to know about the request and the account,
/// passed in by the caller so that nothing here reads a clock or guesses.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    /// The protocol version of the request: 14.1 and 16.1 differ in the
    /// calendar's `UID`, `Location`, exceptions and all-day events.
    pub version: Version,
    /// The account's time zone. Floating times (no `TZID`, no `Z`) are read
    /// in it, and in 14.1 an all-day event starts at its midnight.
    pub zone: chrono_tz::Tz,
    /// The current time: new `DTSTAMP` and `LAST-MODIFIED` values, and the
    /// start of a calendar item added without one ([MS-ASCAL] 3.2.4.4).
    pub now: DateTime<Utc>,
    /// The account's addresses, without `mailto:`: who the organizer is,
    /// and which attendee is the account in `ResponseType`.
    pub addresses: &'a [String],
    /// The account's display name, for the `ORGANIZER` a 16.1 client leaves
    /// to the server ([MS-ASCAL] 2.2.2.36).
    pub name: Option<&'a str>,
    /// How the device wants bodies sent ([MS-ASAIRS] `BodyPreference`).
    pub body: BodyPreference,
}

impl Context<'_> {
    /// Whether `address` is one of the account's own.
    fn is_me(&self, address: &str) -> bool {
        self.addresses
            .iter()
            .any(|own| own.eq_ignore_ascii_case(address))
    }
}

/// What a `Change` means by an element it leaves out ([MS-ASCMD]
/// 2.2.3.24 and 2.2.3.179, "ghosted" properties). Bodies and pictures left
/// out always keep their value, whatever this says (2.2.3.24).
#[derive(Debug, Clone, Copy)]
pub enum Omitted<'a> {
    /// An element left out is deleted: an `Add`, a 14.1 `Change` after an
    /// initial `Sync` without `Supported`, and task and note changes.
    Cleared,
    /// An element left out keeps its value: a 16.1 calendar `Change`, or a
    /// 14.1 one after an empty `Supported`.
    Kept,
    /// A 14.1 `Change` after an initial `Sync` with these `Supported`
    /// children: those are managed by the client and cleared when left
    /// out; every other element keeps its value.
    Supported(&'a [Tag]),
}

impl Omitted<'_> {
    /// Whether an element `tag` that is not in the request keeps its value.
    fn keeps(&self, tag: Tag) -> bool {
        match self {
            Omitted::Cleared => false,
            Omitted::Kept => true,
            Omitted::Supported(managed) => !managed.contains(&tag),
        }
    }
}

/// Why an item could not be converted. The server answers a device with
/// Sync status 6 ("error in client/server conversion") for every one of
/// these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemError {
    /// The stored text is not iCalendar or vCard, or has nothing of the
    /// expected kind in it.
    Unreadable(&'static str),
    /// The stored text or an element is larger than the limits above.
    TooLarge(&'static str),
    /// An element's value is outside what the specification allows.
    Invalid {
        element: &'static str,
        reason: &'static str,
    },
}

impl ItemError {
    fn invalid(tag: Tag, reason: &'static str) -> Self {
        ItemError::Invalid {
            element: tag.name(),
            reason,
        }
    }
}

impl fmt::Display for ItemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ItemError::Unreadable(what) => write!(f, "unreadable item: {what}"),
            ItemError::TooLarge(what) => write!(f, "item too large: {what}"),
            ItemError::Invalid { element, reason } => write!(f, "invalid {element}: {reason}"),
        }
    }
}

impl std::error::Error for ItemError {}

/// The result of every conversion.
pub type Result<T> = std::result::Result<T, ItemError>;
