//! The signed-in client and the request plumbing every DAV call shares:
//! the Authorization header, the body-size cap, and turning HTTP statuses
//! into [`DavError`]s.

use std::{fmt, time::Duration};

use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::{
    Method, RequestBuilder, StatusCode, Url,
    header::{self, HeaderMap, HeaderValue},
};

use crate::{
    DavError, href,
    xml::multistatus::{Multistatus, parse_multistatus},
};

/// A user name and password (or Nextcloud app password) for Basic auth.
#[derive(Clone)]
pub struct Credentials {
    /// The login name.
    pub user: String,
    /// The password or app password.
    pub password: String,
}

impl fmt::Debug for Credentials {
    // Never the password.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

/// Bounds on what one client does, so a slow or hostile server can't hold a
/// device's request or the server's memory.
#[derive(Debug, Clone)]
pub struct Limits {
    /// The whole request, body included.
    pub timeout: Duration,
    /// Establishing the connection.
    pub connect_timeout: Duration,
    /// The largest response body read; more is [`DavError::TooLarge`].
    pub max_body_bytes: usize,
    /// How many hrefs one multiget asks for.
    pub multiget_batch: usize,
    /// How many truncated pages one sync-collection follows before giving
    /// up with [`DavError::Protocol`] (a server that never finishes).
    pub max_sync_pages: usize,
    /// The User-Agent; Nextcloud shows it as the app password's name in
    /// Login Flow v2.
    pub user_agent: String,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(10),
            max_body_bytes: 32 << 20,
            multiget_batch: 100,
            max_sync_pages: 100,
            user_agent: concat!("dav-client/", env!("CARGO_PKG_VERSION")).to_owned(),
        }
    }
}

/// A client for one person's DAV account at one DAV root.
///
/// Cheap to clone (the connection pool is shared). Every method takes
/// absolute URLs as returned by discovery; [`href::resolve`] turns anything
/// else into one.
#[derive(Clone)]
pub struct DavClient {
    http: reqwest::Client,
    base: Url,
    /// Basic auth, encoded once rather than on every request.
    authorization: HeaderValue,
    limits: Limits,
}

impl fmt::Debug for DavClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DavClient")
            .field("base", &self.base.as_str())
            .finish_non_exhaustive()
    }
}

/// The WebDAV methods reqwest has no constant for.
#[derive(Debug, Clone, Copy)]
pub(crate) enum DavMethod {
    Propfind,
    Report,
    Mkcalendar,
}

impl DavMethod {
    pub(crate) fn method(self) -> Method {
        let name: &[u8] = match self {
            Self::Propfind => b"PROPFIND",
            Self::Report => b"REPORT",
            Self::Mkcalendar => b"MKCALENDAR",
        };
        // Proven invariant: all three are valid HTTP method tokens.
        Method::from_bytes(name).expect("WebDAV method names are valid tokens")
    }
}

/// The Depth header of a PROPFIND or REPORT.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Depth {
    Zero,
    One,
}

impl Depth {
    fn header(self) -> HeaderValue {
        HeaderValue::from_static(match self {
            Self::Zero => "0",
            Self::One => "1",
        })
    }
}

/// A finished response: its status, final URL (after redirects, the base for
/// relative hrefs), headers and body.
pub(crate) struct Reply {
    pub status: StatusCode,
    pub url: Url,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Reply {
    /// The body as text.
    pub fn text(self) -> Result<String, DavError> {
        String::from_utf8(self.body).map_err(|_| crate::error::protocol("body isn't UTF-8"))
    }
}

impl DavClient {
    /// A client for the DAV root `base_url` (for Nextcloud,
    /// `https://<host>/remote.php/dav`), signing in with `credentials`.
    pub fn new(base_url: &str, credentials: Credentials, limits: Limits) -> Result<Self, DavError> {
        let mut base = Url::parse(base_url)
            .map_err(|err| DavError::InvalidUrl(format!("{base_url}: {err}")))?;
        if !matches!(base.scheme(), "http" | "https") {
            return Err(DavError::InvalidUrl(format!("{base_url}: not http(s)")));
        }
        href::canonicalize(&mut base);
        let authorization = basic_auth(&credentials)?;
        Ok(Self {
            http: http_client(&limits)?,
            base,
            authorization,
            limits,
        })
    }

    /// The DAV root this client was made for.
    pub fn base_url(&self) -> &Url {
        &self.base
    }

    /// The limits this client applies.
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// A request with the credentials attached.
    pub(crate) fn request(&self, method: Method, url: &Url) -> RequestBuilder {
        self.http
            .request(method, url.clone())
            .header(header::AUTHORIZATION, self.authorization.clone())
    }

    /// A PROPFIND or REPORT with an XML body and a Depth header.
    pub(crate) fn xml_request(
        &self,
        method: DavMethod,
        url: &Url,
        depth: Depth,
        body: impl Into<reqwest::Body>,
    ) -> RequestBuilder {
        self.request(method.method(), url)
            .header("Depth", depth.header())
            .header(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/xml; charset=utf-8"),
            )
            .body(body)
    }

    /// Sends a request and reads the whole (capped) response, whatever the
    /// status.
    pub(crate) async fn send(&self, request: RequestBuilder) -> Result<Reply, DavError> {
        let response = request.send().await?;
        read(response, self.limits.max_body_bytes).await
    }

    /// Sends a request and fails unless the status is 2xx.
    pub(crate) async fn send_ok(&self, request: RequestBuilder) -> Result<Reply, DavError> {
        let reply = self.send(request).await?;
        check(&reply)?;
        Ok(reply)
    }

    /// A PROPFIND or REPORT whose answer must be a multistatus. Returns the
    /// URL the answer came from with the parsed body.
    pub(crate) async fn multistatus(
        &self,
        method: DavMethod,
        url: &Url,
        depth: Depth,
        body: impl Into<reqwest::Body>,
    ) -> Result<(Url, Multistatus), DavError> {
        let reply = self
            .send_ok(self.xml_request(method, url, depth, body))
            .await?;
        let url = reply.url.clone();
        let multistatus = parse_multistatus(&reply.text()?)?;
        Ok((url, multistatus))
    }
}

/// Builds the HTTP client both [`DavClient`] and [`crate::LoginFlow`] use.
pub(crate) fn http_client(limits: &Limits) -> Result<reqwest::Client, DavError> {
    reqwest::Client::builder()
        .timeout(limits.timeout)
        .connect_timeout(limits.connect_timeout)
        .user_agent(limits.user_agent.as_str())
        .build()
        .map_err(|err| DavError::Network(format!("building the HTTP client: {err}")))
}

/// The `Authorization: Basic` value for `credentials`, marked sensitive so
/// it never shows in debug output.
fn basic_auth(credentials: &Credentials) -> Result<HeaderValue, DavError> {
    let token = STANDARD.encode(format!("{}:{}", credentials.user, credentials.password));
    let mut value = HeaderValue::try_from(format!("Basic {token}"))
        .map_err(|_| DavError::InvalidUrl("credentials can't be sent".into()))?;
    value.set_sensitive(true);
    Ok(value)
}

/// Reads a response body, stopping with [`DavError::TooLarge`] past `limit`
/// bytes so an endless body can't exhaust memory.
pub(crate) async fn read(mut response: reqwest::Response, limit: usize) -> Result<Reply, DavError> {
    let declared = response.content_length().unwrap_or(0);
    if declared > limit as u64 {
        return Err(DavError::TooLarge { limit });
    }
    let status = response.status();
    let url = response.url().clone();
    let headers = std::mem::take(response.headers_mut());
    let mut body = Vec::with_capacity(declared as usize);
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > limit {
            return Err(DavError::TooLarge { limit });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Reply {
        status,
        url,
        headers,
        body,
    })
}

/// The error for an unsuccessful status, or nothing for 2xx.
pub(crate) fn check(reply: &Reply) -> Result<(), DavError> {
    match reply.status {
        status if status.is_success() => Ok(()),
        StatusCode::UNAUTHORIZED => Err(DavError::AuthRefused),
        StatusCode::NOT_FOUND | StatusCode::GONE => Err(DavError::NotFound),
        StatusCode::PRECONDITION_FAILED => Err(DavError::PreconditionFailed),
        status => Err(DavError::Status(status.as_u16())),
    }
}

/// The `ETag` header of a reply, if any.
pub(crate) fn etag_header(reply: &Reply) -> Option<String> {
    reply
        .headers
        .get(header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}
