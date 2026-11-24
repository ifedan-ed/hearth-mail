/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The DAV bridge's settings and sign-in: where the cloud is
//! (`INBUXA_DAV_URL`), how Hearth Mail signs in there (`INBUXA_DAV_AUTH`),
//! and the app passwords people link through Nextcloud's Login Flow v2
//! (docs/activesync.md, "Calendars, contacts, tasks, notes: the DAV
//! bridge").
//!
//! - `passthrough`: the credentials the device signed in with are used for
//!   the cloud too, for a cloud on the same sign-in (Hearth, or a Nextcloud
//!   on the same directory). A device signed in with a token gets mail only.
//! - `app-password`: each person links the cloud once on `/setup`; the app
//!   password Nextcloud hands over is kept in the key-value store (with the
//!   rest of the account's data; it can be revoked in Nextcloud at any
//!   time, and unlinking removes it here).
//!
//! Without `INBUXA_DAV_URL` the bridge is off and devices get mail only.
//! One alone, or a value that can't be read, is an error at start that
//! leaves the bridge off and says why in the log.

use common::{KV_ACTIVESYNC, Server};
use dav_client::{Credentials, DavClient, Limits, LoginFlow, LoginPoll, Url};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::Duration;
use store::dispatch::lookup::KeyValue;

/// How Hearth Mail signs in to the cloud.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavAuth {
    Passthrough,
    AppPassword,
}

/// The bridge's settings, read once at start.
#[derive(Debug, Clone)]
pub struct DavSettings {
    /// The cloud's DAV root, e.g. `https://cloud.example.org/remote.php/dav`.
    pub url: String,
    pub auth: DavAuth,
}

static SETTINGS: OnceLock<Option<DavSettings>> = OnceLock::new();

// The kinds of key the bridge keeps (the second byte, after the prefix;
// the device state in crate::state uses 0 and 1).
const KIND_LINK: u8 = 2;
const KIND_PENDING: u8 = 3;
/// A Login Flow waits this long for the person to approve it.
const PENDING_TTL: u64 = 20 * 60;

impl DavSettings {
    /// The settings, or `None` when the bridge is off.
    pub fn get() -> Option<&'static DavSettings> {
        SETTINGS
            .get_or_init(|| {
                let url = types::branding::env_var("DAV_URL").ok();
                let auth = types::branding::env_var("DAV_AUTH").ok();
                match parse(url.as_deref(), auth.as_deref()) {
                    Ok(settings) => settings,
                    Err(reason) => {
                        trc::event!(
                            Server(trc::ServerEvent::StartupError),
                            Details = "The DAV bridge is off",
                            Reason = reason,
                        );
                        None
                    }
                }
            })
            .as_ref()
    }

    /// The cloud's own root for Login Flow v2: the DAV URL before
    /// `/remote.php` (Nextcloud's layout). `None` for a cloud laid out
    /// otherwise, which then can't be linked from the setup page.
    pub fn login_root(&self) -> Option<Url> {
        let (root, _) = self.url.split_once("/remote.php")?;
        Url::parse(&format!("{root}/")).ok()
    }
}

/// Reads the two settings: both or neither.
fn parse(url: Option<&str>, auth: Option<&str>) -> Result<Option<DavSettings>, String> {
    match (url.map(str::trim), auth.map(str::trim)) {
        (None, None) | (Some(""), None) => Ok(None),
        (Some(url), Some(auth)) if !url.is_empty() => {
            let parsed = Url::parse(url).map_err(|err| format!("INBUXA_DAV_URL: {err}"))?;
            if !matches!(parsed.scheme(), "https" | "http") {
                return Err("INBUXA_DAV_URL must be an https URL".into());
            }
            let auth = match auth {
                "passthrough" => DavAuth::Passthrough,
                "app-password" => DavAuth::AppPassword,
                other => {
                    return Err(format!(
                        "INBUXA_DAV_AUTH is {other:?}; it takes passthrough or app-password"
                    ));
                }
            };
            Ok(Some(DavSettings {
                url: url.trim_end_matches('/').to_string(),
                auth,
            }))
        }
        _ => Err("INBUXA_DAV_URL and INBUXA_DAV_AUTH go together".into()),
    }
}

/// A linked cloud login.
#[derive(Serialize, Deserialize, Clone)]
pub struct Link {
    pub login_name: String,
    pub app_password: String,
}

impl std::fmt::Debug for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the password.
        f.debug_struct("Link")
            .field("login_name", &self.login_name)
            .finish()
    }
}

/// A Login Flow waiting for approval.
#[derive(Serialize, Deserialize)]
struct Pending {
    endpoint: String,
    token: String,
}

fn key(kind: u8, account_id: u32) -> Vec<u8> {
    let mut key = Vec::with_capacity(6);
    key.push(kind);
    key.extend_from_slice(&account_id.to_be_bytes());
    KeyValue::<()>::build_key(KV_ACTIVESYNC, key)
}

async fn load<T: for<'de> Deserialize<'de>>(
    server: &Server,
    key: Vec<u8>,
) -> trc::Result<Option<T>> {
    Ok(server
        .in_memory_store()
        .key_get::<String>(key)
        .await?
        .and_then(|json| serde_json::from_str(&json).ok()))
}

async fn store_json<T: Serialize>(
    server: &Server,
    key: Vec<u8>,
    value: &T,
    expires: Option<u64>,
) -> trc::Result<()> {
    let json = serde_json::to_string(value).unwrap_or_default();
    server
        .in_memory_store()
        .key_set(KeyValue {
            key,
            value: json.into_bytes(),
            expires,
        })
        .await
}

/// The account's linked cloud login, if any.
pub async fn link(server: &Server, account_id: u32) -> trc::Result<Option<Link>> {
    load(server, key(KIND_LINK, account_id)).await
}

/// The linked login name, for the profile.
pub async fn linked_login(server: &Server, account_id: u32) -> trc::Result<Option<String>> {
    Ok(link(server, account_id).await?.map(|link| link.login_name))
}

/// Forgets the account's link (the person unlinked, or the cloud refused
/// the app password).
pub async fn unlink(server: &Server, account_id: u32) -> trc::Result<()> {
    server
        .in_memory_store()
        .key_delete(key(KIND_LINK, account_id))
        .await
}

fn limits() -> Limits {
    Limits {
        timeout: Duration::from_secs(20),
        user_agent: "Hearth Mail ActiveSync".into(),
        ..Limits::default()
    }
}

/// Starts linking: Nextcloud's login page for the person to approve.
pub async fn start_link(server: &Server, account_id: u32) -> Result<Url, String> {
    let settings = DavSettings::get().ok_or("the DAV bridge is off")?;
    let root = settings
        .login_root()
        .ok_or("the cloud isn't laid out as Nextcloud")?;
    let flow = LoginFlow::new(&limits()).map_err(|err| err.to_string())?;
    let started = flow.start(&root).await.map_err(|err| err.to_string())?;
    store_json(
        server,
        key(KIND_PENDING, account_id),
        &Pending {
            endpoint: started.poll_endpoint.to_string(),
            token: started.poll_token,
        },
        Some(PENDING_TTL),
    )
    .await
    .map_err(|err| err.to_string())?;
    Ok(started.login)
}

/// Whether the person approved: `Ok(true)` once linked (the app password
/// stored), `Ok(false)` while waiting.
pub async fn poll_link(server: &Server, account_id: u32) -> Result<bool, String> {
    let pending: Pending = load(server, key(KIND_PENDING, account_id))
        .await
        .map_err(|err| err.to_string())?
        .ok_or("no link was started, or it expired")?;
    let endpoint = Url::parse(&pending.endpoint).map_err(|err| err.to_string())?;
    let flow = LoginFlow::new(&limits()).map_err(|err| err.to_string())?;
    match flow
        .poll(&endpoint, &pending.token)
        .await
        .map_err(|err| err.to_string())?
    {
        LoginPoll::Pending => Ok(false),
        LoginPoll::Approved(login) => {
            store_json(
                server,
                key(KIND_LINK, account_id),
                &Link {
                    login_name: login.login_name,
                    app_password: login.app_password,
                },
                None,
            )
            .await
            .map_err(|err| err.to_string())?;
            let _ = server
                .in_memory_store()
                .key_delete(key(KIND_PENDING, account_id))
                .await;
            Ok(true)
        }
    }
}

/// A client signed in to the cloud for this account, or `None` when the
/// bridge is off or the account has no way in (no link; a token sign-in
/// with passthrough). `basic` is the device's own Basic credentials.
pub async fn client(
    server: &Server,
    account_id: u32,
    basic: Option<(&str, &str)>,
) -> trc::Result<Option<DavClient>> {
    let Some(settings) = DavSettings::get() else {
        return Ok(None);
    };
    let credentials = match settings.auth {
        DavAuth::Passthrough => basic.map(|(user, password)| Credentials {
            user: user.to_string(),
            password: password.to_string(),
        }),
        DavAuth::AppPassword => link(server, account_id).await?.map(|link| Credentials {
            user: link.login_name,
            password: link.app_password,
        }),
    };
    Ok(credentials
        .and_then(|credentials| DavClient::new(&settings.url, credentials, limits()).ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_go_together() {
        assert!(parse(None, None).expect("ok").is_none());
        let settings = parse(
            Some("https://cloud.example.org/remote.php/dav/"),
            Some("app-password"),
        )
        .expect("ok")
        .expect("on");
        assert_eq!(settings.url, "https://cloud.example.org/remote.php/dav");
        assert_eq!(settings.auth, DavAuth::AppPassword);
        assert_eq!(
            settings.login_root().map(|url| url.to_string()).as_deref(),
            Some("https://cloud.example.org/")
        );
        assert!(parse(Some("https://x"), None).is_err());
        assert!(parse(None, Some("passthrough")).is_err());
        assert!(parse(Some("https://x"), Some("both")).is_err());
        assert!(parse(Some("ftp://x"), Some("passthrough")).is_err());
        let hearth = parse(Some("https://hearth.example.org/dav"), Some("passthrough"))
            .expect("ok")
            .expect("on");
        assert_eq!(hearth.login_root(), None);
    }
}
