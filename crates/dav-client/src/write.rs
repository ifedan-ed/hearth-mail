//! Writing: PUT and DELETE guarded by ETags, and MKCALENDAR.
//!
//! Every write is conditional, so an edit made on a phone never overwrites
//! one made in Nextcloud meanwhile: the server answers 412
//! ([`DavError::PreconditionFailed`]) and the caller refetches.

use reqwest::{Method, Url, header};

use crate::{
    CollectionKind, Components, DavClient, DavError,
    client::{DavMethod, etag_header},
    error::protocol,
    xml::request,
};

/// The condition a PUT is made under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Condition<'a> {
    /// The resource must not exist yet (`If-None-Match: *`).
    Create,
    /// The resource must still have this ETag (`If-Match`).
    Update(&'a str),
}

impl DavClient {
    /// Writes `body` (iCalendar or vCard text) to `href` under `condition`
    /// and returns the resource's new ETag. When the server sends none (it
    /// may not, if it changed the data while storing it), the ETag is read
    /// with a PROPFIND.
    pub async fn put(
        &self,
        href: &Url,
        kind: CollectionKind,
        body: String,
        condition: Condition<'_>,
    ) -> Result<String, DavError> {
        let request = self
            .request(Method::PUT, href)
            .header(header::CONTENT_TYPE, kind.content_type())
            .body(body);
        let request = match condition {
            Condition::Create => request.header(header::IF_NONE_MATCH, "*"),
            Condition::Update(etag) => request.header(header::IF_MATCH, etag),
        };
        let reply = self.send_ok(request).await?;
        if let Some(etag) = etag_header(&reply) {
            return Ok(etag);
        }
        let (_, response) = self.propfind_one(href, request::PROPFIND_ETAG).await?;
        response
            .props
            .etag
            .ok_or_else(|| protocol("no ETag for a resource just written"))
    }

    /// Deletes `href`, only if it still has `etag` when one is given.
    pub async fn delete(&self, href: &Url, etag: Option<&str>) -> Result<(), DavError> {
        let mut request = self.request(Method::DELETE, href);
        if let Some(etag) = etag {
            request = request.header(header::IF_MATCH, etag);
        }
        self.send_ok(request).await.map(drop)
    }

    /// Creates a calendar at `href` named `display_name` holding
    /// `components` (RFC 4791 section 5.3.1). The bridge uses it for a
    /// VJOURNAL "Notes" calendar when none exists.
    pub async fn mkcalendar(
        &self,
        href: &Url,
        display_name: &str,
        components: Components,
    ) -> Result<(), DavError> {
        let request = self
            .request(DavMethod::Mkcalendar.method(), href)
            .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
            .body(request::mkcalendar(display_name, components));
        self.send_ok(request).await.map(drop)
    }
}
