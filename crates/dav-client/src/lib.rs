//! A CalDAV and CardDAV client for the ActiveSync bridge (docs/activesync.md,
//! "the DAV bridge").
//!
//! It speaks plain WebDAV with the CalDAV (RFC 4791), CardDAV (RFC 6352),
//! principal (RFC 5397) and collection-sync (RFC 6578) extensions, so the
//! same code works against Nextcloud now and Hearth later. Nextcloud's Login
//! Flow v2, which hands out the app password the bridge signs in with, is
//! here too because it is the only Nextcloud-specific piece.
//!
//! The pieces:
//! - [`DavClient`]: one signed-in connection to a DAV root. Discovery
//!   ([`DavClient::discover`], [`DavClient::list_collections`]), changes
//!   ([`DavClient::sync_collection`]), reads ([`DavClient::multiget`],
//!   [`DavClient::get`]) and writes ([`DavClient::put`],
//!   [`DavClient::delete`], [`DavClient::mkcalendar`]).
//! - [`LoginFlow`]: Nextcloud's Login Flow v2.
//! - [`DavError`]: every failure, typed so the caller can act on it (resync
//!   on [`DavError::InvalidSyncToken`], refetch on
//!   [`DavError::PreconditionFailed`], ask for a new password on
//!   [`DavError::AuthRefused`]).
//!
//! Resources and collections are named by absolute [`Url`]s whose paths are
//! in one canonical percent-encoding (see [`href`]), so an href read from
//! one response compares equal to the same href read from another.

mod client;
mod discovery;
mod error;
mod fetch;
pub mod href;
mod login_flow;
#[cfg(test)]
mod rfc_examples;
mod sync;
mod write;
mod xml;

pub use client::{Credentials, DavClient, Limits};
pub use discovery::{ChangeMarkers, Collection, CollectionKind, Components, Homes};
pub use error::DavError;
pub use fetch::{Multiget, Resource};
pub use login_flow::{LoginFlow, LoginFlowStart, LoginPoll, NextcloudLogin};
pub use reqwest::Url;
pub use sync::{Member, SyncChanges};
pub use write::Condition;
