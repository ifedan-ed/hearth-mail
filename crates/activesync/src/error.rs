/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! How a command fails. Most failures an ActiveSync client can act on are
//! answered inside the WBXML document with a status code (a sync key the
//! server doesn't know, a folder that's gone), and commands build those
//! answers themselves; [`Error`] is for what ends the request: a body that
//! can't be read (HTTP 400), a plain HTTP status, or a fault on the server
//! (HTTP 500, logged).

use hyper::StatusCode;
use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// The request can't be understood; answered 400.
    BadRequest(String),
    /// Answered with this HTTP status and no body.
    Http(StatusCode),
    /// A JMAP method refused (its error type and description).
    Jmap(String),
    /// The server failed.
    Server(trc::Error),
    /// The server failed in a way that has only a description.
    Internal(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::BadRequest(reason) => write!(f, "bad request: {reason}"),
            Error::Http(status) => write!(f, "HTTP {status}"),
            Error::Jmap(reason) => write!(f, "JMAP: {reason}"),
            Error::Server(err) => write!(f, "{err}"),
            Error::Internal(reason) => f.write_str(reason),
        }
    }
}

impl From<trc::Error> for Error {
    fn from(err: trc::Error) -> Self {
        Error::Server(err)
    }
}
