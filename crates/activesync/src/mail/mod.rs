/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Mail folders in Sync, GetItemEstimate and Ping: what changed in a
//! mailbox since the device's sync key, and the device's own changes.
//!
//! The device's view of a folder is a [`MailState`]: the JMAP `Email` state
//! it is up to date with, the messages it holds (with their read, flagged,
//! answered and forwarded bits), and the messages still to send it, newest
//! first. Changes come from `Email/changes` since that state, so a sync of
//! an unchanged account costs one call and a busy one only reads the
//! messages that changed; the whole folder is listed only on the first sync
//! and when the server can't say what changed (the state is too old).

pub mod item;

use crate::{
    Ctx,
    error::{Error, Result},
    jmap::Call,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};

/// The bits of a message's keywords a device shows.
pub mod flags {
    pub const SEEN: u8 = 1;
    pub const FLAGGED: u8 = 2;
    pub const ANSWERED: u8 = 4;
    pub const FORWARDED: u8 = 8;
}

/// The keywords of a JMAP `keywords` object as [`flags`] bits.
pub fn flags_of(keywords: &Value) -> u8 {
    let has = |name: &str| keywords.get(name).and_then(Value::as_bool) == Some(true);
    (if has("$seen") { flags::SEEN } else { 0 })
        | (if has("$flagged") { flags::FLAGGED } else { 0 })
        | (if has("$answered") { flags::ANSWERED } else { 0 })
        | (if has("$forwarded") {
            flags::FORWARDED
        } else {
            0
        })
}

/// A mail folder as one device has it.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct MailState {
    /// The JMAP `Email` state the device is up to date with; empty until
    /// the first sync after key 0 lists the folder.
    pub state: String,
    /// The oldest receipt time the device wants (its filter), as a UTC
    /// date; `None` for all mail.
    pub since: Option<String>,
    /// Messages on the device and their [`flags`] bits.
    pub have: BTreeMap<String, u8>,
    /// Messages still to add, newest first.
    pub pending: Vec<String>,
}

/// What a folder needs to send the device.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub deleted: Vec<String>,
    /// Messages on the device whose bits changed, with the new bits.
    pub changed: Vec<(String, u8)>,
}

/// How many changes `Email/changes` is asked for at a time.
const CHANGES_PAGE: usize = 1024;
/// The most messages a folder lists for a device at once (the newest);
/// older ones are left out, as Exchange leaves out mail outside the filter.
const MAX_LISTED: usize = 50_000;

impl Ctx<'_> {
    /// Brings `state` up to date with the mailbox: messages that arrived go
    /// to `pending`, and the deletions and flag changes the device must hear
    /// about are returned. `state` is changed in place; nothing is saved.
    pub async fn mail_changes(&self, mailbox: &str, state: &mut MailState) -> Result<Changes> {
        if state.state.is_empty() {
            return self.mail_full_scan(mailbox, state).await;
        }
        // Everything that changed in the account since the device's state.
        let mut touched: HashSet<String> = HashSet::new();
        let mut destroyed: HashSet<String> = HashSet::new();
        loop {
            let answers = self
                .jmap(vec![Call::new(
                    "Email/changes",
                    "c",
                    json!({ "sinceState": state.state, "maxChanges": CHANGES_PAGE }),
                )])
                .await?;
            let changes = match answers.get("c") {
                Ok(changes) => changes,
                // The state is too old to say: list the folder again.
                Err(Error::Jmap(reason)) if reason.starts_with("cannotCalculateChanges") => {
                    return self.mail_full_scan(mailbox, state).await;
                }
                Err(err) => return Err(err),
            };
            for id in ids(&changes["created"]).chain(ids(&changes["updated"])) {
                touched.insert(id.to_string());
            }
            for id in ids(&changes["destroyed"]) {
                touched.remove(id);
                destroyed.insert(id.to_string());
            }
            let new_state = changes["newState"].as_str().unwrap_or_default().to_string();
            let more = changes["hasMoreChanges"].as_bool() == Some(true);
            if new_state.is_empty() || new_state == state.state {
                break;
            }
            state.state = new_state;
            if !more {
                break;
            }
        }

        let mut result = Changes::default();
        for id in &destroyed {
            if state.have.remove(id).is_some() {
                result.deleted.push(id.clone());
            }
        }
        state.pending.retain(|id| !destroyed.contains(id));
        if touched.is_empty() {
            return Ok(result);
        }

        // Where the touched messages are now, and their bits.
        let touched: Vec<String> = touched.into_iter().collect();
        let mut arrived: Vec<(String, String)> = Vec::new();
        let mut seen = HashSet::with_capacity(touched.len());
        for chunk in touched.chunks(self.server.core.jmap.get_max_objects.max(1)) {
            let mut answers = self
                .jmap(vec![Call::new(
                    "Email/get",
                    "g",
                    json!({ "ids": chunk, "properties": ["mailboxIds", "keywords", "receivedAt"] }),
                )])
                .await?;
            let got = answers.take("g")?;
            for email in got["list"].as_array().into_iter().flatten() {
                let Some(id) = email["id"].as_str() else {
                    continue;
                };
                seen.insert(id.to_string());
                let here = email["mailboxIds"].get(mailbox).and_then(Value::as_bool) == Some(true);
                let received = email["receivedAt"].as_str().unwrap_or_default();
                let in_window = state.since.as_deref().is_none_or(|since| received >= since);
                let bits = flags_of(&email["keywords"]);
                match (here && in_window, state.have.get(id).copied()) {
                    (true, Some(old)) if old != bits => {
                        state.have.insert(id.to_string(), bits);
                        result.changed.push((id.to_string(), bits));
                    }
                    (true, Some(_)) => (),
                    (true, None) => {
                        if !state.pending.iter().any(|pending| pending == id) {
                            arrived.push((received.to_string(), id.to_string()));
                        }
                    }
                    (false, Some(_)) => {
                        state.have.remove(id);
                        result.deleted.push(id.to_string());
                    }
                    (false, None) => state.pending.retain(|pending| pending != id),
                }
            }
        }
        // Gone between the two calls: as destroyed.
        for id in touched.iter().filter(|id| !seen.contains(*id)) {
            if state.have.remove(id).is_some() {
                result.deleted.push(id.clone());
            }
            state.pending.retain(|pending| pending != id);
        }
        // New mail goes first, newest first.
        arrived.sort_by(|a, b| b.0.cmp(&a.0));
        let mut pending: Vec<String> = arrived.into_iter().map(|(_, id)| id).collect();
        pending.append(&mut state.pending);
        state.pending = pending;
        Ok(result)
    }

    /// Lists the folder and compares it with what the device holds: the
    /// first sync, or after the server lost track of the changes.
    async fn mail_full_scan(&self, mailbox: &str, state: &mut MailState) -> Result<Changes> {
        let mut filter = json!({ "inMailbox": mailbox });
        if let Some(since) = &state.since {
            filter["after"] = json!(since);
        }
        let mut listed: Vec<String> = Vec::new();
        let mut new_state = String::new();
        let page = self.server.core.jmap.query_max_results.clamp(1, 5000);
        while listed.len() < MAX_LISTED {
            let mut answers = self
                .jmap(vec![
                    Call::new(
                        "Email/query",
                        "q",
                        json!({
                            "filter": filter,
                            "sort": [{ "property": "receivedAt", "isAscending": false }],
                            "position": listed.len(),
                            "limit": page,
                        }),
                    ),
                    // The state the listing is consistent with.
                    Call::new("Email/get", "s", json!({ "ids": [], "properties": ["id"] })),
                ])
                .await?;
            if new_state.is_empty() {
                new_state = answers.get("s")?["state"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
            }
            let query = answers.take("q")?;
            let before = listed.len();
            listed.extend(ids(&query["ids"]).map(str::to_string));
            if listed.len() - before < page {
                break;
            }
        }
        listed.truncate(MAX_LISTED);

        let now: HashSet<&str> = listed.iter().map(String::as_str).collect();
        let mut result = Changes::default();
        let gone: Vec<String> = state
            .have
            .keys()
            .filter(|id| !now.contains(id.as_str()))
            .cloned()
            .collect();
        for id in gone {
            state.have.remove(&id);
            result.deleted.push(id);
        }
        // The bits of what the device holds, to catch changes missed.
        let held: Vec<String> = state.have.keys().cloned().collect();
        for chunk in held.chunks(self.server.core.jmap.get_max_objects.max(1)) {
            let mut answers = self
                .jmap(vec![Call::new(
                    "Email/get",
                    "g",
                    json!({ "ids": chunk, "properties": ["keywords"] }),
                )])
                .await?;
            let got = answers.take("g")?;
            for email in got["list"].as_array().into_iter().flatten() {
                if let Some(id) = email["id"].as_str() {
                    let bits = flags_of(&email["keywords"]);
                    if state.have.get(id) != Some(&bits) {
                        state.have.insert(id.to_string(), bits);
                        result.changed.push((id.to_string(), bits));
                    }
                }
            }
        }
        state.pending = listed
            .into_iter()
            .filter(|id| !state.have.contains_key(id))
            .collect();
        state.state = new_state;
        Ok(result)
    }
}

/// The ids in a JMAP id list.
fn ids(list: &Value) -> impl Iterator<Item = &str> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
}

/// The oldest receipt time for a Sync `FilterType` ([MS-ASCMD]
/// 2.2.3.68.2): 1 a day, 2 three days, 3 a week, 4 two weeks, 5 a month;
/// 0 (or anything else) all mail.
pub fn filter_since(filter_type: u64, now: u64) -> Option<String> {
    let days = match filter_type {
        1 => 1,
        2 => 3,
        3 => 7,
        4 => 14,
        5 => 30,
        _ => return None,
    };
    Some(utc_date(now.saturating_sub(days * 86_400)))
}

/// A Unix time as a JMAP UTC date (`2026-11-23T10:00:00Z`).
pub fn utc_date(unix: u64) -> String {
    let days = (unix / 86_400) as i64;
    let secs = unix % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        (secs / 60) % 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(utc_date(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_date(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(utc_date(1_795_464_000), "2026-11-23T20:00:00Z");
        assert_eq!(
            filter_since(3, 1_795_464_000).as_deref(),
            Some("2026-11-16T20:00:00Z")
        );
        assert_eq!(filter_since(0, 1_795_464_000), None);
        assert_eq!(filter_since(8, 1_795_464_000), None);
    }

    #[test]
    fn keyword_bits() {
        assert_eq!(flags_of(&json!({})), 0);
        assert_eq!(
            flags_of(&json!({ "$seen": true, "$flagged": true, "$answered": true, "other": true })),
            flags::SEEN | flags::FLAGGED | flags::ANSWERED
        );
        assert_eq!(flags_of(&json!({ "$forwarded": true })), flags::FORWARDED);
    }
}
