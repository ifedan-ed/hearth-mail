/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Settings ([MS-ASCMD] 2.2.1.18): out of office (JMAP
//! `VacationResponse`, the same reply the webmail and Sieve show), the
//! device's description (kept with the device), and the account's
//! addresses. A device password can't be recovered here: it is accepted
//! and not stored.

use crate::{Ctx, Reply, error::Result, jmap::Call, mail::item::eas_date};
use activesync_proto::wbxml::{Element, codepages::settings as st};
use serde_json::{Value, json};

/// Settings status codes ([MS-ASCMD] 2.2.3.177.15).
mod status {
    pub const OK: &str = "1";
    pub const PROTOCOL: &str = "2";
    pub const SERVER: &str = "6";
}

pub async fn settings(ctx: &Ctx<'_>, request: Element) -> Result<Reply> {
    let mut answer = Element::new(st::SETTINGS).with(Element::text(st::STATUS, status::OK));
    for part in request.children() {
        let element = match part.tag {
            st::OOF => oof(ctx, part).await?,
            st::DEVICE_INFORMATION => {
                if let Some(set) = part.child(st::SET) {
                    let mut device = ctx.device().await?;
                    device.info = device_info(set);
                    ctx.save_device(&device).await?;
                }
                Element::new(st::DEVICE_INFORMATION).with(Element::text(st::STATUS, status::OK))
            }
            st::USER_INFORMATION => user_information(ctx).await?,
            st::DEVICE_PASSWORD => {
                Element::new(st::DEVICE_PASSWORD).with(Element::text(st::STATUS, status::OK))
            }
            tag => Element::new(tag).with(Element::text(st::STATUS, status::PROTOCOL)),
        };
        answer.push(element);
    }
    Ok(Reply::Wbxml(answer))
}

/// The children of a DeviceInformation `Set`, as (tag name, value).
pub fn device_info(set: &Element) -> Vec<(String, String)> {
    set.children()
        .iter()
        .filter_map(|field| Some((field.tag.name().to_string(), field.as_str()?.to_string())))
        .collect()
}

async fn oof(ctx: &Ctx<'_>, request: &Element) -> Result<Element> {
    if let Some(set) = request.child(st::SET) {
        let state = set.child_u64(st::OOF_STATE).unwrap_or(0);
        // One message for everyone: the first enabled one the device sent.
        let message = set
            .children_of(st::OOF_MESSAGE)
            .find(|message| message.child_u64(st::ENABLED) == Some(1))
            .or_else(|| set.child(st::OOF_MESSAGE));
        let text = message
            .and_then(|m| m.child_str(st::REPLY_MESSAGE))
            .unwrap_or_default();
        let html = message
            .and_then(|m| m.child_str(st::BODY_TYPE))
            .is_some_and(|kind| kind.eq_ignore_ascii_case("HTML"));
        let mut update = json!({
            "isEnabled": state != 0,
            "fromDate": Value::Null,
            "toDate": Value::Null,
        });
        if state == 2 {
            update["fromDate"] = json!(jmap_date(set.child_str(st::START_TIME)));
            update["toDate"] = json!(jmap_date(set.child_str(st::END_TIME)));
        }
        if html {
            update["htmlBody"] = json!(text);
        } else {
            update["textBody"] = json!(text);
        }
        let answers = ctx
            .jmap(vec![Call::new(
                "VacationResponse/set",
                "v",
                json!({ "update": { "singleton": update } }),
            )])
            .await?;
        let ok = answers.get("v")?["updated"].get("singleton").is_some();
        return Ok(Element::new(st::OOF).with(Element::text(
            st::STATUS,
            if ok { status::OK } else { status::SERVER },
        )));
    }

    let html = request
        .child(st::GET)
        .and_then(|get| get.child_str(st::BODY_TYPE))
        .is_some_and(|kind| kind.eq_ignore_ascii_case("HTML"));
    let answers = ctx
        .jmap(vec![Call::new(
            "VacationResponse/get",
            "v",
            json!({ "ids": ["singleton"] }),
        )])
        .await?;
    let vacation = answers.get("v")?["list"]
        .as_array()
        .and_then(|list| list.first())
        .cloned()
        .unwrap_or_default();
    let enabled = vacation["isEnabled"].as_bool() == Some(true);
    let timed = vacation["fromDate"].is_string() || vacation["toDate"].is_string();
    let state = match (enabled, timed) {
        (false, _) => "0",
        (true, false) => "1",
        (true, true) => "2",
    };
    let text = if html {
        vacation["htmlBody"]
            .as_str()
            .or_else(|| vacation["textBody"].as_str())
    } else {
        vacation["textBody"].as_str()
    }
    .unwrap_or_default();
    let mut get = Element::new(st::GET).with(Element::text(st::OOF_STATE, state));
    if timed {
        if let Some(from) = vacation["fromDate"].as_str() {
            get.push(Element::text(st::START_TIME, eas_date(from)));
        }
        if let Some(to) = vacation["toDate"].as_str() {
            get.push(Element::text(st::END_TIME, eas_date(to)));
        }
    }
    // The same message for each audience: JMAP has one reply.
    for audience in [
        st::APPLIES_TO_INTERNAL,
        st::APPLIES_TO_EXTERNAL_KNOWN,
        st::APPLIES_TO_EXTERNAL_UNKNOWN,
    ] {
        get.push(
            Element::new(st::OOF_MESSAGE)
                .with(Element::new(audience))
                .with(Element::text(st::ENABLED, if enabled { "1" } else { "0" }))
                .with(Element::text(st::REPLY_MESSAGE, text))
                .with(Element::text(
                    st::BODY_TYPE,
                    if html { "HTML" } else { "Text" },
                )),
        );
    }
    Ok(Element::new(st::OOF)
        .with(Element::text(st::STATUS, status::OK))
        .with(get))
}

/// An ActiveSync date (`2026-11-23T10:00:00.000Z`) as JMAP writes one.
fn jmap_date(date: Option<&str>) -> Option<String> {
    let date = date?.trim();
    let base = date.strip_suffix('Z').unwrap_or(date);
    let base = base.split_once('.').map_or(base, |(base, _)| base);
    (base.len() == 19).then(|| format!("{base}Z"))
}

/// UserInformation: the account's addresses, from its identities (the
/// 14.1 `Accounts` form).
async fn user_information(ctx: &Ctx<'_>) -> Result<Element> {
    let answers = ctx
        .jmap(vec![Call::new("Identity/get", "i", json!({}))])
        .await?;
    let identities = answers.get("i")?["list"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut addresses = Element::new(st::EMAIL_ADDRESSES);
    let mut seen = Vec::new();
    for identity in &identities {
        if let Some(email) = identity["email"].as_str() {
            let email = email.to_lowercase();
            if !seen.contains(&email) {
                addresses.push(Element::text(st::SMTP_ADDRESS, email.as_str()));
                seen.push(email);
            }
        }
    }
    if let Some(primary) = seen.first() {
        addresses.push(Element::text(st::PRIMARY_SMTP_ADDRESS, primary.as_str()));
    }
    let name = identities
        .first()
        .and_then(|identity| identity["name"].as_str())
        .filter(|name| !name.is_empty())
        .or_else(|| seen.first().map(String::as_str))
        .unwrap_or_default()
        .to_string();
    Ok(Element::new(st::USER_INFORMATION)
        .with(Element::text(st::STATUS, status::OK))
        .with(
            Element::new(st::GET).with(
                Element::new(st::ACCOUNTS).with(
                    Element::new(st::ACCOUNT)
                        .with(Element::text(st::USER_DISPLAY_NAME, name))
                        .with(addresses),
                ),
            ),
        ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_both_ways() {
        assert_eq!(
            jmap_date(Some("2026-11-23T10:00:00.000Z")).as_deref(),
            Some("2026-11-23T10:00:00Z")
        );
        assert_eq!(jmap_date(Some("bad")), None);
        assert_eq!(eas_date("2026-11-23T10:00:00Z"), "2026-11-23T10:00:00.000Z");
    }
}
