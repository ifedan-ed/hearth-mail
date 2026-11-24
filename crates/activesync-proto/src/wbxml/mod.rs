/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! WBXML as Exchange ActiveSync uses it ([MS-ASWBXML]): WBXML 1.3 with the
//! UTF-8 charset, no string table, no attributes, and the ActiveSync code
//! pages ([`codepages`]).
//!
//! A document is decoded into an [`Element`] tree and encoded back from one.
//! Bodies are small (a Sync request is a few kilobytes, a message's MIME at
//! most the upload limit the HTTP layer enforces), so a tree is simpler than
//! a streaming reader and costs one allocation per element. The decoder is
//! iterative with a depth limit, so a hostile body can't exhaust the stack.

pub mod codepages;
mod xml;

pub use xml::{XmlError, from_xml, to_xml};

use std::fmt;

/// One tag of one code page.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Tag {
    page: u8,
    token: u8,
}

impl Tag {
    /// The tag `token` of code page `page`; the constants in [`codepages`]
    /// are made with this.
    pub const fn new(page: u8, token: u8) -> Self {
        Tag { page, token }
    }

    pub fn page(self) -> u8 {
        self.page
    }

    pub fn token(self) -> u8 {
        self.token
    }

    /// The tag's name, as the specification spells it (`SyncKey`).
    pub fn name(self) -> &'static str {
        codepages::PAGES
            .get(self.page as usize)
            .and_then(|page| {
                page.tags
                    .binary_search_by_key(&self.token, |(token, _)| *token)
                    .ok()
                    .map(|index| page.tags[index].1)
            })
            .unwrap_or("Unknown")
    }

    /// The name of the tag's code page (`AirSync`).
    pub fn namespace(self) -> &'static str {
        codepages::PAGES
            .get(self.page as usize)
            .map_or("Unknown", |page| page.namespace)
    }

    /// The tag called `name` in the code page called `namespace`, if there
    /// is one. Used for the XML form only; code uses the constants.
    pub fn lookup(namespace: &str, name: &str) -> Option<Tag> {
        let page = page_of(namespace)?;
        codepages::PAGES[page as usize]
            .tags
            .iter()
            .find(|(_, tag)| *tag == name)
            .map(|(token, _)| Tag::new(page, *token))
    }

    /// Whether this tag is defined in the specification's tables.
    fn is_known(self) -> bool {
        self.name() != "Unknown"
    }
}

/// The code page a namespace name stands for, ignoring case and a trailing
/// colon (`AirSync:` in XML), and accepting the older names the
/// specification's examples still use (`POOMMAIL:` for `Email`).
fn page_of(namespace: &str) -> Option<u8> {
    let name = namespace.strip_suffix(':').unwrap_or(namespace);
    let name = match name.to_ascii_uppercase().as_str() {
        "POOMCONTACTS" => "Contacts",
        "POOMMAIL" => "Email",
        "POOMCAL" => "Calendar",
        "POOMTASKS" => "Tasks",
        "POOMCONTACTS2" => "Contacts2",
        "POOMMAIL2" => "Email2",
        _ => name,
    };
    codepages::PAGES
        .iter()
        .position(|page| page.namespace.eq_ignore_ascii_case(name))
        .map(|page| page as u8)
}

impl fmt::Debug for Tag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.namespace(), self.name())
    }
}

/// What an element holds. ActiveSync never mixes text and child elements.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum Content {
    #[default]
    Empty,
    Text(String),
    /// Bytes sent as WBXML `OPAQUE`: MIME messages, attachments, the
    /// calendar `TimeZone` blob, conversation ids.
    Opaque(Vec<u8>),
    Children(Vec<Element>),
}

/// One element of an ActiveSync document.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Element {
    pub tag: Tag,
    pub content: Content,
}

impl Element {
    /// An empty element (`<Tag/>`).
    pub fn new(tag: Tag) -> Self {
        Element {
            tag,
            content: Content::Empty,
        }
    }

    /// An element holding text.
    pub fn text(tag: Tag, text: impl Into<String>) -> Self {
        Element {
            tag,
            content: Content::Text(text.into()),
        }
    }

    /// An element holding bytes, sent as `OPAQUE`.
    pub fn opaque(tag: Tag, bytes: Vec<u8>) -> Self {
        Element {
            tag,
            content: Content::Opaque(bytes),
        }
    }

    /// This element with `child` added at the end, for building answers.
    pub fn with(mut self, child: Element) -> Self {
        self.push(child);
        self
    }

    /// This element with `child` added when there is one.
    pub fn with_opt(self, child: Option<Element>) -> Self {
        match child {
            Some(child) => self.with(child),
            None => self,
        }
    }

    /// Adds `child` at the end. Text or bytes held before are dropped:
    /// an element holds one or the other.
    pub fn push(&mut self, child: Element) {
        match &mut self.content {
            Content::Children(children) => children.push(child),
            content => *content = Content::Children(vec![child]),
        }
    }

    /// The child elements (none for text, bytes or an empty element).
    pub fn children(&self) -> &[Element] {
        match &self.content {
            Content::Children(children) => children,
            _ => &[],
        }
    }

    /// The first child with `tag`.
    pub fn child(&self, tag: Tag) -> Option<&Element> {
        self.children().iter().find(|child| child.tag == tag)
    }

    /// Every child with `tag`, in order.
    pub fn children_of(&self, tag: Tag) -> impl Iterator<Item = &Element> {
        self.children().iter().filter(move |child| child.tag == tag)
    }

    /// The element's text; bytes count when they are UTF-8 (a client may
    /// send either for a string).
    pub fn as_str(&self) -> Option<&str> {
        match &self.content {
            Content::Text(text) => Some(text),
            Content::Opaque(bytes) => std::str::from_utf8(bytes).ok(),
            _ => None,
        }
    }

    /// The element's bytes, text or opaque; empty for anything else.
    pub fn as_bytes(&self) -> &[u8] {
        match &self.content {
            Content::Text(text) => text.as_bytes(),
            Content::Opaque(bytes) => bytes,
            _ => &[],
        }
    }

    /// The text of the first child with `tag`.
    pub fn child_str(&self, tag: Tag) -> Option<&str> {
        self.child(tag).and_then(Element::as_str)
    }

    /// The first child with `tag` read as an unsigned number.
    pub fn child_u64(&self, tag: Tag) -> Option<u64> {
        self.child_str(tag)
            .and_then(|text| text.trim().parse().ok())
    }
}

/// Why a body could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// The body ended in the middle of something.
    Truncated,
    /// A header field this decoder doesn't handle (charset other than
    /// UTF-8, a WBXML version before 1.1).
    Header(&'static str),
    /// A WBXML construct ActiveSync doesn't use (attributes, extensions,
    /// processing instructions, literal tags).
    Unsupported(u8),
    /// A tag no code page defines.
    UnknownTag { page: u8, token: u8 },
    /// Text where child elements were, or the other way round.
    MixedContent,
    /// Text that isn't UTF-8.
    NotUtf8,
    /// Nesting deeper than [`MAX_DEPTH`].
    TooDeep,
    /// More than one root element, or none.
    Root,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Truncated => write!(f, "the WBXML body ends too early"),
            DecodeError::Header(what) => write!(f, "unsupported WBXML header: {what}"),
            DecodeError::Unsupported(token) => {
                write!(f, "unsupported WBXML token 0x{token:02X}")
            }
            DecodeError::UnknownTag { page, token } => {
                write!(f, "unknown tag 0x{token:02X} in code page {page}")
            }
            DecodeError::MixedContent => write!(f, "an element holds both text and elements"),
            DecodeError::NotUtf8 => write!(f, "text that isn't UTF-8"),
            DecodeError::TooDeep => write!(f, "elements nested too deep"),
            DecodeError::Root => write!(f, "not exactly one root element"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// The deepest nesting accepted. ActiveSync's deepest documents (a Sync
/// with a calendar exception's attendees) are about 12 levels.
pub const MAX_DEPTH: usize = 32;

// Global WBXML tokens (WBXML 1.3, section 7.1).
const SWITCH_PAGE: u8 = 0x00;
const END: u8 = 0x01;
const ENTITY: u8 = 0x02;
const STR_I: u8 = 0x03;
const STR_T: u8 = 0x83;
const OPAQUE: u8 = 0xC3;
/// The content bit of a tag token: the element has content and an END.
const HAS_CONTENT: u8 = 0x40;
/// The attribute bit of a tag token: never used by ActiveSync.
const HAS_ATTRIBUTES: u8 = 0x80;
/// WBXML version 1.3, public id 1 (unknown), charset 106 (UTF-8), no string
/// table: the header every ActiveSync body starts with.
const HEADER: [u8; 4] = [0x03, 0x01, 0x6A, 0x00];

/// A cursor over the body.
struct Reader<'x> {
    bytes: &'x [u8],
    pos: usize,
}

impl<'x> Reader<'x> {
    fn byte(&mut self) -> Result<u8, DecodeError> {
        let byte = *self.bytes.get(self.pos).ok_or(DecodeError::Truncated)?;
        self.pos += 1;
        Ok(byte)
    }

    /// A WBXML multi-byte unsigned integer: 7 bits a byte, high bit set on
    /// every byte but the last; at most 5 bytes for 32 bits.
    fn mb_u32(&mut self) -> Result<u32, DecodeError> {
        let mut value: u32 = 0;
        for _ in 0..5 {
            let byte = self.byte()?;
            value = (value << 7) | u32::from(byte & 0x7F);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(DecodeError::Header("integer longer than 32 bits"))
    }

    fn take(&mut self, len: usize) -> Result<&'x [u8], DecodeError> {
        let end = self.pos.checked_add(len).ok_or(DecodeError::Truncated)?;
        let slice = self
            .bytes
            .get(self.pos..end)
            .ok_or(DecodeError::Truncated)?;
        self.pos = end;
        Ok(slice)
    }

    /// An inline string: bytes up to a NUL.
    fn terminated(&mut self) -> Result<&'x [u8], DecodeError> {
        let rest = &self.bytes[self.pos..];
        let len = rest
            .iter()
            .position(|byte| *byte == 0)
            .ok_or(DecodeError::Truncated)?;
        self.pos += len + 1;
        Ok(&rest[..len])
    }
}

/// Decodes an ActiveSync WBXML body into its root element.
pub fn decode(bytes: &[u8]) -> Result<Element, DecodeError> {
    let mut reader = Reader { bytes, pos: 0 };
    let version = reader.byte()?;
    if !(0x01..=0x03).contains(&version) {
        return Err(DecodeError::Header("WBXML version"));
    }
    if reader.mb_u32()? == 0 {
        // Public id 0: the id is an index into the string table.
        reader.mb_u32()?;
    }
    if reader.mb_u32()? != 106 {
        return Err(DecodeError::Header("charset other than UTF-8"));
    }
    let table_len = reader.mb_u32()? as usize;
    let table = reader.take(table_len)?;

    let mut page = 0u8;
    // The open elements, innermost last; the root ends up in `root`.
    let mut open: Vec<Element> = Vec::with_capacity(8);
    let mut root = None;
    while reader.pos < bytes.len() {
        let token = reader.byte()?;
        match token {
            SWITCH_PAGE => page = reader.byte()?,
            END => {
                let element = open.pop().ok_or(DecodeError::Root)?;
                close(element, &mut open, &mut root)?;
            }
            STR_I => {
                let text = reader.terminated()?;
                append_text(&mut open, text)?;
            }
            STR_T => {
                let offset = reader.mb_u32()? as usize;
                let rest = table.get(offset..).ok_or(DecodeError::Truncated)?;
                let len = rest
                    .iter()
                    .position(|byte| *byte == 0)
                    .ok_or(DecodeError::Truncated)?;
                append_text(&mut open, &rest[..len])?;
            }
            ENTITY => {
                let code = reader.mb_u32()?;
                let ch = char::from_u32(code).ok_or(DecodeError::NotUtf8)?;
                append_text(&mut open, ch.encode_utf8(&mut [0; 4]).as_bytes())?;
            }
            OPAQUE => {
                let len = reader.mb_u32()? as usize;
                let data = reader.take(len)?;
                let element = open.last_mut().ok_or(DecodeError::Root)?;
                match &mut element.content {
                    Content::Empty => element.content = Content::Opaque(data.to_vec()),
                    Content::Opaque(bytes) => bytes.extend_from_slice(data),
                    Content::Text(text) => {
                        // Text then bytes: keep it all as bytes.
                        let mut bytes = std::mem::take(text).into_bytes();
                        bytes.extend_from_slice(data);
                        element.content = Content::Opaque(bytes);
                    }
                    Content::Children(_) => return Err(DecodeError::MixedContent),
                }
            }
            _ if token & 0x3F < 0x05 => return Err(DecodeError::Unsupported(token)),
            _ if token & HAS_ATTRIBUTES != 0 => return Err(DecodeError::Unsupported(token)),
            _ => {
                let tag = Tag::new(page, token & 0x3F);
                if !tag.is_known() {
                    return Err(DecodeError::UnknownTag {
                        page,
                        token: token & 0x3F,
                    });
                }
                let element = Element::new(tag);
                if token & HAS_CONTENT != 0 {
                    if open.len() >= MAX_DEPTH {
                        return Err(DecodeError::TooDeep);
                    }
                    open.push(element);
                } else {
                    close(element, &mut open, &mut root)?;
                }
            }
        }
    }
    if !open.is_empty() {
        return Err(DecodeError::Truncated);
    }
    root.ok_or(DecodeError::Root)
}

/// Hands a finished element to its parent, or makes it the root.
fn close(
    element: Element,
    open: &mut [Element],
    root: &mut Option<Element>,
) -> Result<(), DecodeError> {
    match open.last_mut() {
        Some(parent) => match &mut parent.content {
            Content::Empty => {
                parent.content = Content::Children(vec![element]);
                Ok(())
            }
            Content::Children(children) => {
                children.push(element);
                Ok(())
            }
            // Whitespace before a child is layout, not content.
            Content::Text(text) if text.trim().is_empty() => {
                parent.content = Content::Children(vec![element]);
                Ok(())
            }
            _ => Err(DecodeError::MixedContent),
        },
        None if root.is_none() => {
            *root = Some(element);
            Ok(())
        }
        None => Err(DecodeError::Root),
    }
}

fn append_text(open: &mut [Element], text: &[u8]) -> Result<(), DecodeError> {
    let text = std::str::from_utf8(text).map_err(|_| DecodeError::NotUtf8)?;
    let element = open.last_mut().ok_or(DecodeError::Root)?;
    match &mut element.content {
        Content::Empty => element.content = Content::Text(text.to_string()),
        Content::Text(existing) => existing.push_str(text),
        Content::Opaque(bytes) => bytes.extend_from_slice(text.as_bytes()),
        // Whitespace between children is layout.
        Content::Children(_) if text.trim().is_empty() => (),
        Content::Children(_) => return Err(DecodeError::MixedContent),
    }
    Ok(())
}

/// Encodes `root` as an ActiveSync WBXML body.
pub fn encode(root: &Element) -> Vec<u8> {
    let mut out = Vec::with_capacity(256);
    out.extend_from_slice(&HEADER);
    let mut page = 0u8;
    write_element(root, &mut page, &mut out);
    out
}

fn write_element(element: &Element, page: &mut u8, out: &mut Vec<u8>) {
    if element.tag.page != *page {
        out.push(SWITCH_PAGE);
        out.push(element.tag.page);
        *page = element.tag.page;
    }
    match &element.content {
        Content::Empty => out.push(element.tag.token),
        Content::Text(text) if text.as_bytes().contains(&0) => {
            // STR_I can't hold a NUL; bytes can.
            out.push(element.tag.token | HAS_CONTENT);
            write_opaque(text.as_bytes(), out);
            out.push(END);
        }
        Content::Text(text) => {
            out.push(element.tag.token | HAS_CONTENT);
            out.push(STR_I);
            out.extend_from_slice(text.as_bytes());
            out.push(0);
            out.push(END);
        }
        Content::Opaque(bytes) => {
            out.push(element.tag.token | HAS_CONTENT);
            write_opaque(bytes, out);
            out.push(END);
        }
        Content::Children(children) if children.is_empty() => out.push(element.tag.token),
        Content::Children(children) => {
            out.push(element.tag.token | HAS_CONTENT);
            for child in children {
                write_element(child, page, out);
            }
            out.push(END);
        }
    }
}

fn write_opaque(bytes: &[u8], out: &mut Vec<u8>) {
    out.push(OPAQUE);
    write_mb_u32(bytes.len() as u32, out);
    out.extend_from_slice(bytes);
}

/// Writes a WBXML multi-byte unsigned integer.
fn write_mb_u32(mut value: u32, out: &mut Vec<u8>) {
    let mut buf = [0u8; 5];
    let mut pos = buf.len() - 1;
    buf[pos] = (value & 0x7F) as u8;
    value >>= 7;
    while value > 0 {
        pos -= 1;
        buf[pos] = (value & 0x7F) as u8 | 0x80;
        value >>= 7;
    }
    out.extend_from_slice(&buf[pos..]);
}

#[cfg(test)]
mod tests {
    use super::codepages::{airsync, airsyncbase};
    use super::*;

    /// The example of [MS-ASWBXML] section 3 (v20250520): a Sync answer
    /// adding a contact, byte for byte, and the same document read from its
    /// XML form in that section.
    #[test]
    fn spec_example_sync_contact() {
        let bytes: Vec<u8> = "03 01 6A 00 45 5C 4F 50 03 43 6F 6E 74 61 63 74 73 00 01 4B 03 32 00 01 52 03 32 00 01 4E 03 \
             31 00 01 56 47 4D 03 32 3A 31 00 01 5D 00 11 4A 46 03 31 00 01 4C 03 30 00 01 4D 03 31 00 01 \
             01 00 01 5E 03 46 75 6E 6B 2C 20 44 6F 6E 00 01 5F 03 44 6F 6E 00 01 69 03 46 75 6E 6B 00 01 \
             00 11 56 03 31 00 01 01 01 01 01 01 01"
            .split_whitespace()
            .map(|hex| u8::from_str_radix(hex, 16).expect("hex"))
            .collect();
        let element = decode(&bytes).expect("decodes");
        assert_eq!(element.tag, airsync::SYNC);
        assert_eq!(encode(&element), bytes);

        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
      <Sync xmlns="AirSync" xmlns:airsyncbase="AirSyncBase" xmlns:contacts="Contacts">
        <Collections>
          <Collection>
            <Class>Contacts</Class>
            <SyncKey>2</SyncKey>
            <CollectionId>2</CollectionId>
            <Status>1</Status>
            <Commands>
              <Add>
                <ServerId>2:1</ServerId>
                <ApplicationData>
                  <airsyncbase:Body>
                    <airsyncbase:Type>1</airsyncbase:Type>
                    <airsyncbase:EstimatedDataSize>0</airsyncbase:EstimatedDataSize>
                    <airsyncbase:Truncated>1</airsyncbase:Truncated>
                  </airsyncbase:Body>
                  <contacts:FileAs>Funk, Don</contacts:FileAs>
                  <contacts:FirstName>Don</contacts:FirstName>
                  <contacts:LastName>Funk</contacts:LastName>
                  <airsyncbase:NativeBodyType>1</airsyncbase:NativeBodyType>
                </ApplicationData>
              </Add>
            </Commands>
          </Collection>
        </Collections>
      </Sync>"#;
        assert_eq!(from_xml(xml).expect("parses"), element);
        // And back to XML and in again.
        assert_eq!(from_xml(&to_xml(&element)).expect("parses"), element);
    }

    #[test]
    fn round_trip_with_pages_bytes_and_empty_elements() {
        let doc = Element::new(airsync::SYNC).with(
            Element::new(airsync::COLLECTIONS).with(
                Element::new(airsync::COLLECTION)
                    .with(Element::text(airsync::SYNC_KEY, "1"))
                    .with(Element::text(airsync::COLLECTION_ID, "m:5"))
                    .with(Element::new(airsync::DELETES_AS_MOVES))
                    .with(
                        Element::new(airsync::OPTIONS).with(
                            Element::new(airsyncbase::BODY_PREFERENCE)
                                .with(Element::text(airsyncbase::TYPE, "2")),
                        ),
                    )
                    .with(Element::opaque(airsync::CLIENT_ID, vec![0, 1, 2, 255])),
            ),
        );
        let bytes = encode(&doc);
        assert_eq!(decode(&bytes).expect("decodes"), doc);
    }

    #[test]
    fn text_with_nul_goes_as_bytes() {
        let doc = Element::text(airsync::SYNC_KEY, "a\0b");
        let back = decode(&encode(&doc)).expect("decodes");
        assert_eq!(back.as_bytes(), b"a\0b");
    }

    #[test]
    fn long_opaque_lengths() {
        for len in [0usize, 127, 128, 16_383, 16_384, 300_000] {
            let doc = Element::opaque(airsync::CLIENT_ID, vec![7; len]);
            assert_eq!(
                decode(&encode(&doc)).expect("decodes").as_bytes().len(),
                len
            );
        }
    }

    #[test]
    fn refuses_what_activesync_does_not_use() {
        let header = HEADER.to_vec();
        for (body, expected) in [
            (vec![0x45], DecodeError::Truncated),
            (vec![0x45, 0x01, 0x01], DecodeError::Root),
            (vec![0xC5], DecodeError::Unsupported(0xC5)),
            (
                vec![0x00, 0x03, 0x45, 0x01],
                DecodeError::UnknownTag { page: 3, token: 5 },
            ),
            (
                vec![0x00, 0x1F, 0x05],
                DecodeError::UnknownTag { page: 31, token: 5 },
            ),
            (
                vec![0x45, 0x03, b'x', 0x00, 0x0B, 0x01],
                DecodeError::MixedContent,
            ),
            (vec![0x45, 0x03, 0xFF, 0x00, 0x01], DecodeError::NotUtf8),
            (vec![], DecodeError::Root),
        ] {
            let mut bytes = header.clone();
            bytes.extend_from_slice(&body);
            assert_eq!(decode(&bytes), Err(expected), "{body:02X?}");
        }
        // The charset must be UTF-8.
        assert!(matches!(
            decode(&[0x03, 0x01, 0x04, 0x00, 0x05]),
            Err(DecodeError::Header(_))
        ));
    }

    #[test]
    fn nesting_is_limited() {
        let mut bytes = HEADER.to_vec();
        bytes.extend(std::iter::repeat_n(0x56, MAX_DEPTH + 1)); // Commands, with content
        assert_eq!(decode(&bytes), Err(DecodeError::TooDeep));
    }

    #[test]
    fn tag_names() {
        assert_eq!(format!("{:?}", airsync::SYNC_KEY), "AirSync:SyncKey");
        assert_eq!(Tag::lookup("AirSync:", "SyncKey"), Some(airsync::SYNC_KEY));
        assert_eq!(Tag::lookup("POOMMAIL:", "Subject").map(Tag::page), Some(2));
        assert_eq!(Tag::lookup("AirSync", "Nope"), None);
    }
}
