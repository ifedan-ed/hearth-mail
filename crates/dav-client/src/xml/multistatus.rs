//! Reading `DAV:multistatus` (RFC 4918 section 13) and `DAV:error` bodies.
//!
//! Only the properties the bridge asks for are read, and only from a
//! `propstat` whose status is 2xx: a 404 propstat lists properties the
//! resource doesn't have, which must not read as empty values. The tree is
//! consumed, so calendar and address data are moved out, not copied.

use super::dom::{Element, Ns, parse};
use crate::{DavError, discovery::Components, error::protocol};

/// A parsed multistatus body.
#[derive(Debug, Default)]
pub(crate) struct Multistatus {
    pub responses: Vec<Response>,
    /// The top-level `DAV:sync-token` of a sync-collection report.
    pub sync_token: Option<String>,
}

/// One `DAV:response`.
#[derive(Debug, Default)]
pub(crate) struct Response {
    /// The hrefs as written by the server (RFC 4918 allows several when the
    /// response carries a bare status).
    pub hrefs: Vec<String>,
    /// The response-level status, when the response has no propstat
    /// (a removed member in a sync report, a missing href in a multiget,
    /// the 507 of a truncated sync report).
    pub status: Option<u16>,
    /// Properties from the 2xx propstats.
    pub props: Props,
    /// The local names of the conditions in the response's `DAV:error`
    /// (RFC 6578 uses `number-of-matches-within-limits`).
    pub errors: Vec<String>,
}

/// The properties this crate reads; `None` when absent or not found.
#[derive(Debug, Default)]
pub(crate) struct Props {
    pub display_name: Option<String>,
    pub resource_type: Option<ResourceType>,
    pub components: Option<Components>,
    pub ctag: Option<String>,
    pub sync_token: Option<String>,
    pub color: Option<String>,
    pub etag: Option<String>,
    /// `C:calendar-data` or `CR:address-data`.
    pub data: Option<String>,
    pub current_user_principal: Option<String>,
    pub calendar_home_set: Option<String>,
    pub addressbook_home_set: Option<String>,
    /// From `DAV:current-user-privilege-set` (RFC 3744 section 5.4): whether
    /// `write`, `write-content` or `all` is granted.
    pub writable: Option<bool>,
}

/// What `DAV:resourcetype` says beyond "collection".
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResourceType {
    pub calendar: bool,
    pub addressbook: bool,
}

/// Parses a multistatus body.
pub(crate) fn parse_multistatus(xml: &str) -> Result<Multistatus, DavError> {
    let root = parse(xml)?;
    if !root.is(Ns::Dav, "multistatus") {
        return Err(protocol(format!("expected multistatus, got {}", root.name)));
    }
    let mut multistatus = Multistatus::default();
    for child in root.children {
        if child.is(Ns::Dav, "response") {
            multistatus.responses.push(response(child)?);
        } else if child.is(Ns::Dav, "sync-token") {
            multistatus.sync_token = Some(child.trimmed().to_owned());
        }
    }
    Ok(multistatus)
}

/// Whether `xml` is a `DAV:error` body naming the condition `name` in `ns`
/// (RFC 4918 section 16). Anything unreadable is "no".
pub(crate) fn error_names(xml: &str, ns: Ns, name: &str) -> bool {
    parse(xml).is_ok_and(|root| root.is(Ns::Dav, "error") && root.child(ns, name).is_some())
}

fn response(element: Element) -> Result<Response, DavError> {
    let mut response = Response::default();
    for child in element.children {
        match (child.ns, child.name.as_str()) {
            (Ns::Dav, "href") => response.hrefs.push(child.trimmed().to_owned()),
            (Ns::Dav, "status") => response.status = Some(status(&child)?),
            (Ns::Dav, "propstat") => propstat(child, &mut response.props)?,
            (Ns::Dav, "error") => response
                .errors
                .extend(child.children.into_iter().map(|condition| condition.name)),
            _ => {}
        }
    }
    if response.hrefs.is_empty() {
        return Err(protocol("a response without an href"));
    }
    Ok(response)
}

/// Reads a propstat's properties into `props` when its status is 2xx.
fn propstat(element: Element, props: &mut Props) -> Result<(), DavError> {
    let ok = match element.child(Ns::Dav, "status") {
        Some(status_element) => (200..300).contains(&status(status_element)?),
        None => return Err(protocol("a propstat without a status")),
    };
    if !ok {
        return Ok(());
    }
    for prop in element.children {
        if prop.is(Ns::Dav, "prop") {
            for property in prop.children {
                read_property(property, props);
            }
        }
    }
    Ok(())
}

/// Stores one property element in `props`; unknown properties are skipped.
fn read_property(property: Element, props: &mut Props) {
    match (property.ns, property.name.as_str()) {
        (Ns::Dav, "displayname") => props.display_name = Some(property.trimmed().to_owned()),
        (Ns::Dav, "resourcetype") => {
            props.resource_type = Some(ResourceType {
                calendar: property.child(Ns::CalDav, "calendar").is_some(),
                addressbook: property.child(Ns::CardDav, "addressbook").is_some(),
            })
        }
        (Ns::CalDav, "supported-calendar-component-set") => {
            props.components = Some(components(&property))
        }
        (Ns::CalendarServer, "getctag") => props.ctag = Some(property.trimmed().to_owned()),
        (Ns::Dav, "sync-token") => props.sync_token = Some(property.trimmed().to_owned()),
        (Ns::AppleIcal, "calendar-color") => props.color = Some(property.trimmed().to_owned()),
        (Ns::Dav, "getetag") => props.etag = Some(property.trimmed().to_owned()),
        (Ns::CalDav, "calendar-data") | (Ns::CardDav, "address-data") => {
            props.data = Some(data(property.text))
        }
        (Ns::Dav, "current-user-principal") => props.current_user_principal = first_href(&property),
        (Ns::CalDav, "calendar-home-set") => props.calendar_home_set = first_href(&property),
        (Ns::CardDav, "addressbook-home-set") => props.addressbook_home_set = first_href(&property),
        (Ns::Dav, "current-user-privilege-set") => props.writable = Some(writable(&property)),
        _ => {}
    }
}

/// The components a `supported-calendar-component-set` lists.
fn components(set: &Element) -> Components {
    let mut components = Components::NONE;
    for comp in set.children_named(Ns::CalDav, "comp") {
        match comp.attribute("name") {
            Some(name) if name.eq_ignore_ascii_case("VEVENT") => components.event = true,
            Some(name) if name.eq_ignore_ascii_case("VTODO") => components.todo = true,
            Some(name) if name.eq_ignore_ascii_case("VJOURNAL") => components.journal = true,
            _ => {}
        }
    }
    components
}

/// Whether a privilege set grants changing the collection's members.
fn writable(set: &Element) -> bool {
    set.children_named(Ns::Dav, "privilege").any(|privilege| {
        privilege.children.iter().any(|granted| {
            granted.ns == Ns::Dav
                && matches!(granted.name.as_str(), "write" | "write-content" | "all")
        })
    })
}

/// The first `DAV:href` inside a property such as `current-user-principal`.
fn first_href(property: &Element) -> Option<String> {
    property
        .child(Ns::Dav, "href")
        .map(|href| href.trimmed().to_owned())
}

/// Calendar or address data without the indentation a pretty-printing server
/// puts before it (iCalendar and vCard can't start with whitespace). Moves
/// the text when there is nothing to strip.
fn data(text: String) -> String {
    let start = text.len() - text.trim_start().len();
    if start == 0 {
        text
    } else {
        text[start..].to_owned()
    }
}

/// The code in a `DAV:status` line ("HTTP/1.1 200 OK").
fn status(element: &Element) -> Result<u16, DavError> {
    element
        .trimmed()
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| protocol(format!("unreadable status {:?}", element.trimmed())))
}
