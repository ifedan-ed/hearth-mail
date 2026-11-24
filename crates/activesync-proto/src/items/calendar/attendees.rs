/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Organizer and attendees ([MS-ASCAL] 2.2.2.3 to 2.2.2.6, 2.2.2.35,
//! 2.2.2.36) and `ORGANIZER`/`ATTENDEE` (RFC 5545 3.8.4.1, 3.8.4.3).
//!
//! | ActiveSync | iCalendar |
//! |---|---|
//! | `AttendeeStatus` 2, 3, 4, 5 | `PARTSTAT` TENTATIVE, ACCEPTED, DECLINED, NEEDS-ACTION |
//! | `AttendeeType` 1, 2 | `ROLE` REQ-PARTICIPANT, OPT-PARTICIPANT |
//! | `AttendeeType` 3 | `CUTYPE=RESOURCE` (or ROOM) |
//!
//! The organizer is not in ActiveSync's attendee list ([MS-ASCAL] 4.3),
//! but Nextcloud also lists the organizer as an attendee: that `ATTENDEE`
//! line is left alone. An attendee the device sends again keeps its
//! line's other parameters (`RSVP`, `SCHEDULE-STATUS`, `X-` ones).

use crate::items::{
    Context, ItemError, MAX_ITEMS, Result,
    element::{number, push_number, trimmed},
    ical,
};
use crate::wbxml::{Element, codepages::calendar};
use calcard::icalendar::{
    ICalendarComponent, ICalendarEntry, ICalendarParameter, ICalendarParameterName,
    ICalendarParameterValue, ICalendarParticipationRole, ICalendarParticipationStatus,
    ICalendarProperty, ICalendarUserTypes, ICalendarValue, Uri,
};

/// One attendee as ActiveSync has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Attendee {
    pub(crate) email: String,
    pub(crate) name: String,
    /// `AttendeeStatus`; `None` when the device did not say (16.1 devices
    /// never do, [MS-ASCAL] 2.2.2.5).
    pub(crate) status: Option<u8>,
    /// `AttendeeType`.
    pub(crate) kind: Option<u8>,
}

/// The address in an `Email`/`OrganizerEmail` value, which devices send
/// as `"Name" <address>` as often as plain.
pub(crate) fn address_of(value: &str) -> &str {
    let value = value.trim();
    match (value.rfind('<'), value.rfind('>')) {
        (Some(start), Some(end)) if start < end => value[start + 1..end].trim(),
        _ => value.strip_prefix("mailto:").unwrap_or(value),
    }
}

/// The address of an `ORGANIZER` or `ATTENDEE` line.
pub(crate) fn line_address(entry: &ICalendarEntry) -> Option<&str> {
    entry.calendar_address().map(str::trim)
}

/// The `CN` of a line.
fn line_name(entry: &ICalendarEntry) -> Option<&str> {
    entry
        .parameter(&ICalendarParameterName::Cn)
        .and_then(ICalendarParameterValue::as_text)
}

/// The organizer's address and name.
pub(crate) fn organizer(event: &ICalendarComponent) -> Option<(&str, Option<&str>)> {
    let entry = ical::first(event, &ICalendarProperty::Organizer)?;
    Some((line_address(entry)?, line_name(entry)))
}

/// The attendees as ActiveSync lists them: everyone but the organizer.
pub(crate) fn read(event: &ICalendarComponent) -> Vec<Attendee> {
    let organizer = organizer(event).map(|(address, _)| address);
    event
        .entries
        .iter()
        .filter(|entry| entry.name == ICalendarProperty::Attendee)
        .filter_map(|entry| {
            let email = line_address(entry)?;
            if organizer.is_some_and(|organizer| organizer.eq_ignore_ascii_case(email)) {
                return None;
            }
            Some(Attendee {
                email: email.to_string(),
                name: line_name(entry).unwrap_or(email).to_string(),
                status: Some(status_of(entry)),
                kind: Some(kind_of(entry)),
            })
        })
        .take(MAX_ITEMS)
        .collect()
}

/// `AttendeeStatus` from `PARTSTAT`.
pub(crate) fn status_of(entry: &ICalendarEntry) -> u8 {
    match entry.parameter(&ICalendarParameterName::Partstat) {
        Some(ICalendarParameterValue::Partstat(status)) => match status {
            ICalendarParticipationStatus::Tentative => 2,
            ICalendarParticipationStatus::Accepted => 3,
            ICalendarParticipationStatus::Declined => 4,
            ICalendarParticipationStatus::NeedsAction => 5,
            _ => 0,
        },
        _ => 0,
    }
}

/// `AttendeeType` from `CUTYPE` and `ROLE`.
fn kind_of(entry: &ICalendarEntry) -> u8 {
    let resource = matches!(
        entry.parameter(&ICalendarParameterName::Cutype),
        Some(ICalendarParameterValue::Cutype(
            ICalendarUserTypes::Resource | ICalendarUserTypes::Room
        ))
    );
    let optional = matches!(
        entry.parameter(&ICalendarParameterName::Role),
        Some(ICalendarParameterValue::Role(
            ICalendarParticipationRole::OptParticipant | ICalendarParticipationRole::NonParticipant
        ))
    );
    if resource {
        3
    } else if optional {
        2
    } else {
        1
    }
}

/// `PARTSTAT` for an `AttendeeStatus`; `None` for 0, "unknown".
pub(crate) fn partstat_of(status: u8) -> Option<ICalendarParticipationStatus> {
    match status {
        2 => Some(ICalendarParticipationStatus::Tentative),
        3 => Some(ICalendarParticipationStatus::Accepted),
        4 => Some(ICalendarParticipationStatus::Declined),
        5 => Some(ICalendarParticipationStatus::NeedsAction),
        _ => None,
    }
}

/// The `Attendees` element a device sent.
pub(crate) fn parse(element: &Element) -> Result<Vec<Attendee>> {
    let mut attendees = Vec::new();
    for attendee in element.children_of(calendar::ATTENDEE) {
        if attendees.len() == MAX_ITEMS {
            return Err(ItemError::TooLarge("too many attendees"));
        }
        let email = attendee
            .child(calendar::EMAIL)
            .map(trimmed)
            .transpose()?
            .map(address_of)
            .filter(|email| !email.is_empty())
            .ok_or_else(|| ItemError::invalid(calendar::ATTENDEE, "no Email"))?;
        let name = attendee
            .child(calendar::NAME)
            .map(trimmed)
            .transpose()?
            .filter(|name| !name.is_empty())
            .unwrap_or(email);
        let status = attendee
            .child(calendar::ATTENDEE_STATUS)
            .map(|status| number(status, 5))
            .transpose()?
            .map(|status| status as u8);
        let kind = attendee
            .child(calendar::ATTENDEE_TYPE)
            .map(|kind| number(kind, 3))
            .transpose()?
            .map(|kind| kind as u8);
        attendees.push(Attendee {
            email: email.to_string(),
            name: name.to_string(),
            status,
            kind,
        });
    }
    Ok(attendees)
}

/// The `Attendees` element; `None` without attendees. Status and type are
/// always sent: responses carry both in 14.1 and 16.1 alike.
pub(crate) fn element(attendees: &[Attendee]) -> Option<Element> {
    if attendees.is_empty() {
        return None;
    }
    let mut list = Element::new(calendar::ATTENDEES);
    for attendee in attendees {
        let mut element = Element::new(calendar::ATTENDEE)
            .with(Element::text(calendar::EMAIL, attendee.email.as_str()))
            .with(Element::text(calendar::NAME, attendee.name.as_str()));
        push_number(
            &mut element,
            calendar::ATTENDEE_STATUS,
            attendee.status.unwrap_or(0),
        );
        push_number(
            &mut element,
            calendar::ATTENDEE_TYPE,
            attendee.kind.unwrap_or(1),
        );
        list.push(element);
    }
    Some(list)
}

/// Replaces the attendees with `attendees`. Lines for addresses the device
/// sends again are kept with their parameters, updated where the device
/// said something new; `rsvp` (from `ResponseRequested`) is set on every
/// line when given. Nothing changes when the device sent what is stored.
pub(crate) fn write(event: &mut ICalendarComponent, attendees: &[Attendee], rsvp: Option<bool>) {
    let current = read(event);
    let unchanged = rsvp.is_none()
        && current.len() == attendees.len()
        && current.iter().zip(attendees).all(|(stored, sent)| {
            stored.email.eq_ignore_ascii_case(&sent.email)
                && stored.name == sent.name
                && sent
                    .status
                    .is_none_or(|status| status == 0 || Some(status) == stored.status)
                && sent.kind.is_none_or(|kind| Some(kind) == stored.kind)
        });
    if unchanged {
        return;
    }
    let organizer = organizer(event).map(|(address, _)| address.to_ascii_lowercase());
    // Take the attendee lines out, keeping the organizer's.
    let mut old = Vec::new();
    let mut index = 0;
    while index < event.entries.len() {
        let entry = &event.entries[index];
        let is_organizer = organizer.as_deref().is_some_and(|organizer| {
            line_address(entry).is_some_and(|address| address.eq_ignore_ascii_case(organizer))
        });
        if entry.name == ICalendarProperty::Attendee && !is_organizer {
            old.push(event.entries.remove(index));
        } else {
            index += 1;
        }
    }
    for attendee in attendees {
        let previous = old.iter().position(|entry| {
            line_address(entry).is_some_and(|address| address.eq_ignore_ascii_case(&attendee.email))
        });
        let mut entry = match previous {
            Some(at) => old.swap_remove(at),
            None => ICalendarEntry::new(ICalendarProperty::Attendee)
                .with_value(ICalendarValue::Uri(Uri::Location(format!(
                    "mailto:{}",
                    attendee.email
                ))))
                .with_param(ICalendarParameter::partstat(
                    ICalendarParameterValue::Partstat(ICalendarParticipationStatus::NeedsAction),
                )),
        };
        set_param(&mut entry, ICalendarParameter::cn(attendee.name.clone()));
        if let Some(partstat) = attendee.status.and_then(partstat_of) {
            set_param(
                &mut entry,
                ICalendarParameter::partstat(ICalendarParameterValue::Partstat(partstat)),
            );
        }
        match attendee.kind {
            Some(3) => set_param(
                &mut entry,
                ICalendarParameter::cutype(ICalendarParameterValue::Cutype(
                    ICalendarUserTypes::Resource,
                )),
            ),
            Some(kind) => {
                if kind_of(&entry) == 3 {
                    remove_param(&mut entry, &ICalendarParameterName::Cutype);
                }
                set_param(
                    &mut entry,
                    ICalendarParameter::role(ICalendarParameterValue::Role(if kind == 2 {
                        ICalendarParticipationRole::OptParticipant
                    } else {
                        ICalendarParticipationRole::ReqParticipant
                    })),
                );
            }
            None if previous.is_none() => set_param(
                &mut entry,
                ICalendarParameter::role(ICalendarParameterValue::Role(
                    ICalendarParticipationRole::ReqParticipant,
                )),
            ),
            None => {}
        }
        if let Some(rsvp) = rsvp.or(previous.is_none().then_some(true)) {
            set_param(
                &mut entry,
                ICalendarParameter::rsvp(ICalendarParameterValue::Bool(rsvp)),
            );
        }
        event.entries.push(entry);
    }
}

/// Sets the `ORGANIZER`, keeping the line's other parameters when the
/// address stays.
pub(crate) fn set_organizer(event: &mut ICalendarComponent, email: &str, name: Option<&str>) {
    if let Some((address, current_name)) = organizer(event)
        && address.eq_ignore_ascii_case(email)
    {
        if name.is_none() || name == current_name {
            return;
        }
        if let Some(entry) = event
            .entries
            .iter_mut()
            .find(|entry| entry.name == ICalendarProperty::Organizer)
            && let Some(name) = name
        {
            set_param(entry, ICalendarParameter::cn(name.to_string()));
        }
        return;
    }
    let mut entry = ICalendarEntry::new(ICalendarProperty::Organizer).with_value(
        ICalendarValue::Uri(Uri::Location(format!("mailto:{email}"))),
    );
    if let Some(name) = name {
        entry.add_param(ICalendarParameter::cn(name.to_string()));
    }
    ical::replace(event, entry);
}

/// Whether any attendee is asked to reply (`RSVP=TRUE`).
pub(crate) fn response_requested(event: &ICalendarComponent) -> bool {
    event
        .entries
        .iter()
        .filter(|entry| entry.name == ICalendarProperty::Attendee)
        .any(|entry| {
            matches!(
                entry.parameter(&ICalendarParameterName::Rsvp),
                Some(ICalendarParameterValue::Bool(true))
            )
        })
}

/// `ResponseType` ([MS-ASCAL] 2.2.2.40): 1 for the organizer, else the
/// account's own `PARTSTAT` (5 not yet answered), 0 when the account is
/// not on the list.
pub(crate) fn response_type(event: &ICalendarComponent, ctx: &Context<'_>) -> u8 {
    if organizer(event).is_none_or(|(address, _)| ctx.is_me(address)) {
        return 1;
    }
    event
        .entries
        .iter()
        .filter(|entry| entry.name == ICalendarProperty::Attendee)
        .find(|entry| line_address(entry).is_some_and(|address| ctx.is_me(address)))
        .map_or(0, status_of)
}

/// Replaces the first parameter named like `param`, or adds it.
pub(crate) fn set_param(entry: &mut ICalendarEntry, param: ICalendarParameter) {
    match entry
        .params
        .iter_mut()
        .find(|current| current.name == param.name)
    {
        Some(current) => *current = param,
        None => entry.params.push(param),
    }
}

fn remove_param(entry: &mut ICalendarEntry, name: &ICalendarParameterName) {
    entry.params.retain(|param| &param.name != name);
}

/// The 14.1 `OrganizerName`/`OrganizerEmail` elements.
pub(crate) fn organizer_elements(event: &ICalendarComponent, out: &mut Element) {
    if let Some((email, name)) = organizer(event) {
        out.push(Element::text(
            calendar::ORGANIZER_NAME,
            name.unwrap_or(email),
        ));
        out.push(Element::text(calendar::ORGANIZER_EMAIL, email));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::ical::Calendar;
    use calcard::icalendar::ICalendarComponentType;

    const EVENT: &str = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a\r\n\
        ORGANIZER;CN=Anat Kerry:mailto:anat@contoso.com\r\n\
        ATTENDEE;CN=Anat Kerry;ROLE=CHAIR;PARTSTAT=ACCEPTED:mailto:anat@contoso.com\r\n\
        ATTENDEE;CN=Chris Gray;PARTSTAT=ACCEPTED;SCHEDULE-STATUS=2.0:mailto:chris@fourthcoffee.com\r\n\
        ATTENDEE;CN=Room;CUTYPE=ROOM:mailto:room@contoso.com\r\n\
        ATTENDEE;ROLE=OPT-PARTICIPANT:mailto:me@example.com\r\n\
        END:VEVENT\r\nEND:VCALENDAR\r\n";

    fn event() -> Calendar {
        Calendar::parse(EVENT).unwrap()
    }

    fn master(calendar: &Calendar) -> u32 {
        calendar.master(&ICalendarComponentType::VEvent).unwrap()
    }

    #[test]
    fn attendees_are_read_without_the_organizer() {
        let calendar = event();
        let attendees = read(calendar.component(master(&calendar)));
        let summary: Vec<_> = attendees
            .iter()
            .map(|a| (a.email.as_str(), a.name.as_str(), a.status, a.kind))
            .collect();
        assert_eq!(
            summary,
            [
                ("chris@fourthcoffee.com", "Chris Gray", Some(3), Some(1)),
                ("room@contoso.com", "Room", Some(0), Some(3)),
                ("me@example.com", "me@example.com", Some(0), Some(2)),
            ]
        );
    }

    #[test]
    fn writing_keeps_known_lines() {
        let mut calendar = event();
        let id = master(&calendar);
        let mut attendees = read(calendar.component(id));
        attendees.remove(1);
        attendees.push(Attendee {
            email: "new@example.com".into(),
            name: "New Person".into(),
            status: None,
            kind: Some(2),
        });
        write(calendar.component_mut(id), &attendees, None);
        let text = calendar.write();
        assert!(text.contains("SCHEDULE-STATUS=2.0"), "{text}");
        assert!(text.contains("ROLE=CHAIR"));
        assert!(!text.contains("room@contoso.com"));
        assert!(text.contains("CN=\"New Person\""));
        assert!(text.contains("ROLE=OPT-PARTICIPANT"));
        assert_eq!(read(calendar.component(id)), {
            let mut expected = attendees.clone();
            expected[2].status = Some(5);
            expected
        });
    }

    #[test]
    fn the_account_answer_is_its_response_type() {
        let calendar = event();
        let ctx = crate::items::fixtures::context(crate::request::Version::V14_1);
        assert_eq!(
            response_type(calendar.component(master(&calendar)), &ctx),
            0
        );
        assert_eq!(
            address_of("\"Anat\" <anat@contoso.com>"),
            "anat@contoso.com"
        );
        assert_eq!(address_of(" mailto:x@y "), "x@y");
    }
}
