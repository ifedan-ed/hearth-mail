/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! ItemOperations ([MS-ASCMD] 2.2.1.10): fetch a whole message (by folder
//! and id, or by a search result's `LongId`), fetch an attachment (by its
//! `FileReference`), and empty a folder; and MoveItems (2.2.1.12).
//!
//! Attachments go inline as base64, or as binary parts of a multipart
//! answer (2.2.1.10.1.1) when the device asked for one, which saves the
//! base64 third on large files.

use super::sync::MailFolder;
use crate::{
    Ctx, Reply,
    error::Result,
    folders,
    jmap::Call,
    mail::item::{self, BodyOptions},
};
use activesync_proto::wbxml::{
    self, Element,
    codepages::{airsync, airsyncbase as base, itemoperations as io, r#move as mv, search},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use jmap::blob::download::BlobDownload;
use serde_json::{Value, json};
use types::blob::BlobId;

/// ItemOperations status codes ([MS-ASCMD] 2.2.3.177.8).
mod status {
    pub const OK: &str = "1";
    pub const PROTOCOL: &str = "2";
    pub const BAD_RANGE: &str = "8";
    pub const UNKNOWN_STORE: &str = "9";
    pub const ITEM_FAILED: &str = "14";
    pub const ATTACHMENT_FAILED: &str = "15";
}

/// The multipart media type (2.2.1.10.1).
const MULTIPART: &str = "application/vnd.ms-sync.multipart";

pub async fn item_operations(ctx: &Ctx<'_>, request: Element) -> Result<Reply> {
    let multipart = ctx.line.accept_multipart;
    let mut parts: Vec<Vec<u8>> = Vec::new();
    let mut response = Element::new(io::RESPONSE);
    for operation in request.children() {
        let element = match operation.tag {
            io::FETCH => fetch(ctx, operation, multipart.then_some(&mut parts)).await?,
            io::EMPTY_FOLDER_CONTENTS => empty_folder(ctx, operation).await?,
            _ => Element::new(operation.tag).with(Element::text(io::STATUS, status::PROTOCOL)),
        };
        response.push(element);
    }
    let document = Element::new(io::ITEM_OPERATIONS)
        .with(Element::text(io::STATUS, status::OK))
        .with(response);
    if !multipart {
        return Ok(Reply::Wbxml(document));
    }
    // Part 0 is the document; the rest are the data its `Part`s name.
    let mut all = Vec::with_capacity(parts.len() + 1);
    all.push(wbxml::encode(&document));
    all.append(&mut parts);
    Ok(Reply::Raw(MULTIPART, multipart_body(&all)))
}

/// The MultiPartResponse structure: a count, (offset, length) per part, the
/// parts; integers little-endian.
fn multipart_body(parts: &[Vec<u8>]) -> Vec<u8> {
    let header = 4 + parts.len() * 8;
    let total: usize = parts.iter().map(Vec::len).sum();
    let mut out = Vec::with_capacity(header + total);
    out.extend_from_slice(&(parts.len() as u32).to_le_bytes());
    let mut offset = header;
    for part in parts {
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        out.extend_from_slice(&(part.len() as u32).to_le_bytes());
        offset += part.len();
    }
    for part in parts {
        out.extend_from_slice(part);
    }
    out
}

/// One Fetch. `parts` collects binary parts when the answer is multipart.
async fn fetch(
    ctx: &Ctx<'_>,
    request: &Element,
    parts: Option<&mut Vec<Vec<u8>>>,
) -> Result<Element> {
    let store = request.child_str(io::STORE).unwrap_or("Mailbox");
    if !store.eq_ignore_ascii_case("Mailbox") {
        return Ok(Element::new(io::FETCH).with(Element::text(io::STATUS, status::UNKNOWN_STORE)));
    }
    let options = request
        .child(io::OPTIONS)
        .map(BodyOptions::parse)
        .unwrap_or_default();

    if let Some(reference) = request.child_str(base::FILE_REFERENCE) {
        return fetch_attachment(ctx, reference, request.child(io::OPTIONS), parts).await;
    }
    let (email_id, echo) = if let Some(long_id) = request.child_str(search::LONG_ID) {
        (long_id, Element::text(search::LONG_ID, long_id))
    } else if let Some(server_id) = request.child_str(airsync::SERVER_ID) {
        (server_id, Element::text(airsync::SERVER_ID, server_id))
    } else {
        return Ok(Element::new(io::FETCH).with(Element::text(io::STATUS, status::PROTOCOL)));
    };
    let rendered = ctx.render_emails(&[email_id.to_string()], &options).await?;
    let Some((_, _, data)) = rendered.into_iter().next() else {
        return Ok(Element::new(io::FETCH)
            .with(Element::text(io::STATUS, status::ITEM_FAILED))
            .with(echo));
    };
    // The same children as Sync's ApplicationData, under Properties.
    let mut properties = Element::new(io::PROPERTIES);
    for child in data.children() {
        properties.push(child.clone());
    }
    Ok(Element::new(io::FETCH)
        .with(Element::text(io::STATUS, status::OK))
        .with_opt(
            request
                .child_str(airsync::COLLECTION_ID)
                .map(|id| Element::text(airsync::COLLECTION_ID, id)),
        )
        .with(echo)
        .with(Element::text(airsync::CLASS, "Email"))
        .with(properties))
}

/// An attachment's bytes, whole or the `Range` asked for.
async fn fetch_attachment(
    ctx: &Ctx<'_>,
    reference: &str,
    options: Option<&Element>,
    parts: Option<&mut Vec<Vec<u8>>>,
) -> Result<Element> {
    let failed = |code: &str| {
        Element::new(io::FETCH)
            .with(Element::text(io::STATUS, code))
            .with(Element::text(base::FILE_REFERENCE, reference))
    };
    let Some((email_id, blob_id)) = item::parse_file_reference(reference) else {
        return Ok(failed(status::ATTACHMENT_FAILED));
    };
    // The attachment must belong to a message this account can read.
    let answers = ctx
        .jmap(vec![Call::new(
            "Email/get",
            "g",
            json!({ "ids": [email_id], "properties": ["attachments"] }),
        )])
        .await?;
    let attachment = answers.get("g")?["list"]
        .as_array()
        .and_then(|list| list.first())
        .and_then(|email| email["attachments"].as_array())
        .and_then(|list| {
            list.iter()
                .find(|part| part["blobId"].as_str() == Some(blob_id))
                .cloned()
        });
    let (Some(attachment), Some(blob)) = (attachment, BlobId::from_base32(blob_id)) else {
        return Ok(failed(status::ATTACHMENT_FAILED));
    };
    let Some(bytes) = ctx.server.blob_download(&blob, ctx.token).await? else {
        return Ok(failed(status::ATTACHMENT_FAILED));
    };
    let total = bytes.len();
    let range = match options.and_then(|options| options.child_str(io::RANGE)) {
        None => None,
        Some(range) => match parse_range(range) {
            Some((start, end)) if start < total || total == 0 => {
                Some((start.min(total), (end + 1).min(total)))
            }
            _ => return Ok(failed(status::BAD_RANGE)),
        },
    };
    let slice = match range {
        Some((start, end)) => &bytes[start..end],
        None => &bytes[..],
    };
    let mut properties = Element::new(io::PROPERTIES).with(Element::text(
        base::CONTENT_TYPE,
        attachment["type"]
            .as_str()
            .unwrap_or("application/octet-stream"),
    ));
    if let Some((start, end)) = range {
        // The range actually sent, inclusive.
        properties.push(Element::text(
            io::RANGE,
            format!("{start}-{}", end.saturating_sub(1).max(start)),
        ));
        properties.push(Element::text(io::TOTAL, total.to_string()));
    }
    match parts {
        Some(parts) => {
            parts.push(slice.to_vec());
            // Part 0 is the document, so this one is parts.len().
            properties.push(Element::text(io::PART, parts.len().to_string()));
        }
        None => properties.push(Element::text(io::DATA, STANDARD.encode(slice))),
    }
    Ok(Element::new(io::FETCH)
        .with(Element::text(io::STATUS, status::OK))
        .with(Element::text(base::FILE_REFERENCE, reference))
        .with(properties))
}

/// `start-end`, inclusive, as ItemOperations writes ranges.
fn parse_range(range: &str) -> Option<(usize, usize)> {
    let (start, end) = range.trim().split_once('-')?;
    let (start, end) = (start.trim().parse().ok()?, end.trim().parse().ok()?);
    (start <= end).then_some((start, end))
}

/// EmptyFolderContents: everything in a mail folder (Deleted Items, as
/// devices use it) is deleted.
async fn empty_folder(ctx: &Ctx<'_>, request: &Element) -> Result<Element> {
    let failed =
        |code: &str| Element::new(io::EMPTY_FOLDER_CONTENTS).with(Element::text(io::STATUS, code));
    let Some(folder_id) = request.child_str(airsync::COLLECTION_ID) else {
        return Ok(failed(status::PROTOCOL));
    };
    let Some(mailbox) = folders::mailbox_of(folder_id) else {
        return Ok(failed(status::PROTOCOL));
    };
    loop {
        let answers = ctx
            .jmap(vec![
                Call::new(
                    "Email/query",
                    "q",
                    json!({ "filter": { "inMailbox": mailbox }, "limit": 500 }),
                ),
                Call::new(
                    "Email/set",
                    "d",
                    json!({ "#destroy": { "resultOf": "q", "name": "Email/query", "path": "/ids" } }),
                ),
            ])
            .await?;
        let found = answers.get("q")?["ids"].as_array().map_or(0, Vec::len);
        answers.get("d")?;
        if found < 500 {
            break;
        }
    }
    Ok(Element::new(io::EMPTY_FOLDER_CONTENTS)
        .with(Element::text(io::STATUS, status::OK))
        .with(Element::text(airsync::COLLECTION_ID, folder_id)))
}

/// MoveItems status codes ([MS-ASCMD] 2.2.3.177.11).
mod move_status {
    pub const BAD_SOURCE: &str = "1";
    pub const BAD_DESTINATION: &str = "2";
    pub const OK: &str = "3";
    pub const SAME: &str = "4";
    pub const FAILED: &str = "5";
}

/// MoveItems: messages to another mail folder. The message keeps its id,
/// and the device's state of both folders is updated, so the next Sync
/// neither deletes it from the old folder nor adds it to the new one again.
pub async fn move_items(ctx: &Ctx<'_>, request: Element) -> Result<Reply> {
    let mut answer = Element::new(mv::MOVE_ITEMS);
    for item in request.children_of(mv::MOVE) {
        let source = item.child_str(mv::SRC_MSG_ID).unwrap_or_default();
        let from = item.child_str(mv::SRC_FLD_ID).unwrap_or_default();
        let to = item.child_str(mv::DST_FLD_ID).unwrap_or_default();
        let code = move_one(ctx, source, from, to).await?;
        answer.push(
            Element::new(mv::RESPONSE)
                .with(Element::text(mv::SRC_MSG_ID, source))
                .with(Element::text(mv::STATUS, code))
                .with_opt((code == move_status::OK).then(|| Element::text(mv::DST_MSG_ID, source))),
        );
    }
    Ok(Reply::Wbxml(answer))
}

async fn move_one(ctx: &Ctx<'_>, email: &str, from: &str, to: &str) -> Result<&'static str> {
    if from == to {
        return Ok(move_status::SAME);
    }
    let Some(source) = folders::mailbox_of(from) else {
        return Ok(move_status::BAD_SOURCE);
    };
    let Some(destination) = folders::mailbox_of(to) else {
        return Ok(move_status::BAD_DESTINATION);
    };
    let mut patch = serde_json::Map::new();
    patch.insert(format!("mailboxIds/{source}"), Value::Null);
    patch.insert(format!("mailboxIds/{destination}"), Value::Bool(true));
    let answers = ctx
        .jmap(vec![Call::new(
            "Email/set",
            "m",
            json!({ "update": { email: patch } }),
        )])
        .await?;
    let set = answers.get("m")?;
    if set["updated"].get(email).is_none() {
        return Ok(match set["notUpdated"][email]["type"].as_str() {
            Some("notFound") => move_status::BAD_SOURCE,
            Some("invalidProperties") => move_status::BAD_DESTINATION,
            _ => move_status::FAILED,
        });
    }
    // The device moved it itself: keep both folders' states in step.
    let mut bits = None;
    if let Some(mut collection) = ctx.collection::<MailFolder>(from).await? {
        bits = collection.data.mail.have.remove(email);
        collection.data.mail.pending.retain(|id| id != email);
        ctx.save_collection(from, &collection).await?;
    }
    if let (Some(bits), Some(mut collection)) = (bits, ctx.collection::<MailFolder>(to).await?)
        && !collection.data.mail.state.is_empty()
    {
        collection.data.mail.have.insert(email.to_string(), bits);
        ctx.save_collection(to, &collection).await?;
    }
    Ok(move_status::OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multipart_layout() {
        let body = multipart_body(&[b"abc".to_vec(), b"de".to_vec()]);
        // count 2, then (20, 3) and (23, 2), then the bytes.
        assert_eq!(&body[..4], &2u32.to_le_bytes());
        assert_eq!(&body[4..8], &20u32.to_le_bytes());
        assert_eq!(&body[8..12], &3u32.to_le_bytes());
        assert_eq!(&body[12..16], &23u32.to_le_bytes());
        assert_eq!(&body[16..20], &2u32.to_le_bytes());
        assert_eq!(&body[20..], b"abcde");
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_range("0-99"), Some((0, 99)));
        assert_eq!(parse_range(" 5 - 9 "), Some((5, 9)));
        assert_eq!(parse_range("9-5"), None);
        assert_eq!(parse_range("x"), None);
    }
}
