//! Changes since a sync token: the `sync-collection` report (RFC 6578).
//!
//! A server may answer a large change set in pages: it returns part of the
//! changes, a new token, and a 507 response for the collection itself
//! (section 3.6). The client repeats the report with each new token until a
//! page comes back complete, and merges the pages, so the caller always sees
//! one consistent change set and one token.

use std::collections::HashMap;

use reqwest::{StatusCode, Url};

use crate::{
    DavClient, DavError,
    client::{DavMethod, Depth, check},
    error::protocol,
    href,
    xml::{
        dom::Ns,
        multistatus::{Multistatus, Response, error_names, parse_multistatus},
        request,
    },
};

/// A member that is new or changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// The member's URL (canonical, see [`href`]).
    pub href: Url,
    /// Its ETag, when the server sent one.
    pub etag: Option<String>,
}

/// What changed in a collection since a token.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyncChanges {
    /// Members added or changed (on an initial sync, every member).
    pub changed: Vec<Member>,
    /// Members removed.
    pub removed: Vec<Url>,
    /// The token to pass next time.
    pub token: String,
}

/// One page's worth of changes, before merging.
pub(crate) struct Page {
    pub changed: Vec<Member>,
    pub removed: Vec<Url>,
    pub token: String,
    /// The server stopped early (RFC 6578 section 3.6); ask again with
    /// `token`.
    pub truncated: bool,
}

impl DavClient {
    /// The changes in `collection` since `token`; an empty `token` asks for
    /// every member (the initial sync). A token the server no longer accepts
    /// is [`DavError::InvalidSyncToken`]: start again with an empty one.
    pub async fn sync_collection(
        &self,
        collection: &Url,
        token: &str,
    ) -> Result<SyncChanges, DavError> {
        let first = self.sync_page(collection, token).await?;
        if !first.truncated {
            return Ok(SyncChanges {
                changed: first.changed,
                removed: first.removed,
                token: first.token,
            });
        }
        // Paged: merge by href, the later page winning, so a member changed
        // on one page and removed on the next ends up removed.
        let mut merged: HashMap<Url, Change> = HashMap::new();
        let mut page = first;
        for _ in 1..self.limits().max_sync_pages {
            merge(&mut merged, page.changed, page.removed);
            page = self.sync_page(collection, &page.token).await?;
            if !page.truncated {
                merge(&mut merged, page.changed, page.removed);
                return Ok(split(merged, page.token));
            }
        }
        Err(protocol(format!(
            "sync-collection still truncated after {} pages",
            self.limits().max_sync_pages
        )))
    }

    /// One sync-collection request.
    async fn sync_page(&self, collection: &Url, token: &str) -> Result<Page, DavError> {
        let reply = self
            .send(self.xml_request(
                DavMethod::Report,
                collection,
                Depth::Zero,
                request::sync_collection(token),
            ))
            .await?;
        // RFC 6578 section 3.2: an unknown token fails the valid-sync-token
        // precondition with 403 (409 on some servers).
        if matches!(reply.status, StatusCode::FORBIDDEN | StatusCode::CONFLICT)
            && error_names(
                std::str::from_utf8(&reply.body).unwrap_or_default(),
                Ns::Dav,
                "valid-sync-token",
            )
        {
            return Err(DavError::InvalidSyncToken);
        }
        check(&reply)?;
        let url = reply.url.clone();
        page(&url, parse_multistatus(&reply.text()?)?)
    }
}

/// Reads one page of a sync report.
pub(crate) fn page(base: &Url, multistatus: Multistatus) -> Result<Page, DavError> {
    let token = multistatus
        .sync_token
        .ok_or_else(|| protocol("a sync-collection answer without a sync-token"))?;
    let mut page = Page {
        changed: Vec::with_capacity(multistatus.responses.len()),
        removed: Vec::new(),
        token,
        truncated: false,
    };
    for response in multistatus.responses {
        read_response(base, response, &mut page)?;
    }
    Ok(page)
}

/// Sorts one response into changed, removed, or the truncation marker.
fn read_response(base: &Url, response: Response, page: &mut Page) -> Result<(), DavError> {
    if response.status == Some(507)
        || response
            .errors
            .iter()
            .any(|condition| condition == "number-of-matches-within-limits")
    {
        page.truncated = true;
        return Ok(());
    }
    match response.status {
        // RFC 6578 section 3.5.2: a removed member is a response with a 404
        // status and no properties.
        Some(404) => {
            for removed in &response.hrefs {
                page.removed.push(href::resolve(base, removed)?);
            }
        }
        Some(status) if !(200..300).contains(&status) => {
            return Err(protocol(format!(
                "sync-collection member with status {status}"
            )));
        }
        _ => {
            for changed in &response.hrefs {
                let href = href::resolve(base, changed)?;
                // Some servers list the collection itself; it isn't a member.
                if !href::same_collection(&href, base) {
                    page.changed.push(Member {
                        href,
                        etag: response.props.etag.clone(),
                    });
                }
            }
        }
    }
    Ok(())
}

/// A member's latest state across pages.
enum Change {
    Changed(Option<String>),
    Removed,
}

/// Folds one page into the running result.
fn merge(merged: &mut HashMap<Url, Change>, changed: Vec<Member>, removed: Vec<Url>) {
    for member in changed {
        merged.insert(member.href, Change::Changed(member.etag));
    }
    for href in removed {
        merged.insert(href, Change::Removed);
    }
}

/// The merged pages as one change set.
fn split(merged: HashMap<Url, Change>, token: String) -> SyncChanges {
    let mut changes = SyncChanges {
        token,
        ..SyncChanges::default()
    };
    for (href, change) in merged {
        match change {
            Change::Changed(etag) => changes.changed.push(Member { href, etag }),
            Change::Removed => changes.removed.push(href),
        }
    }
    changes
}
