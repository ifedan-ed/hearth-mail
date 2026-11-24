//! Resolving and normalising hrefs.
//!
//! Servers write the same path in different ways: relative or absolute,
//! `@` raw or as `%40`, a space as `%20` or (wrongly) raw. The bridge keeps
//! hrefs as keys (which items a device has), so every href is resolved
//! against the URL of the response it came in and its path re-encoded one
//! way: each segment percent-decoded, then encoded again with everything but
//! RFC 3986's unreserved characters escaped. That is what sabre/dav
//! (Nextcloud) writes itself (PHP's `rawurlencode`), so its hrefs come back
//! unchanged.

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use reqwest::Url;

use crate::DavError;

/// Everything but RFC 3986's unreserved characters (ALPHA, DIGIT, `-._~`).
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// Resolves `href` (as found in a response, or given by a caller) against
/// `base` and returns it with its path in canonical form, without a query or
/// fragment.
pub fn resolve(base: &Url, href: &str) -> Result<Url, DavError> {
    let mut url = base
        .join(href.trim())
        .map_err(|err| DavError::InvalidUrl(format!("{href}: {err}")))?;
    canonicalize(&mut url);
    Ok(url)
}

/// Rewrites `url`'s path in canonical percent-encoding and drops its query
/// and fragment, which never name a DAV resource.
pub fn canonicalize(url: &mut Url) {
    let path = canonical_path(url.path());
    if path != url.path() {
        url.set_path(&path);
    }
    url.set_query(None);
    url.set_fragment(None);
}

/// `path` with each segment decoded and re-encoded with [`SEGMENT`].
fn canonical_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for (i, segment) in path.split('/').enumerate() {
        if i > 0 {
            out.push('/');
        }
        let decoded = percent_decode_str(segment).collect::<Vec<u8>>();
        // A segment that isn't UTF-8 once decoded is encoded byte by byte.
        match std::str::from_utf8(&decoded) {
            Ok(text) => out.extend(utf8_percent_encode(text, SEGMENT)),
            Err(_) => out.extend(percent_encoding::percent_encode(&decoded, SEGMENT)),
        }
    }
    out
}

/// Whether `a` and `b` name the same collection, ignoring a trailing slash
/// (servers answer `/cal` for a request to `/cal/` and back).
pub(crate) fn same_collection(a: &Url, b: &Url) -> bool {
    a.as_str().trim_end_matches('/') == b.as_str().trim_end_matches('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://cloud.example/remote.php/dav/calendars/ann/").unwrap()
    }

    #[test]
    fn relative_and_absolute_paths_resolve_the_same() {
        let a = resolve(&base(), "personal/").unwrap();
        let b = resolve(&base(), "/remote.php/dav/calendars/ann/personal/").unwrap();
        let c = resolve(
            &base(),
            "https://cloud.example/remote.php/dav/calendars/ann/personal/",
        )
        .unwrap();
        assert_eq!(a, b);
        assert_eq!(b, c);
        assert_eq!(a.path(), "/remote.php/dav/calendars/ann/personal/");
    }

    #[test]
    fn encodings_of_one_path_are_equal() {
        let raw = resolve(&base(), "/dav/a@b.example/x y.ics").unwrap();
        let encoded = resolve(&base(), "/dav/a%40b.example/x%20y.ics").unwrap();
        let lower = resolve(&base(), "/dav/a%40b.example/x%20y.ics?x=1#f").unwrap();
        assert_eq!(raw, encoded);
        assert_eq!(encoded, lower);
        assert_eq!(encoded.path(), "/dav/a%40b.example/x%20y.ics");
    }

    #[test]
    fn unicode_and_reserved_characters_are_escaped_once() {
        let url = resolve(&base(), "/dav/caf%C3%A9/a%2Fb+c.vcf").unwrap();
        assert_eq!(url.path(), "/dav/caf%C3%A9/a%2Fb%2Bc.vcf");
        let again = resolve(&base(), url.as_str()).unwrap();
        assert_eq!(url, again);
    }

    #[test]
    fn trailing_slash_is_ignored_for_collections() {
        let a = resolve(&base(), "/x/cal/").unwrap();
        let b = resolve(&base(), "/x/cal").unwrap();
        assert!(same_collection(&a, &b));
    }
}
