//! The client's errors.
//!
//! Each variant is something the caller does something different about, so
//! they are kept apart rather than folded into a status code: a refused
//! sign-in asks the person to link again, an invalid sync token restarts the
//! folder from scratch, a failed precondition means someone else changed the
//! item first.

use std::fmt;

/// Why a DAV request failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DavError {
    /// The server refused the credentials (401).
    AuthRefused,
    /// The resource or collection does not exist (404, or 410 Gone).
    NotFound,
    /// An `If-Match` or `If-None-Match` condition failed (412): the item
    /// changed (or already exists) on the server.
    PreconditionFailed,
    /// The sync token given to `sync-collection` is unknown or expired
    /// (RFC 6578 section 3.2, the `DAV:valid-sync-token` precondition). The
    /// caller starts again from an empty token.
    InvalidSyncToken,
    /// The response body was larger than [`crate::Limits::max_body_bytes`].
    TooLarge {
        /// The limit that was exceeded, in bytes.
        limit: usize,
    },
    /// The request did not finish within the configured timeout.
    Timeout,
    /// The connection failed (DNS, TCP, TLS, a reset).
    Network(String),
    /// The server answered something this client can't read: malformed XML,
    /// a missing element the protocol requires, an unexpected JSON shape.
    Protocol(String),
    /// Any other unsuccessful HTTP status.
    Status(u16),
    /// A URL given to the client, or an href it can't resolve.
    InvalidUrl(String),
}

impl fmt::Display for DavError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AuthRefused => f.write_str("the DAV server refused the credentials"),
            Self::NotFound => f.write_str("not found on the DAV server"),
            Self::PreconditionFailed => f.write_str("the item changed on the DAV server"),
            Self::InvalidSyncToken => f.write_str("the sync token is no longer valid"),
            Self::TooLarge { limit } => write!(f, "the DAV response is over {limit} bytes"),
            Self::Timeout => f.write_str("the DAV server did not answer in time"),
            Self::Network(why) => write!(f, "DAV connection failed: {why}"),
            Self::Protocol(why) => write!(f, "unreadable DAV response: {why}"),
            Self::Status(status) => write!(f, "the DAV server answered HTTP {status}"),
            Self::InvalidUrl(why) => write!(f, "invalid DAV URL: {why}"),
        }
    }
}

impl std::error::Error for DavError {}

impl From<reqwest::Error> for DavError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            Self::Timeout
        } else {
            // The chain carries the useful part ("connection refused").
            let mut why = err.to_string();
            let mut source = std::error::Error::source(&err);
            while let Some(inner) = source {
                why.push_str(": ");
                why.push_str(&inner.to_string());
                source = inner.source();
            }
            Self::Network(why)
        }
    }
}

/// A protocol error with `why`.
pub(crate) fn protocol(why: impl Into<String>) -> DavError {
    DavError::Protocol(why.into())
}
