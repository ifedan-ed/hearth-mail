/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! MeetingResponse ([MS-ASCMD] 2.2.1.11): accept, tentatively accept or
//! decline an invitation, from its mail or from the calendar.
//!
//! The event goes into the account's default calendar in the cloud with the
//! account's answer (its `PARTSTAT`), through the DAV bridge. In 16.x, when
//! the device asks (`SendResponse`), the server also sends the iTIP REPLY
//! (RFC 5546) to the organizer; in 14.1 the device sends it itself, as the
//! specification has it. An invitation answered from the mail goes to
//! Deleted Items, as Exchange does.

use super::send;
use crate::{
    Ctx, Reply,
    dav::DavFolder,
    error::{Error, Result},
    folders::{self, kind},
    jmap::Call,
};
use activesync_proto::{
    items::{Context, meeting},
    wbxml::{Element, codepages::meetingresponse as mr},
};
use dav_client::{CollectionKind, Condition, DavClient, DavError, Url};
use jmap::blob::download::BlobDownload;
use serde_json::json;
use types::blob::BlobId;

/// MeetingResponse status codes ([MS-ASCMD] 2.2.3.177.10).
mod status {
    pub const OK: &str = "1";
    pub const INVALID: &str = "2";
    pub const MAILBOX: &str = "3";
    pub const SERVER: &str = "4";
}

pub async fn meeting_response(ctx: &Ctx<'_>, request: Element) -> Result<Reply> {
    let mut answer = Element::new(mr::MEETING_RESPONSE);
    for item in request.children_of(mr::REQUEST) {
        let request_id = item
            .child_str(mr::REQUEST_ID)
            .unwrap_or_default()
            .to_string();
        let (code, calendar_id) = match respond_one(ctx, item).await {
            Ok(result) => result,
            Err(err) => {
                trc::event!(
                    Http(trc::HttpEvent::Error),
                    SpanId = ctx.session.session_id,
                    Details = "MeetingResponse failed",
                    Reason = err.to_string(),
                );
                (status::SERVER, None)
            }
        };
        answer.push(
            Element::new(mr::RESULT)
                .with(Element::text(mr::REQUEST_ID, request_id))
                .with(Element::text(mr::STATUS, code))
                .with_opt(calendar_id.map(|id| Element::text(mr::CALENDAR_ID, id))),
        );
    }
    Ok(Reply::Wbxml(answer))
}

async fn respond_one(ctx: &Ctx<'_>, item: &Element) -> Result<(&'static str, Option<String>)> {
    let response = match item
        .child_u64(mr::USER_RESPONSE)
        .and_then(|answer| meeting::Response::from_user_response(answer as u8))
    {
        Some(response) => response,
        None => return Ok((status::INVALID, None)),
    };
    let folder_id = item.child_str(mr::COLLECTION_ID).unwrap_or_default();
    let request_id = item.child_str(mr::REQUEST_ID).unwrap_or_default();
    let instance = item.child_str(mr::INSTANCE_ID);
    let device = ctx.device().await?;

    // The default calendar, which takes the answered event.
    let Some((calendar_folder, calendar_href)) = device
        .hierarchy
        .folders
        .iter()
        .find(|(_, entry)| entry.kind == kind::CALENDAR)
        .and_then(|(id, entry)| Some((id.clone(), entry.href.clone()?)))
    else {
        return Ok((status::MAILBOX, None));
    };
    let Some(client) = ctx.dav_client().await? else {
        return Ok((status::MAILBOX, None));
    };
    let (addresses, name, body) = ctx.item_context(&Default::default()).await?;
    let conv = Context {
        version: ctx.version,
        zone: chrono_tz::UTC,
        now: chrono::Utc::now(),
        addresses: &addresses,
        name: name.as_deref(),
        body,
    };

    // The invitation: from the mail, or the calendar's copy.
    let from_mail = folders::mailbox_of(folder_id).is_some();
    let invitation = if from_mail {
        match invitation_of(ctx, request_id).await? {
            Some(text) => text,
            None => return Ok((status::INVALID, None)),
        }
    } else {
        let Some((href, _)) = ctx
            .collection::<DavFolder>(folder_id)
            .await?
            .and_then(|c| c.data.have.get(request_id).cloned())
        else {
            return Ok((status::INVALID, None));
        };
        let url = Url::parse(&href).map_err(|err| Error::Internal(err.to_string()))?;
        client
            .get(&url)
            .await
            .map_err(|err| Error::Internal(err.to_string()))?
            .data
    };

    // Into the calendar with the answer.
    let uid = activesync_proto::items::calendar::uid(&invitation)
        .ok()
        .flatten()
        .ok_or_else(|| Error::Internal("the invitation has no UID".into()))?;
    let collection = Url::parse(&calendar_href).map_err(|err| Error::Internal(err.to_string()))?;
    let name: String = uid
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let href = collection
        .join(&format!("{name}.ics"))
        .map_err(|err| Error::Internal(err.to_string()))?;
    let etag = store_answer(&client, &href, &invitation, response, instance, &conv).await?;

    // Tell the device's calendar folder it already has the event, under the
    // id the answer names.
    let mut calendar_id = None;
    if response != meeting::Response::Declined
        && let Some(mut state) = ctx.collection::<DavFolder>(&calendar_folder).await?
    {
        let href = href.to_string();
        let id = match state.data.have.iter().find(|(_, (h, _))| *h == href) {
            Some((id, _)) => id.clone(),
            None => {
                state.data.next_id += 1;
                state.data.next_id.to_string()
            }
        };
        state.data.have.insert(id.clone(), (href.clone(), etag));
        state.data.pending.retain(|pending| *pending != href);
        ctx.save_collection(&calendar_folder, &state).await?;
        calendar_id = Some(id);
    }

    // 16.x: the server sends the answer when asked.
    if ctx.version.is_16() && item.child(mr::SEND_RESPONSE).is_some() {
        let reply = meeting::reply(&invitation, response, instance, None, &conv)
            .map_err(|err| Error::Internal(err.to_string()))?;
        if let Some(mime) = reply_message(&reply, addresses.first(), response) {
            send::submit_message(ctx, &mime, true).await?;
        }
    }

    if from_mail {
        move_to_trash(ctx, folder_id, request_id).await?;
    }
    Ok((status::OK, calendar_id))
}

/// Writes the account's copy of the event with its answer: over the copy
/// already there (`If-Match`), or as a new one.
async fn store_answer(
    client: &DavClient,
    href: &Url,
    invitation: &str,
    response: meeting::Response,
    instance: Option<&str>,
    conv: &Context<'_>,
) -> Result<String> {
    let existing = match client.get(href).await {
        Ok(resource) => Some(resource),
        Err(DavError::NotFound) => None,
        Err(err) => return Err(Error::Internal(err.to_string())),
    };
    let stored = existing.as_ref().map_or(invitation, |r| r.data.as_str());
    let text = meeting::respond(stored, response, instance, conv)
        .map_err(|err| Error::Internal(err.to_string()))?;
    let condition = match existing.as_ref().and_then(|r| r.etag.as_deref()) {
        Some(etag) => Condition::Update(etag),
        None => Condition::Create,
    };
    client
        .put(href, CollectionKind::Calendar, text, condition)
        .await
        .map_err(|err| Error::Internal(err.to_string()))
}

/// The `text/calendar` part of an invitation mail.
async fn invitation_of(ctx: &Ctx<'_>, email_id: &str) -> Result<Option<String>> {
    let answers = ctx
        .jmap(vec![Call::new(
            "Email/get",
            "g",
            json!({ "ids": [email_id], "properties": ["bodyStructure"],
                    "bodyProperties": ["type", "blobId", "subParts"] }),
        )])
        .await?;
    let Some(structure) = answers.get("g")?["list"]
        .as_array()
        .and_then(|list| list.first())
        .map(|email| email["bodyStructure"].clone())
    else {
        return Ok(None);
    };
    let Some(blob) = find_calendar_part(&structure).and_then(BlobId::from_base32) else {
        return Ok(None);
    };
    Ok(ctx
        .server
        .blob_download(&blob, ctx.token)
        .await?
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
}

/// The blob id of the first `text/calendar` part of a body structure.
fn find_calendar_part(part: &serde_json::Value) -> Option<&str> {
    if part["type"]
        .as_str()
        .is_some_and(|t| t.eq_ignore_ascii_case("text/calendar"))
    {
        return part["blobId"].as_str();
    }
    part["subParts"]
        .as_array()?
        .iter()
        .find_map(find_calendar_part)
}

/// The organizer's address in an iTIP text.
fn organizer(itip: &str) -> Option<String> {
    itip.lines()
        .find(|line| line.to_ascii_uppercase().starts_with("ORGANIZER"))
        .and_then(|line| {
            let lower = line.to_ascii_lowercase();
            let at = lower.find("mailto:")?;
            Some(line[at + 7..].trim().to_string())
        })
}

/// A mail carrying an iTIP REPLY to the organizer.
fn reply_message(
    reply: &str,
    from: Option<&String>,
    response: meeting::Response,
) -> Option<Vec<u8>> {
    let to = organizer(reply)?;
    let from = from?.clone();
    let summary = reply
        .lines()
        .find_map(|line| line.strip_prefix("SUMMARY:"))
        .unwrap_or("Meeting")
        .trim()
        .to_string();
    let verb = match response {
        meeting::Response::Accepted => "Accepted",
        meeting::Response::Tentative => "Tentative",
        meeting::Response::Declined => "Declined",
    };
    mail_builder::MessageBuilder::new()
        .from(from)
        .to(to)
        .subject(format!("{verb}: {summary}"))
        .text_body(format!("{verb}: {summary}"))
        .attachment(
            "text/calendar; method=REPLY; charset=utf-8",
            "invite.ics",
            reply.as_bytes().to_vec(),
        )
        .write_to_vec()
        .ok()
}

async fn move_to_trash(ctx: &Ctx<'_>, folder_id: &str, email_id: &str) -> Result<()> {
    let (Some(mailbox), Some(trash)) = (
        folders::mailbox_of(folder_id),
        super::sync::trash_mailbox(ctx).await?,
    ) else {
        return Ok(());
    };
    if mailbox == trash {
        return Ok(());
    }
    let mut patch = serde_json::Map::new();
    patch.insert(format!("mailboxIds/{mailbox}"), serde_json::Value::Null);
    patch.insert(format!("mailboxIds/{trash}"), serde_json::Value::Bool(true));
    ctx.jmap(vec![Call::new(
        "Email/set",
        "t",
        json!({ "update": { email_id: patch } }),
    )])
    .await?;
    // The device moved it itself: keep its Inbox state in step.
    if let Some(mut state) = ctx.collection::<super::sync::MailFolder>(folder_id).await? {
        state.data.mail.have.remove(email_id);
        ctx.save_collection(folder_id, &state).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn organizer_and_calendar_part() {
        assert_eq!(
            organizer("BEGIN:VEVENT\r\nORGANIZER;CN=Ann:mailto:ann@example.org\r\n").as_deref(),
            Some("ann@example.org")
        );
        let structure = json!({ "type": "multipart/mixed", "subParts": [
            { "type": "text/plain", "blobId": "a" },
            { "type": "multipart/alternative", "subParts": [{ "type": "text/calendar", "blobId": "c" }] }
        ]});
        assert_eq!(find_calendar_part(&structure), Some("c"));
    }
}
