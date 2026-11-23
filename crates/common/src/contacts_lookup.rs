/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Hearth Mail: "is this sender in the recipient's contacts?", asked of the
//! cloud that keeps the contacts (Nextcloud through Tidings now, Hearth
//! later), for the spam filter's address-book override.
//!
//! The contract (docs/integration.md, section 1b): `POST <url>` with
//! `Authorization: Bearer <secret>` and `{"account": <mailbox address>,
//! "sender": <address>}`, answered `{"known": true|false}`.
//!
//! Delivery never waits on it for long and never fails because of it: one
//! request per (account, sender) at most every [`CACHE_TTL`], a
//! [`TIMEOUT`] on each, and any failure (timeout, refused, wrong status,
//! bad body) counts as "not known", so the message is scored as it would
//! be without the lookup.

use std::time::Duration;
use utils::cache::CacheWithTtl;

/// How long one answer is reused, per account and sender.
pub const CACHE_TTL: Duration = Duration::from_secs(600);
/// The most a delivery waits for an answer.
pub const TIMEOUT: Duration = Duration::from_millis(300);
/// Room for about 16,000 cached answers.
const CACHE_WEIGHT: u64 = 2 * 1024 * 1024;

pub struct ContactsLookup {
    url: String,
    secret: String,
    client: reqwest::Client,
    cache: CacheWithTtl<String, u64>,
}

impl std::fmt::Debug for ContactsLookup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the secret
        f.debug_struct("ContactsLookup")
            .field("url", &self.url)
            .finish()
    }
}

impl ContactsLookup {
    /// A lookup against `url`, authenticated with `secret`.
    pub fn new(url: String, secret: String) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .connect_timeout(TIMEOUT)
            .user_agent(concat!(types::brand!(), " contacts lookup"))
            .build()
            .map_err(|err| format!("Failed to build the contacts lookup client: {err}"))?;
        Ok(Self {
            url,
            secret,
            client,
            cache: CacheWithTtl::new(CACHE_WEIGHT, 128),
        })
    }

    /// The endpoint this asks, for logs.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Whether `sender` is in the contacts of whoever owns `account`. False
    /// whenever the answer isn't a clear yes, so mail is never held up or
    /// trusted by mistake.
    pub async fn is_known(&self, account_id: u32, account: &str, sender: &str) -> bool {
        let key = format!("{account_id}\0{}", sender.to_lowercase());
        if let Some(known) = self.cache.get(&key) {
            return known == 1;
        }
        let known = match self.ask(account, sender).await {
            Ok(known) => known,
            Err(reason) => {
                trc::event!(
                    Spam(trc::SpamEvent::Classify),
                    Details = "Contacts lookup failed; scored without it",
                    Reason = reason,
                    Url = self.url.clone(),
                );
                false
            }
        };
        self.cache.insert(key, known as u64, CACHE_TTL);
        known
    }

    async fn ask(&self, account: &str, sender: &str) -> Result<bool, String> {
        #[derive(serde::Deserialize)]
        struct Answer {
            known: bool,
        }
        let response = self
            .client
            .post(&self.url)
            .bearer_auth(&self.secret)
            .json(&serde_json::json!({ "account": account, "sender": sender }))
            .send()
            .await
            .map_err(|err| err.to_string())?;
        if !response.status().is_success() {
            return Err(format!("status {}", response.status()));
        }
        response
            .json::<Answer>()
            .await
            .map(|answer| answer.known)
            .map_err(|err| err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    /// A one-route HTTP server answering every request with `body` (after
    /// `delay`), counting requests, and checking the bearer secret.
    async fn server(body: &'static str, delay: Duration) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/known", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let counter = counter.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buf[..n]).to_lowercase();
                    counter.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(delay).await;
                    let (status, body) = if request.contains("authorization: bearer s3cret") {
                        ("200 OK", body)
                    } else {
                        ("401 Unauthorized", "{}")
                    };
                    let reply = format!(
                        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(reply.as_bytes()).await;
                });
            }
        });
        (url, hits)
    }

    #[tokio::test]
    async fn known_unknown_and_cached() {
        let (url, hits) = server(r#"{"known": true}"#, Duration::ZERO).await;
        let lookup = ContactsLookup::new(url, "s3cret".into()).unwrap();
        assert!(
            lookup
                .is_known(1, "ann@example.com", "Bob@Example.org")
                .await
        );
        // Cached per account and sender, case-insensitively
        assert!(
            lookup
                .is_known(1, "ann@example.com", "bob@example.org")
                .await
        );
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        // Another account asks again
        assert!(
            lookup
                .is_known(2, "joe@example.com", "bob@example.org")
                .await
        );
        assert_eq!(hits.load(Ordering::SeqCst), 2);

        let (url, _) = server(r#"{"known": false}"#, Duration::ZERO).await;
        let lookup = ContactsLookup::new(url, "s3cret".into()).unwrap();
        assert!(
            !lookup
                .is_known(1, "ann@example.com", "eve@example.net")
                .await
        );
    }

    #[tokio::test]
    async fn every_failure_is_not_known_and_fast() {
        // Wrong secret: refused
        let (url, _) = server(r#"{"known": true}"#, Duration::ZERO).await;
        let lookup = ContactsLookup::new(url, "wrong".into()).unwrap();
        assert!(
            !lookup
                .is_known(1, "ann@example.com", "bob@example.org")
                .await
        );

        // A garbled answer
        let (url, _) = server("not json", Duration::ZERO).await;
        let lookup = ContactsLookup::new(url, "s3cret".into()).unwrap();
        assert!(
            !lookup
                .is_known(1, "ann@example.com", "bob@example.org")
                .await
        );

        // Nobody listening
        let lookup =
            ContactsLookup::new("http://127.0.0.1:9/known".into(), "s3cret".into()).unwrap();
        assert!(
            !lookup
                .is_known(1, "ann@example.com", "bob@example.org")
                .await
        );

        // Too slow: given up after TIMEOUT, not after the server's 5 seconds
        let (url, _) = server(r#"{"known": true}"#, Duration::from_secs(5)).await;
        let lookup = ContactsLookup::new(url, "s3cret".into()).unwrap();
        let started = std::time::Instant::now();
        assert!(
            !lookup
                .is_known(1, "ann@example.com", "bob@example.org")
                .await
        );
        assert!(started.elapsed() < TIMEOUT * 3, "{:?}", started.elapsed());
    }
}
