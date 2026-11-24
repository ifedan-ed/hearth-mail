/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! One module per ActiveSync command ([MS-ASCMD] 2.2.1), and [`dispatch`]
//! choosing between them.

pub mod estimate;
pub mod items;
pub mod meeting;
pub mod ping;
pub mod provision;
pub mod push;
pub mod search;
pub mod send;
pub mod settings;
pub mod sync;

use crate::{Body, Ctx, Reply, error::Result, folders};
use activesync_proto::request::Command;
use hyper::StatusCode;

/// Runs the request's command.
pub async fn dispatch(ctx: &Ctx<'_>, body: Body) -> Result<Reply> {
    match ctx.line.command {
        Command::FolderSync => folders::folder_sync(ctx, body.wbxml()?).await,
        Command::FolderCreate => folders::folder_create(ctx, body.wbxml()?).await,
        Command::FolderUpdate => folders::folder_update(ctx, body.wbxml()?).await,
        Command::FolderDelete => folders::folder_delete(ctx, body.wbxml()?).await,
        Command::Sync => sync::sync(ctx, body).await,
        Command::GetItemEstimate => estimate::get_item_estimate(ctx, body.wbxml()?).await,
        Command::Ping => ping::ping(ctx, body).await,
        Command::ItemOperations => items::item_operations(ctx, body.wbxml()?).await,
        Command::MoveItems => items::move_items(ctx, body.wbxml()?).await,
        Command::SendMail => send::send_mail(ctx, body).await,
        Command::SmartReply => send::smart(ctx, body, send::Smart::Reply).await,
        Command::SmartForward => send::smart(ctx, body, send::Smart::Forward).await,
        Command::Search => search::search(ctx, body.wbxml()?).await,
        Command::Find => search::find(ctx, body.wbxml()?).await,
        Command::ResolveRecipients => search::resolve_recipients(ctx, body.wbxml()?).await,
        Command::Settings => settings::settings(ctx, body.wbxml()?).await,
        Command::Provision => provision::provision(ctx, body.wbxml()?).await,
        Command::MeetingResponse => meeting::meeting_response(ctx, body.wbxml()?).await,
        // ValidateCert and GetAttachment aren't offered.
        Command::ValidateCert | Command::GetAttachment => {
            Err(crate::error::Error::Http(StatusCode::NOT_IMPLEMENTED))
        }
    }
}
