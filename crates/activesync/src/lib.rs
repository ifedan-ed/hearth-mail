/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Exchange ActiveSync 14.1 and 16.1 at `/Microsoft-Server-ActiveSync`
//! (docs/activesync.md).
//!
//! The HTTP listener signs the request in (Basic with a password or an app
//! password, or a bearer token) and hands it to [`handle`]. From there:
//! the request line and the WBXML body are read ([`activesync_proto`]),
//! the command runs against the account (mail through in-process JMAP,
//! [`jmap`]), and its answer goes back as WBXML. Per-device state (sync
//! keys and what each device holds) is kept by [`state`].

pub mod bridge;
pub mod commands;
pub mod dav;
pub mod error;
pub mod folders;
pub mod jmap;
pub mod mail;
pub mod setup;
pub mod state;

use activesync_proto::{
    request::{RequestLine, Version},
    wbxml::{self, Element},
};
use common::{Server, auth::AccessToken};
use error::{Error, Result};
use http_proto::{HttpRequest, HttpResponse, HttpSessionData, request::fetch_body};
use hyper::{Method, StatusCode, header};

/// The commands offered in `MS-ASProtocolCommands`. ValidateCert is left
/// out (devices check certificates themselves), as are GetAttachment and
/// the 2.5-era folder commands, which 14.1 and 16.1 don't use.
const COMMANDS: &str = "Sync,SendMail,SmartForward,SmartReply,FolderSync,FolderCreate,\
FolderDelete,FolderUpdate,MoveItems,GetItemEstimate,MeetingResponse,Search,Settings,Ping,\
ItemOperations,Provision,ResolveRecipients,Find";

/// The server version announced in `MS-Server-ActiveSync`; clients only
/// log it.
const SERVER_VERSION: &str = "16.1";

/// The WBXML media type of requests and answers.
pub const WBXML: &str = "application/vnd.ms-sync.wbxml";

/// Everything a command needs to know about its request.
pub struct Ctx<'a> {
    pub server: &'a Server,
    pub token: &'a AccessToken,
    pub session: &'a HttpSessionData,
    pub line: RequestLine,
    pub version: Version,
    /// `User-Agent`, kept with the device for the logs.
    pub user_agent: String,
    /// `X-MS-PolicyKey`, or the base64 query's policy key.
    pub policy_key: Option<u32>,
    /// The Basic credentials the device signed in with, for the DAV
    /// bridge's `passthrough` sign-in; `None` for a token.
    pub basic: Option<(String, String)>,
}

impl Ctx<'_> {
    /// The signed-in account's id.
    pub fn account_id(&self) -> u32 {
        self.token.account_id()
    }
}

/// What a command answers.
pub enum Reply {
    /// A WBXML document.
    Wbxml(Element),
    /// HTTP 200 with no body (SendMail's success, an unchanged Sync).
    Empty,
    /// Bytes of another type (ItemOperations' multipart answer).
    Raw(&'static str, Vec<u8>),
}

/// Serves one ActiveSync request of a signed-in account.
pub async fn handle(
    server: &Server,
    req: &mut HttpRequest,
    session: &HttpSessionData,
    token: &AccessToken,
) -> trc::Result<HttpResponse> {
    if req.method() == Method::OPTIONS {
        return Ok(base_response(StatusCode::OK)
            .with_header("MS-ASProtocolVersions", Version::OFFERED)
            .with_header("MS-ASProtocolCommands", COMMANDS)
            .with_header("Allow", "OPTIONS,POST")
            .with_header("Public", "OPTIONS,POST"));
    }
    if req.method() != Method::POST {
        return Ok(base_response(StatusCode::METHOD_NOT_ALLOWED));
    }

    let line = match RequestLine::parse(req.uri().query().unwrap_or_default()) {
        Ok(line) => line,
        Err(err) => return Ok(bad_request(session, &err.to_string())),
    };
    // The plain query leaves the version to the header; the base64 query
    // carries it.
    let version = match line
        .version
        .or_else(|| header_str(req, "MS-ASProtocolVersion").and_then(Version::parse))
    {
        Some(version) => version,
        None => {
            return Ok(bad_request(
                session,
                "unsupported or missing protocol version",
            ));
        }
    };
    let policy_key = line
        .policy_key
        .or_else(|| header_str(req, "X-MS-PolicyKey").and_then(|key| key.trim().parse().ok()));
    let user_agent = header_str(req, header::USER_AGENT.as_str())
        .unwrap_or_default()
        .to_string();
    let content_type = header_str(req, header::CONTENT_TYPE.as_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    let max_size = server.core.jmap.upload_max_size;
    let Some(bytes) = fetch_body(req, max_size, session.session_id).await else {
        return Ok(base_response(StatusCode::PAYLOAD_TOO_LARGE));
    };
    // SendMail, SmartReply and SmartForward may carry the message itself
    // (`message/rfc822`) instead of WBXML; everything else is WBXML.
    let body = if bytes.is_empty() {
        Body::None
    } else if content_type.starts_with("message/rfc822") {
        Body::Mime(bytes)
    } else {
        match wbxml::decode(&bytes) {
            Ok(element) => Body::Wbxml(element),
            Err(err) => return Ok(bad_request(session, &format!("WBXML: {err}"))),
        }
    };

    let command = line.command;
    let basic = basic_credentials(req);
    let ctx = Ctx {
        server,
        token,
        session,
        line,
        version,
        user_agent,
        policy_key,
        basic,
    };
    let reply = commands::dispatch(&ctx, body).await;
    Ok(match reply {
        Ok(Reply::Wbxml(element)) => base_response(StatusCode::OK)
            .with_content_type(WBXML)
            .with_binary_body(wbxml::encode(&element)),
        Ok(Reply::Empty) => base_response(StatusCode::OK),
        Ok(Reply::Raw(content_type, bytes)) => base_response(StatusCode::OK)
            .with_content_type(content_type)
            .with_binary_body(bytes),
        Err(Error::Http(status)) => base_response(status),
        Err(Error::BadRequest(reason)) => bad_request(session, &reason),
        Err(err) => {
            trc::event!(
                Http(trc::HttpEvent::Error),
                SpanId = session.session_id,
                Details = format!("ActiveSync {command} failed"),
                Reason = err.to_string(),
            );
            base_response(StatusCode::INTERNAL_SERVER_ERROR)
        }
    })
}

/// A request's body, read once.
pub enum Body {
    None,
    Wbxml(Element),
    /// A message sent as `message/rfc822`.
    Mime(Vec<u8>),
}

impl Body {
    /// The WBXML document, or a protocol error for commands that need one.
    pub fn wbxml(self) -> Result<Element> {
        match self {
            Body::Wbxml(element) => Ok(element),
            _ => Err(Error::BadRequest("this command needs a WBXML body".into())),
        }
    }
}

fn base_response(status: StatusCode) -> HttpResponse {
    HttpResponse::new(status)
        .with_header("MS-Server-ActiveSync", SERVER_VERSION)
        .with_no_store()
}

fn bad_request(session: &HttpSessionData, reason: &str) -> HttpResponse {
    trc::event!(
        Http(trc::HttpEvent::Error),
        SpanId = session.session_id,
        Details = "ActiveSync request refused",
        Reason = reason.to_string(),
    );
    base_response(StatusCode::BAD_REQUEST)
}

/// The user and password of a Basic `Authorization` header.
fn basic_credentials(req: &HttpRequest) -> Option<(String, String)> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let value = header_str(req, header::AUTHORIZATION.as_str())?;
    let (scheme, encoded) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = String::from_utf8(STANDARD.decode(encoded.trim()).ok()?).ok()?;
    let (user, password) = decoded.split_once(':')?;
    Some((user.to_string(), password.to_string()))
}

/// The signed-in account's address and display name, from its first
/// identity (for pages outside an ActiveSync request).
pub async fn account_identity(
    server: &Server,
    token: &AccessToken,
    session: &HttpSessionData,
) -> trc::Result<(String, String)> {
    let identity = jmap::run(
        server,
        token,
        session,
        &jmap::USING,
        vec![jmap::Call::new("Identity/get", "i", serde_json::json!({}))],
    )
    .await
    .ok()
    .and_then(|answers| answers.get("i").ok().cloned())
    .and_then(|identities| {
        identities["list"]
            .as_array()
            .and_then(|l| l.first())
            .cloned()
    })
    .unwrap_or_default();
    Ok((
        identity["email"].as_str().unwrap_or_default().to_string(),
        identity["name"].as_str().unwrap_or_default().to_string(),
    ))
}

fn header_str<'x>(req: &'x HttpRequest, name: &str) -> Option<&'x str> {
    req.headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
}
