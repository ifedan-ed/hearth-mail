//! A small namespace-resolved element tree read from a DAV response.
//!
//! Multistatus bodies are small trees whose meaning depends on nesting
//! (a `getetag` counts only inside a `propstat` whose status is 200), so they
//! are read into a tree first and interpreted after, rather than matched
//! event by event. Element names are kept as (namespace, local name), so any
//! prefix a server chooses (`d:`, `D:`, a default namespace) reads the same.

use quick_xml::{NsReader, XmlVersion, events::Event, name::ResolveResult};

use crate::{DavError, error::protocol};

/// Deeper nesting than any DAV response has; stops a hostile body from
/// building an arbitrarily deep tree.
const MAX_DEPTH: usize = 64;

/// The namespaces DAV responses use. Anything else is [`Ns::Other`] and is
/// never matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ns {
    /// `DAV:` (RFC 4918).
    Dav,
    /// `urn:ietf:params:xml:ns:caldav` (RFC 4791).
    CalDav,
    /// `urn:ietf:params:xml:ns:carddav` (RFC 6352).
    CardDav,
    /// `http://calendarserver.org/ns/` (getctag).
    CalendarServer,
    /// `http://apple.com/ns/ical/` (calendar-color).
    AppleIcal,
    /// Any other namespace, or none.
    Other,
}

impl Ns {
    fn from_uri(uri: &[u8]) -> Self {
        match uri {
            b"DAV:" => Self::Dav,
            b"urn:ietf:params:xml:ns:caldav" => Self::CalDav,
            b"urn:ietf:params:xml:ns:carddav" => Self::CardDav,
            b"http://calendarserver.org/ns/" => Self::CalendarServer,
            b"http://apple.com/ns/ical/" => Self::AppleIcal,
            _ => Self::Other,
        }
    }
}

/// One element: its name, the attributes this crate reads, its children and
/// its text.
#[derive(Debug)]
pub(crate) struct Element {
    pub ns: Ns,
    pub name: String,
    /// Unprefixed attributes as (name, value); DAV only uses `name` on
    /// `C:comp`.
    pub attributes: Vec<(String, String)>,
    pub children: Vec<Element>,
    /// The element's own character data, unescaped, as sent (line ends are
    /// kept, so iCalendar's CRLF survives).
    pub text: String,
}

impl Element {
    /// Whether this element is `name` in `ns`.
    pub fn is(&self, ns: Ns, name: &str) -> bool {
        self.ns == ns && self.name == name
    }

    /// The first child `name` in `ns`.
    pub fn child(&self, ns: Ns, name: &str) -> Option<&Element> {
        self.children.iter().find(|child| child.is(ns, name))
    }

    /// The children `name` in `ns`.
    pub fn children_named<'a>(
        &'a self,
        ns: Ns,
        name: &'a str,
    ) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |child| child.is(ns, name))
    }

    /// The value of the unprefixed attribute `name`.
    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// The text, trimmed: for values that can't carry meaningful outer
    /// whitespace (hrefs, statuses, tokens, ETags).
    pub fn trimmed(&self) -> &str {
        self.text.trim()
    }
}

/// Reads `xml` into its root element.
pub(crate) fn parse(xml: &str) -> Result<Element, DavError> {
    let mut reader = NsReader::from_str(xml);
    let mut open: Vec<Element> = Vec::new();
    loop {
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|err| protocol(format!("XML: {err}")))?;
        match event {
            Event::Start(_) | Event::Empty(_) if open.len() >= MAX_DEPTH => {
                return Err(protocol("XML nested too deep"));
            }
            Event::Start(start) => open.push(element(&namespace, &start)?),
            Event::Empty(start) => {
                let element = element(&namespace, &start)?;
                if let Some(root) = close(element, &mut open) {
                    return Ok(root);
                }
            }
            Event::End(_) => {
                let element = open
                    .pop()
                    .ok_or_else(|| protocol("XML end without start"))?;
                if let Some(root) = close(element, &mut open) {
                    return Ok(root);
                }
            }
            Event::Text(text) => {
                // Entities arrive as their own events, so the raw text holds
                // none; `decode` keeps CRLF as sent.
                let text = text
                    .decode()
                    .map_err(|err| protocol(format!("XML text: {err}")))?;
                push_text(&mut open, &text);
            }
            Event::CData(data) => {
                let text = data
                    .decode()
                    .map_err(|err| protocol(format!("XML CDATA: {err}")))?;
                push_text(&mut open, &text);
            }
            Event::GeneralRef(reference) => {
                let ch = entity(&reference)?;
                push_text(&mut open, ch.encode_utf8(&mut [0; 4]));
            }
            Event::Eof => return Err(protocol("XML document has no complete root element")),
            // Declarations, comments, processing instructions, doctype.
            _ => {}
        }
    }
}

/// Builds an element from a start tag.
fn element(
    namespace: &ResolveResult,
    start: &quick_xml::events::BytesStart,
) -> Result<Element, DavError> {
    let ns = match namespace {
        ResolveResult::Bound(uri) => Ns::from_uri(uri.as_ref()),
        _ => Ns::Other,
    };
    let name = utf8(start.local_name().as_ref())?.to_owned();
    let mut attributes = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|err| protocol(format!("XML attribute: {err}")))?;
        let key = attribute.key.as_ref();
        // Namespace declarations and prefixed attributes are never read.
        if key.contains(&b':') || key == b"xmlns" {
            continue;
        }
        let value = attribute
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|err| protocol(format!("XML attribute: {err}")))?;
        attributes.push((utf8(key)?.to_owned(), value.into_owned()));
    }
    Ok(Element {
        ns,
        name,
        attributes,
        children: Vec::new(),
        text: String::new(),
    })
}

/// Attaches a finished element to its parent, or returns it as the root.
fn close(element: Element, open: &mut [Element]) -> Option<Element> {
    match open.last_mut() {
        Some(parent) => {
            parent.children.push(element);
            None
        }
        None => Some(element),
    }
}

/// Adds character data to the innermost open element (text outside the root
/// is whitespace and dropped).
fn push_text(open: &mut [Element], text: &str) {
    if let Some(element) = open.last_mut() {
        element.text.push_str(text);
    }
}

/// The character an entity or character reference stands for.
fn entity(reference: &quick_xml::events::BytesRef) -> Result<char, DavError> {
    match reference.resolve_char_ref() {
        Ok(Some(ch)) => Ok(ch),
        Ok(None) => match reference.as_ref() {
            b"amp" => Ok('&'),
            b"lt" => Ok('<'),
            b"gt" => Ok('>'),
            b"quot" => Ok('"'),
            b"apos" => Ok('\''),
            other => Err(protocol(format!(
                "unknown XML entity &{};",
                String::from_utf8_lossy(other)
            ))),
        },
        Err(err) => Err(protocol(format!("XML character reference: {err}"))),
    }
}

fn utf8(bytes: &[u8]) -> Result<&str, DavError> {
    std::str::from_utf8(bytes).map_err(|_| protocol("XML name isn't UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_resolve_to_namespaces() {
        let root = parse(
            r#"<?xml version="1.0"?>
            <x:multistatus xmlns:x="DAV:" xmlns="urn:ietf:params:xml:ns:caldav">
              <x:href> /a </x:href>
              <calendar-data>A&amp;B&#13;
C</calendar-data>
              <comp name="VTODO"/>
            </x:multistatus>"#,
        )
        .unwrap();
        assert!(root.is(Ns::Dav, "multistatus"));
        assert_eq!(root.child(Ns::Dav, "href").unwrap().trimmed(), "/a");
        assert_eq!(
            root.child(Ns::CalDav, "calendar-data").unwrap().text,
            "A&B\r\nC"
        );
        assert_eq!(
            root.child(Ns::CalDav, "comp").unwrap().attribute("name"),
            Some("VTODO")
        );
    }

    #[test]
    fn crlf_in_text_is_kept() {
        let root = parse("<d:a xmlns:d=\"DAV:\">BEGIN:X\r\nEND:X\r\n</d:a>").unwrap();
        assert_eq!(root.text, "BEGIN:X\r\nEND:X\r\n");
    }

    #[test]
    fn cdata_is_text() {
        let root = parse("<d:a xmlns:d=\"DAV:\"><![CDATA[<b>&]]></d:a>").unwrap();
        assert_eq!(root.text, "<b>&");
    }

    #[test]
    fn broken_documents_are_errors() {
        assert!(parse("<d:a xmlns:d=\"DAV:\">").is_err());
        assert!(parse("<d:a xmlns:d=\"DAV:\"></d:b>").is_err());
        assert!(parse("").is_err());
        let deep = "<a>".repeat(MAX_DEPTH + 1);
        assert!(parse(&deep).is_err());
    }
}
