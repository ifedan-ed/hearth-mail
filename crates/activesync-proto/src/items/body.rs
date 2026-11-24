/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Item bodies ([MS-ASAIRS] 2.2.2.9 `Body`): the notes of an event, a
//! contact, a task or a note.
//!
//! iCalendar and vCard keep plain text (`DESCRIPTION`, `NOTE`). A device
//! may send HTML (Outlook does, and [MS-ASNOTE] 4's example): the text is
//! kept as plain text for every other client, and the HTML beside it in
//! `X-ALT-DESC;FMTTYPE=text/html` (as Outlook's own iCalendar export does)
//! so the same device gets it back. RTF (type 3) is not converted: a body
//! sent as RTF leaves the stored one as it is.

use super::{
    ItemError, Result,
    element::{Field, push_number},
};
use crate::wbxml::{Element, codepages::airsyncbase};

/// The body type a device asks for ([MS-ASAIRS] 2.2.2.41.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BodyType {
    #[default]
    Plain,
    Html,
}

/// A device's `BodyPreference` for a folder.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BodyPreference {
    pub kind: BodyType,
    /// `TruncationSize`: the most bytes of the body to send.
    pub truncation: Option<u32>,
}

/// A body as stored: the plain text, and the HTML when a device sent it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StoredBody<'x> {
    pub(crate) text: &'x str,
    pub(crate) html: Option<&'x str>,
}

/// What a request says about the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NewBody {
    /// No body, or one that cannot be read (RTF): the stored one stays.
    Keep,
    /// An empty body: the stored one goes.
    Clear,
    /// Plain text, and the HTML it came from.
    Set { text: String, html: Option<String> },
}

impl NewBody {
    /// Reads `airsyncbase:Body` from `data`. A body left out keeps the
    /// stored one ([MS-ASCMD] 2.2.3.24).
    pub(crate) fn read(data: &Element) -> Result<NewBody> {
        let body = match Field::kept_when_absent(data, airsyncbase::BODY) {
            Field::Set(body) => body,
            Field::Clear => return Ok(NewBody::Clear),
            Field::Keep => return Ok(NewBody::Keep),
        };
        let kind = body
            .child_u64(airsyncbase::TYPE)
            .ok_or_else(|| ItemError::invalid(airsyncbase::BODY, "no Type"))?;
        let data = body.child_str(airsyncbase::DATA).unwrap_or_default();
        Ok(match kind {
            1 if data.is_empty() => NewBody::Clear,
            1 => NewBody::Set {
                text: data.replace("\r\n", "\n"),
                html: None,
            },
            2 if data.is_empty() => NewBody::Clear,
            2 => NewBody::Set {
                text: html_to_text(data),
                html: Some(data.to_string()),
            },
            3 | 4 => NewBody::Keep,
            _ => return Err(ItemError::invalid(airsyncbase::TYPE, "unknown body type")),
        })
    }
}

/// The `airsyncbase:Body` element for a stored body, in the form and size
/// the device asked for. An empty body is sent as an empty plain one, as
/// [MS-ASTASK] 4.2's example does.
pub(crate) fn element(body: Option<StoredBody<'_>>, preference: &BodyPreference) -> Element {
    let (kind, content) = match (preference.kind, body) {
        (
            BodyType::Html,
            Some(StoredBody {
                html: Some(html), ..
            }),
        ) => (2u8, html.to_string()),
        (BodyType::Html, Some(StoredBody { text, .. })) => (2u8, text_to_html(text)),
        (BodyType::Plain, Some(StoredBody { text, .. })) => (1u8, text.replace('\n', "\r\n")),
        (_, None) => (1u8, String::new()),
    };
    let size = content.len();
    let limit = preference
        .truncation
        .map_or(usize::MAX, |limit| limit as usize);
    let mut element = Element::new(airsyncbase::BODY);
    push_number(&mut element, airsyncbase::TYPE, kind);
    push_number(&mut element, airsyncbase::ESTIMATED_DATA_SIZE, size as u64);
    if size > limit {
        push_number(&mut element, airsyncbase::TRUNCATED, 1u8);
        let mut end = limit;
        while !content.is_char_boundary(end) {
            end -= 1;
        }
        element.push(Element::text(airsyncbase::DATA, &content[..end]));
    } else if size > 0 {
        element.push(Element::text(airsyncbase::DATA, content));
    }
    element
}

/// `airsyncbase:NativeBodyType`: how the body is kept, 2 when there is HTML.
pub(crate) fn native_type(body: Option<StoredBody<'_>>) -> Element {
    let kind = if body.is_some_and(|body| body.html.is_some()) {
        2u8
    } else {
        1u8
    };
    Element::text(airsyncbase::NATIVE_BODY_TYPE, kind.to_string())
}

/// Plain text from HTML: tags dropped, line breaks for `<br>`, `<p>` and
/// `<div>`, the common entities decoded. Enough for the bodies devices
/// write; not a renderer.
pub(crate) fn html_to_text(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find(['<', '&']) {
        text.push_str(&rest[..start]);
        rest = &rest[start..];
        if rest.starts_with('<') {
            let end = rest.find('>').map_or(rest.len(), |end| end + 1);
            let tag = rest[1..end.saturating_sub(1).max(1)].trim_start_matches('/');
            let name: String = tag
                .chars()
                .take_while(char::is_ascii_alphanumeric)
                .map(|c| c.to_ascii_lowercase())
                .collect();
            if matches!(name.as_str(), "br" | "p" | "div" | "li" | "tr")
                && rest[1..].starts_with('/') != (name == "br")
                && !text.is_empty()
                && !text.ends_with('\n')
            {
                text.push('\n');
            }
            rest = &rest[end..];
        } else {
            // Entities are short; a `;` further on belongs to the text.
            let end = rest
                .char_indices()
                .take(10)
                .find(|(_, c)| *c == ';')
                .map(|(at, _)| at);
            let entity = end.map(|end| &rest[1..end]);
            let decoded = match entity {
                Some("amp") => Some('&'),
                Some("lt") => Some('<'),
                Some("gt") => Some('>'),
                Some("quot") => Some('"'),
                Some("apos") | Some("#39") => Some('\''),
                Some("nbsp") => Some(' '),
                Some(code) if code.starts_with('#') => code[1..]
                    .parse::<u32>()
                    .ok()
                    .or_else(|| {
                        code[1..]
                            .strip_prefix(['x', 'X'])
                            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                    })
                    .and_then(char::from_u32),
                _ => None,
            };
            match (decoded, end) {
                (Some(c), Some(end)) => {
                    text.push(c);
                    rest = &rest[end + 1..];
                }
                _ => {
                    text.push('&');
                    rest = &rest[1..];
                }
            }
        }
    }
    text.push_str(rest);
    text.trim().to_string()
}

/// HTML for plain text, for a device that asks for HTML bodies.
fn text_to_html(text: &str) -> String {
    let mut html = String::with_capacity(text.len() + 32);
    html.push_str("<html><body>");
    for c in text.chars() {
        match c {
            '&' => html.push_str("&amp;"),
            '<' => html.push_str("&lt;"),
            '>' => html.push_str("&gt;"),
            '"' => html.push_str("&quot;"),
            '\n' => html.push_str("<br>"),
            '\r' => {}
            c => html.push(c),
        }
    }
    html.push_str("</body></html>");
    html
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wbxml::codepages::airsync;

    fn data(kind: &str, body: &str) -> Element {
        Element::new(airsync::APPLICATION_DATA).with(
            Element::new(airsyncbase::BODY)
                .with(Element::text(airsyncbase::TYPE, kind))
                .with(Element::text(airsyncbase::DATA, body)),
        )
    }

    #[test]
    fn bodies_are_read() {
        assert_eq!(
            NewBody::read(&data("1", "Line 1\r\nLine 2")).unwrap(),
            NewBody::Set {
                text: "Line 1\nLine 2".into(),
                html: None
            }
        );
        // [MS-ASTASK] 4.1's HTML body.
        let html = "<strong>Must</strong> complete TPS reports using\nthe new cover sheet.";
        assert_eq!(
            NewBody::read(&data("2", html)).unwrap(),
            NewBody::Set {
                text: "Must complete TPS reports using\nthe new cover sheet.".into(),
                html: Some(html.into())
            }
        );
        assert_eq!(
            NewBody::read(&data("3", "e1xydGY=")).unwrap(),
            NewBody::Keep
        );
        assert_eq!(NewBody::read(&data("1", "")).unwrap(), NewBody::Clear);
        assert!(NewBody::read(&data("9", "x")).is_err());
        let none = Element::new(airsync::APPLICATION_DATA);
        assert_eq!(NewBody::read(&none).unwrap(), NewBody::Keep);
    }

    #[test]
    fn html_becomes_text() {
        assert_eq!(
            html_to_text("<p>One &amp; two</p><p>Three<br/>Four &#x263A; &bogus</p>"),
            "One & two\nThree\nFour \u{263A} &bogus"
        );
        assert_eq!(html_to_text("<b>unclosed"), "unclosed");
        assert_eq!(html_to_text("a < b"), "a");
    }

    #[test]
    fn bodies_are_written_and_truncated() {
        let stored = StoredBody {
            text: "Grüße\nzwei",
            html: None,
        };
        let plain = element(Some(stored), &BodyPreference::default());
        assert_eq!(plain.child_str(airsyncbase::DATA), Some("Grüße\r\nzwei"));
        assert_eq!(plain.child_u64(airsyncbase::ESTIMATED_DATA_SIZE), Some(13));
        let short = element(
            Some(stored),
            &BodyPreference {
                kind: BodyType::Plain,
                truncation: Some(4),
            },
        );
        // "Grü" is 4 bytes; the cut never splits a character.
        assert_eq!(short.child_str(airsyncbase::DATA), Some("Grü"));
        assert_eq!(short.child_u64(airsyncbase::TRUNCATED), Some(1));
        let html = element(
            Some(stored),
            &BodyPreference {
                kind: BodyType::Html,
                truncation: None,
            },
        );
        assert_eq!(
            html.child_str(airsyncbase::DATA),
            Some("<html><body>Grüße<br>zwei</body></html>")
        );
        let empty = element(None, &BodyPreference::default());
        assert_eq!(empty.child(airsyncbase::DATA), None);
    }
}
