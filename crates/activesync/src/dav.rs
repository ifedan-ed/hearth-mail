/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Calendars, contacts, tasks and notes through the DAV bridge: their
//! folders in FolderSync, and Sync, GetItemEstimate and Ping for them
//! (docs/activesync.md, "the DAV bridge").
//!
//! Each cloud collection the account can write becomes a folder: a
//! calendar holding events a calendar folder, one holding to-dos a task
//! folder (a Nextcloud calendar holding both is both), one holding
//! journals a notes folder, an address book a contacts folder. The first of
//! each kind is the default one the device shows. When no calendar holds
//! to-dos or journals, a "Tasks" or "Notes" calendar is made for them.
//!
//! A folder's state on a device is a [`DavFolder`]: the collection's
//! sync token (RFC 6578), the items the device holds (a short server id
//! for each href, with the ETag it has), and the items still to send.
//! Changes come from `sync-collection` since the token; items are read
//! with one multiget per window and converted by
//! [`activesync_proto::items`]. Writes from the device are `PUT` with
//! `If-Match` (or `If-None-Match: *` for new ones), so a change made in the
//! cloud meanwhile is never overwritten blindly: the device's change is
//! refused (status 7, conflict) and the cloud's version comes down.

use crate::{
    Ctx, bridge,
    error::{Error, Result},
    folders::{Class, ROOT, kind},
    jmap::Call,
    mail::item::{BodyOptions, body_type},
    state::FolderEntry,
};
use activesync_proto::{
    items::{self, BodyPreference, BodyType, Context, Omitted},
    wbxml::{Element, Tag, codepages::airsync as sync},
};
use dav_client::{Collection, CollectionKind, Components, Condition, DavClient, DavError, Url};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

/// A cloud folder as one device has it.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct DavFolder {
    /// The collection's sync token; empty before the first listing.
    pub token: String,
    pub primed: bool,
    /// Items on the device: server id -> (href, ETag).
    pub have: BTreeMap<String, (String, String)>,
    /// Items still to add, by href.
    pub pending: Vec<String>,
    /// The last server id handed out.
    pub next_id: u64,
    pub body: BodyOptions,
    /// The `Supported` elements the device named on its first Sync (14.1),
    /// as (code page, token).
    pub supported: Option<Vec<(u8, u8)>>,
}

impl DavFolder {
    fn id_of(&self, href: &str) -> Option<&String> {
        self.have
            .iter()
            .find(|(_, (h, _))| h == href)
            .map(|(id, _)| id)
    }

    fn new_id(&mut self) -> String {
        self.next_id += 1;
        self.next_id.to_string()
    }
}

/// What a cloud folder needs to send the device.
#[derive(Default)]
pub struct DavChanges {
    pub deleted: Vec<String>,
    /// Server ids whose item changed in the cloud.
    pub changed: Vec<String>,
}

/// A short, stable folder id for a collection: a class letter and a hash
/// of the href (FNV-1a), so the id survives a rename.
fn folder_id(class: Class, href: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in href.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let letter = match class {
        Class::Calendar => 'c',
        Class::Tasks => 't',
        Class::Notes => 'n',
        Class::Contacts => 'k',
        Class::Email => 'm',
    };
    format!("{letter}{:012x}", hash & 0xffff_ffff_ffff)
}

/// Whether a folder id is a cloud folder's.
pub fn is_dav_folder(id: &str) -> bool {
    matches!(id.chars().next(), Some('c' | 't' | 'n' | 'k')) && id.len() == 13
}

impl Ctx<'_> {
    /// A client for the cloud, or `None` when this device has no way in.
    pub async fn dav_client(&self) -> Result<Option<DavClient>> {
        Ok(bridge::client(
            self.server,
            self.account_id(),
            self.basic.as_ref().map(|(u, p)| (u.as_str(), p.as_str())),
        )
        .await?)
    }

    /// The cloud's folders, added to `folders`. A cloud that can't be
    /// reached leaves them out of this answer (and logs why); mail goes on.
    pub async fn dav_folders(&self, folders: &mut BTreeMap<String, FolderEntry>) -> Result<()> {
        let Some(client) = self.dav_client().await? else {
            return Ok(());
        };
        match list(&client).await {
            Ok(collections) => {
                for (id, entry) in collections {
                    folders.insert(id, entry);
                }
            }
            Err(err) => self.dav_failed("Listing the cloud's folders failed", &err),
        }
        Ok(())
    }

    fn dav_failed(&self, what: &str, err: &DavError) {
        trc::event!(
            Http(trc::HttpEvent::Error),
            SpanId = self.session.session_id,
            Details = what.to_string(),
            Reason = err.to_string(),
        );
    }

    /// The conversion context for this account and request.
    pub async fn item_context(
        &self,
        body: &BodyOptions,
    ) -> Result<(Vec<String>, Option<String>, BodyPreference)> {
        let answers = self
            .jmap(vec![Call::new("Identity/get", "i", json!({}))])
            .await?;
        let identities = answers.get("i")?["list"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let addresses: Vec<String> = identities
            .iter()
            .filter_map(|identity| identity["email"].as_str().map(str::to_lowercase))
            .collect();
        let name = identities
            .first()
            .and_then(|identity| identity["name"].as_str())
            .filter(|name| !name.is_empty())
            .map(str::to_string);
        let preference = body
            .preferences
            .iter()
            .find(|p| p.kind == body_type::PLAIN || p.kind == body_type::HTML)
            .map(|p| BodyPreference {
                kind: if p.kind == body_type::HTML {
                    BodyType::Html
                } else {
                    BodyType::Plain
                },
                truncation: p.truncation,
            })
            .unwrap_or_default();
        Ok((addresses, name, preference))
    }
}

/// The writable collections as folders, defaults first. A missing notes
/// calendar is made.
async fn list(client: &DavClient) -> std::result::Result<Vec<(String, FolderEntry)>, DavError> {
    let homes = client.discover().await?;
    let mut collections: Vec<Collection> = Vec::new();
    if let Some(home) = &homes.calendar_home {
        collections.extend(client.list_collections(home).await?);
        // Tasks and notes need a calendar each; make the missing ones
        // (Nextcloud's own calendars hold events only).
        let mut made = false;
        for (wanted, path, name) in [
            (
                Components {
                    event: false,
                    todo: true,
                    journal: false,
                },
                "tasks-activesync/",
                "Tasks",
            ),
            (Components::JOURNAL, "notes-activesync/", "Notes"),
        ] {
            let present = collections.iter().any(|c| {
                c.kind == CollectionKind::Calendar
                    && c.writable != Some(false)
                    && ((wanted.todo && c.components.todo)
                        || (wanted.journal && c.components.journal))
            });
            if !present {
                let href = home
                    .join(path)
                    .map_err(|err| DavError::InvalidUrl(err.to_string()))?;
                client.mkcalendar(&href, name, wanted).await?;
                made = true;
            }
        }
        if made {
            collections = client.list_collections(home).await?;
        }
    }
    if let Some(home) = &homes.addressbook_home {
        collections.extend(client.list_collections(home).await?);
    }
    let mut out = Vec::new();
    let mut seen_default = [false; 4];
    for collection in collections.iter().filter(|c| c.writable != Some(false)) {
        let href = collection.href.as_str();
        let name = collection
            .display_name
            .clone()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| {
                href.trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or("")
                    .to_string()
            });
        let mut add = |class: Class, default: u8, user: u8, slot: usize| {
            let kind = if seen_default[slot] { user } else { default };
            seen_default[slot] = true;
            out.push((
                folder_id(class, href),
                FolderEntry {
                    parent: ROOT.to_string(),
                    name: name.clone(),
                    kind,
                    href: Some(href.to_string()),
                },
            ));
        };
        match collection.kind {
            CollectionKind::AddressBook => {
                add(Class::Contacts, kind::CONTACTS, kind::USER_CONTACTS, 0)
            }
            CollectionKind::Calendar => {
                if collection.components.event {
                    add(Class::Calendar, kind::CALENDAR, kind::USER_CALENDAR, 1);
                }
                if collection.components.todo {
                    add(Class::Tasks, kind::TASKS, kind::USER_TASKS, 2);
                }
                if collection.components.journal {
                    add(Class::Notes, kind::NOTES, kind::USER_NOTES, 3);
                }
            }
        }
    }
    Ok(out)
}

/// Whether a resource's text holds items of `class` (a calendar shared by
/// events and to-dos lists both in each folder's collection).
fn holds(class: Class, text: &str) -> bool {
    match class {
        Class::Calendar => text.contains("BEGIN:VEVENT"),
        Class::Tasks => text.contains("BEGIN:VTODO"),
        Class::Notes => text.contains("BEGIN:VJOURNAL"),
        _ => true,
    }
}

fn collection_kind(class: Class) -> CollectionKind {
    match class {
        Class::Contacts => CollectionKind::AddressBook,
        _ => CollectionKind::Calendar,
    }
}

/// The device's item as iCalendar or vCard text.
fn to_text(
    class: Class,
    data: &Element,
    existing: Option<&str>,
    omitted: Omitted<'_>,
    ctx: &Context<'_>,
) -> items::Result<String> {
    match class {
        Class::Calendar => items::calendar::to_ical(data, existing, omitted, ctx),
        Class::Tasks => items::task::to_ical(data, existing, omitted, ctx),
        Class::Notes => items::note::to_ical(data, existing, omitted, ctx),
        Class::Contacts => items::contact::to_vcard(data, existing, omitted, ctx),
        Class::Email => Err(items::ItemError::Unreadable("mail isn't a cloud item")),
    }
}

/// A cloud item as `ApplicationData`.
fn from_text(class: Class, text: &str, ctx: &Context<'_>) -> items::Result<Element> {
    match class {
        Class::Calendar => items::calendar::from_ical(text, ctx),
        Class::Tasks => items::task::from_ical(text, ctx),
        Class::Notes => items::note::from_ical(text, ctx),
        Class::Contacts => items::contact::from_vcard(text, ctx),
        Class::Email => Err(items::ItemError::Unreadable("mail isn't a cloud item")),
    }
}

/// Sync status codes used here ([MS-ASCMD] 2.2.3.177.17).
mod status {
    pub const OK: &str = "1";
    pub const CONVERSION: &str = "6";
    pub const CONFLICT: &str = "7";
    pub const NOT_FOUND: &str = "8";
}

/// One cloud folder's part of a Sync: the device's commands applied, then
/// up to `window` changes from the cloud. Returns the Commands and
/// Responses elements' children and whether more wait.
pub struct DavSyncResult {
    pub commands: Vec<Element>,
    pub responses: Vec<Element>,
    pub more: bool,
}

/// Runs one Sync step for a cloud folder whose state is `folder`.
#[allow(clippy::too_many_arguments)]
pub async fn sync_folder(
    ctx: &Ctx<'_>,
    client: &DavClient,
    class: Class,
    href: &str,
    folder: &mut DavFolder,
    commands: Option<&Element>,
    get_changes: bool,
    window: usize,
) -> Result<DavSyncResult> {
    let collection = Url::parse(href).map_err(|err| Error::Internal(err.to_string()))?;
    let (addresses, name, body) = ctx.item_context(&folder.body).await?;
    let conv = Context {
        version: ctx.version,
        zone: chrono_tz::UTC,
        now: chrono::Utc::now(),
        addresses: &addresses,
        name: name.as_deref(),
        body,
    };
    let mut result = DavSyncResult {
        commands: Vec::new(),
        responses: Vec::new(),
        more: false,
    };
    if let Some(commands) = commands {
        for command in commands.children() {
            if let Some(response) =
                apply(ctx, client, class, &collection, folder, command, &conv).await?
            {
                result.responses.push(response);
            }
        }
    }
    if !get_changes {
        return Ok(result);
    }
    let changes = match changes(client, &collection, folder).await {
        Ok(changes) => changes,
        Err(err) => {
            ctx.dav_failed("Reading the cloud's changes failed", &err);
            return Ok(result);
        }
    };
    let mut room = window;
    for id in changes.deleted {
        result
            .commands
            .push(Element::new(sync::DELETE).with(Element::text(sync::SERVER_ID, id)));
        room = room.saturating_sub(1);
    }
    // Changed and new items, read in one multiget.
    let changed: Vec<(String, String)> = changes
        .changed
        .iter()
        .filter_map(|id| {
            folder
                .have
                .get(id)
                .map(|(href, _)| (id.clone(), href.clone()))
        })
        .take(room)
        .collect();
    room = room.saturating_sub(changed.len());
    let take = room.min(folder.pending.len());
    let added: Vec<String> = folder.pending.drain(..take).collect();
    let hrefs: Vec<Url> = changed
        .iter()
        .map(|(_, href)| href.as_str())
        .chain(added.iter().map(String::as_str))
        .filter_map(|href| Url::parse(href).ok())
        .collect();
    if !hrefs.is_empty() {
        let read = client
            .multiget(&collection, collection_kind(class), &hrefs)
            .await
            .map_err(|err| Error::Internal(err.to_string()))?;
        for resource in read.found {
            let href = resource.href.to_string();
            if !holds(class, &resource.data) {
                continue;
            }
            let data = match from_text(class, &resource.data, &conv) {
                Ok(data) => data,
                // An item the conversion can't read is left out, logged.
                Err(err) => {
                    trc::event!(
                        Http(trc::HttpEvent::Error),
                        SpanId = ctx.session.session_id,
                        Details = "A cloud item couldn't be converted",
                        Url = href.clone(),
                        Reason = err.to_string(),
                    );
                    continue;
                }
            };
            let etag = resource.etag.clone().unwrap_or_default();
            let (tag, id) = match folder.id_of(&href).cloned() {
                Some(id) => (sync::CHANGE, id),
                None => (sync::ADD, folder.new_id()),
            };
            folder.have.insert(id.clone(), (href, etag));
            result.commands.push(
                Element::new(tag)
                    .with(Element::text(sync::SERVER_ID, id))
                    .with(data),
            );
        }
    }
    result.more = !folder.pending.is_empty();
    Ok(result)
}

/// The cloud's changes since the folder's token; the folder's token and
/// pending list are brought up to date.
pub(crate) async fn changes(
    client: &DavClient,
    collection: &Url,
    folder: &mut DavFolder,
) -> std::result::Result<DavChanges, DavError> {
    let token = if folder.primed {
        folder.token.clone()
    } else {
        String::new()
    };
    let (sync, full) = match client.sync_collection(collection, &token).await {
        Ok(sync) => (sync, token.is_empty()),
        Err(DavError::InvalidSyncToken) => (client.sync_collection(collection, "").await?, true),
        Err(err) => return Err(err),
    };
    let mut result = DavChanges::default();
    let mut listed = std::collections::HashSet::new();
    for member in &sync.changed {
        let href = member.href.to_string();
        listed.insert(href.clone());
        let etag = member.etag.clone().unwrap_or_default();
        match folder.id_of(&href).cloned() {
            Some(id) => {
                if folder.have.get(&id).map(|(_, e)| e.as_str()) != Some(etag.as_str()) {
                    result.changed.push(id);
                }
            }
            None => {
                if !folder.pending.contains(&href) {
                    folder.pending.push(href);
                }
            }
        }
    }
    let removed: Vec<String> = if full {
        // A full listing: what isn't listed is gone.
        folder
            .have
            .iter()
            .filter(|(_, (href, _))| !listed.contains(href))
            .map(|(_, (href, _))| href.clone())
            .collect()
    } else {
        sync.removed.iter().map(Url::to_string).collect()
    };
    for href in removed {
        if let Some(id) = folder.id_of(&href).cloned() {
            folder.have.remove(&id);
            result.deleted.push(id);
        }
        folder.pending.retain(|pending| *pending != href);
    }
    folder.token = sync.token;
    folder.primed = true;
    Ok(result)
}

/// Applies one device command; returns its response element, if any.
async fn apply(
    ctx: &Ctx<'_>,
    client: &DavClient,
    class: Class,
    collection: &Url,
    folder: &mut DavFolder,
    command: &Element,
    conv: &Context<'_>,
) -> Result<Option<Element>> {
    let server_id = command
        .child_str(sync::SERVER_ID)
        .unwrap_or_default()
        .to_string();
    let supported: Vec<Tag> = folder
        .supported
        .iter()
        .flatten()
        .map(|(page, token)| Tag::new(*page, *token))
        .collect();
    match command.tag {
        sync::ADD => {
            let client_id = command
                .child_str(sync::CLIENT_ID)
                .unwrap_or_default()
                .to_string();
            let empty = Element::new(sync::APPLICATION_DATA);
            let data = command.child(sync::APPLICATION_DATA).unwrap_or(&empty);
            let reply = |code: &str, id: Option<String>| {
                Element::new(sync::ADD)
                    .with(Element::text(sync::CLIENT_ID, client_id.as_str()))
                    .with_opt(id.map(|id| Element::text(sync::SERVER_ID, id)))
                    .with(Element::text(sync::STATUS, code))
            };
            let Ok(text) = to_text(class, data, None, Omitted::Cleared, conv) else {
                return Ok(Some(reply(status::CONVERSION, None)));
            };
            let extension = if class == Class::Contacts {
                "vcf"
            } else {
                "ics"
            };
            let name = format!("{:032x}.{extension}", rand::random::<u128>());
            let href = collection
                .join(&name)
                .map_err(|err| Error::Internal(err.to_string()))?;
            match client
                .put(&href, collection_kind(class), text, Condition::Create)
                .await
            {
                Ok(etag) => {
                    let id = folder.new_id();
                    folder.have.insert(id.clone(), (href.to_string(), etag));
                    Ok(Some(reply(status::OK, Some(id))))
                }
                Err(err) => {
                    ctx.dav_failed("Adding a cloud item failed", &err);
                    Ok(Some(reply(status::CONVERSION, None)))
                }
            }
        }
        sync::CHANGE => {
            let Some((href, etag)) = folder.have.get(&server_id).cloned() else {
                return Ok(Some(failed(sync::CHANGE, &server_id, status::NOT_FOUND)));
            };
            let empty = Element::new(sync::APPLICATION_DATA);
            let data = command.child(sync::APPLICATION_DATA).unwrap_or(&empty);
            let url = Url::parse(&href).map_err(|err| Error::Internal(err.to_string()))?;
            let existing = match client.get(&url).await {
                Ok(resource) => resource,
                Err(DavError::NotFound) => {
                    return Ok(Some(failed(sync::CHANGE, &server_id, status::NOT_FOUND)));
                }
                Err(err) => return Err(Error::Internal(err.to_string())),
            };
            // The cloud changed it since the device saw it: the cloud wins.
            if existing
                .etag
                .as_deref()
                .is_some_and(|current| current != etag)
            {
                return Ok(Some(failed(sync::CHANGE, &server_id, status::CONFLICT)));
            }
            let omitted = match (class, ctx.version.is_16(), supported.is_empty()) {
                (Class::Calendar, true, _) => Omitted::Kept,
                (Class::Calendar | Class::Contacts, false, false) => Omitted::Supported(&supported),
                (Class::Calendar | Class::Contacts, false, true) if folder.supported.is_some() => {
                    Omitted::Kept
                }
                _ => Omitted::Cleared,
            };
            let Ok(text) = to_text(class, data, Some(&existing.data), omitted, conv) else {
                return Ok(Some(failed(sync::CHANGE, &server_id, status::CONVERSION)));
            };
            match client
                .put(&url, collection_kind(class), text, Condition::Update(&etag))
                .await
            {
                Ok(new_etag) => {
                    folder.have.insert(server_id, (href, new_etag));
                    Ok(None)
                }
                Err(DavError::PreconditionFailed) => {
                    Ok(Some(failed(sync::CHANGE, &server_id, status::CONFLICT)))
                }
                Err(err) => Err(Error::Internal(err.to_string())),
            }
        }
        sync::DELETE => {
            let Some((href, etag)) = folder.have.remove(&server_id) else {
                return Ok(Some(failed(sync::DELETE, &server_id, status::NOT_FOUND)));
            };
            let url = Url::parse(&href).map_err(|err| Error::Internal(err.to_string()))?;
            match client.delete(&url, Some(&etag)).await {
                Ok(()) | Err(DavError::NotFound) => Ok(None),
                // Changed in the cloud meanwhile: it comes back down.
                Err(DavError::PreconditionFailed) => {
                    folder.pending.push(href);
                    Ok(None)
                }
                Err(err) => Err(Error::Internal(err.to_string())),
            }
        }
        sync::FETCH => {
            let Some((href, _)) = folder.have.get(&server_id).cloned() else {
                return Ok(Some(failed(sync::FETCH, &server_id, status::NOT_FOUND)));
            };
            let url = Url::parse(&href).map_err(|err| Error::Internal(err.to_string()))?;
            let resource = match client.get(&url).await {
                Ok(resource) => resource,
                Err(_) => return Ok(Some(failed(sync::FETCH, &server_id, status::NOT_FOUND))),
            };
            Ok(Some(match from_text(class, &resource.data, conv) {
                Ok(data) => Element::new(sync::FETCH)
                    .with(Element::text(sync::SERVER_ID, server_id))
                    .with(Element::text(sync::STATUS, status::OK))
                    .with(data),
                Err(_) => failed(sync::FETCH, &server_id, status::CONVERSION),
            }))
        }
        _ => Ok(None),
    }
}

fn failed(tag: Tag, id: &str, code: &str) -> Element {
    Element::new(tag)
        .with(Element::text(sync::SERVER_ID, id))
        .with(Element::text(sync::STATUS, code))
}

/// Whether the cloud collection changed since the folder's token (one
/// small PROPFIND), for Ping and GetItemEstimate.
pub async fn has_changes(client: &DavClient, href: &str, folder: &DavFolder) -> bool {
    if !folder.pending.is_empty() {
        return true;
    }
    let Ok(url) = Url::parse(href) else {
        return false;
    };
    match client.change_markers(&url).await {
        Ok(markers) => markers
            .sync_token
            .as_deref()
            .is_some_and(|token| token != folder.token),
        Err(_) => false,
    }
}

/// How many changes a Sync from `folder` would bring (GetItemEstimate),
/// counted on a copy.
pub async fn estimate(client: &DavClient, href: &str, folder: &DavFolder) -> Option<usize> {
    let url = Url::parse(href).ok()?;
    let mut copy = folder.clone();
    let changes = changes(client, &url, &mut copy).await.ok()?;
    Some(changes.deleted.len() + changes.changed.len() + copy.pending.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_ids_are_short_and_stable() {
        let a = folder_id(
            Class::Calendar,
            "https://c.example/remote.php/dav/calendars/ann/personal/",
        );
        assert_eq!(a.len(), 13);
        assert!(a.starts_with('c') && is_dav_folder(&a));
        assert_eq!(
            a,
            folder_id(
                Class::Calendar,
                "https://c.example/remote.php/dav/calendars/ann/personal/"
            )
        );
        assert_ne!(
            a,
            folder_id(
                Class::Tasks,
                "https://c.example/remote.php/dav/calendars/ann/personal/"
            )
        );
        assert!(!is_dav_folder("mabc"));
    }

    #[test]
    fn components_filter_shared_calendars() {
        assert!(holds(Class::Tasks, "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\n"));
        assert!(!holds(
            Class::Calendar,
            "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\n"
        ));
    }
}
