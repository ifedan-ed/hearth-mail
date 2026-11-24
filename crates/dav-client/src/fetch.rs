//! Reading members: many at once with a multiget report (RFC 4791 section
//! 7.9, RFC 6352 section 8.7), or one with GET.

use std::collections::HashSet;

use reqwest::{Method, Url};

use crate::{
    CollectionKind, DavClient, DavError,
    client::{DavMethod, Depth, etag_header},
    href,
    xml::{multistatus::Multistatus, request},
};

/// A calendar object or vCard with its ETag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    /// The resource's URL (canonical, see [`href`]).
    pub href: Url,
    /// Its ETag, for a later `If-Match`.
    pub etag: Option<String>,
    /// The iCalendar or vCard text, line ends as the server sent them.
    pub data: String,
}

/// The answer to a multiget.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Multiget {
    /// The resources that were found.
    pub found: Vec<Resource>,
    /// The hrefs asked for that the server doesn't have (any more).
    pub missing: Vec<Url>,
}

impl DavClient {
    /// Fetches `hrefs` from `collection`, in batches of
    /// [`crate::Limits::multiget_batch`] per request.
    pub async fn multiget(
        &self,
        collection: &Url,
        kind: CollectionKind,
        hrefs: &[Url],
    ) -> Result<Multiget, DavError> {
        let mut result = Multiget {
            found: Vec::with_capacity(hrefs.len()),
            missing: Vec::new(),
        };
        for batch in hrefs.chunks(self.limits().multiget_batch.max(1)) {
            self.multiget_batch(collection, kind, batch, &mut result)
                .await?;
        }
        Ok(result)
    }

    /// One multiget request, its results added to `result`.
    async fn multiget_batch(
        &self,
        collection: &Url,
        kind: CollectionKind,
        hrefs: &[Url],
        result: &mut Multiget,
    ) -> Result<(), DavError> {
        let (url, multistatus) = self
            .multistatus(
                DavMethod::Report,
                collection,
                Depth::One,
                request::multiget(kind, hrefs),
            )
            .await?;
        read_multiget(&url, multistatus, hrefs, result)
    }

    /// Fetches one resource with GET.
    pub async fn get(&self, href: &Url) -> Result<Resource, DavError> {
        let reply = self.send_ok(self.request(Method::GET, href)).await?;
        let etag = etag_header(&reply);
        Ok(Resource {
            href: href.clone(),
            etag,
            data: reply.text()?,
        })
    }
}

/// Sorts a multiget answer for `hrefs` into found and missing.
pub(crate) fn read_multiget(
    url: &Url,
    multistatus: Multistatus,
    hrefs: &[Url],
    result: &mut Multiget,
) -> Result<(), DavError> {
    let mut answered = HashSet::with_capacity(hrefs.len());
    for response in multistatus.responses {
        let Some(first) = response.hrefs.first() else {
            continue;
        };
        let href = href::resolve(url, first)?;
        answered.insert(href.clone());
        match (response.status, response.props.data) {
            (None | Some(200..=299), Some(data)) => result.found.push(Resource {
                href,
                etag: response.props.etag,
                data,
            }),
            // A 404 status, or a 404 propstat for the data: not there.
            _ => result.missing.push(href),
        }
    }
    // A server that leaves an href out has nothing for it either.
    result.missing.extend(
        hrefs
            .iter()
            .filter(|href| !answered.contains(*href))
            .cloned(),
    );
    Ok(())
}
