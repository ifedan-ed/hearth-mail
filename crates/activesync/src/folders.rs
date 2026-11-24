/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The folder hierarchy: FolderSync, FolderCreate, FolderUpdate and
//! FolderDelete ([MS-ASCMD] 2.2.1.3 to 2.2.1.6).
//!
//! Mail folders are the account's mailboxes, with ids `m<mailbox id>`; the
//! special ones get the types ActiveSync gives them (Inbox 2, Drafts 3,
//! Deleted Items 4, Sent Items 5), the rest are user mail folders (12).
//! The device's copy of the list is kept in its [`Device`] record, so each
//! FolderSync answers with the differences only.

use crate::{
    Ctx, Reply,
    error::Result,
    jmap::Call,
    state::{Device, FolderEntry, Hierarchy},
};
use activesync_proto::wbxml::{Element, codepages::folderhierarchy as fh};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// ActiveSync folder types ([MS-ASCMD] 2.2.3.186.3).
pub mod kind {
    pub const INBOX: u8 = 2;
    pub const DRAFTS: u8 = 3;
    pub const DELETED: u8 = 4;
    pub const SENT: u8 = 5;
    pub const OUTBOX: u8 = 6;
    pub const TASKS: u8 = 7;
    pub const CALENDAR: u8 = 8;
    pub const CONTACTS: u8 = 9;
    pub const NOTES: u8 = 10;
    pub const USER_MAIL: u8 = 12;
    pub const USER_CALENDAR: u8 = 13;
    pub const USER_CONTACTS: u8 = 14;
    pub const USER_TASKS: u8 = 15;
    pub const USER_NOTES: u8 = 17;

    /// Whether a folder of this type holds mail.
    pub fn is_mail(kind: u8) -> bool {
        matches!(kind, 1..=6 | 12)
    }
}

/// What kind of items a folder holds, as Sync's `Class` names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Email,
    Calendar,
    Contacts,
    Tasks,
    Notes,
}

impl Class {
    pub fn of_kind(kind: u8) -> Class {
        match kind {
            kind::TASKS | kind::USER_TASKS => Class::Tasks,
            kind::CALENDAR | kind::USER_CALENDAR => Class::Calendar,
            kind::CONTACTS | kind::USER_CONTACTS => Class::Contacts,
            kind::NOTES | kind::USER_NOTES => Class::Notes,
            _ => Class::Email,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Class::Email => "Email",
            Class::Calendar => "Calendar",
            Class::Contacts => "Contacts",
            Class::Tasks => "Tasks",
            Class::Notes => "Notes",
        }
    }
}

/// The prefix of mail folder ids.
const MAIL_PREFIX: char = 'm';
/// The parent id of top-level folders.
pub const ROOT: &str = "0";

/// A mail folder's id from its mailbox id.
pub fn mail_folder_id(mailbox_id: &str) -> String {
    format!("{MAIL_PREFIX}{mailbox_id}")
}

/// The mailbox id of a mail folder id, if it is one.
pub fn mailbox_of(folder_id: &str) -> Option<&str> {
    folder_id
        .strip_prefix(MAIL_PREFIX)
        .filter(|id| !id.is_empty())
}

/// The account's folders as they are now, by id: its mailboxes, and the
/// cloud's collections when the DAV bridge reaches them.
pub async fn current(ctx: &Ctx<'_>) -> Result<BTreeMap<String, FolderEntry>> {
    let mut answers = ctx
        .jmap(vec![Call::new(
            "Mailbox/get",
            "m",
            json!({ "properties": ["id", "name", "parentId", "role"] }),
        )])
        .await?;
    let mailboxes = answers.take("m")?;
    let mut folders = BTreeMap::new();
    for mailbox in mailboxes["list"].as_array().into_iter().flatten() {
        let Some(id) = mailbox["id"].as_str() else {
            continue;
        };
        let kind = match mailbox["role"].as_str() {
            Some("inbox") => kind::INBOX,
            Some("drafts") => kind::DRAFTS,
            Some("trash") => kind::DELETED,
            Some("sent") => kind::SENT,
            _ => kind::USER_MAIL,
        };
        let parent = mailbox["parentId"]
            .as_str()
            .map_or_else(|| ROOT.to_string(), mail_folder_id);
        folders.insert(
            mail_folder_id(id),
            FolderEntry {
                parent,
                name: mailbox["name"].as_str().unwrap_or_default().to_string(),
                kind,
                href: None,
            },
        );
    }
    ctx.dav_folders(&mut folders).await?;
    Ok(folders)
}

/// FolderSync status codes ([MS-ASCMD] 2.2.3.177.5).
mod status {
    pub const OK: &str = "1";
    pub const BAD_SYNC_KEY: &str = "9";
    pub const MALFORMED: &str = "10";
}

/// FolderSync: the differences between the device's list and the account's.
pub async fn folder_sync(ctx: &Ctx<'_>, request: Element) -> Result<Reply> {
    let Some(key) = request.child_str(fh::SYNC_KEY).map(str::trim) else {
        return Ok(answer(fh::FOLDER_SYNC, status::MALFORMED, None));
    };
    let mut device = ctx.device().await?;
    let known = if key == "0" {
        Some(BTreeMap::new())
    } else {
        key.parse::<u32>()
            .ok()
            .and_then(|key| device_list_at(&device.hierarchy, key))
    };
    let Some(known) = known else {
        return Ok(answer(fh::FOLDER_SYNC, status::BAD_SYNC_KEY, None));
    };
    let now = current(ctx).await?;
    let changes = diff(&known, &now);
    let sent_key: u32 = key.parse().unwrap_or(0);
    let new_key = if changes.is_empty() && sent_key != 0 {
        // Nothing changed: the key stays, and the state with it.
        sent_key
    } else {
        let next = sent_key.wrapping_add(1).max(1);
        device.hierarchy = Hierarchy {
            key: next,
            folders: now,
            previous: (sent_key != 0).then_some((sent_key, known)),
        };
        device.device_type = ctx.line.device_type.clone();
        device.user_agent = ctx.user_agent.clone();
        ctx.save_device(&device).await?;
        next
    };

    let mut list =
        Element::new(fh::CHANGES).with(Element::text(fh::COUNT, changes.len().to_string()));
    for change in changes {
        list.push(change);
    }
    Ok(Reply::Wbxml(
        Element::new(fh::FOLDER_SYNC)
            .with(Element::text(fh::STATUS, status::OK))
            .with(Element::text(fh::SYNC_KEY, new_key.to_string()))
            .with(list),
    ))
}

/// The device's list at its key, the current or the previous one.
fn device_list_at(hierarchy: &Hierarchy, key: u32) -> Option<BTreeMap<String, FolderEntry>> {
    if key == hierarchy.key {
        Some(hierarchy.folders.clone())
    } else {
        hierarchy
            .previous
            .as_ref()
            .filter(|(previous, _)| *previous == key)
            .map(|(_, folders)| folders.clone())
    }
}

/// Add, Update and Delete elements turning `known` into `now`. Parents are
/// added before their children: the list is ordered by depth.
fn diff(
    known: &BTreeMap<String, FolderEntry>,
    now: &BTreeMap<String, FolderEntry>,
) -> Vec<Element> {
    let mut changes = Vec::new();
    let mut added: Vec<(&String, &FolderEntry)> = Vec::new();
    for (id, entry) in now {
        match known.get(id) {
            None => added.push((id, entry)),
            Some(old) if old != entry => changes.push(folder_element(fh::UPDATE, id, entry)),
            Some(_) => (),
        }
    }
    added.sort_by_key(|(id, _)| depth(now, id));
    let mut adds: Vec<Element> = added
        .into_iter()
        .map(|(id, entry)| folder_element(fh::ADD, id, entry))
        .collect();
    adds.append(&mut changes);
    for id in known.keys().filter(|id| !now.contains_key(*id)) {
        adds.push(Element::new(fh::DELETE).with(Element::text(fh::SERVER_ID, id.as_str())));
    }
    adds
}

fn depth(folders: &BTreeMap<String, FolderEntry>, id: &str) -> usize {
    let mut depth = 0;
    let mut current = id;
    while let Some(entry) = folders.get(current) {
        if entry.parent == ROOT || depth > 64 {
            break;
        }
        depth += 1;
        current = &entry.parent;
    }
    depth
}

fn folder_element(tag: activesync_proto::wbxml::Tag, id: &str, entry: &FolderEntry) -> Element {
    Element::new(tag)
        .with(Element::text(fh::SERVER_ID, id))
        .with(Element::text(fh::PARENT_ID, entry.parent.as_str()))
        .with(Element::text(fh::DISPLAY_NAME, entry.name.as_str()))
        .with(Element::text(fh::TYPE, entry.kind.to_string()))
}

fn answer(tag: activesync_proto::wbxml::Tag, status: &str, key: Option<u32>) -> Reply {
    Reply::Wbxml(
        Element::new(tag)
            .with(Element::text(fh::STATUS, status))
            .with_opt(key.map(|key| Element::text(fh::SYNC_KEY, key.to_string()))),
    )
}

/// The device's hierarchy key must match for the folder commands: they
/// change the list and hand out the next key.
fn checked_device(device: &Device, key: Option<&str>) -> Option<u32> {
    let key: u32 = key?.trim().parse().ok()?;
    (key == device.hierarchy.key && key != 0).then_some(key)
}

/// Saves the account's list as the device's after one of its own changes,
/// under the next key, and returns that key.
async fn bump(ctx: &Ctx<'_>, mut device: Device, from: u32) -> Result<u32> {
    let now = current(ctx).await?;
    let next = from.wrapping_add(1).max(1);
    let old = std::mem::take(&mut device.hierarchy.folders);
    device.hierarchy = Hierarchy {
        key: next,
        folders: now,
        previous: Some((from, old)),
    };
    ctx.save_device(&device).await?;
    Ok(next)
}

/// FolderCreate status codes ([MS-ASCMD] 2.2.3.177.2; FolderUpdate and
/// FolderDelete use the same numbers).
mod folder_status {
    pub const OK: &str = "1";
    pub const EXISTS: &str = "2";
    pub const SPECIAL: &str = "3";
    pub const NOT_FOUND: &str = "4";
    pub const PARENT_NOT_FOUND: &str = "5";
    pub const SERVER_ERROR: &str = "6";
    pub const BAD_SYNC_KEY: &str = "9";
    pub const MALFORMED: &str = "10";
}

/// FolderCreate: a new mail folder (other types come with the DAV bridge).
pub async fn folder_create(ctx: &Ctx<'_>, request: Element) -> Result<Reply> {
    let device = ctx.device().await?;
    let Some(key) = checked_device(&device, request.child_str(fh::SYNC_KEY)) else {
        return Ok(answer(fh::FOLDER_CREATE, folder_status::BAD_SYNC_KEY, None));
    };
    let (Some(parent), Some(name), Some(kind)) = (
        request.child_str(fh::PARENT_ID),
        request.child_str(fh::DISPLAY_NAME),
        request.child_u64(fh::TYPE),
    ) else {
        return Ok(answer(fh::FOLDER_CREATE, folder_status::MALFORMED, None));
    };
    if kind != u64::from(kind::USER_MAIL) {
        return Ok(answer(fh::FOLDER_CREATE, folder_status::SERVER_ERROR, None));
    }
    let parent_id = if parent == ROOT {
        Value::Null
    } else {
        match mailbox_of(parent) {
            Some(id) if device.hierarchy.folders.contains_key(parent) => json!(id),
            _ => {
                return Ok(answer(
                    fh::FOLDER_CREATE,
                    folder_status::PARENT_NOT_FOUND,
                    None,
                ));
            }
        }
    };
    if device
        .hierarchy
        .folders
        .values()
        .any(|entry| entry.parent == parent && entry.name == name)
    {
        return Ok(answer(fh::FOLDER_CREATE, folder_status::EXISTS, None));
    }
    let answers = ctx
        .jmap(vec![Call::new(
            "Mailbox/set",
            "c",
            json!({ "create": { "new": { "name": name, "parentId": parent_id } } }),
        )])
        .await?;
    let Some(id) = answers.get("c")?["created"]["new"]["id"]
        .as_str()
        .map(str::to_string)
    else {
        return Ok(answer(fh::FOLDER_CREATE, folder_status::SERVER_ERROR, None));
    };
    let next = bump(ctx, device, key).await?;
    Ok(Reply::Wbxml(
        Element::new(fh::FOLDER_CREATE)
            .with(Element::text(fh::STATUS, folder_status::OK))
            .with(Element::text(fh::SYNC_KEY, next.to_string()))
            .with(Element::text(fh::SERVER_ID, mail_folder_id(&id))),
    ))
}

/// FolderUpdate: rename or move a mail folder.
pub async fn folder_update(ctx: &Ctx<'_>, request: Element) -> Result<Reply> {
    let device = ctx.device().await?;
    let Some(key) = checked_device(&device, request.child_str(fh::SYNC_KEY)) else {
        return Ok(answer(fh::FOLDER_UPDATE, folder_status::BAD_SYNC_KEY, None));
    };
    let (Some(id), Some(parent), Some(name)) = (
        request.child_str(fh::SERVER_ID),
        request.child_str(fh::PARENT_ID),
        request.child_str(fh::DISPLAY_NAME),
    ) else {
        return Ok(answer(fh::FOLDER_UPDATE, folder_status::MALFORMED, None));
    };
    let Some(entry) = device.hierarchy.folders.get(id) else {
        return Ok(answer(fh::FOLDER_UPDATE, folder_status::NOT_FOUND, None));
    };
    if entry.kind != kind::USER_MAIL {
        return Ok(answer(fh::FOLDER_UPDATE, folder_status::SPECIAL, None));
    }
    let Some(mailbox) = mailbox_of(id) else {
        return Ok(answer(fh::FOLDER_UPDATE, folder_status::NOT_FOUND, None));
    };
    let parent_id = if parent == ROOT {
        Value::Null
    } else {
        match mailbox_of(parent) {
            Some(parent) => json!(parent),
            None => {
                return Ok(answer(
                    fh::FOLDER_UPDATE,
                    folder_status::PARENT_NOT_FOUND,
                    None,
                ));
            }
        }
    };
    let answers = ctx
        .jmap(vec![Call::new(
            "Mailbox/set",
            "u",
            json!({ "update": { mailbox: { "name": name, "parentId": parent_id } } }),
        )])
        .await?;
    if answers.get("u")?["updated"].get(mailbox).is_none() {
        return Ok(answer(fh::FOLDER_UPDATE, folder_status::SERVER_ERROR, None));
    }
    let next = bump(ctx, device, key).await?;
    Ok(answer(fh::FOLDER_UPDATE, folder_status::OK, Some(next)))
}

/// FolderDelete: remove a mail folder and its mail (into undelete, as any
/// client's deletion).
pub async fn folder_delete(ctx: &Ctx<'_>, request: Element) -> Result<Reply> {
    let device = ctx.device().await?;
    let Some(key) = checked_device(&device, request.child_str(fh::SYNC_KEY)) else {
        return Ok(answer(fh::FOLDER_DELETE, folder_status::BAD_SYNC_KEY, None));
    };
    let Some(id) = request.child_str(fh::SERVER_ID) else {
        return Ok(answer(fh::FOLDER_DELETE, folder_status::MALFORMED, None));
    };
    match device.hierarchy.folders.get(id) {
        None => return Ok(answer(fh::FOLDER_DELETE, folder_status::NOT_FOUND, None)),
        Some(entry) if entry.kind != kind::USER_MAIL => {
            return Ok(answer(fh::FOLDER_DELETE, folder_status::SPECIAL, None));
        }
        Some(_) => (),
    }
    let Some(mailbox) = mailbox_of(id) else {
        return Ok(answer(fh::FOLDER_DELETE, folder_status::NOT_FOUND, None));
    };
    let answers = ctx
        .jmap(vec![Call::new(
            "Mailbox/set",
            "d",
            json!({ "destroy": [mailbox], "onDestroyRemoveEmails": true }),
        )])
        .await?;
    let destroyed = answers.get("d")?["destroyed"]
        .as_array()
        .is_some_and(|list| list.iter().any(|v| v.as_str() == Some(mailbox)));
    if !destroyed {
        return Ok(answer(fh::FOLDER_DELETE, folder_status::SERVER_ERROR, None));
    }
    ctx.drop_collection(id).await?;
    let next = bump(ctx, device, key).await?;
    Ok(answer(fh::FOLDER_DELETE, folder_status::OK, Some(next)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(parent: &str, name: &str, kind: u8) -> FolderEntry {
        FolderEntry {
            parent: parent.into(),
            name: name.into(),
            kind,
            href: None,
        }
    }

    #[test]
    fn diff_orders_parents_first_and_lists_every_change() {
        let known: BTreeMap<String, FolderEntry> = [
            ("ma".to_string(), entry(ROOT, "Inbox", kind::INBOX)),
            ("mb".to_string(), entry(ROOT, "Old", kind::USER_MAIL)),
            ("mc".to_string(), entry(ROOT, "Renamed", kind::USER_MAIL)),
        ]
        .into();
        let now: BTreeMap<String, FolderEntry> = [
            ("ma".to_string(), entry(ROOT, "Inbox", kind::INBOX)),
            ("mc".to_string(), entry(ROOT, "New name", kind::USER_MAIL)),
            // A child whose id sorts before its parent's.
            ("md".to_string(), entry("mz", "Child", kind::USER_MAIL)),
            ("mz".to_string(), entry(ROOT, "Parent", kind::USER_MAIL)),
        ]
        .into();
        let changes = diff(&known, &now);
        let summary: Vec<(String, String)> = changes
            .iter()
            .map(|change| {
                (
                    change.tag.name().to_string(),
                    change
                        .child_str(fh::SERVER_ID)
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("Add".to_string(), "mz".to_string()),
                ("Add".to_string(), "md".to_string()),
                ("Update".to_string(), "mc".to_string()),
                ("Delete".to_string(), "mb".to_string()),
            ]
        );
        assert!(diff(&now, &now).is_empty());
    }

    #[test]
    fn folder_ids() {
        assert_eq!(mail_folder_id("abc"), "mabc");
        assert_eq!(mailbox_of("mabc"), Some("abc"));
        assert_eq!(mailbox_of("m"), None);
        assert_eq!(mailbox_of("cxyz"), None);
        assert_eq!(Class::of_kind(kind::INBOX), Class::Email);
        assert_eq!(Class::of_kind(kind::USER_CALENDAR), Class::Calendar);
        assert!(kind::is_mail(kind::SENT) && !kind::is_mail(kind::CALENDAR));
    }
}
