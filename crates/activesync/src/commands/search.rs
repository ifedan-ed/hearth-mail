/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Search ([MS-ASCMD] 2.2.1.16) in the mailbox and in the address list,
//! Find (2.2.1.2, 16.1) in the mailbox, and ResolveRecipients (2.2.1.15).
//!
//! Mail search is JMAP `Email/query` with a `text` filter, in one folder or
//! everywhere; the address list is the directory, read through JMAP
//! `Principal/query` with the device's own permissions, so a person finds
//! whom the server lets them see and no one else.

use crate::{
    Ctx, Reply,
    error::Result,
    folders,
    jmap::Call,
    mail::item::{BodyOptions, BodyPreference, body_type},
};
use activesync_proto::wbxml::{
    Element,
    codepages::{
        airsync, email, email2, find, gal, itemoperations, resolverecipients as rr, search as s,
    },
};
use serde_json::{Value, json};

/// Search status codes ([MS-ASCMD] 2.2.3.177.16).
mod status {
    pub const OK: &str = "1";
    pub const PROTOCOL: &str = "2";
}

/// The most results one answer holds, whatever range is asked.
const MAX_RESULTS: usize = 100;

/// A `Range` of `first-last`, inclusive, capped to [`MAX_RESULTS`].
fn range(text: Option<&str>) -> (usize, usize) {
    let (first, last) = text
        .and_then(|text| text.trim().split_once('-'))
        .and_then(|(a, b)| Some((a.trim().parse().ok()?, b.trim().parse().ok()?)))
        .filter(|(a, b): &(usize, usize)| a <= b)
        .unwrap_or((0, 9));
    (first, last.min(first + MAX_RESULTS - 1))
}

pub async fn search(ctx: &Ctx<'_>, request: Element) -> Result<Reply> {
    let Some(store) = request.child(s::STORE) else {
        return Ok(top(s::SEARCH, s::STATUS, status::PROTOCOL));
    };
    let name = store.child_str(s::NAME).unwrap_or_default();
    let options = store.child(s::OPTIONS);
    let (first, last) = range(options.and_then(|o| o.child_str(s::RANGE)));
    let store_answer = if name.eq_ignore_ascii_case("Mailbox") {
        let query = store.child(s::QUERY);
        let text = query.and_then(free_text);
        let folder = query.and_then(collection_of);
        let body = options.map(BodyOptions::parse).unwrap_or_default();
        let (ids, total) = mail_query(ctx, text.as_deref(), folder.as_deref(), first, last).await?;
        let mut answer = Element::new(s::STORE).with(Element::text(s::STATUS, status::OK));
        for (id, mailbox, _bits, data) in render(ctx, &ids, &body).await? {
            let mut properties = Element::new(s::PROPERTIES);
            for child in data.children() {
                properties.push(child.clone());
            }
            answer.push(
                Element::new(s::RESULT)
                    .with(Element::text(airsync::CLASS, "Email"))
                    .with(Element::text(s::LONG_ID, id))
                    .with_opt(mailbox.map(|m| {
                        Element::text(airsync::COLLECTION_ID, folders::mail_folder_id(&m))
                    }))
                    .with(properties),
            );
        }
        finish(answer, s::RANGE, s::TOTAL, first, ids.len(), total)
    } else if name.eq_ignore_ascii_case("GAL") {
        let text = store.child_str(s::QUERY).unwrap_or_default();
        let people = directory(ctx, text, first + MAX_RESULTS).await?;
        let total = people.len();
        let mut answer = Element::new(s::STORE).with(Element::text(s::STATUS, status::OK));
        let shown: Vec<_> = people
            .into_iter()
            .skip(first)
            .take(last - first + 1)
            .collect();
        for person in &shown {
            answer.push(
                Element::new(s::RESULT).with(
                    Element::new(s::PROPERTIES)
                        .with(Element::text(gal::DISPLAY_NAME, person.name.as_str()))
                        .with(Element::text(gal::EMAIL_ADDRESS, person.email.as_str())),
                ),
            );
        }
        finish(answer, s::RANGE, s::TOTAL, first, shown.len(), total)
    } else {
        Element::new(s::STORE).with(Element::text(s::STATUS, status::PROTOCOL))
    };
    Ok(Reply::Wbxml(
        Element::new(s::SEARCH)
            .with(Element::text(s::STATUS, status::OK))
            .with(Element::new(s::RESPONSE).with(store_answer)),
    ))
}

/// Find (16.1): mail search with a lighter result (no body).
pub async fn find(ctx: &Ctx<'_>, request: Element) -> Result<Reply> {
    let Some(criterion) = request
        .child(find::EXECUTE_SEARCH)
        .and_then(|search| search.child(find::MAIL_BOX_SEARCH_CRITERION))
    else {
        // The address-list criterion: answered from the directory too.
        if let Some(criterion) = request
            .child(find::EXECUTE_SEARCH)
            .and_then(|search| search.child(find::GAL_SEARCH_CRITERION))
        {
            return find_people(ctx, criterion).await;
        }
        return Ok(top(find::FIND, find::STATUS, status::PROTOCOL));
    };
    let query = criterion.child(find::QUERY);
    let text = query
        .and_then(|q| q.child_str(find::FREE_TEXT))
        .map(clean_free_text);
    let folder = query.and_then(|q| q.child_str(airsync::COLLECTION_ID));
    let (first, last) = range(
        criterion
            .child(find::OPTIONS)
            .and_then(|o| o.child_str(find::RANGE)),
    );
    let (ids, total) = mail_query(ctx, text.as_deref(), folder, first, last).await?;
    let mut answer = Element::new(find::RESPONSE)
        .with(Element::text(itemoperations::STORE, "Mailbox"))
        .with(Element::text(find::STATUS, status::OK));
    // The preview is the start of the text: read no more than that.
    let preview = BodyOptions {
        preferences: vec![BodyPreference {
            kind: body_type::PLAIN,
            truncation: Some(255),
            all_or_none: false,
            preview: None,
        }],
        mime_support: 0,
    };
    for (id, mailbox, bits, data) in render(ctx, &ids, &preview).await? {
        let pick = |tag| data.child(tag).cloned();
        let properties = Element::new(find::PROPERTIES)
            .with_opt(pick(email::SUBJECT))
            .with_opt(pick(email::DATE_RECEIVED))
            .with_opt(pick(email::DISPLAY_TO))
            .with_opt(pick(email::IMPORTANCE))
            .with(Element::text(
                email::READ,
                if bits & 1 != 0 { "1" } else { "0" },
            ))
            .with(Element::text(email2::IS_DRAFT, "0"))
            .with_opt(
                data.child(activesync_proto::wbxml::codepages::airsyncbase::BODY)
                    .and_then(|body| {
                        body.child_str(activesync_proto::wbxml::codepages::airsyncbase::DATA)
                    })
                    .map(|text| {
                        Element::text(find::PREVIEW, text.chars().take(255).collect::<String>())
                    }),
            )
            .with(Element::text(
                find::HAS_ATTACHMENTS,
                if data
                    .child(activesync_proto::wbxml::codepages::airsyncbase::ATTACHMENTS)
                    .is_some()
                {
                    "1"
                } else {
                    "0"
                },
            ))
            .with_opt(pick(email::FROM));
        answer.push(
            Element::new(find::RESULT)
                .with(Element::text(airsync::CLASS, "Email"))
                .with(Element::text(airsync::SERVER_ID, id))
                .with_opt(
                    mailbox.map(|m| {
                        Element::text(airsync::COLLECTION_ID, folders::mail_folder_id(&m))
                    }),
                )
                .with(properties),
        );
    }
    let answer = finish(answer, find::RANGE, find::TOTAL, first, ids.len(), total);
    Ok(Reply::Wbxml(
        Element::new(find::FIND)
            .with(Element::text(find::STATUS, status::OK))
            .with(answer),
    ))
}

async fn find_people(ctx: &Ctx<'_>, criterion: &Element) -> Result<Reply> {
    let text = criterion
        .child(find::QUERY)
        .and_then(|q| q.child_str(find::FREE_TEXT))
        .unwrap_or_default();
    let (first, last) = range(
        criterion
            .child(find::OPTIONS)
            .and_then(|o| o.child_str(find::RANGE)),
    );
    let people = directory(ctx, text, first + MAX_RESULTS).await?;
    let total = people.len();
    let mut answer = Element::new(find::RESPONSE)
        .with(Element::text(itemoperations::STORE, "GAL"))
        .with(Element::text(find::STATUS, status::OK));
    let shown: Vec<_> = people
        .into_iter()
        .skip(first)
        .take(last - first + 1)
        .collect();
    for person in &shown {
        answer.push(
            Element::new(find::RESULT).with(
                Element::new(find::PROPERTIES)
                    .with(Element::text(gal::DISPLAY_NAME, person.name.as_str()))
                    .with(Element::text(gal::EMAIL_ADDRESS, person.email.as_str())),
            ),
        );
    }
    let answer = finish(answer, find::RANGE, find::TOTAL, first, shown.len(), total);
    Ok(Reply::Wbxml(
        Element::new(find::FIND)
            .with(Element::text(find::STATUS, status::OK))
            .with(answer),
    ))
}

/// ResolveRecipients status codes ([MS-ASCMD] 2.2.3.177.14).
mod resolve_status {
    pub const OK: &str = "1";
    pub const AMBIGUOUS: &str = "2";
    pub const NONE: &str = "4";
}

/// ResolveRecipients: each name or address the device typed, matched
/// against the directory. Certificates and free/busy aren't offered.
pub async fn resolve_recipients(ctx: &Ctx<'_>, request: Element) -> Result<Reply> {
    let max = request
        .child(rr::OPTIONS)
        .and_then(|o| o.child_u64(rr::MAX_AMBIGUOUS_RECIPIENTS))
        .map_or(20, |max| (max as usize).clamp(1, MAX_RESULTS));
    let mut answer = Element::new(rr::RESOLVE_RECIPIENTS).with(Element::text(rr::STATUS, "1"));
    for to in request.children_of(rr::TO) {
        let text = to.as_str().unwrap_or_default().trim();
        let people = directory(ctx, text, max).await?;
        let exact = people
            .iter()
            .filter(|person| person.email.eq_ignore_ascii_case(text))
            .count();
        let code = match (people.len(), exact) {
            (0, _) => resolve_status::NONE,
            (1, _) | (_, 1) => resolve_status::OK,
            _ => resolve_status::AMBIGUOUS,
        };
        let mut response = Element::new(rr::RESPONSE)
            .with(Element::text(rr::TO, text))
            .with(Element::text(rr::STATUS, code))
            .with(Element::text(rr::RECIPIENT_COUNT, people.len().to_string()));
        for person in people {
            response.push(
                Element::new(rr::RECIPIENT)
                    .with(Element::text(rr::TYPE, "1"))
                    .with(Element::text(rr::DISPLAY_NAME, person.name))
                    .with(Element::text(rr::EMAIL_ADDRESS, person.email)),
            );
        }
        answer.push(response);
    }
    Ok(Reply::Wbxml(answer))
}

/// A top-level status only.
fn top(
    root: activesync_proto::wbxml::Tag,
    status_tag: activesync_proto::wbxml::Tag,
    code: &str,
) -> Reply {
    Reply::Wbxml(Element::new(root).with(Element::text(status_tag, code)))
}

/// Adds `Range` and `Total` (none when nothing was found).
fn finish(
    mut answer: Element,
    range_tag: activesync_proto::wbxml::Tag,
    total_tag: activesync_proto::wbxml::Tag,
    first: usize,
    shown: usize,
    total: usize,
) -> Element {
    if shown > 0 {
        answer.push(Element::text(
            range_tag,
            format!("{first}-{}", first + shown - 1),
        ));
    }
    answer.push(Element::text(total_tag, total.to_string()));
    answer
}

/// The `FreeText` of a Search query, inside `And` or not.
fn free_text(query: &Element) -> Option<String> {
    query
        .child_str(s::FREE_TEXT)
        .or_else(|| {
            query
                .child(s::AND)
                .and_then(|and| and.child_str(s::FREE_TEXT))
        })
        .map(clean_free_text)
}

/// The folder of a Search query, inside `And` or not.
fn collection_of(query: &Element) -> Option<String> {
    query
        .child_str(airsync::COLLECTION_ID)
        .or_else(|| {
            query
                .child(s::AND)
                .and_then(|and| and.child_str(airsync::COLLECTION_ID))
        })
        .map(str::to_string)
}

/// Free text without KQL property prefixes (`subject: mobile` is searched
/// as `mobile`; the text filter covers subject, addresses and body).
fn clean_free_text(text: &str) -> String {
    text.split_whitespace()
        .filter(|word| !word.ends_with(':'))
        .map(|word| word.split_once(':').map_or(word, |(_, rest)| rest))
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Ids of matching mail, newest first, from `first` to `last`, and the
/// total count.
async fn mail_query(
    ctx: &Ctx<'_>,
    text: Option<&str>,
    folder: Option<&str>,
    first: usize,
    last: usize,
) -> Result<(Vec<String>, usize)> {
    let mut filter = serde_json::Map::new();
    if let Some(text) = text.filter(|text| !text.is_empty()) {
        filter.insert("text".into(), json!(text));
    }
    if let Some(mailbox) = folder.and_then(folders::mailbox_of) {
        filter.insert("inMailbox".into(), json!(mailbox));
    }
    let answers = ctx
        .jmap(vec![Call::new(
            "Email/query",
            "q",
            json!({
                "filter": filter,
                "sort": [{ "property": "receivedAt", "isAscending": false }],
                "position": first,
                "limit": last - first + 1,
                "calculateTotal": true,
            }),
        )])
        .await?;
    let query = answers.get("q")?;
    let ids = query["ids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    Ok((ids, query["total"].as_u64().unwrap_or(0) as usize))
}

/// Rendered results with the mailbox each is in.
async fn render(
    ctx: &Ctx<'_>,
    ids: &[String],
    body: &BodyOptions,
) -> Result<Vec<(String, Option<String>, u8, Element)>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let answers = ctx
        .jmap(vec![Call::new(
            "Email/get",
            "m",
            json!({ "ids": ids, "properties": ["mailboxIds"] }),
        )])
        .await?;
    let places = answers.get("m")?["list"].clone();
    let mailbox_of = |id: &str| {
        places.as_array().and_then(|list| {
            list.iter()
                .find(|email| email["id"].as_str() == Some(id))
                .and_then(|email| email["mailboxIds"].as_object())
                .and_then(|mailboxes| mailboxes.keys().next().cloned())
        })
    };
    Ok(ctx
        .render_emails(ids, body)
        .await?
        .into_iter()
        .map(|(id, bits, data)| {
            let mailbox = mailbox_of(&id);
            (id, mailbox, bits, data)
        })
        .collect())
}

/// One person of the directory.
struct Person {
    name: String,
    email: String,
}

/// People of the directory matching `text`, at most `limit`. A server that
/// doesn't let this account list people answers with nobody.
async fn directory(ctx: &Ctx<'_>, text: &str, limit: usize) -> Result<Vec<Person>> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let answers = ctx
        .jmap_with(
            &[
                "urn:ietf:params:jmap:core",
                "urn:ietf:params:jmap:principals",
            ],
            vec![
                Call::new(
                    "Principal/query",
                    "q",
                    json!({ "filter": { "text": text }, "limit": limit }),
                ),
                Call::new(
                    "Principal/get",
                    "g",
                    json!({
                        "#ids": { "resultOf": "q", "name": "Principal/query", "path": "/ids" },
                        "properties": ["name", "email", "type"],
                    }),
                ),
            ],
        )
        .await?;
    let Ok(people) = answers.get("g") else {
        return Ok(Vec::new());
    };
    Ok(people["list"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| {
            p["type"]
                .as_str()
                .is_none_or(|t| t == "individual" || t == "group")
        })
        .filter_map(|p| {
            let email = p["email"].as_str()?.to_string();
            Some(Person {
                name: p["name"].as_str().unwrap_or(&email).to_string(),
                email,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_and_text() {
        assert_eq!(range(Some("0-4")), (0, 4));
        assert_eq!(range(Some("10-1000")), (10, 109));
        assert_eq!(range(Some("9-1")), (0, 9));
        assert_eq!(range(None), (0, 9));
        assert_eq!(clean_free_text("subject: mobile"), "mobile");
        assert_eq!(clean_free_text("from:ann budget"), "ann budget");
        assert_eq!(clean_free_text("Presentation"), "Presentation");
    }
}
