/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use super::type_state::DataType;
use compact_str::CompactString;
use std::{
    fmt::{self, Display, Formatter},
    str::FromStr,
};
use utils::map::bitmap::BitmapItem;

#[derive(
    rkyv::Archive,
    rkyv::Deserialize,
    rkyv::Serialize,
    Debug,
    Clone,
    Copy,
    Hash,
    PartialEq,
    Eq,
    Default,
)]
#[repr(u8)]
/// The kinds of object an account's data is stored under. The number is part
/// of every store key, so it never changes: 8 to 13 were calendars, events,
/// address books, contact cards, file nodes and event notifications, which
/// Hearth Mail does not keep (docs/decisions.md, "Calendars, contacts and
/// files removed"). Keys with those numbers left in an older store are
/// never read, and are removed with the account.
pub enum Collection {
    Email = 0,
    Mailbox = 1,
    Thread = 2,
    Identity = 3,
    EmailSubmission = 4,
    SieveScript = 5,
    PushSubscription = 6,
    Principal = 7,
    #[default]
    None = 14,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Default)]
#[repr(u8)]
/// Change-log streams. Numbers are stored, so they stay fixed: 2, 3, 4 and 8
/// belonged to the removed calendar, contact and file collections.
pub enum SyncCollection {
    Email = 0,
    Thread = 1,
    Identity = 5,
    EmailSubmission = 6,
    SieveScript = 7,
    ShareNotification = 9,
    #[default]
    None = 10,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
#[repr(u8)]
/// Tombstone streams for deleted items; 252 to 254 were calendars, address
/// books and files and are never written now.
pub enum VanishedCollection {
    Email = 251,
}

impl Collection {
    pub const MAX: usize = Collection::None as usize;

    pub fn main_collection(&self) -> Collection {
        match self {
            Collection::Email => Collection::Mailbox,
            _ => *self,
        }
    }

    pub fn parent_collection(&self) -> Option<Collection> {
        match self {
            Collection::Email => Some(Collection::Mailbox),
            _ => None,
        }
    }

    pub fn child_collection(&self) -> Option<Collection> {
        match self {
            Collection::Mailbox => Some(Collection::Email),
            _ => None,
        }
    }
}

impl SyncCollection {
    pub fn collection(&self, is_container: bool) -> Collection {
        match self {
            SyncCollection::Email => {
                if is_container {
                    Collection::Mailbox
                } else {
                    Collection::Email
                }
            }
            SyncCollection::Thread => Collection::Thread,
            SyncCollection::Identity => Collection::Identity,
            SyncCollection::EmailSubmission => Collection::EmailSubmission,
            SyncCollection::SieveScript => Collection::SieveScript,
            SyncCollection::ShareNotification | SyncCollection::None => Collection::None,
        }
    }

    pub fn vanished_collection(&self) -> Option<VanishedCollection> {
        match self {
            SyncCollection::Email => Some(VanishedCollection::Email),
            _ => None,
        }
    }
}

impl From<Collection> for SyncCollection {
    fn from(v: Collection) -> Self {
        match v {
            Collection::Email => SyncCollection::Email,
            Collection::Mailbox => SyncCollection::Email,
            Collection::Thread => SyncCollection::Thread,
            Collection::Identity => SyncCollection::Identity,
            Collection::EmailSubmission => SyncCollection::EmailSubmission,
            Collection::SieveScript => SyncCollection::SieveScript,
            Collection::PushSubscription => SyncCollection::None,
            Collection::Principal => SyncCollection::None,
            _ => SyncCollection::None,
        }
    }
}

impl From<u8> for Collection {
    fn from(v: u8) -> Self {
        match v {
            0 => Collection::Email,
            1 => Collection::Mailbox,
            2 => Collection::Thread,
            3 => Collection::Identity,
            4 => Collection::EmailSubmission,
            5 => Collection::SieveScript,
            6 => Collection::PushSubscription,
            7 => Collection::Principal,
            _ => Collection::None,
        }
    }
}

impl From<u8> for SyncCollection {
    fn from(v: u8) -> Self {
        match v {
            0 => SyncCollection::Email,
            1 => SyncCollection::Thread,
            5 => SyncCollection::Identity,
            6 => SyncCollection::EmailSubmission,
            7 => SyncCollection::SieveScript,
            9 => SyncCollection::ShareNotification,
            _ => SyncCollection::None,
        }
    }
}

impl From<u64> for SyncCollection {
    fn from(v: u64) -> Self {
        match v {
            0 => SyncCollection::Email,
            1 => SyncCollection::Thread,
            5 => SyncCollection::Identity,
            6 => SyncCollection::EmailSubmission,
            7 => SyncCollection::SieveScript,
            9 => SyncCollection::ShareNotification,
            _ => SyncCollection::None,
        }
    }
}

impl From<u64> for Collection {
    fn from(v: u64) -> Self {
        match v {
            0 => Collection::Email,
            1 => Collection::Mailbox,
            2 => Collection::Thread,
            3 => Collection::Identity,
            4 => Collection::EmailSubmission,
            5 => Collection::SieveScript,
            6 => Collection::PushSubscription,
            7 => Collection::Principal,
            _ => Collection::None,
        }
    }
}

impl From<Collection> for u8 {
    fn from(v: Collection) -> Self {
        v as u8
    }
}

impl From<SyncCollection> for u8 {
    fn from(v: SyncCollection) -> Self {
        v as u8
    }
}

impl From<SyncCollection> for u64 {
    fn from(v: SyncCollection) -> Self {
        v as u64
    }
}

impl From<VanishedCollection> for u8 {
    fn from(v: VanishedCollection) -> Self {
        v as u8
    }
}

impl From<Collection> for u64 {
    fn from(collection: Collection) -> u64 {
        collection as u64
    }
}

impl TryFrom<Collection> for DataType {
    type Error = ();

    fn try_from(value: Collection) -> Result<Self, Self::Error> {
        match value {
            Collection::Email => Ok(DataType::Email),
            Collection::Mailbox => Ok(DataType::Mailbox),
            Collection::Thread => Ok(DataType::Thread),
            Collection::Identity => Ok(DataType::Identity),
            Collection::EmailSubmission => Ok(DataType::EmailSubmission),
            Collection::SieveScript => Ok(DataType::SieveScript),
            Collection::PushSubscription => Ok(DataType::PushSubscription),
            Collection::Principal => Ok(DataType::Principal),
            _ => Err(()),
        }
    }
}

impl TryFrom<DataType> for Collection {
    type Error = ();

    fn try_from(value: DataType) -> Result<Self, Self::Error> {
        match value {
            DataType::Email => Ok(Collection::Email),
            DataType::Mailbox => Ok(Collection::Mailbox),
            DataType::Thread => Ok(Collection::Thread),
            DataType::Identity => Ok(Collection::Identity),
            DataType::EmailSubmission => Ok(Collection::EmailSubmission),
            DataType::SieveScript => Ok(Collection::SieveScript),
            DataType::PushSubscription => Ok(Collection::PushSubscription),
            DataType::Principal => Ok(Collection::Principal),
            _ => Err(()),
        }
    }
}

impl Display for Collection {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(f)
    }
}

impl Collection {
    pub fn as_str(&self) -> &'static str {
        match self {
            Collection::PushSubscription => "pushSubscription",
            Collection::Email => "email",
            Collection::Mailbox => "mailbox",
            Collection::Thread => "thread",
            Collection::Identity => "identity",
            Collection::EmailSubmission => "emailSubmission",
            Collection::SieveScript => "sieveScript",
            Collection::Principal => "principal",
            Collection::None => "",
        }
    }

    pub fn as_config_case(&self) -> &'static str {
        match self {
            Collection::PushSubscription => "push-subscription",
            Collection::Email => "email",
            Collection::Mailbox => "mailbox",
            Collection::Thread => "thread",
            Collection::Identity => "identity",
            Collection::EmailSubmission => "email-submission",
            Collection::SieveScript => "sieve-script",
            Collection::Principal => "principal",
            Collection::None => "",
        }
    }
}

impl FromStr for Collection {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        hashify::tiny_map!(s.as_bytes(),
            "pushSubscription" => Collection::PushSubscription,
            "email" => Collection::Email,
            "mailbox" => Collection::Mailbox,
            "thread" => Collection::Thread,
            "identity" => Collection::Identity,
            "emailSubmission" => Collection::EmailSubmission,
            "sieveScript" => Collection::SieveScript,
            "principal" => Collection::Principal,
        )
        .ok_or(())
    }
}

impl From<Collection> for trc::Value {
    fn from(value: Collection) -> Self {
        trc::Value::String(CompactString::const_new(value.as_str()))
    }
}

impl BitmapItem for Collection {
    fn max() -> u64 {
        Collection::None as u64
    }

    fn is_valid(&self) -> bool {
        !matches!(self, Collection::None)
    }
}

impl BitmapItem for SyncCollection {
    fn max() -> u64 {
        SyncCollection::None as u64
    }

    fn is_valid(&self) -> bool {
        !matches!(self, SyncCollection::None)
    }
}

impl SyncCollection {
    pub fn as_str(&self) -> &'static str {
        match self {
            SyncCollection::Email => "email",
            SyncCollection::Thread => "thread",
            SyncCollection::Identity => "identity",
            SyncCollection::EmailSubmission => "emailSubmission",
            SyncCollection::SieveScript => "sieveScript",
            SyncCollection::ShareNotification => "shareNotification",
            SyncCollection::None => "",
        }
    }
}
