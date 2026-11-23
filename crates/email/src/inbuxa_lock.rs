/*
 * SPDX-FileCopyrightText: 2026 Coffey Labs LLC
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! inbuxa: a locked account's grants (audit-hold-lock spec, AL-7, AL-10):
//! its mailboxes, the only containers Hearth Mail keeps.
//!
//! A delegate's access is real ACL grants on the locked account's
//! containers, the sharing IMAP and JMAP already honor, so a delegate
//! sees the account as a shared one everywhere. The lock notes what each
//! delegate had on a container before, so ending a delegation or the lock
//! puts it back. Idempotent: run again, it grants on containers made since
//! and changes nothing else.

use crate::{cache::MessageCacheFetch, mailbox::Mailbox};
use common::{
    Server, cache::invalidate::CacheInvalidationBuilder, ipc::CacheInvalidation,
    storage::index::ObjectIndexBuilder,
};
use inbuxa_features::lock::{self, Lock, Replaced};
use store::{
    ValueKey,
    write::{AlignedBytes, Archive, BatchBuilder, now},
};
use trc::AddContext;
use types::{collection::Collection, special_use::SpecialUse};

/// Grants a lock's delegates their rights on every container of the locked
/// account, and takes away those of delegations that ended. Returns what the
/// lock now has to remember.
pub async fn apply_grants(
    server: &Server,
    account_id: u32,
    old: Option<&Lock>,
    new: Option<&Lock>,
) -> trc::Result<Vec<Replaced>> {
    let now = now();
    let mut replaced = Vec::new();
    let mut batch = BatchBuilder::new();

    let cache = server
        .get_cached_messages(account_id)
        .await
        .caused_by(trc::location!())?;
    for mailbox in cache.mailboxes.items.iter() {
        // Mail in Trash and Junk is destroyed in time: an organizing
        // delegate may look, not move mail in
        let is_trash = matches!(mailbox.role, SpecialUse::Trash | SpecialUse::Junk);
        let current = mailbox.acls.to_vec();
        let Some(acls) = lock::merge_grants(
            &current,
            Collection::Mailbox,
            mailbox.document_id,
            is_trash,
            old,
            new,
            now,
            &mut replaced,
        ) else {
            continue;
        };
        let Some(archive) = server
            .store()
            .get_value::<Archive<AlignedBytes>>(ValueKey::archive(
                account_id,
                Collection::Mailbox,
                mailbox.document_id,
            ))
            .await
            .caused_by(trc::location!())?
        else {
            continue;
        };
        let current = archive
            .into_deserialized::<Mailbox>()
            .caused_by(trc::location!())?;
        let mut changed = current.inner.clone();
        changed.acls = acls;
        batch
            .with_account_id(account_id)
            .with_collection(Collection::Mailbox)
            .with_document(mailbox.document_id)
            .custom(
                ObjectIndexBuilder::new()
                    .with_changes(changed)
                    .with_current(current),
            )
            .caused_by(trc::location!())?;
    }

    if !batch.is_empty() {
        server
            .commit_batch(batch)
            .await
            .caused_by(trc::location!())?;
    }
    Ok(replaced)
}

/// Re-applies the lock on `account_id`, if any, so containers made since get
/// its grants: after a delegate creates something there, and daily.
pub async fn reconcile(server: &Server, account_id: u32) -> trc::Result<()> {
    let data = server.store();
    let Some(current) = lock::get(data, account_id).await? else {
        return Ok(());
    };
    let replaced = apply_grants(server, account_id, Some(&current), Some(&current)).await?;
    if !same_replaced(&replaced, &current.replaced) {
        let updated = Lock {
            replaced,
            ..current.clone()
        };
        lock::set(data, &updated, Some(&current)).await?;
    }
    invalidate(server, account_id, Some(&current), Some(&current)).await
}

/// Re-applies every lock: the daily sweep, for containers made by the server
/// itself (a Sieve `fileinto :create`) rather than by a delegate.
pub async fn reconcile_all(server: &Server) -> trc::Result<()> {
    for current in lock::all(server.store()).await? {
        reconcile(server, current.account_id).await?;
    }
    Ok(())
}

/// Drops the cached access tokens of the locked account and of every
/// delegate, old and new, so the changed grants apply at once.
pub async fn invalidate(
    server: &Server,
    account_id: u32,
    old: Option<&Lock>,
    new: Option<&Lock>,
) -> trc::Result<()> {
    let mut builder = CacheInvalidationBuilder::default();
    builder.invalidate(CacheInvalidation::AccessToken(account_id));
    for delegate in old.into_iter().chain(new).flat_map(|l| &l.delegates) {
        builder.invalidate(CacheInvalidation::AccessToken(delegate.account_id));
    }
    server.invalidate_caches(builder).await
}

/// Whether two lists of remembered grants are the same, in any order.
fn same_replaced(a: &[Replaced], b: &[Replaced]) -> bool {
    let key = |r: &Replaced| (r.collection, r.document_id, r.delegate, r.rights);
    let mut a = a.iter().map(key).collect::<Vec<_>>();
    let mut b = b.iter().map(key).collect::<Vec<_>>();
    a.sort();
    b.sort();
    a == b
}
