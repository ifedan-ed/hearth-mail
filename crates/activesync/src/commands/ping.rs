/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Ping ([MS-ASCMD] 2.2.1.13): the device names folders and a heartbeat;
//! the answer comes when one of them has changes (status 2, naming them)
//! or when the heartbeat runs out (status 1). The wait listens to the
//! account's push events ([`super::push`]), so new mail answers at once.
//!
//! A folder "has changes" when a Sync from the device's current key would
//! send something: the same change logic, run on a copy of its state.

use super::{push::Woke, sync::MailFolder};
use crate::dav::{self, DavFolder};
use crate::{
    Body, Ctx, Reply,
    error::Result,
    folders::{self, Class},
    state::PingRequest,
};
use activesync_proto::wbxml::{Element, codepages::ping};
use std::time::{Duration, Instant};

/// Ping status codes ([MS-ASCMD] 2.2.3.177.13).
mod status {
    pub const EXPIRED: &str = "1";
    pub const CHANGES: &str = "2";
    pub const MISSING: &str = "3";
    pub const BAD_HEARTBEAT: &str = "5";
    pub const TOO_MANY: &str = "6";
    pub const HIERARCHY: &str = "7";
}

/// The heartbeat limits ([MS-ASCMD] 2.2.3.110.2) and the folder limit.
const MIN_HEARTBEAT: u64 = 60;
const MAX_HEARTBEAT: u64 = 3540;
const MAX_FOLDERS: usize = 300;
/// How often cloud folders are looked at while a Ping waits.
const CLOUD_POLL: Duration = Duration::from_secs(60);

pub async fn ping(ctx: &Ctx<'_>, body: Body) -> Result<Reply> {
    let mut device = ctx.device().await?;
    let request = match body {
        Body::Wbxml(request) => request,
        _ => Element::new(ping::PING),
    };
    // Whatever the device leaves out comes from its last Ping.
    let last = device.last_ping.clone().unwrap_or_default();
    let heartbeat = request
        .child_u64(ping::HEARTBEAT_INTERVAL)
        .unwrap_or(last.heartbeat);
    let folders: Vec<String> = match request.child(ping::FOLDERS) {
        Some(list) => list
            .children_of(ping::FOLDER)
            .filter_map(|folder| folder.child_str(ping::ID))
            .map(|id| id.trim().to_string())
            .collect(),
        None => last.folders.clone(),
    };
    if heartbeat == 0 || folders.is_empty() {
        return Ok(answer(status::MISSING));
    }
    if !(MIN_HEARTBEAT..=MAX_HEARTBEAT).contains(&heartbeat) {
        return Ok(Reply::Wbxml(
            Element::new(ping::PING)
                .with(Element::text(ping::STATUS, status::BAD_HEARTBEAT))
                .with(Element::text(
                    ping::HEARTBEAT_INTERVAL,
                    heartbeat.clamp(MIN_HEARTBEAT, MAX_HEARTBEAT).to_string(),
                )),
        ));
    }
    if folders.len() > MAX_FOLDERS {
        return Ok(Reply::Wbxml(
            Element::new(ping::PING)
                .with(Element::text(ping::STATUS, status::TOO_MANY))
                .with(Element::text(ping::MAX_FOLDERS, MAX_FOLDERS.to_string())),
        ));
    }
    if folders
        .iter()
        .any(|id| !device.hierarchy.folders.contains_key(id))
    {
        return Ok(answer(status::HIERARCHY));
    }
    let remembered = PingRequest {
        heartbeat,
        folders: folders.clone(),
    };
    if device.last_ping.as_ref().map(|p| (&p.folders, p.heartbeat))
        != Some((&remembered.folders, remembered.heartbeat))
    {
        device.last_ping = Some(remembered);
        ctx.save_device(&device).await?;
    }

    let mut watch = ctx.watch().await?;
    let deadline = Instant::now() + Duration::from_secs(heartbeat);
    loop {
        let changed = changed_folders(ctx, &device, &folders).await?;
        if !changed.is_empty() {
            let mut list = Element::new(ping::FOLDERS);
            for id in changed {
                list.push(Element::text(ping::FOLDER, id));
            }
            return Ok(Reply::Wbxml(
                Element::new(ping::PING)
                    .with(Element::text(ping::STATUS, status::CHANGES))
                    .with(list),
            ));
        }
        // Cloud folders have no push: look again at least every minute.
        let next_look = (Instant::now() + CLOUD_POLL).min(deadline);
        if watch.wait(next_look).await == Woke::TimedOut && Instant::now() >= deadline {
            return Ok(answer(status::EXPIRED));
        }
    }
}

/// The folders among `ids` a Sync would bring changes for.
async fn changed_folders(
    ctx: &Ctx<'_>,
    device: &crate::state::Device,
    ids: &[String],
) -> Result<Vec<String>> {
    let mut changed = Vec::new();
    // Made once, when the first cloud folder needs it.
    let mut dav_client = None;
    for id in ids {
        let Some(folder) = device.hierarchy.folders.get(id) else {
            continue;
        };
        let (Some(mailbox), Class::Email) = (folders::mailbox_of(id), Class::of_kind(folder.kind))
        else {
            // A cloud folder: has its sync token moved?
            if let (Some(href), Some(state)) =
                (&folder.href, ctx.collection::<DavFolder>(id).await?)
                && state.data.primed
            {
                if dav_client.is_none() {
                    dav_client = Some(ctx.dav_client().await?);
                }
                if let Some(Some(client)) = &dav_client
                    && dav::has_changes(client, href, &state.data).await
                {
                    changed.push(id.clone());
                }
            }
            continue;
        };
        // A folder the device never synced has nothing to report yet.
        let Some(collection) = ctx.collection::<MailFolder>(id).await? else {
            continue;
        };
        let mut state = collection.data;
        if state.mail.state.is_empty() {
            continue;
        }
        let changes = ctx.mail_changes(mailbox, &mut state.mail).await?;
        if !changes.deleted.is_empty()
            || !changes.changed.is_empty()
            || !state.mail.pending.is_empty()
        {
            changed.push(id.clone());
        }
    }
    Ok(changed)
}

fn answer(code: &str) -> Reply {
    Reply::Wbxml(Element::new(ping::PING).with(Element::text(ping::STATUS, code)))
}
