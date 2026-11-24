/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! `/setup`: Hearth Mail's own small page for setting up devices
//! (docs/activesync.md, "The Apple profile").
//!
//! The page asks for the address and the password and then talks to the
//! server with them as HTTP Basic, the way a mail app does, so there is no
//! session or cookie to keep: `GET /setup/apple.mobileconfig` answers the
//! profile for the account that signed in, and `/setup/nextcloud/*` links
//! the cloud for the DAV bridge ([`crate::bridge`]).
//!
//! The profile installs the mail account (IMAP on 993 and SMTP on 465, the
//! address as the user name) and, when the cloud is configured, CalDAV and
//! CardDAV accounts for it. It holds no password: the device asks once.
//! It is unsigned, which iOS shows as "Unverified".

use crate::bridge::{self, DavAuth, DavSettings};
use common::{Server, auth::AccessToken};
use http_proto::{HttpResponse, HttpSessionData};
use hyper::{Method, StatusCode};

/// The page: a sign-in form and two buttons. Self-contained (no scripts or
/// styles from elsewhere), so the server's strict content policy applies.
const PAGE: &str = include_str!("setup.html");

/// The page's own policy: its inline script and style, requests to this
/// server only, nothing else.
const PAGE_POLICY: &str = "default-src 'none'; script-src 'unsafe-inline'; \
style-src 'unsafe-inline'; connect-src 'self'; img-src 'self' blob:; form-action 'none'; \
frame-ancestors 'none'; base-uri 'none'";

/// `GET /setup`.
pub fn page() -> HttpResponse {
    HttpResponse::new(StatusCode::OK)
        .with_content_type("text/html; charset=utf-8")
        .with_header("Content-Security-Policy", PAGE_POLICY)
        .with_no_store()
        .with_text_body(PAGE)
}

/// The signed-in parts of `/setup/<path>`.
pub async fn handle(
    server: &Server,
    path: &str,
    method: &Method,
    session: &HttpSessionData,
    token: &AccessToken,
) -> trc::Result<HttpResponse> {
    let account_id = token.account_id();
    let settings = DavSettings::get();
    let can_link =
        settings.is_some_and(|s| s.auth == DavAuth::AppPassword && s.login_root().is_some());
    Ok(match (method, path) {
        (&Method::GET, "account") => {
            let linked = bridge::link(server, account_id).await?.is_some();
            json_response(
                StatusCode::OK,
                serde_json::json!({ "cloudLink": can_link, "cloudLinked": linked }),
            )
        }
        (&Method::GET, "apple.mobileconfig") => apple_profile(server, token, session).await?,
        (&Method::POST, "nextcloud/start") if can_link => {
            match bridge::start_link(server, account_id).await {
                Ok(login) => json_response(
                    StatusCode::OK,
                    serde_json::json!({ "login": login.to_string() }),
                ),
                Err(reason) => failed(session, "Starting the Nextcloud link failed", reason),
            }
        }
        (&Method::POST, "nextcloud/poll") if can_link => {
            match bridge::poll_link(server, account_id).await {
                Ok(true) => json_response(StatusCode::OK, serde_json::json!({ "linked": true })),
                Ok(false) => HttpResponse::new(StatusCode::ACCEPTED).with_no_store(),
                Err(reason) => failed(session, "Polling the Nextcloud link failed", reason),
            }
        }
        (&Method::POST, "nextcloud/unlink") => {
            bridge::unlink(server, account_id).await?;
            json_response(StatusCode::OK, serde_json::json!({ "linked": false }))
        }
        _ => HttpResponse::new(StatusCode::NOT_FOUND),
    })
}

fn json_response(status: StatusCode, value: serde_json::Value) -> HttpResponse {
    HttpResponse::new(status)
        .with_content_type("application/json")
        .with_no_store()
        .with_text_body(value.to_string())
}

fn failed(session: &HttpSessionData, what: &str, reason: String) -> HttpResponse {
    trc::event!(
        Http(trc::HttpEvent::Error),
        SpanId = session.session_id,
        Details = what.to_string(),
        Reason = reason,
    );
    HttpResponse::new(StatusCode::BAD_GATEWAY).with_no_store()
}

/// What the profile needs to know about the account.
pub struct ProfileAccount<'x> {
    pub email: &'x str,
    pub name: &'x str,
    /// The cloud's DAV root and the user name there, when there is one.
    pub dav: Option<(&'x str, Option<&'x str>)>,
}

/// `GET /setup/apple.mobileconfig` for a signed-in account.
pub async fn apple_profile(
    server: &Server,
    token: &AccessToken,
    session: &HttpSessionData,
) -> trc::Result<HttpResponse> {
    let (email, name) = crate::account_identity(server, token, session).await?;
    let dav = DavSettings::get();
    let linked = match &dav {
        Some(_) => crate::bridge::linked_login(server, token.account_id()).await?,
        None => None,
    };
    let host = server.core.network.server_name.clone();
    let profile = build_profile(
        &host,
        &ProfileAccount {
            email: &email,
            name: &name,
            dav: dav
                .as_ref()
                .map(|dav| (dav.url.as_str(), linked.as_deref())),
        },
    );
    Ok(HttpResponse::new(StatusCode::OK)
        .with_content_type("application/x-apple-aspen-config")
        .with_content_disposition(format!(
            "attachment; filename=\"{}.mobileconfig\"",
            email.replace(['"', '\\', '/'], "_")
        ))
        .with_no_store()
        .with_text_body(profile))
}

/// Escapes text for a property list.
fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A UUID derived from `parts`, so the same account on the same server
/// always gets the same one and installing again replaces the profile
/// instead of adding a second. (FNV-1a, twice with different offsets.)
fn stable_uuid(parts: &[&str]) -> String {
    let hash = |seed: u64| {
        let mut h = seed;
        for part in parts {
            for byte in part.bytes().chain(std::iter::once(0)) {
                h ^= u64::from(byte);
                h = h.wrapping_mul(0x100000001b3);
            }
        }
        h
    };
    let (a, b) = (hash(0xcbf29ce484222325), hash(0x84222325cbf29ce4));
    // Version 4 and variant bits, as a random UUID would have.
    let a = (a & 0xffff_ffff_ffff_0fff) | 0x4000;
    let b = (b & 0x3fff_ffff_ffff_ffff) | 0x8000_0000_0000_0000;
    format!(
        "{:08X}-{:04X}-{:04X}-{:04X}-{:012X}",
        a >> 32,
        (a >> 16) & 0xffff,
        a & 0xffff,
        b >> 48,
        b & 0xffff_ffff_ffff
    )
}

/// The host and port of an https or http URL.
fn host_port(url: &str) -> Option<(String, u16)> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split('/').next()?;
    let default = if scheme.eq_ignore_ascii_case("http") {
        80
    } else {
        443
    };
    Some(match authority.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => {
            (host.to_string(), port.parse().ok()?)
        }
        _ => (authority.to_string(), default),
    })
}

/// The property list of the profile.
pub fn build_profile(host: &str, account: &ProfileAccount<'_>) -> String {
    let email = esc(account.email);
    let name = esc(if account.name.is_empty() {
        account.email
    } else {
        account.name
    });
    // A reverse-DNS prefix from the server's name.
    let prefix: String = host.split('.').rev().collect::<Vec<_>>().join(".");
    let prefix = esc(&prefix);
    let host_x = esc(host);
    let mut payloads = format!(
        "<dict>\
<key>EmailAccountDescription</key><string>{email}</string>\
<key>EmailAccountName</key><string>{name}</string>\
<key>EmailAccountType</key><string>EmailTypeIMAP</string>\
<key>EmailAddress</key><string>{email}</string>\
<key>IncomingMailServerAuthentication</key><string>EmailAuthPassword</string>\
<key>IncomingMailServerHostName</key><string>{host_x}</string>\
<key>IncomingMailServerPortNumber</key><integer>993</integer>\
<key>IncomingMailServerUseSSL</key><true/>\
<key>IncomingMailServerUsername</key><string>{email}</string>\
<key>OutgoingMailServerAuthentication</key><string>EmailAuthPassword</string>\
<key>OutgoingMailServerHostName</key><string>{host_x}</string>\
<key>OutgoingMailServerPortNumber</key><integer>465</integer>\
<key>OutgoingMailServerUseSSL</key><true/>\
<key>OutgoingMailServerUsername</key><string>{email}</string>\
<key>OutgoingPasswordSameAsIncomingPassword</key><true/>\
<key>PayloadDisplayName</key><string>Mail</string>\
<key>PayloadIdentifier</key><string>{prefix}.mail.{email}</string>\
<key>PayloadType</key><string>com.apple.mail.managed</string>\
<key>PayloadUUID</key><string>{}</string>\
<key>PayloadVersion</key><integer>1</integer>\
</dict>",
        stable_uuid(&[host, account.email, "mail"])
    );
    if let Some((url, user)) = account.dav
        && let Some((dav_host, port)) = host_port(url)
    {
        let ssl = !url.to_ascii_lowercase().starts_with("http://");
        let url = esc(url);
        let dav_host = esc(&dav_host);
        let user = user.map(esc);
        for (kind, label, payload) in [
            ("CalDAV", "Calendars", "com.apple.caldav.account"),
            ("CardDAV", "Contacts", "com.apple.carddav.account"),
        ] {
            payloads.push_str(&format!(
                "<dict>\
<key>{kind}AccountDescription</key><string>{label}</string>\
<key>{kind}HostName</key><string>{dav_host}</string>\
<key>{kind}Port</key><integer>{port}</integer>\
<key>{kind}PrincipalURL</key><string>{url}</string>\
<key>{kind}UseSSL</key>{}\
{}\
<key>PayloadDisplayName</key><string>{label}</string>\
<key>PayloadIdentifier</key><string>{prefix}.{}.{email}</string>\
<key>PayloadType</key><string>{payload}</string>\
<key>PayloadUUID</key><string>{}</string>\
<key>PayloadVersion</key><integer>1</integer>\
</dict>",
                if ssl { "<true/>" } else { "<false/>" },
                user.as_deref()
                    .map(|user| format!("<key>{kind}Username</key><string>{user}</string>"))
                    .unwrap_or_default(),
                kind.to_ascii_lowercase(),
                stable_uuid(&[host, account.email, kind]),
            ));
        }
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\"><dict>\
<key>PayloadContent</key><array>{payloads}</array>\
<key>PayloadDescription</key><string>Mail, calendars and contacts for {email}</string>\
<key>PayloadDisplayName</key><string>{email}</string>\
<key>PayloadIdentifier</key><string>{prefix}.profile.{email}</string>\
<key>PayloadRemovalDisallowed</key><false/>\
<key>PayloadType</key><string>Configuration</string>\
<key>PayloadUUID</key><string>{}</string>\
<key>PayloadVersion</key><integer>1</integer>\
</dict></plist>\n",
        stable_uuid(&[host, account.email, "profile"])
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_with_and_without_the_cloud() {
        let mail_only = build_profile(
            "mail.example.org",
            &ProfileAccount {
                email: "ann@example.org",
                name: "Ann & Co",
                dav: None,
            },
        );
        assert!(mail_only.contains("<string>com.apple.mail.managed</string>"));
        assert!(
            mail_only
                .contains("<key>IncomingMailServerHostName</key><string>mail.example.org</string>")
        );
        assert!(mail_only.contains("Ann &amp; Co"));
        assert!(!mail_only.contains("caldav"));
        assert!(!mail_only.to_lowercase().contains("password</key><string>"));

        let full = build_profile(
            "mail.example.org",
            &ProfileAccount {
                email: "ann@example.org",
                name: "Ann",
                dav: Some(("https://cloud.example.org/remote.php/dav", Some("ann"))),
            },
        );
        for wanted in [
            "<string>com.apple.caldav.account</string>",
            "<string>com.apple.carddav.account</string>",
            "<key>CalDAVHostName</key><string>cloud.example.org</string>",
            "<key>CardDAVPort</key><integer>443</integer>",
            "<key>CalDAVUsername</key><string>ann</string>",
            "<key>CardDAVPrincipalURL</key><string>https://cloud.example.org/remote.php/dav</string>",
        ] {
            assert!(full.contains(wanted), "{wanted}");
        }
        // The same account gets the same identifiers each time.
        assert_eq!(stable_uuid(&["a", "b"]), stable_uuid(&["a", "b"]));
        assert_ne!(stable_uuid(&["a", "b"]), stable_uuid(&["a", "c"]));
        assert_eq!(stable_uuid(&["x"]).len(), 36);
        assert_eq!(&stable_uuid(&["x"])[14..15], "4");
    }

    #[test]
    fn hosts_and_ports() {
        assert_eq!(
            host_port("https://cloud.example.org/remote.php/dav"),
            Some(("cloud.example.org".into(), 443))
        );
        assert_eq!(
            host_port("http://10.0.0.5:8080/dav/"),
            Some(("10.0.0.5".into(), 8080))
        );
        assert_eq!(host_port("nonsense"), None);
    }
}
