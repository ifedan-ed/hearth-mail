/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The XML form of ActiveSync documents, as the specifications print them
//! (`<Sync xmlns="AirSync:" xmlns:email="Email:">`). Used by the replay
//! fixtures, which are written as XML and sent as WBXML, and by logs and
//! test failures, which are easier to read as XML. Not used on the wire.
//!
//! Bytes that aren't UTF-8 are written as base64 with the attribute
//! `opaque="base64"`, and read back the same way; any other attribute is
//! refused, since WBXML as ActiveSync uses it has none.

use super::{Content, Element, MAX_DEPTH, Tag};
use base64::{Engine, engine::general_purpose::STANDARD};
use quick_xml::{
    NsReader, XmlVersion,
    escape::escape,
    events::{BytesStart, Event},
    name::ResolveResult,
};
use std::fmt::{self, Write};

/// Why an XML document could not be read as ActiveSync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlError(pub String);

impl fmt::Display for XmlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for XmlError {}

fn err(what: impl Into<String>) -> XmlError {
    XmlError(what.into())
}

/// An element being read, and whether its text is base64.
struct Open {
    element: Element,
    base64: bool,
}

/// Reads an ActiveSync document from its XML form.
pub fn from_xml(xml: &str) -> Result<Element, XmlError> {
    let mut reader = NsReader::from_str(xml);
    let mut open: Vec<Open> = Vec::new();
    let mut root = None;
    loop {
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|e| err(format!("XML: {e}")))?;
        match event {
            Event::Start(start) => {
                if open.len() >= MAX_DEPTH {
                    return Err(err("elements nested too deep"));
                }
                open.push(start_element(&namespace, &start)?);
            }
            Event::Empty(start) => {
                let element = start_element(&namespace, &start)?;
                finish(element, &mut open, &mut root)?;
            }
            Event::End(_) => {
                let element = open.pop().ok_or_else(|| err("an end without a start"))?;
                finish(element, &mut open, &mut root)?;
            }
            Event::Text(text) => {
                let text = text
                    .xml_content(XmlVersion::Implicit1_0)
                    .map_err(|e| err(format!("XML text: {e}")))?;
                add_text(&mut open, &text)?;
            }
            Event::CData(data) => {
                let text = std::str::from_utf8(&data).map_err(|_| err("CDATA isn't UTF-8"))?;
                add_text(&mut open, text)?;
            }
            Event::GeneralRef(reference) => {
                let ch = match reference.resolve_char_ref() {
                    Ok(Some(ch)) => ch,
                    Ok(None) => match reference.as_ref() {
                        b"amp" => '&',
                        b"lt" => '<',
                        b"gt" => '>',
                        b"quot" => '"',
                        b"apos" => '\'',
                        other => {
                            return Err(err(format!(
                                "unknown entity &{};",
                                String::from_utf8_lossy(other)
                            )));
                        }
                    },
                    Err(e) => return Err(err(format!("XML character reference: {e}"))),
                };
                add_text(&mut open, ch.encode_utf8(&mut [0; 4]))?;
            }
            Event::Eof => break,
            // Declarations, comments, processing instructions, doctype.
            _ => (),
        }
    }
    if !open.is_empty() {
        return Err(err("the document ends inside an element"));
    }
    root.ok_or_else(|| err("no root element"))
}

fn start_element(namespace: &ResolveResult, start: &BytesStart) -> Result<Open, XmlError> {
    let ns = match namespace {
        ResolveResult::Bound(ns) => {
            std::str::from_utf8(ns.as_ref()).map_err(|_| err("namespace isn't UTF-8"))?
        }
        _ => return Err(err("an element without a namespace")),
    };
    let local = start.local_name();
    let name = std::str::from_utf8(local.as_ref()).map_err(|_| err("name isn't UTF-8"))?;
    let tag =
        Tag::lookup(ns, name).ok_or_else(|| err(format!("no tag {name} in namespace {ns}")))?;
    let mut base64 = false;
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|e| err(format!("XML attribute: {e}")))?;
        let key = attribute.key.as_ref();
        if key == b"xmlns" || key.starts_with(b"xmlns:") {
            continue;
        }
        if key == b"opaque" && attribute.value.as_ref() == b"base64" {
            base64 = true;
        } else {
            return Err(err(format!(
                "attribute {} on {name}: ActiveSync has none",
                String::from_utf8_lossy(key)
            )));
        }
    }
    Ok(Open {
        element: Element::new(tag),
        base64,
    })
}

fn add_text(open: &mut [Open], text: &str) -> Result<(), XmlError> {
    let Some(current) = open.last_mut() else {
        return if text.trim().is_empty() {
            Ok(())
        } else {
            Err(err("text outside the root element"))
        };
    };
    match &mut current.element.content {
        Content::Empty => current.element.content = Content::Text(text.to_string()),
        Content::Text(existing) => existing.push_str(text),
        Content::Children(_) if text.trim().is_empty() => (),
        _ => {
            return Err(err(format!(
                "{:?} holds text and elements",
                current.element.tag
            )));
        }
    }
    Ok(())
}

fn finish(open: Open, stack: &mut [Open], root: &mut Option<Element>) -> Result<(), XmlError> {
    let mut element = open.element;
    if let Content::Text(text) = &element.content {
        if open.base64 {
            let bytes = STANDARD
                .decode(text.split_whitespace().collect::<String>())
                .map_err(|e| err(format!("base64 in {:?}: {e}", element.tag)))?;
            element.content = Content::Opaque(bytes);
        } else if text.trim().is_empty() && text.contains('\n') {
            // Indentation inside an element with no children.
            element.content = Content::Empty;
        }
    }
    match stack.last_mut() {
        Some(parent) => {
            if let Content::Text(text) = &parent.element.content
                && text.trim().is_empty()
            {
                parent.element.content = Content::Empty;
            }
            match parent.element.content {
                Content::Text(_) | Content::Opaque(_) => Err(err(format!(
                    "{:?} holds text and elements",
                    parent.element.tag
                ))),
                _ => {
                    parent.element.push(element);
                    Ok(())
                }
            }
        }
        None if root.is_none() => {
            *root = Some(element);
            Ok(())
        }
        None => Err(err("more than one root element")),
    }
}

/// Writes `root` in its XML form, indented, with every code page it uses
/// declared on the root (`xmlns:email="Email:"`).
pub fn to_xml(root: &Element) -> String {
    let mut pages = Vec::new();
    collect_pages(root, &mut pages);
    let mut out = String::with_capacity(512);
    out.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    write_element(root, root.tag.page(), &pages, 0, &mut out);
    out
}

fn collect_pages(element: &Element, pages: &mut Vec<u8>) {
    if !pages.contains(&element.tag.page()) {
        pages.push(element.tag.page());
    }
    for child in element.children() {
        collect_pages(child, pages);
    }
}

fn prefix(tag: Tag) -> String {
    tag.namespace().to_ascii_lowercase()
}

fn write_element(element: &Element, default: u8, pages: &[u8], depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    let name = if element.tag.page() == default {
        element.tag.name().to_string()
    } else {
        format!("{}:{}", prefix(element.tag), element.tag.name())
    };
    let _ = write!(out, "{indent}<{name}");
    if depth == 0 {
        let _ = write!(out, " xmlns=\"{}:\"", element.tag.namespace());
        for page in pages.iter().filter(|page| **page != default) {
            let tag = Tag::new(*page, 0x05);
            let _ = write!(out, " xmlns:{}=\"{}:\"", prefix(tag), tag.namespace());
        }
    }
    match &element.content {
        Content::Empty => out.push_str("/>\n"),
        Content::Children(children) if children.is_empty() => out.push_str("/>\n"),
        Content::Text(text) => {
            let _ = writeln!(out, ">{}</{name}>", escape(text.as_str()));
        }
        Content::Opaque(bytes) => match std::str::from_utf8(bytes) {
            Ok(text) => {
                let _ = writeln!(out, ">{}</{name}>", escape(text));
            }
            Err(_) => {
                let _ = writeln!(
                    out,
                    " opaque=\"base64\">{}</{name}>",
                    STANDARD.encode(bytes)
                );
            }
        },
        Content::Children(children) => {
            out.push_str(">\n");
            for child in children {
                write_element(child, default, pages, depth + 1, out);
            }
            let _ = writeln!(out, "{indent}</{name}>");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::codepages::{airsync, composemail, email};
    use super::*;

    #[test]
    fn opaque_bytes_round_trip_as_base64() {
        let doc = Element::new(composemail::SEND_MAIL)
            .with(Element::text(composemail::CLIENT_ID, "a & <b>"))
            .with(Element::opaque(composemail::MIME, vec![0xFF, 0x00, 0x10]));
        let xml = to_xml(&doc);
        assert!(xml.contains("opaque=\"base64\""), "{xml}");
        assert!(xml.contains("a &amp; &lt;b&gt;"), "{xml}");
        assert_eq!(from_xml(&xml).expect("parses"), doc);
    }

    #[test]
    fn spec_namespaces_with_and_without_colon() {
        let doc = from_xml(
            r#"<Sync xmlns="AirSync:" xmlns:email="POOMMAIL:"><Collections><Collection>
               <email:Read>1</email:Read><GetChanges/></Collection></Collections></Sync>"#,
        )
        .expect("parses");
        let collection = doc
            .child(airsync::COLLECTIONS)
            .and_then(|c| c.child(airsync::COLLECTION))
            .expect("collection");
        assert_eq!(collection.child_str(email::READ), Some("1"));
        assert!(collection.child(airsync::GET_CHANGES).is_some());
    }

    #[test]
    fn refuses_unknown_tags_and_attributes() {
        assert!(from_xml(r#"<Sync xmlns="AirSync:"><Nope/></Sync>"#).is_err());
        assert!(from_xml(r#"<Sync xmlns="AirSync:" a="b"/>"#).is_err());
        assert!(from_xml(r#"<Sync/>"#).is_err());
        assert!(from_xml(r#"<Sync xmlns="AirSync:">x<Collections/></Sync>"#).is_err());
    }
}
