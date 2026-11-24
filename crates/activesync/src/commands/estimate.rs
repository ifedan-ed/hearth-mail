/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! GetItemEstimate ([MS-ASCMD] 2.2.1.9): how many changes a Sync of each
//! folder would bring, counted with the same change logic as Sync on a copy
//! of the device's state, so nothing is saved.

use crate::{
    Ctx, Reply,
    error::Result,
    folders::{self, Class},
};
use activesync_proto::wbxml::{
    Element,
    codepages::{airsync, getitemestimate as gie},
};

use super::sync::MailFolder;
use crate::dav::{self, DavFolder};

/// GetItemEstimate status codes ([MS-ASCMD] 2.2.3.177.6).
mod status {
    pub const OK: &str = "1";
    pub const BAD_COLLECTION: &str = "2";
    pub const NOT_PRIMED: &str = "3";
    pub const BAD_SYNC_KEY: &str = "4";
}

pub async fn get_item_estimate(ctx: &Ctx<'_>, request: Element) -> Result<Reply> {
    let device = ctx.device().await?;
    let mut answer = Element::new(gie::GET_ITEM_ESTIMATE);
    for collection in request
        .child(gie::COLLECTIONS)
        .map(|list| list.children_of(gie::COLLECTION).collect::<Vec<_>>())
        .unwrap_or_default()
    {
        let id = collection
            .child_str(gie::COLLECTION_ID)
            .unwrap_or_default()
            .trim()
            .to_string();
        let key = collection
            .child_str(airsync::SYNC_KEY)
            .unwrap_or_default()
            .trim()
            .to_string();
        let (code, estimate) = estimate(ctx, &device, &id, &key).await?;
        answer.push(
            Element::new(gie::RESPONSE)
                .with(Element::text(gie::STATUS, code))
                .with(
                    Element::new(gie::COLLECTION)
                        .with(Element::text(gie::COLLECTION_ID, id))
                        .with_opt(estimate.map(|n| Element::text(gie::ESTIMATE, n.to_string()))),
                ),
        );
    }
    Ok(Reply::Wbxml(answer))
}

async fn estimate(
    ctx: &Ctx<'_>,
    device: &crate::state::Device,
    id: &str,
    key: &str,
) -> Result<(&'static str, Option<usize>)> {
    let Some(folder) = device.hierarchy.folders.get(id) else {
        return Ok((status::BAD_COLLECTION, None));
    };
    if key == "0" {
        return Ok((status::NOT_PRIMED, None));
    }
    let (Some(mailbox), Class::Email) = (folders::mailbox_of(id), Class::of_kind(folder.kind))
    else {
        // A cloud folder.
        let (Some(href), Some(client)) = (&folder.href, ctx.dav_client().await?) else {
            return Ok((status::BAD_COLLECTION, None));
        };
        let Some(state) = ctx
            .collection::<DavFolder>(id)
            .await?
            .zip(key.parse::<u32>().ok())
            .and_then(|(collection, key)| collection.at_key(key))
        else {
            return Ok((status::BAD_SYNC_KEY, None));
        };
        return Ok((
            status::OK,
            Some(dav::estimate(&client, href, &state).await.unwrap_or(0)),
        ));
    };
    let Some(mut state) = ctx
        .collection::<MailFolder>(id)
        .await?
        .zip(key.parse::<u32>().ok())
        .and_then(|(collection, key)| collection.at_key(key))
    else {
        return Ok((status::BAD_SYNC_KEY, None));
    };
    let changes = ctx.mail_changes(mailbox, &mut state.mail).await?;
    Ok((
        status::OK,
        Some(changes.deleted.len() + changes.changed.len() + state.mail.pending.len()),
    ))
}
