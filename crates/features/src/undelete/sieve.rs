/*
 * SPDX-FileCopyrightText: 2026 Coffey Labs LLC
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * Modified for Hearth Mail in 2026.
 */

//! Deleted Sieve scripts (UD-1, a deliberate extension: upstream archives
//! only email). A script is archived whole, its text in the record, so a
//! restore needs nothing else. This was part of the module that also kept
//! deleted files, events and contacts; those went with calendars, contacts
//! and files (docs/decisions.md).

use crate::undelete::{data::Extra, records};
use registry::{schema::structs::ArchivedItem, types::datetime::UTCDateTime};
use store::{RegistryStore, Store, write::now};
use types::{blob::BlobId, blob_hash::BlobHash, id::Id};

/// Archives a deleted Sieve script for `retention` seconds and returns the
/// archive record's id.
pub async fn archive_sieve(
    data: &Store,
    registry: &RegistryStore,
    account_id: u32,
    name: &str,
    content: String,
    blob_hash: BlobHash,
    retention: u64,
) -> trc::Result<Id> {
    let archived_at = now();
    let size = content.len() as u64;
    let item = ArchivedItem::SieveScript(registry::schema::structs::ArchivedSieveScript {
        name: name.to_string(),
        created_at: UTCDateTime::from_timestamp(archived_at as i64),
        content,
        account_id: types::id::Id::from(account_id),
        archived_at: UTCDateTime::from_timestamp(archived_at as i64),
        archived_until: UTCDateTime::from_timestamp((archived_at + retention) as i64),
        blob_id: BlobId::new(blob_hash, Default::default()),
    });
    records::insert(
        data,
        registry,
        &item,
        &Extra::SieveScript {
            name: name.to_string(),
        },
        size,
    )
    .await
}

/// The names a restored script tries, in order: the original, then with
/// ` (restored)`, then numbered (UD-8).
pub fn candidates(name: &str) -> impl Iterator<Item = String> + '_ {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    std::iter::once(name.to_string())
        .chain(std::iter::once(format!("{stem} (restored){ext}")))
        .chain((2u32..).map(move |n| format!("{stem} (restored {n}){ext}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first free name, as a restore picks it.
    fn free_name(name: &str, is_taken: impl Fn(&str) -> bool) -> String {
        candidates(name).find(|c| !is_taken(c)).unwrap()
    }

    #[test]
    fn restored_names() {
        assert_eq!(free_name("a.sieve", |_| false), "a.sieve");
        assert_eq!(
            free_name("a.sieve", |n| n == "a.sieve"),
            "a (restored).sieve"
        );
        assert_eq!(
            free_name("a.sieve", |n| n == "a.sieve" || n == "a (restored).sieve"),
            "a (restored 2).sieve"
        );
        assert_eq!(free_name("script", |n| n == "script"), "script (restored)");
    }
}
