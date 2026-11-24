//! Nextcloud's Login Flow v2: how a person gives Hearth Mail an app password
//! without typing their Nextcloud password into it.
//!
//! `start` asks Nextcloud for a login page and a poll token; the person opens
//! the page and approves; `poll` meanwhile answers 404 until then, and once
//! with the login name and a new app password after (the token is then
//! spent). The person can revoke the app password in Nextcloud at any time.
//! https://docs.nextcloud.com/server/latest/developer_manual/client_apis/LoginFlow/index.html

use std::fmt;

use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::{Method, StatusCode, Url, header};
use serde::Deserialize;

use crate::{
    DavError, Limits,
    client::{Reply, check, http_client, read},
    error::protocol,
};

/// A client for Login Flow v2. It signs in as nobody: the flow itself is the
/// sign-in.
#[derive(Debug, Clone)]
pub struct LoginFlow {
    http: reqwest::Client,
    max_body_bytes: usize,
}

/// A started flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginFlowStart {
    /// The page the person opens to approve.
    pub login: Url,
    /// Where to poll.
    pub poll_endpoint: Url,
    /// The poll token (a secret until it is spent).
    pub poll_token: String,
}

/// What a poll found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginPoll {
    /// Not approved yet (or the token expired: Nextcloud says 404 for both).
    Pending,
    /// Approved: the credentials to sign in with.
    Approved(NextcloudLogin),
}

/// The credentials a Login Flow hands out.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct NextcloudLogin {
    /// The Nextcloud server's address.
    pub server: String,
    /// The person's login name.
    #[serde(rename = "loginName")]
    pub login_name: String,
    /// The app password.
    #[serde(rename = "appPassword")]
    pub app_password: String,
}

impl fmt::Debug for NextcloudLogin {
    // Never the app password.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NextcloudLogin")
            .field("server", &self.server)
            .field("login_name", &self.login_name)
            .finish_non_exhaustive()
    }
}

/// The JSON `start` returns.
#[derive(Deserialize)]
struct StartReply {
    poll: PollReply,
    login: String,
}

#[derive(Deserialize)]
struct PollReply {
    token: String,
    endpoint: String,
}

impl LoginFlow {
    /// A Login Flow client; `limits.user_agent` becomes the app password's
    /// name in Nextcloud's security settings.
    pub fn new(limits: &Limits) -> Result<Self, DavError> {
        Ok(Self {
            http: http_client(limits)?,
            max_body_bytes: limits.max_body_bytes,
        })
    }

    /// Starts a flow at the Nextcloud at `server` (its web address, such as
    /// `https://cloud.example`, with or without a trailing slash).
    pub async fn start(&self, server: &Url) -> Result<LoginFlowStart, DavError> {
        let mut root = server.clone();
        if !root.path().ends_with('/') {
            let path = format!("{}/", root.path());
            root.set_path(&path);
        }
        let url = root
            .join("index.php/login/v2")
            .map_err(|err| DavError::InvalidUrl(format!("{server}: {err}")))?;
        let reply = self.send(self.http.request(Method::POST, url)).await?;
        check(&reply)?;
        let start: StartReply = serde_json::from_slice(&reply.body)
            .map_err(|err| protocol(format!("Login Flow start: {err}")))?;
        let parse = |what: &str, url: &str| {
            Url::parse(url).map_err(|err| protocol(format!("Login Flow {what} {url:?}: {err}")))
        };
        Ok(LoginFlowStart {
            login: parse("login", &start.login)?,
            poll_endpoint: parse("endpoint", &start.poll.endpoint)?,
            poll_token: start.poll.token,
        })
    }

    /// Asks once whether the person approved the flow.
    pub async fn poll(&self, endpoint: &Url, token: &str) -> Result<LoginPoll, DavError> {
        let body = format!("token={}", utf8_percent_encode(token, NON_ALPHANUMERIC));
        let request = self
            .http
            .request(Method::POST, endpoint.clone())
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body);
        let reply = self.send(request).await?;
        if reply.status == StatusCode::NOT_FOUND {
            return Ok(LoginPoll::Pending);
        }
        check(&reply)?;
        serde_json::from_slice(&reply.body)
            .map(LoginPoll::Approved)
            .map_err(|err| protocol(format!("Login Flow poll: {err}")))
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<Reply, DavError> {
        read(request.send().await?, self.max_body_bytes).await
    }
}
