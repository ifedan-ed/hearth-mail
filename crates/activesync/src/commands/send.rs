/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! SendMail, SmartReply and SmartForward ([MS-ASCMD] 2.2.1.17, 2.2.1.18,
//! 2.2.1.19). The device sends a whole message (MIME); it is imported into
//! the account and submitted with JMAP `EmailSubmission`, so sending from
//! a phone goes through the same identity checks, signing and queue as any
//! other client. Without `SaveInSentItems` the copy is removed once sent.
//!
//! SmartReply and SmartForward carry only what the person wrote; the server
//! adds the original: quoted below the text for a reply, quoted with its
//! attachments for a forward. With `ReplaceMime` the device's message is
//! sent as it is.
//!
//! The `Bcc` header is taken out of the message that is stored and sent;
//! its addresses stay in the envelope.

use crate::{
    Body, Ctx, Reply,
    error::{Error, Result},
    jmap::Call,
};
use activesync_proto::wbxml::{Element, codepages::composemail as cm};
use jmap::blob::{download::BlobDownload, upload::BlobUpload};
use mail_builder::{MessageBuilder, headers::address::Address as BuilderAddress};
use mail_parser::{Address, Message, MessageParser, MimeHeaders};
use serde_json::{Value, json};
use types::{blob::BlobId, id::Id};

/// Status codes of the three commands ([MS-ASCMD] 2.2.2).
mod status {
    pub const INVALID_MIME: &str = "107";
    pub const QUOTA: &str = "113";
    pub const NO_RECIPIENT: &str = "119";
    pub const SUBMISSION_FAILED: &str = "120";
    pub const REPLY_FAILED: &str = "121";
    pub const NOT_FOUND: &str = "150";
}

/// Which smart command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Smart {
    Reply,
    Forward,
}

/// What the device asked to send.
struct Outgoing {
    mime: Vec<u8>,
    save: bool,
    replace_mime: bool,
    /// The original's message id (SmartReply, SmartForward).
    source: Option<String>,
}

fn read_request(ctx: &Ctx<'_>, body: Body, root: activesync_proto::wbxml::Tag) -> Result<Outgoing> {
    match body {
        // The plain form: the message itself, options in the query.
        Body::Mime(mime) => Ok(Outgoing {
            mime,
            save: ctx.line.save_in_sent,
            replace_mime: false,
            source: ctx
                .line
                .item_id
                .clone()
                .or_else(|| ctx.line.long_id.clone()),
        }),
        Body::Wbxml(request) if request.tag == root => {
            let source = request.child(cm::SOURCE);
            Ok(Outgoing {
                mime: request
                    .child(cm::MIME)
                    .map(|mime| mime.as_bytes().to_vec())
                    .unwrap_or_default(),
                save: request.child(cm::SAVE_IN_SENT_ITEMS).is_some() || ctx.line.save_in_sent,
                replace_mime: request.child(cm::REPLACE_MIME).is_some(),
                source: source
                    .and_then(|source| {
                        source
                            .child_str(cm::ITEM_ID)
                            .or_else(|| source.child_str(cm::LONG_ID))
                    })
                    .map(str::to_string),
            })
        }
        _ => Err(Error::BadRequest("expected a message".into())),
    }
}

pub async fn send_mail(ctx: &Ctx<'_>, body: Body) -> Result<Reply> {
    let outgoing = read_request(ctx, body, cm::SEND_MAIL)?;
    let code = submit(ctx, &outgoing.mime, outgoing.save, None).await?;
    Ok(result(cm::SEND_MAIL, code))
}

pub async fn smart(ctx: &Ctx<'_>, body: Body, kind: Smart) -> Result<Reply> {
    let root = match kind {
        Smart::Reply => cm::SMART_REPLY,
        Smart::Forward => cm::SMART_FORWARD,
    };
    let outgoing = read_request(ctx, body, root)?;
    let Some(source) = outgoing.source.clone() else {
        return Ok(result(root, Some(status::NOT_FOUND)));
    };
    let mime = if outgoing.replace_mime {
        outgoing.mime.clone()
    } else {
        let Some(original) = original(ctx, &source).await? else {
            return Ok(result(root, Some(status::NOT_FOUND)));
        };
        match combine(&outgoing.mime, &original, kind) {
            Some(mime) => mime,
            None => return Ok(result(root, Some(status::INVALID_MIME))),
        }
    };
    let keyword = match kind {
        Smart::Reply => "$answered",
        Smart::Forward => "$forwarded",
    };
    let code = submit(ctx, &mime, outgoing.save, Some((&source, keyword))).await?;
    // A failed submission of a reply has its own code.
    let code = code.map(|code| {
        if code == status::SUBMISSION_FAILED && kind == Smart::Reply {
            status::REPLY_FAILED
        } else {
            code
        }
    });
    Ok(result(root, code))
}

/// Success is an empty answer; a failure names its status.
fn result(root: activesync_proto::wbxml::Tag, code: Option<&str>) -> Reply {
    match code {
        None => Reply::Empty,
        Some(code) => Reply::Wbxml(Element::new(root).with(Element::text(cm::STATUS, code))),
    }
}

/// The original message's raw bytes.
async fn original(ctx: &Ctx<'_>, email_id: &str) -> Result<Option<Vec<u8>>> {
    let answers = ctx
        .jmap(vec![Call::new(
            "Email/get",
            "o",
            json!({ "ids": [email_id], "properties": ["blobId"] }),
        )])
        .await?;
    let Some(blob) = answers.get("o")?["list"]
        .as_array()
        .and_then(|list| list.first())
        .and_then(|email| email["blobId"].as_str())
        .and_then(BlobId::from_base32)
    else {
        return Ok(None);
    };
    Ok(ctx.server.blob_download(&blob, ctx.token).await?)
}

/// The person's message with the original added: quoted text, and for a
/// forward the original's attachments. `None` when either can't be read.
fn combine(mine: &[u8], original: &[u8], kind: Smart) -> Option<Vec<u8>> {
    let parser = MessageParser::default();
    let mine = parser.parse(mine)?;
    let original = parser.parse(original)?;

    let original_text = original.body_text(0).unwrap_or_default();
    let from = original
        .from()
        .and_then(|from| from.first())
        .map(|from| match (&from.name, &from.address) {
            (Some(name), Some(address)) => format!("{name} <{address}>"),
            (_, Some(address)) => address.to_string(),
            (Some(name), None) => name.to_string(),
            (None, None) => String::new(),
        })
        .unwrap_or_default();
    let date = original
        .date()
        .map(|date| date.to_rfc822())
        .unwrap_or_default();
    let header = match kind {
        Smart::Reply => format!("On {date}, {from} wrote:"),
        Smart::Forward => format!(
            "---------- Forwarded message ----------\r\nFrom: {from}\r\nDate: {date}\r\nSubject: {}",
            original.subject().unwrap_or_default()
        ),
    };
    let quoted: String = match kind {
        Smart::Reply => original_text
            .lines()
            .map(|line| format!("> {line}\r\n"))
            .collect(),
        Smart::Forward => original_text.replace('\n', "\r\n"),
    };
    let text = format!(
        "{}\r\n\r\n{header}\r\n{quoted}",
        mine.body_text(0).unwrap_or_default().trim_end()
    );

    let mut builder = MessageBuilder::new()
        .subject(mine.subject().unwrap_or_default().to_string())
        .text_body(text);
    if let Some(from) = mine.from().and_then(to_builder) {
        builder = builder.from(from);
    }
    if let Some(to) = mine.to().and_then(to_builder) {
        builder = builder.to(to);
    }
    if let Some(cc) = mine.cc().and_then(to_builder) {
        builder = builder.cc(cc);
    }
    // HTML only when the person's message has a real text/html part:
    // mail-parser offers plain text as HTML too, which would turn a
    // plain-text reply into an HTML one.
    if mine
        .html_part(0)
        .is_some_and(mail_parser::MessagePart::is_text_html)
        && let Some(html) = mine.body_html(0)
    {
        let original_html = original
            .body_html(0)
            .map(|html| html.into_owned())
            .unwrap_or_else(|| escape_html(&original_text).replace('\n', "<br>"));
        builder = builder.html_body(format!(
            "{html}<br><div>{}</div><blockquote style=\"margin:0 0 0 .8ex;border-left:1px solid #ccc;padding-left:1ex\">{original_html}</blockquote>",
            escape_html(&header).replace("\r\n", "<br>")
        ));
    }
    if kind == Smart::Reply
        && let Some(id) = original.message_id()
    {
        builder = builder.in_reply_to(id.to_string());
        let mut references: Vec<String> = original
            .references()
            .as_text_list()
            .map(|list| list.iter().map(|id| id.to_string()).collect())
            .unwrap_or_default();
        references.push(id.to_string());
        builder = builder.references(references);
    }
    let mut attachments: Vec<(String, String, Vec<u8>)> = Vec::new();
    let mut add_from = |message: &Message<'_>| {
        for part in message.attachments() {
            let content_type = part
                .content_type()
                .map(|ct| match &ct.c_subtype {
                    Some(sub) => format!("{}/{sub}", ct.c_type),
                    None => ct.c_type.to_string(),
                })
                .unwrap_or_else(|| "application/octet-stream".into());
            attachments.push((
                content_type,
                part.attachment_name().unwrap_or("attachment").to_string(),
                part.contents().to_vec(),
            ));
        }
    };
    add_from(&mine);
    if kind == Smart::Forward {
        add_from(&original);
    }
    for (content_type, name, bytes) in attachments {
        builder = builder.attachment(content_type, name, bytes);
    }
    builder.write_to_vec().ok()
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A parsed address list as mail-builder takes it.
fn to_builder(address: &Address<'_>) -> Option<BuilderAddress<'static>> {
    let list: Vec<BuilderAddress<'static>> = address
        .iter()
        .filter_map(|addr| {
            let email = addr.address.as_deref()?.to_string();
            Some(BuilderAddress::new_address(
                addr.name.as_deref().map(|name| name.to_string()),
                email,
            ))
        })
        .collect();
    match list.len() {
        0 => None,
        1 => list.into_iter().next(),
        _ => Some(BuilderAddress::new_list(list)),
    }
}

/// The message without its `Bcc` header (and the header's folded lines).
pub fn strip_bcc(raw: &[u8]) -> Vec<u8> {
    let header_end = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|pos| pos + 2)
        .or_else(|| raw.windows(2).position(|w| w == b"\n\n").map(|pos| pos + 1))
        .unwrap_or(raw.len());
    let (headers, body) = raw.split_at(header_end);
    let mut out = Vec::with_capacity(raw.len());
    let mut skipping = false;
    for line in headers.split_inclusive(|byte| *byte == b'\n') {
        let folded = line
            .first()
            .is_some_and(|byte| *byte == b' ' || *byte == b'\t');
        if !folded {
            skipping = line.len() >= 4 && line[..4].eq_ignore_ascii_case(b"bcc:");
        }
        if !skipping {
            out.extend_from_slice(line);
        }
    }
    out.extend_from_slice(body);
    out
}

/// Imports and submits `mime`. Returns the failure status, or `None` when
/// it was sent. `answered` marks the original with a keyword.
async fn submit(
    ctx: &Ctx<'_>,
    mime: &[u8],
    save: bool,
    answered: Option<(&str, &str)>,
) -> Result<Option<&'static str>> {
    let Some(message) = MessageParser::default().parse(mime) else {
        return Ok(Some(status::INVALID_MIME));
    };
    let recipients: Vec<String> = [message.to(), message.cc(), message.bcc()]
        .into_iter()
        .flatten()
        .flat_map(|list| list.iter())
        .filter_map(|addr| addr.address.as_deref().map(str::to_lowercase))
        .collect();
    if recipients.is_empty() {
        return Ok(Some(status::NO_RECIPIENT));
    }
    let from = message
        .from()
        .and_then(|from| from.first())
        .and_then(|from| from.address.as_deref())
        .map(str::to_lowercase);
    let stripped = strip_bcc(mime);

    // The identity the message is sent as: the one with the From address,
    // or the account's first when the device left From out.
    let mut answers = ctx
        .jmap(vec![
            Call::new("Identity/get", "i", json!({})),
            Call::new(
                "Mailbox/query",
                "s",
                json!({ "filter": { "role": "sent" } }),
            ),
            Call::new(
                "Mailbox/query",
                "d",
                json!({ "filter": { "role": "drafts" } }),
            ),
        ])
        .await?;
    let identities = answers.take("i")?;
    let identity = identities["list"].as_array().and_then(|list| {
        list.iter()
            .find(|identity| {
                from.as_deref().is_none_or(|from| {
                    identity["email"]
                        .as_str()
                        .is_some_and(|email| email.eq_ignore_ascii_case(from))
                })
            })
            .cloned()
    });
    let Some(identity) = identity else {
        return Ok(Some(status::SUBMISSION_FAILED));
    };
    let mail_from = from
        .or_else(|| identity["email"].as_str().map(str::to_string))
        .unwrap_or_default();
    let mailbox = first_id(&answers.take("s")?)
        .filter(|_| save)
        .or_else(|| first_id(&answers.get("d").ok().cloned().unwrap_or_default()));
    let Some(mailbox) = mailbox else {
        return Ok(Some(status::SUBMISSION_FAILED));
    };

    let upload = ctx
        .server
        .blob_upload(
            Id::from(ctx.account_id()),
            "message/rfc822",
            &stripped,
            ctx.token,
        )
        .await?;
    let upload = serde_json::to_value(&upload).map_err(|err| Error::Internal(err.to_string()))?;
    let blob = upload["blobId"].as_str().unwrap_or_default().to_string();

    let mut calls = vec![
        Call::new(
            "Email/import",
            "imp",
            json!({ "emails": { "m": {
                "blobId": blob,
                "mailboxIds": { mailbox: true },
                "keywords": { "$seen": true },
            } } }),
        ),
        Call::new(
            "EmailSubmission/set",
            "sub",
            json!({
                "create": { "s": {
                    "identityId": identity["id"],
                    "emailId": "#m",
                    "envelope": {
                        "mailFrom": { "email": mail_from },
                        "rcptTo": recipients.iter().map(|r| json!({ "email": r })).collect::<Vec<_>>(),
                    },
                } },
                // Not kept: the copy goes once it is sent.
                "onSuccessDestroyEmail": if save { json!([]) } else { json!(["#s"]) },
            }),
        ),
    ];
    if let Some((original, keyword)) = answered {
        let mut patch = serde_json::Map::new();
        patch.insert(format!("keywords/{keyword}"), Value::Bool(true));
        calls.push(Call::new(
            "Email/set",
            "orig",
            json!({ "update": { original: patch } }),
        ));
    }
    let answers = ctx.jmap(calls).await?;
    let imported = answers.get("imp")?;
    if imported["created"]["m"]["id"].as_str().is_none() {
        let reason = imported["notCreated"]["m"]["type"]
            .as_str()
            .unwrap_or_default();
        return Ok(Some(if reason == "overQuota" {
            status::QUOTA
        } else {
            status::INVALID_MIME
        }));
    }
    let submitted = answers.get("sub")?;
    if submitted["created"]["s"]["id"].as_str().is_none() {
        trc::event!(
            Http(trc::HttpEvent::Error),
            SpanId = ctx.session.session_id,
            Details = "ActiveSync message not submitted",
            Reason = submitted["notCreated"]["s"].to_string(),
        );
        return Ok(Some(status::SUBMISSION_FAILED));
    }
    Ok(None)
}

/// Sends a message the server made (a meeting answer), keeping a copy in
/// Sent when `save`. `Err` only for server faults; a refusal is logged.
pub async fn submit_message(ctx: &Ctx<'_>, mime: &[u8], save: bool) -> Result<()> {
    if let Some(code) = submit(ctx, mime, save, None).await? {
        trc::event!(
            Http(trc::HttpEvent::Error),
            SpanId = ctx.session.session_id,
            Details = "A meeting answer wasn't sent",
            Reason = code.to_string(),
        );
    }
    Ok(())
}

fn first_id(query: &Value) -> Option<String> {
    query["ids"]
        .as_array()
        .and_then(|ids| ids.first())
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bcc_is_taken_out_with_its_folded_lines() {
        let raw = b"From: a@b.c\r\nBcc: x@y.z,\r\n w@y.z\r\nTo: d@e.f\r\nbcc: q@r.s\r\n\r\nBcc: in the body stays\r\n";
        let stripped = strip_bcc(raw);
        assert_eq!(
            String::from_utf8_lossy(&stripped),
            "From: a@b.c\r\nTo: d@e.f\r\n\r\nBcc: in the body stays\r\n"
        );
    }

    #[test]
    fn reply_quotes_and_threads() {
        let original = b"From: Ann <ann@example.org>\r\nTo: me@example.com\r\nSubject: Plans\r\nMessage-ID: <orig@example.org>\r\nDate: Mon, 23 Nov 2026 10:00:00 +0000\r\n\r\nShall we meet?\r\n";
        let mine = b"From: me@example.com\r\nTo: ann@example.org\r\nSubject: Re: Plans\r\n\r\nYes, at noon.\r\n";
        let combined = combine(mine, original, Smart::Reply).expect("combines");
        let parsed = MessageParser::default().parse(&combined).expect("parses");
        let text = parsed.body_text(0).expect("text");
        assert!(text.starts_with("Yes, at noon."), "{text}");
        assert!(text.contains("> Shall we meet?"), "{text}");
        assert_eq!(parsed.in_reply_to().as_text(), Some("orig@example.org"));
        assert_eq!(parsed.subject(), Some("Re: Plans"));
        // A plain-text reply stays plain text.
        assert!(
            !parsed.parts.iter().any(|part| part.is_text_html()),
            "{}",
            String::from_utf8_lossy(&combined)
        );
    }

    #[test]
    fn forward_carries_the_attachments() {
        let original = b"From: ann@example.org\r\nSubject: Report\r\nContent-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nSee attached.\r\n--b\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"r.pdf\"\r\n\r\nPDFDATA\r\n--b--\r\n";
        let mine = b"From: me@example.com\r\nTo: bob@example.net\r\nSubject: Fwd: Report\r\n\r\nFor you.\r\n";
        let combined = combine(mine, original, Smart::Forward).expect("combines");
        let parsed = MessageParser::default().parse(&combined).expect("parses");
        assert!(parsed.body_text(0).expect("text").contains("See attached."));
        let names: Vec<_> = parsed
            .attachments()
            .filter_map(|part| part.attachment_name())
            .collect();
        assert_eq!(names, ["r.pdf"]);
    }
}
