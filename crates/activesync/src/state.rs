/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! What the server remembers about each device: its folder list as the
//! device has it, and per folder the sync key and what the device holds.
//!
//! Kept in the in-memory key-value store (the data store's key-value space,
//! or Redis where one is configured) under [`common::KV_ACTIVESYNC`], with
//! an expiry, not in a new store layout: nothing changes for the data the
//! rollback to inbuxa reads (docs/activesync.md, "Device state"). Values are
//! JSON, read and written whole; a device's state is at most a few hundred
//! kilobytes (one short id and a byte per message it holds).
//!
//! Sync keys: each answer moves a folder from key `n` to `n + 1`. The state
//! at `n` is kept beside the new one until the device uses `n + 1`, so a
//! device that lost an answer and repeats its request with `n` gets the
//! same changes again instead of a "sync key unknown" and a full resync
//! ([MS-ASCMD] 2.2.3.181.4).

use crate::{
    Ctx,
    error::{Error, Result},
};
use common::KV_ACTIVESYNC;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::BTreeMap;
use store::dispatch::lookup::KeyValue;

/// How long state is kept after its last use: a device away longer syncs
/// again from the start.
pub const STATE_TTL: u64 = 60 * 24 * 60 * 60;

// The second byte of a key: what it holds.
const KIND_DEVICE: u8 = 0;
const KIND_COLLECTION: u8 = 1;

/// A device's own record.
#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct Device {
    pub device_type: String,
    pub user_agent: String,
    /// The policy key handed out by Provision (not enforced).
    pub policy_key: Option<u32>,
    /// What Settings/DeviceInformation told (model, OS, phone number...),
    /// as (tag name, value).
    pub info: Vec<(String, String)>,
    pub hierarchy: Hierarchy,
    /// The last Sync request with collections, for an empty Sync request
    /// that means "the same again" ([MS-ASCMD] 4.5.10).
    pub last_sync: Option<Vec<u8>>,
    /// The last Ping's heartbeat and folders, for an empty Ping.
    pub last_ping: Option<PingRequest>,
}

/// The folder list as the device has it, at its FolderSync key.
#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct Hierarchy {
    pub key: u32,
    pub folders: BTreeMap<String, FolderEntry>,
    /// The list at the previous key, until the device uses the new one.
    pub previous: Option<(u32, BTreeMap<String, FolderEntry>)>,
}

/// One folder as the device was told about it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct FolderEntry {
    pub parent: String,
    pub name: String,
    /// The ActiveSync folder type ([MS-ASCMD] 2.2.3.186.3).
    pub kind: u8,
    /// The cloud collection behind a calendar, contacts, task or notes
    /// folder; `None` for mail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
}

/// A Ping's folders and heartbeat, kept for an empty Ping.
#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct PingRequest {
    pub heartbeat: u64,
    pub folders: Vec<String>,
}

/// A folder's sync state for one device.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Collection<T> {
    pub key: u32,
    pub data: T,
    /// The state at the previous key, until the device uses the new one.
    pub previous: Option<(u32, T)>,
}

impl<T: Clone> Collection<T> {
    /// The state the device's `key` refers to: the current one, or the one
    /// before it when the device repeats a request whose answer it lost.
    /// `None` for any other key: the device must start again from 0.
    pub fn at_key(&self, key: u32) -> Option<T> {
        if key == self.key {
            Some(self.data.clone())
        } else {
            self.previous
                .as_ref()
                .filter(|(previous, _)| *previous == key)
                .map(|(_, data)| data.clone())
        }
    }

    /// Moves to the next key with `data`, keeping the state `from` (the key
    /// the device sent) as the previous one.
    pub fn advance(&mut self, from: u32, from_data: T, data: T) -> u32 {
        let next = from.wrapping_add(1).max(1);
        self.previous = Some((from, from_data));
        self.key = next;
        self.data = data;
        next
    }
}

fn device_key(ctx: &Ctx<'_>) -> Vec<u8> {
    let device = ctx.line.device_id.as_bytes();
    let mut key = Vec::with_capacity(device.len() + 7);
    key.push(KIND_DEVICE);
    key.extend_from_slice(&ctx.account_id().to_be_bytes());
    key.push(device.len() as u8);
    key.extend_from_slice(device);
    key
}

fn collection_key(ctx: &Ctx<'_>, collection_id: &str) -> Vec<u8> {
    let mut key = device_key(ctx);
    key[0] = KIND_COLLECTION;
    key.extend_from_slice(collection_id.as_bytes());
    key
}

async fn load<T: DeserializeOwned>(ctx: &Ctx<'_>, key: Vec<u8>) -> Result<Option<T>> {
    let value = ctx
        .server
        .in_memory_store()
        .key_get::<String>(KeyValue::<()>::build_key(KV_ACTIVESYNC, key))
        .await?;
    match value {
        // State that can't be read (written by a different version) is as
        // good as none: the device syncs again from the start.
        Some(json) => Ok(serde_json::from_str(&json).ok()),
        None => Ok(None),
    }
}

async fn save<T: Serialize>(ctx: &Ctx<'_>, key: Vec<u8>, value: &T) -> Result<()> {
    let json = serde_json::to_string(value).map_err(|err| Error::Internal(err.to_string()))?;
    ctx.server
        .in_memory_store()
        .key_set(KeyValue::with_prefix(KV_ACTIVESYNC, key, json.into_bytes()).expires(STATE_TTL))
        .await?;
    Ok(())
}

impl Ctx<'_> {
    /// This device's record, or a new one.
    pub async fn device(&self) -> Result<Device> {
        Ok(load(self, device_key(self))
            .await?
            .unwrap_or_else(|| Device {
                device_type: self.line.device_type.clone(),
                user_agent: self.user_agent.clone(),
                ..Default::default()
            }))
    }

    pub async fn save_device(&self, device: &Device) -> Result<()> {
        save(self, device_key(self), device).await
    }

    /// A folder's sync state on this device.
    pub async fn collection<T: DeserializeOwned>(&self, id: &str) -> Result<Option<Collection<T>>> {
        load(self, collection_key(self, id)).await
    }

    pub async fn save_collection<T: Serialize>(
        &self,
        id: &str,
        collection: &Collection<T>,
    ) -> Result<()> {
        save(self, collection_key(self, id), collection).await
    }

    /// Forgets a folder's state (the device started it again from 0, or the
    /// folder is gone).
    pub async fn drop_collection(&self, id: &str) -> Result<()> {
        self.server
            .in_memory_store()
            .key_delete(KeyValue::<()>::build_key(
                KV_ACTIVESYNC,
                collection_key(self, id),
            ))
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Collection;

    #[test]
    fn keys_advance_and_the_previous_one_is_kept() {
        let mut collection = Collection {
            key: 1,
            data: "a".to_string(),
            previous: None,
        };
        assert_eq!(collection.at_key(1).as_deref(), Some("a"));
        let next = collection.advance(1, "a".into(), "b".into());
        assert_eq!(next, 2);
        // The device repeats key 1 (it lost the answer): still known.
        assert_eq!(collection.at_key(1).as_deref(), Some("a"));
        assert_eq!(collection.at_key(2).as_deref(), Some("b"));
        assert_eq!(collection.at_key(7), None);
        // Using key 2 drops key 1.
        collection.advance(2, "b".into(), "c".into());
        assert_eq!(collection.at_key(1), None);
        // Wrapping never lands on 0, which means "start again".
        let mut collection = Collection {
            key: u32::MAX,
            data: 0u8,
            previous: None,
        };
        assert_eq!(collection.advance(u32::MAX, 0, 1), 1);
    }
}
