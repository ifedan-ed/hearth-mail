/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Sync ([MS-ASCMD] 2.2.1.21): the device's changes in, the server's
//! changes out, folder by folder, each under its sync key.
//!
//! Per folder: key 0 starts it (an empty answer with key 1); with a known
//! key, the device's commands are applied first (so they are not sent
//! back to it), then up to `WindowSize` server changes are sent (deletes,
//! flag changes, then new mail, newest first) with `MoreAvailable` when
//! more wait. A request that changes nothing and finds nothing is
//! answered with an empty body, after waiting for changes when it asked to
//! (`Wait`, `HeartbeatInterval`). An empty request repeats the last one
//! ([MS-ASCMD] 4.5.10).

use super::push::Woke;
use crate::{
    Body, Ctx, Reply,
    dav::{self, DavFolder},
    error::Result,
    folders::{self, Class},
    jmap::Call,
    mail::{
        self, MailState, flags,
        item::{self, BodyOptions},
    },
    state::Collection,
};
use activesync_proto::wbxml::{self, Element, codepages::airsync as sync};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Sync status codes ([MS-ASCMD] 2.2.3.177.17).
pub mod status {
    pub const OK: &str = "1";
    pub const BAD_SYNC_KEY: &str = "3";
    pub const PROTOCOL_ERROR: &str = "4";
    pub const SERVER_ERROR: &str = "5";
    pub const CONVERSION: &str = "6";
    pub const NOT_FOUND: &str = "8";
    pub const HIERARCHY_CHANGED: &str = "12";
    pub const INCOMPLETE: &str = "13";
    pub const BAD_WAIT: &str = "14";
}

/// The default and the largest `WindowSize` ([MS-ASCMD] 2.2.3.199).
const DEFAULT_WINDOW: usize = 100;
const MAX_WINDOW: usize = 512;

/// A mail folder's state on a device: what it holds and the options it
/// synced with (a device may leave them out of later requests).
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct MailFolder {
    pub mail: MailState,
    pub filter_type: u64,
    pub body: BodyOptions,
}

/// One collection of a request.
struct Request<'x> {
    id: String,
    key: String,
    window: usize,
    get_changes: Option<bool>,
    deletes_as_moves: bool,
    options: Option<&'x Element>,
    commands: Option<&'x Element>,
    supported: Option<&'x Element>,
}

/// What a collection answers.
struct Answer {
    element: Element,
    /// Whether the device has anything to do (commands or responses or a
    /// new key); an answer with nothing is left out of an empty response.
    has_news: bool,
}

pub async fn sync(ctx: &Ctx<'_>, body: Body) -> Result<Reply> {
    let mut device = ctx.device().await?;
    // An empty request repeats the last full one.
    let request = match body {
        Body::None => match device.last_sync.as_deref().map(wbxml::decode) {
            Some(Ok(request)) => request,
            _ => return Ok(top_status(status::INCOMPLETE)),
        },
        body => {
            let request = body.wbxml()?;
            if request.child(sync::COLLECTIONS).is_some() {
                device.last_sync = Some(wbxml::encode(&request));
                ctx.save_device(&device).await?;
            }
            request
        }
    };

    // How long the device lets the server wait for changes.
    let wait = match (
        request.child_u64(sync::WAIT),
        request.child_u64(sync::HEARTBEAT_INTERVAL),
    ) {
        (Some(minutes), _) if !(1..=59).contains(&minutes) => {
            return Ok(limit_status(if minutes < 1 { 1 } else { 59 }));
        }
        (Some(minutes), _) => Some(Duration::from_secs(minutes * 60)),
        (None, Some(seconds)) if !(60..=3540).contains(&seconds) => {
            return Ok(limit_status(seconds.clamp(60, 3540)));
        }
        (None, Some(seconds)) => Some(Duration::from_secs(seconds)),
        (None, None) => None,
    };
    let global_window = request
        .child_u64(sync::WINDOW_SIZE)
        .map(|size| (size as usize).clamp(1, MAX_WINDOW));

    let Some(collections) = request.child(sync::COLLECTIONS) else {
        return Ok(top_status(status::INCOMPLETE));
    };
    let requests: Vec<Request> = collections
        .children_of(sync::COLLECTION)
        .filter_map(|collection| {
            Some(Request {
                id: collection
                    .child_str(sync::COLLECTION_ID)?
                    .trim()
                    .to_string(),
                key: collection.child_str(sync::SYNC_KEY)?.trim().to_string(),
                window: collection
                    .child_u64(sync::WINDOW_SIZE)
                    .map(|size| (size as usize).clamp(1, MAX_WINDOW))
                    .or(global_window)
                    .unwrap_or(DEFAULT_WINDOW),
                get_changes: collection.child(sync::GET_CHANGES).map(|element| {
                    // <GetChanges/> means yes; <GetChanges>0</GetChanges> no.
                    element.as_str().is_none_or(|value| value.trim() != "0")
                }),
                deletes_as_moves: collection
                    .child(sync::DELETES_AS_MOVES)
                    .is_none_or(|element| element.as_str().is_none_or(|v| v.trim() != "0")),
                options: collection.child(sync::OPTIONS),
                commands: collection.child(sync::COMMANDS),
                supported: collection.child(sync::SUPPORTED),
            })
        })
        .collect();

    // Listen before looking, so a change in between still ends the wait.
    let mut watch = match wait {
        Some(_) => Some(ctx.watch().await?),
        None => None,
    };
    let deadline = wait.map(|wait| Instant::now() + wait);
    loop {
        let mut answers = Vec::with_capacity(requests.len());
        for request in &requests {
            answers.push(sync_collection(ctx, &device, request).await?);
        }
        let news = answers.iter().any(|answer| answer.has_news);
        if news {
            let mut list = Element::new(sync::COLLECTIONS);
            for answer in answers {
                list.push(answer.element);
            }
            return Ok(Reply::Wbxml(Element::new(sync::SYNC).with(list)));
        }
        let woke = match (&mut watch, deadline) {
            (Some(watch), Some(deadline)) => watch.wait(deadline).await,
            _ => Woke::TimedOut,
        };
        if woke == Woke::TimedOut {
            return Ok(Reply::Empty);
        }
        // Something changed: look again with the device's state re-read
        // (a request in between may have moved it).
        device = ctx.device().await?;
    }
}

/// One folder of a Sync request.
async fn sync_collection(
    ctx: &Ctx<'_>,
    device: &crate::state::Device,
    request: &Request<'_>,
) -> Result<Answer> {
    let Some(folder) = device.hierarchy.folders.get(&request.id) else {
        return Ok(failed(request, status::HIERARCHY_CHANGED));
    };
    match Class::of_kind(folder.kind) {
        Class::Email => match folders::mailbox_of(&request.id) {
            Some(mailbox) => sync_mail(ctx, request, mailbox).await,
            None => Ok(failed(request, status::HIERARCHY_CHANGED)),
        },
        class => match &folder.href {
            Some(href) => sync_dav(ctx, request, class, href).await,
            None => Ok(failed(request, status::HIERARCHY_CHANGED)),
        },
    }
}

/// One cloud folder of a Sync request ([`crate::dav`]).
async fn sync_dav(
    ctx: &Ctx<'_>,
    request: &Request<'_>,
    class: Class,
    href: &str,
) -> Result<Answer> {
    let Some(client) = ctx.dav_client().await? else {
        // The cloud was unlinked since FolderSync listed it.
        return Ok(failed(request, status::HIERARCHY_CHANGED));
    };
    let read_options = |folder: &mut DavFolder| {
        if let Some(options) = request.options {
            folder.body = BodyOptions::parse(options);
        }
    };
    if request.key == "0" {
        let mut folder = DavFolder {
            supported: request.supported.map(|supported| {
                supported
                    .children()
                    .iter()
                    .map(|tag| (tag.tag.page(), tag.tag.token()))
                    .collect()
            }),
            ..DavFolder::default()
        };
        read_options(&mut folder);
        ctx.save_collection(
            &request.id,
            &Collection {
                key: 1,
                data: folder,
                previous: None,
            },
        )
        .await?;
        return Ok(Answer {
            element: Element::new(sync::COLLECTION)
                .with(Element::text(sync::SYNC_KEY, "1"))
                .with(Element::text(sync::COLLECTION_ID, request.id.as_str()))
                .with(Element::text(sync::STATUS, status::OK)),
            has_news: true,
        });
    }
    let (Ok(key), Some(mut collection)) = (
        request.key.parse::<u32>(),
        ctx.collection::<DavFolder>(&request.id).await?,
    ) else {
        return Ok(failed(request, status::BAD_SYNC_KEY));
    };
    let Some(before) = collection.at_key(key) else {
        return Ok(failed(request, status::BAD_SYNC_KEY));
    };
    let mut folder = before.clone();
    read_options(&mut folder);
    let result = dav::sync_folder(
        ctx,
        &client,
        class,
        href,
        &mut folder,
        request.commands,
        request.get_changes != Some(false),
        request.window,
    )
    .await?;
    let has_news = !result.commands.is_empty() || !result.responses.is_empty() || result.more;
    let new_key = if has_news || folder.have != before.have {
        collection.advance(key, before, folder)
    } else {
        if key == collection.key {
            collection.data = folder;
        }
        key
    };
    ctx.save_collection(&request.id, &collection).await?;
    let mut element = Element::new(sync::COLLECTION)
        .with(Element::text(sync::SYNC_KEY, new_key.to_string()))
        .with(Element::text(sync::COLLECTION_ID, request.id.as_str()))
        .with(Element::text(sync::STATUS, status::OK));
    if !result.commands.is_empty() {
        let mut commands = Element::new(sync::COMMANDS);
        for command in result.commands {
            commands.push(command);
        }
        element.push(commands);
    }
    if !result.responses.is_empty() {
        let mut responses = Element::new(sync::RESPONSES);
        for response in result.responses {
            responses.push(response);
        }
        element.push(responses);
    }
    if result.more {
        element.push(Element::new(sync::MORE_AVAILABLE));
    }
    Ok(Answer {
        element,
        has_news: has_news || new_key != key,
    })
}

/// An answer with only a status (and the key the device sent).
fn failed(request: &Request<'_>, code: &str) -> Answer {
    Answer {
        element: Element::new(sync::COLLECTION)
            .with(Element::text(sync::SYNC_KEY, request.key.as_str()))
            .with(Element::text(sync::COLLECTION_ID, request.id.as_str()))
            .with(Element::text(sync::STATUS, code)),
        has_news: true,
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Reads a collection's `Options` into the folder's state.
fn apply_options(folder: &mut MailFolder, options: &Element) {
    folder.filter_type = options.child_u64(sync::FILTER_TYPE).unwrap_or(0);
    folder.mail.since = mail::filter_since(folder.filter_type, now());
    folder.body = BodyOptions::parse(options);
}

async fn sync_mail(ctx: &Ctx<'_>, request: &Request<'_>, mailbox: &str) -> Result<Answer> {
    if request.key == "0" {
        // A fresh start: remember the options, list nothing yet.
        let mut folder = MailFolder::default();
        if let Some(options) = request.options {
            apply_options(&mut folder, options);
        }
        ctx.save_collection(
            &request.id,
            &Collection {
                key: 1,
                data: folder,
                previous: None,
            },
        )
        .await?;
        return Ok(Answer {
            element: Element::new(sync::COLLECTION)
                .with(Element::text(sync::SYNC_KEY, "1"))
                .with(Element::text(sync::COLLECTION_ID, request.id.as_str()))
                .with(Element::text(sync::STATUS, status::OK)),
            has_news: true,
        });
    }
    let (Ok(key), Some(mut collection)) = (
        request.key.parse::<u32>(),
        ctx.collection::<MailFolder>(&request.id).await?,
    ) else {
        return Ok(failed(request, status::BAD_SYNC_KEY));
    };
    let Some(before) = collection.at_key(key) else {
        return Ok(failed(request, status::BAD_SYNC_KEY));
    };
    let mut folder = before.clone();
    if let Some(options) = request.options {
        let filter_before = folder.filter_type;
        apply_options(&mut folder, options);
        if folder.filter_type != filter_before {
            // A new window: list the folder again against what the device
            // holds.
            folder.mail.state.clear();
        }
    }

    // The device's own changes first.
    let mut responses = Element::new(sync::RESPONSES);
    if let Some(commands) = request.commands {
        apply_mail_commands(ctx, mailbox, request, &mut folder, commands, &mut responses).await?;
    }

    // Then the server's.
    let mut commands = Element::new(sync::COMMANDS);
    let mut more = false;
    if request.get_changes != Some(false) {
        let changes = ctx.mail_changes(mailbox, &mut folder.mail).await?;
        let mut room = request.window;
        for id in changes.deleted {
            commands.push(Element::new(sync::DELETE).with(Element::text(sync::SERVER_ID, id)));
            room = room.saturating_sub(1);
        }
        for (id, bits) in changes.changed {
            let [read, flag] = item::flag_elements(bits);
            commands.push(
                Element::new(sync::CHANGE)
                    .with(Element::text(sync::SERVER_ID, id))
                    .with(Element::new(sync::APPLICATION_DATA).with(read).with(flag)),
            );
            room = room.saturating_sub(1);
        }
        let take = room.min(folder.mail.pending.len());
        if take > 0 {
            let ids: Vec<String> = folder.mail.pending.drain(..take).collect();
            for (id, bits, data) in ctx.render_emails(&ids, &folder.body).await? {
                folder.mail.have.insert(id.clone(), bits);
                commands.push(
                    Element::new(sync::ADD)
                        .with(Element::text(sync::SERVER_ID, id))
                        .with(data),
                );
            }
        }
        more = !folder.mail.pending.is_empty();
    }

    let has_commands = !commands.children().is_empty();
    let has_responses = !responses.children().is_empty();
    let new_key = if has_commands || has_responses || folder.mail.have != before.mail.have {
        collection.advance(key, before, folder)
    } else {
        // Nothing for the device: same key, but keep any state the server
        // moved on (the JMAP state, new options).
        if key == collection.key {
            collection.data = folder;
        }
        key
    };
    ctx.save_collection(&request.id, &collection).await?;

    let mut element = Element::new(sync::COLLECTION)
        .with(Element::text(sync::SYNC_KEY, new_key.to_string()))
        .with(Element::text(sync::COLLECTION_ID, request.id.as_str()))
        .with(Element::text(sync::STATUS, status::OK));
    if has_commands {
        element.push(commands);
    }
    if has_responses {
        element.push(responses);
    }
    if more {
        element.push(Element::new(sync::MORE_AVAILABLE));
    }
    Ok(Answer {
        element,
        has_news: has_commands || has_responses || new_key != key || more,
    })
}

/// Applies the device's Change, Delete and Fetch commands to a mail folder;
/// failures and fetched items go into `responses`.
async fn apply_mail_commands(
    ctx: &Ctx<'_>,
    mailbox: &str,
    request: &Request<'_>,
    folder: &mut MailFolder,
    commands: &Element,
    responses: &mut Element,
) -> Result<()> {
    let mut update = serde_json::Map::new();
    let mut destroy = Vec::new();
    let mut moves = serde_json::Map::new();
    let mut fetch = Vec::new();
    let trash = if request.deletes_as_moves {
        trash_mailbox(ctx).await?.filter(|trash| trash != mailbox)
    } else {
        None
    };
    for command in commands.children() {
        let server_id = command.child_str(sync::SERVER_ID).unwrap_or_default();
        match command.tag {
            sync::CHANGE => {
                let Some(data) = command.child(sync::APPLICATION_DATA) else {
                    continue;
                };
                let (read, flagged) = item::keyword_patch(data);
                let mut patch = serde_json::Map::new();
                let mut bits = folder.mail.have.get(server_id).copied().unwrap_or(0);
                if let Some(read) = read {
                    patch.insert("keywords/$seen".into(), json!(read.then_some(true)));
                    bits = if read {
                        bits | flags::SEEN
                    } else {
                        bits & !flags::SEEN
                    };
                }
                if let Some(flagged) = flagged {
                    patch.insert("keywords/$flagged".into(), json!(flagged.then_some(true)));
                    bits = if flagged {
                        bits | flags::FLAGGED
                    } else {
                        bits & !flags::FLAGGED
                    };
                }
                if !patch.is_empty() {
                    update.insert(server_id.to_string(), Value::Object(patch));
                    // The device has it this way already: don't send it back.
                    if folder.mail.have.contains_key(server_id) {
                        folder.mail.have.insert(server_id.to_string(), bits);
                    }
                }
            }
            sync::DELETE => {
                folder.mail.have.remove(server_id);
                folder.mail.pending.retain(|id| id != server_id);
                match &trash {
                    Some(trash) => {
                        let mut patch = serde_json::Map::new();
                        patch.insert(format!("mailboxIds/{mailbox}"), Value::Null);
                        patch.insert(format!("mailboxIds/{trash}"), Value::Bool(true));
                        moves.insert(server_id.to_string(), Value::Object(patch));
                    }
                    None => destroy.push(server_id.to_string()),
                }
            }
            sync::FETCH => fetch.push(server_id.to_string()),
            sync::ADD => {
                // New mail from a device (drafts) isn't taken.
                responses.push(
                    Element::new(sync::ADD)
                        .with_opt(
                            command
                                .child_str(sync::CLIENT_ID)
                                .map(|id| Element::text(sync::CLIENT_ID, id)),
                        )
                        .with(Element::text(sync::STATUS, status::CONVERSION)),
                );
            }
            _ => (),
        }
    }

    for (id, patch) in moves {
        update.insert(id, patch);
    }
    if !update.is_empty() || !destroy.is_empty() {
        let answers = ctx
            .jmap(vec![Call::new(
                "Email/set",
                "s",
                json!({ "update": update, "destroy": destroy }),
            )])
            .await?;
        let set = answers.get("s")?;
        for (id, _) in set["notUpdated"].as_object().into_iter().flatten() {
            let tag = if folder.mail.have.contains_key(id) {
                sync::CHANGE
            } else {
                sync::DELETE
            };
            responses.push(
                Element::new(tag)
                    .with(Element::text(sync::SERVER_ID, id.as_str()))
                    .with(Element::text(sync::STATUS, status::NOT_FOUND)),
            );
        }
        for (id, _) in set["notDestroyed"].as_object().into_iter().flatten() {
            responses.push(
                Element::new(sync::DELETE)
                    .with(Element::text(sync::SERVER_ID, id.as_str()))
                    .with(Element::text(sync::STATUS, status::NOT_FOUND)),
            );
        }
    }

    if !fetch.is_empty() {
        let rendered = ctx.render_emails(&fetch, &folder.body).await?;
        for id in fetch {
            let element = match rendered.iter().find(|(found, _, _)| *found == id) {
                Some((_, _, data)) => Element::new(sync::FETCH)
                    .with(Element::text(sync::SERVER_ID, id))
                    .with(Element::text(sync::STATUS, status::OK))
                    .with(data.clone()),
                None => Element::new(sync::FETCH)
                    .with(Element::text(sync::SERVER_ID, id))
                    .with(Element::text(sync::STATUS, status::NOT_FOUND)),
            };
            responses.push(element);
        }
    }
    Ok(())
}

/// The account's Deleted Items mailbox, if it has one.
pub async fn trash_mailbox(ctx: &Ctx<'_>) -> Result<Option<String>> {
    let answers = ctx
        .jmap(vec![Call::new(
            "Mailbox/query",
            "t",
            json!({ "filter": { "role": "trash" } }),
        )])
        .await?;
    Ok(answers.get("t")?["ids"]
        .as_array()
        .and_then(|ids| ids.first())
        .and_then(Value::as_str)
        .map(str::to_string))
}

/// A whole-request status ([MS-ASCMD] 2.2.3.177.17), for requests that
/// can't be read collection by collection.
fn top_status(code: &str) -> Reply {
    Reply::Wbxml(Element::new(sync::SYNC).with(Element::text(sync::STATUS, code)))
}

/// Status 14 with the nearest acceptable `Wait` or heartbeat.
fn limit_status(limit: u64) -> Reply {
    Reply::Wbxml(
        Element::new(sync::SYNC)
            .with(Element::text(sync::STATUS, status::BAD_WAIT))
            .with(Element::text(sync::LIMIT, limit.to_string())),
    )
}
