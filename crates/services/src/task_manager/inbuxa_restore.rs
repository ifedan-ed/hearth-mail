/*
 * SPDX-FileCopyrightText: 2026 Coffey Labs LLC
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Restoring archived Sieve scripts (`docs/spec/features/undelete.md`, UD-8
//! to UD-11). Email restores in `restore_item.rs`. Files, events and contacts
//! went with calendars, contacts and files (docs/decisions.md).

use crate::task_manager::TaskResult;
use common::{Server, auth::BuildAccessToken};
use email::sieve::{SieveScript, ingest::SieveScriptIngest};
use inbuxa_features::undelete::{self, sieve::candidates};
use registry::schema::structs::{ArchivedItem, TaskRestoreArchivedItem};
use store::write::BatchBuilder;
use trc::AddContext;
use types::{collection::Collection, id::Id};

/// Restores an archived Sieve script. `Ok(None)` once restored;
/// `Ok(Some(reason))` when it can't be, and stays archived.
pub(crate) async fn restore_other(
    server: &Server,
    task: &TaskRestoreArchivedItem,
    item_id: Id,
    item: &ArchivedItem,
) -> trc::Result<Option<String>> {
    let account_id = task.account_id.document_id();
    let access_token = server.access_token(account_id).await?.build();
    let account = server.account(account_id).await?;
    let changed_by = access_token.account_tenant_ids();
    let hash = &task.blob_id.hash;

    let Some(bytes) = server
        .blob_store()
        .get_blob(hash.as_slice(), 0..usize::MAX)
        .await?
    else {
        return Ok(Some("The kept copy is gone.".into()));
    };

    // UD-10, UD-14: a restore counts against quota like anything new, less
    // what the kept copy already counts
    let counted = undelete::data::counted(&server.core.storage.data, account_id, item_id)
        .await?
        .unwrap_or(0);
    if server
        .has_available_quota(&account, (bytes.len() as u64).saturating_sub(counted))
        .await
        .is_err()
    {
        return Ok(Some(
            "Not restored: the account or its tenant is over quota.".into(),
        ));
    }

    let mut batch = BatchBuilder::new();
    match item {
        ArchivedItem::SieveScript(archived) => {
            // Back as an inactive script, never activated (UD-8)
            let mut name = None;
            for candidate in candidates(&archived.name).take(100) {
                if server
                    .sieve_script_get_by_name(account_id, &candidate)
                    .await?
                    .is_none()
                {
                    name = Some(candidate);
                    break;
                }
            }
            let Some(name) = name else {
                return Ok(Some("No free script name.".into()));
            };
            let document_id = server
                .store()
                .assign_document_ids(account_id, Collection::SieveScript, 1)
                .await?;
            batch
                .with_account_id(account_id)
                .with_collection(Collection::SieveScript)
                .with_document(document_id)
                .custom(
                    common::storage::index::ObjectIndexBuilder::<(), _>::new()
                        .with_changes(
                            SieveScript::new(name, hash.clone()).with_size(bytes.len() as u32),
                        )
                        .with_changed_by(changed_by),
                )
                .caused_by(trc::location!())?;
        }
        ArchivedItem::Email(_) => return Ok(Some("Not an email restore.".into())),
    }
    server.commit_batch(batch).await?;

    // UD-9: the record goes once the item is back
    undelete::records::remove_restored(&server.core.storage.data, server.registry(), item_id, item)
        .await?;
    Ok(None)
}

/// The result of a restore that couldn't happen: the item stays archived,
/// and a new request can try again (UD-10).
pub(crate) async fn not_restored(
    server: &Server,
    item_id: Id,
    reason: String,
) -> trc::Result<TaskResult> {
    let mut batch = BatchBuilder::new();
    undelete::data::clear_restore_requested(&mut batch, item_id);
    server.core.storage.data.write(batch.build_all()).await?;
    Ok(TaskResult::permanent(reason))
}
