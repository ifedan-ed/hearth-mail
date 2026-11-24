/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! A message as ActiveSync shows it: the Email class's `ApplicationData`
//! ([MS-ASEMAIL] 2.2.2) with its body chosen and cut as the device asked
//! (AirSyncBase `BodyPreference`, [MS-ASAIRS] 2.2.2.12), and the flag
//! changes a device sends back.
//!
//! Elements are written in the order of the specification's examples
//! ([MS-ASEMAIL] 4.1.2): some clients read them in that order.

use super::flags;
use crate::{
    Ctx,
    error::{Error, Result},
    jmap::Call,
};
use activesync_proto::wbxml::{
    Element,
    codepages::{airsync, airsyncbase as base, email, email2},
};
use jmap::blob::download::BlobDownload;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use types::blob::BlobId;

/// Body types ([MS-ASAIRS] 2.2.2.22.1).
pub mod body_type {
    pub const PLAIN: u8 = 1;
    pub const HTML: u8 = 2;
    pub const MIME: u8 = 4;
}

/// One `BodyPreference` of a request.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct BodyPreference {
    pub kind: u8,
    pub truncation: Option<u32>,
    pub all_or_none: bool,
    pub preview: Option<u32>,
}

/// How a device wants message bodies: its preferences in its order, and
/// `MIMESupport` (0 never MIME, 1 S/MIME only, 2 always).
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct BodyOptions {
    pub preferences: Vec<BodyPreference>,
    pub mime_support: u8,
}

impl BodyOptions {
    /// Reads the body options of an `Options` element (Sync, ItemOperations
    /// and Search use the same AirSyncBase and AirSync tags).
    pub fn parse(options: &Element) -> BodyOptions {
        BodyOptions {
            preferences: options
                .children_of(base::BODY_PREFERENCE)
                .filter_map(|preference| {
                    Some(BodyPreference {
                        kind: preference.child_u64(base::TYPE)? as u8,
                        truncation: preference
                            .child_u64(base::TRUNCATION_SIZE)
                            .map(|size| size.min(u64::from(u32::MAX)) as u32),
                        all_or_none: preference.child_u64(base::ALL_OR_NONE) == Some(1),
                        preview: preference
                            .child_u64(base::PREVIEW)
                            .map(|size| size.min(255) as u32),
                    })
                })
                .collect(),
            mime_support: options.child_u64(airsync::MIME_SUPPORT).unwrap_or(0) as u8,
        }
    }

    /// The preference that applies: the first the server can serve. MIME
    /// only when the device said it takes MIME; plain text when the device
    /// named nothing.
    fn choose(&self) -> BodyPreference {
        self.preferences
            .iter()
            .find(|preference| match preference.kind {
                body_type::PLAIN | body_type::HTML => true,
                body_type::MIME => self.mime_support == 2,
                _ => false,
            })
            .cloned()
            .unwrap_or(BodyPreference {
                kind: body_type::PLAIN,
                truncation: None,
                all_or_none: false,
                preview: None,
            })
    }
}

/// The JMAP properties a full message needs.
const PROPERTIES: [&str; 18] = [
    "id",
    "blobId",
    "threadId",
    "keywords",
    "receivedAt",
    "subject",
    "from",
    "to",
    "cc",
    "replyTo",
    "size",
    "preview",
    "attachments",
    "textBody",
    "htmlBody",
    "bodyValues",
    "header:Importance:asText",
    "header:X-Priority:asText",
];

impl Ctx<'_> {
    /// The messages `ids` as `ApplicationData` with their [`flags`] bits,
    /// in the order given; ids that no longer exist are left out.
    pub async fn render_emails(
        &self,
        ids: &[String],
        options: &BodyOptions,
    ) -> Result<Vec<(String, u8, Element)>> {
        let preference = options.choose();
        // Read no more of a body than the device takes (plus room to find
        // a character boundary), unless it wants all of it.
        let max_bytes = preference
            .truncation
            .filter(|_| !preference.all_or_none)
            .map_or(0, |size| size.saturating_add(8));
        let mut out = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(self.server.core.jmap.get_max_objects.clamp(1, 100)) {
            let mut answers = self
                .jmap(vec![Call::new(
                    "Email/get",
                    "g",
                    json!({
                        "ids": chunk,
                        "properties": PROPERTIES,
                        "fetchTextBodyValues": preference.kind == body_type::PLAIN,
                        "fetchHTMLBodyValues": preference.kind == body_type::HTML,
                        "maxBodyValueBytes": max_bytes,
                    }),
                )])
                .await?;
            let got = answers.take("g")?;
            for email in got["list"].as_array().into_iter().flatten() {
                let Some(id) = email["id"].as_str() else {
                    continue;
                };
                let body = self.body(email, &preference).await?;
                out.push((
                    id.to_string(),
                    super::flags_of(&email["keywords"]),
                    application_data(email, body),
                ));
            }
        }
        // Keep the caller's order (newest first).
        out.sort_by_key(|(id, _, _)| ids.iter().position(|wanted| wanted == id));
        Ok(out)
    }

    /// The `Body` element for the chosen preference.
    async fn body(&self, email: &Value, preference: &BodyPreference) -> Result<Element> {
        let (kind, text, full_size) = match preference.kind {
            body_type::MIME => {
                let blob = email["blobId"]
                    .as_str()
                    .and_then(BlobId::from_base32)
                    .ok_or_else(|| Error::Internal("message without a blob".into()))?;
                let bytes = self
                    .server
                    .blob_download(&blob, self.token)
                    .await?
                    .unwrap_or_default();
                let size = bytes.len();
                (
                    body_type::MIME,
                    String::from_utf8_lossy(&bytes).into_owned(),
                    size,
                )
            }
            body_type::HTML if has_type(&email["htmlBody"], "text/html") => {
                let (text, size) = joined(email, "htmlBody");
                (body_type::HTML, text, size)
            }
            _ => {
                let (text, size) = joined(email, "textBody");
                // A message with HTML only: its text, for a plain-text view.
                if has_type(&email["textBody"], "text/html") {
                    let text = mail_parser::decoders::html::html_to_text(&text);
                    let size = text.len();
                    (body_type::PLAIN, text, size)
                } else {
                    (body_type::PLAIN, text, size)
                }
            }
        };
        let mut body = Element::new(base::BODY)
            .with(Element::text(base::TYPE, kind.to_string()))
            .with(Element::text(
                base::ESTIMATED_DATA_SIZE,
                full_size.max(text.len()).to_string(),
            ));
        let limit = preference.truncation.map(|size| size as usize);
        match limit {
            Some(limit) if text.len() > limit || full_size > limit => {
                body.push(Element::text(base::TRUNCATED, "1"));
                if !preference.all_or_none {
                    body.push(Element::text(base::DATA, cut(&text, limit)));
                }
            }
            _ => body.push(Element::text(base::DATA, text)),
        }
        if let Some(chars) = preference.preview {
            let preview: String = email["preview"]
                .as_str()
                .unwrap_or_default()
                .chars()
                .take(chars as usize)
                .collect();
            body.push(Element::text(base::PREVIEW, preview));
        }
        Ok(body)
    }
}

/// Whether a JMAP body part list has a part of `mime`.
fn has_type(parts: &Value, mime: &str) -> bool {
    parts
        .as_array()
        .into_iter()
        .flatten()
        .any(|part| part["type"].as_str() == Some(mime))
}

/// The text of a body part list from `bodyValues`, and the parts' full
/// size (the values may be cut short).
fn joined(email: &Value, list: &str) -> (String, usize) {
    let mut text = String::new();
    let mut size = 0usize;
    for part in email[list].as_array().into_iter().flatten() {
        size += part["size"].as_u64().unwrap_or(0) as usize;
        if let Some(value) = part["partId"]
            .as_str()
            .and_then(|part_id| email["bodyValues"][part_id]["value"].as_str())
        {
            if !text.is_empty() {
                text.push_str("\r\n");
            }
            text.push_str(value);
        }
    }
    (text, size)
}

/// `text` cut to at most `limit` bytes, on a character boundary.
fn cut(text: &str, limit: usize) -> String {
    let mut end = limit.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// Addresses as a header writes them: `"Name" <a@b>, c@d`.
fn addresses(list: &Value) -> Option<String> {
    let list = list.as_array().filter(|list| !list.is_empty())?;
    Some(
        list.iter()
            .map(|address| {
                let email = address["email"].as_str().unwrap_or_default();
                match address["name"].as_str().filter(|name| !name.is_empty()) {
                    Some(name) => format!("\"{}\" <{email}>", name.replace('"', "'")),
                    None => email.to_string(),
                }
            })
            .collect::<Vec<_>>()
            .join(", "),
    )
}

/// The names of the To recipients, as Exchange shows them (`DisplayTo`).
fn display_names(list: &Value) -> Option<String> {
    let list = list.as_array().filter(|list| !list.is_empty())?;
    Some(
        list.iter()
            .map(|address| {
                address["name"]
                    .as_str()
                    .filter(|name| !name.is_empty())
                    .or_else(|| address["email"].as_str())
                    .unwrap_or_default()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("; "),
    )
}

/// ActiveSync importance (0 low, 1 normal, 2 high) from the `Importance`
/// or `X-Priority` header.
fn importance(email: &Value) -> &'static str {
    let importance = email["header:Importance:asText"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let priority = email["header:X-Priority:asText"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .chars()
        .next();
    match (importance.as_str(), priority) {
        ("high", _) | (_, Some('1' | '2')) => "2",
        ("low", _) | (_, Some('4' | '5')) => "0",
        _ => "1",
    }
}

/// A JMAP UTC date as ActiveSync writes dates (milliseconds, `Z`).
pub fn eas_date(jmap_date: &str) -> String {
    match jmap_date.strip_suffix('Z') {
        Some(base) if !base.contains('.') => format!("{base}.000Z"),
        _ => jmap_date.to_string(),
    }
}

/// The read and flag elements, for an Add and for a Change.
pub fn flag_elements(bits: u8) -> [Element; 2] {
    let flag = if bits & flags::FLAGGED != 0 {
        Element::new(email::FLAG)
            .with(Element::text(email::STATUS, "2"))
            .with(Element::text(email::FLAG_TYPE, "Flag for follow up"))
    } else {
        Element::new(email::FLAG)
    };
    [
        Element::text(email::READ, if bits & flags::SEEN != 0 { "1" } else { "0" }),
        flag,
    ]
}

/// A message as `ApplicationData`.
fn application_data(message: &Value, body: Element) -> Element {
    let bits = super::flags_of(&message["keywords"]);
    let [read, flag] = flag_elements(bits);
    let subject = message["subject"].as_str().unwrap_or_default();
    let mut data = Element::new(airsync::APPLICATION_DATA)
        .with_opt(addresses(&message["to"]).map(|to| Element::text(email::TO, to)))
        .with_opt(addresses(&message["cc"]).map(|cc| Element::text(email::CC, cc)))
        .with_opt(addresses(&message["from"]).map(|from| Element::text(email::FROM, from)))
        .with(Element::text(email::SUBJECT, subject))
        .with_opt(addresses(&message["replyTo"]).map(|to| Element::text(email::REPLY_TO, to)))
        .with(Element::text(
            email::DATE_RECEIVED,
            eas_date(
                message["receivedAt"]
                    .as_str()
                    .unwrap_or("1970-01-01T00:00:00Z"),
            ),
        ))
        .with_opt(display_names(&message["to"]).map(|to| Element::text(email::DISPLAY_TO, to)))
        .with(Element::text(email::THREAD_TOPIC, subject))
        .with(Element::text(email::IMPORTANCE, importance(message)))
        .with(read);

    let mut attachments = Element::new(base::ATTACHMENTS);
    for part in message["attachments"].as_array().into_iter().flatten() {
        let Some(blob) = part["blobId"].as_str() else {
            continue;
        };
        let id = message["id"].as_str().unwrap_or_default();
        let inline = part["disposition"].as_str() == Some("inline") && part["cid"].is_string();
        attachments.push(
            Element::new(base::ATTACHMENT)
                .with(Element::text(
                    base::DISPLAY_NAME,
                    part["name"].as_str().unwrap_or("attachment"),
                ))
                .with(Element::text(
                    base::FILE_REFERENCE,
                    file_reference(id, blob),
                ))
                .with(Element::text(base::METHOD, "1"))
                .with(Element::text(
                    base::ESTIMATED_DATA_SIZE,
                    part["size"].as_u64().unwrap_or(0).to_string(),
                ))
                .with_opt(
                    part["cid"]
                        .as_str()
                        .map(|cid| Element::text(base::CONTENT_ID, cid)),
                )
                .with_opt(inline.then(|| Element::text(base::IS_INLINE, "1"))),
        );
    }
    if !attachments.children().is_empty() {
        data.push(attachments);
    }
    let native = if has_type(&message["htmlBody"], "text/html") {
        body_type::HTML
    } else {
        body_type::PLAIN
    };
    data = data
        .with(body)
        .with(Element::text(email::MESSAGE_CLASS, "IPM.Note"))
        .with(Element::text(email::INTERNET_CPID, "65001"))
        .with(flag)
        .with(Element::text(
            email::CONTENT_CLASS,
            "urn:content-classes:message",
        ))
        .with(Element::text(base::NATIVE_BODY_TYPE, native.to_string()));
    if let Some(thread) = message["threadId"].as_str() {
        data.push(Element::opaque(
            email2::CONVERSATION_ID,
            thread.as_bytes().to_vec(),
        ));
    }
    if bits & flags::FORWARDED != 0 {
        data.push(Element::text(email2::LAST_VERB_EXECUTED, "3"));
    } else if bits & flags::ANSWERED != 0 {
        data.push(Element::text(email2::LAST_VERB_EXECUTED, "1"));
    }
    data
}

/// The `FileReference` of an attachment: the message and the blob, so
/// ItemOperations can check the message is the device's to read.
pub fn file_reference(email_id: &str, blob_id: &str) -> String {
    format!("{email_id}.{blob_id}")
}

/// The message and blob of a `FileReference`.
pub fn parse_file_reference(reference: &str) -> Option<(&str, &str)> {
    reference
        .split_once('.')
        .filter(|(email, blob)| !email.is_empty() && !blob.is_empty())
}

/// The keyword changes in a device's `Change` of a message: `email:Read`
/// and `email:Flag` (status 2 flags; 0, 1 or an empty flag clear it).
/// `None` for each the device didn't send.
pub fn keyword_patch(data: &Element) -> (Option<bool>, Option<bool>) {
    let read = data.child_u64(email::READ).map(|read| read == 1);
    let flagged = data
        .child(email::FLAG)
        .map(|flag| flag.child_u64(email::STATUS) == Some(2));
    (read, flagged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use activesync_proto::wbxml::from_xml;

    #[test]
    fn body_options_from_the_spec_request() {
        // [MS-ASEMAIL] 4.1.2, the request's Options.
        let sync = from_xml(
            r#"<Sync xmlns:airsyncbase="AirSyncBase:" xmlns="AirSync:"><Collections><Collection>
              <Options><MIMESupport>0</MIMESupport><airsyncbase:BodyPreference>
              <airsyncbase:Type>2</airsyncbase:Type>
              <airsyncbase:TruncationSize>5120</airsyncbase:TruncationSize>
              </airsyncbase:BodyPreference></Options></Collection></Collections></Sync>"#,
        )
        .expect("parses");
        let options = sync
            .child(airsync::COLLECTIONS)
            .and_then(|c| c.child(airsync::COLLECTION))
            .and_then(|c| c.child(airsync::OPTIONS))
            .expect("options");
        let parsed = BodyOptions::parse(options);
        assert_eq!(parsed.mime_support, 0);
        assert_eq!(
            parsed.choose(),
            BodyPreference {
                kind: body_type::HTML,
                truncation: Some(5120),
                all_or_none: false,
                preview: None
            }
        );
        // MIME is chosen only with MIMESupport 2.
        let mime = BodyOptions {
            preferences: vec![
                BodyPreference {
                    kind: body_type::MIME,
                    truncation: None,
                    all_or_none: false,
                    preview: None,
                },
                BodyPreference {
                    kind: body_type::PLAIN,
                    truncation: Some(10),
                    all_or_none: false,
                    preview: None,
                },
            ],
            mime_support: 0,
        };
        assert_eq!(mime.choose().kind, body_type::PLAIN);
        assert_eq!(
            BodyOptions {
                mime_support: 2,
                ..mime
            }
            .choose()
            .kind,
            body_type::MIME
        );
    }

    #[test]
    fn message_as_application_data() {
        let message = json!({
            "id": "e1", "threadId": "t1", "keywords": { "$seen": true, "$flagged": true },
            "receivedAt": "2009-02-19T01:43:25Z", "subject": "Sample",
            "from": [{ "name": "Device \"User2\"", "email": "someone2@example.com" }],
            "to": [{ "name": "Device User", "email": "someone1@example.com" }, { "email": "x@example.com" }],
            "attachments": [{ "blobId": "b1", "name": "Test.txt", "size": 84 }],
            "htmlBody": [{ "partId": "2", "type": "text/html" }],
            "header:X-Priority:asText": " 1 (Highest)",
        });
        let body = Element::new(base::BODY);
        let data = application_data(&message, body);
        assert_eq!(
            data.child_str(email::TO),
            Some("\"Device User\" <someone1@example.com>, x@example.com")
        );
        assert_eq!(
            data.child_str(email::FROM),
            Some("\"Device 'User2'\" <someone2@example.com>")
        );
        assert_eq!(
            data.child_str(email::DISPLAY_TO),
            Some("Device User; x@example.com")
        );
        assert_eq!(
            data.child_str(email::DATE_RECEIVED),
            Some("2009-02-19T01:43:25.000Z")
        );
        assert_eq!(data.child_str(email::IMPORTANCE), Some("2"));
        assert_eq!(data.child_str(email::READ), Some("1"));
        assert_eq!(
            data.child(email::FLAG)
                .and_then(|f| f.child_str(email::STATUS)),
            Some("2")
        );
        assert_eq!(data.child_str(base::NATIVE_BODY_TYPE), Some("2"));
        let attachment = data
            .child(base::ATTACHMENTS)
            .and_then(|a| a.child(base::ATTACHMENT))
            .expect("attachment");
        assert_eq!(attachment.child_str(base::FILE_REFERENCE), Some("e1.b1"));
        assert_eq!(parse_file_reference("e1.b1"), Some(("e1", "b1")));
        assert_eq!(parse_file_reference("e1"), None);
    }

    #[test]
    fn cutting_and_patches() {
        assert_eq!(cut("héllo", 2), "h");
        assert_eq!(cut("abc", 10), "abc");
        let change = from_xml(
            r#"<ApplicationData xmlns="AirSync:" xmlns:email="Email:"><email:Read>1</email:Read>
               <email:Flag><email:Status>2</email:Status></email:Flag></ApplicationData>"#,
        )
        .expect("parses");
        assert_eq!(keyword_patch(&change), (Some(true), Some(true)));
        let clear = from_xml(
            r#"<ApplicationData xmlns="AirSync:" xmlns:email="Email:"><email:Flag/></ApplicationData>"#,
        )
        .expect("parses");
        assert_eq!(keyword_patch(&clear), (None, Some(false)));
    }
}
