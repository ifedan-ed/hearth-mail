//! The XML request bodies the client sends.
//!
//! Fixed bodies are constants (built once, at compile time); the rest are
//! written into one `String` sized up front. Every value from outside
//! (tokens, hrefs, names) is XML-escaped.

use std::fmt::Write;

use quick_xml::escape::escape;
use reqwest::Url;

use crate::discovery::{CollectionKind, Components};

/// PROPFIND for discovery. Asks for the home sets along with the principal,
/// so a server that answers them at the DAV root needs only this one request
/// (RFC 5397 section 3, RFC 4791 section 6.2.1, RFC 6352 section 7.1.1).
pub(crate) const PROPFIND_PRINCIPAL: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:cr="urn:ietf:params:xml:ns:carddav">
<d:prop><d:current-user-principal/><c:calendar-home-set/><cr:addressbook-home-set/></d:prop>
</d:propfind>"#;

/// PROPFIND for the home sets on a principal.
pub(crate) const PROPFIND_HOMES: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:cr="urn:ietf:params:xml:ns:carddav">
<d:prop><c:calendar-home-set/><cr:addressbook-home-set/></d:prop>
</d:propfind>"#;

/// PROPFIND (depth 1) for the collections in a home: everything a folder list
/// needs in one request.
pub(crate) const PROPFIND_COLLECTIONS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:cs="http://calendarserver.org/ns/" xmlns:ic="http://apple.com/ns/ical/">
<d:prop><d:resourcetype/><d:displayname/><c:supported-calendar-component-set/><cs:getctag/><d:sync-token/><ic:calendar-color/><d:current-user-privilege-set/></d:prop>
</d:propfind>"#;

/// PROPFIND (depth 0) for one resource's ETag.
pub(crate) const PROPFIND_ETAG: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:"><d:prop><d:getetag/></d:prop></d:propfind>"#;

/// PROPFIND (depth 0) for a collection's change markers, the cheap check a
/// waiting Ping repeats.
pub(crate) const PROPFIND_TOKENS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:cs="http://calendarserver.org/ns/">
<d:prop><d:sync-token/><cs:getctag/></d:prop></d:propfind>"#;

/// A `sync-collection` report from `token` (empty for the first sync), with
/// members' ETags (RFC 6578 section 3.2).
pub(crate) fn sync_collection(token: &str) -> String {
    let token = escape(token);
    let mut body = String::with_capacity(200 + token.len());
    body.push_str(r#"<?xml version="1.0" encoding="utf-8"?><d:sync-collection xmlns:d="DAV:">"#);
    if token.is_empty() {
        body.push_str("<d:sync-token/>");
    } else {
        // Writing into a String cannot fail.
        let _ = write!(body, "<d:sync-token>{token}</d:sync-token>");
    }
    body.push_str(
        "<d:sync-level>1</d:sync-level><d:prop><d:getetag/></d:prop></d:sync-collection>",
    );
    body
}

/// A `calendar-multiget` (RFC 4791 section 7.9) or `addressbook-multiget`
/// (RFC 6352 section 8.7) report for `hrefs`, asking for ETags and data.
pub(crate) fn multiget(kind: CollectionKind, hrefs: &[Url]) -> String {
    let (root, ns, data) = match kind {
        CollectionKind::Calendar => (
            "c:calendar-multiget",
            "urn:ietf:params:xml:ns:caldav",
            "c:calendar-data",
        ),
        CollectionKind::AddressBook => (
            "c:addressbook-multiget",
            "urn:ietf:params:xml:ns:carddav",
            "c:address-data",
        ),
    };
    let size = 200
        + hrefs
            .iter()
            .map(|href| href.path().len() + 16)
            .sum::<usize>();
    let mut body = String::with_capacity(size);
    let _ = write!(
        body,
        r#"<?xml version="1.0" encoding="utf-8"?><{root} xmlns:d="DAV:" xmlns:c="{ns}"><d:prop><d:getetag/><{data}/></d:prop>"#
    );
    for href in hrefs {
        let _ = write!(body, "<d:href>{}</d:href>", escape(href.path()));
    }
    let _ = write!(body, "</{root}>");
    body
}

/// An extended MKCALENDAR body (RFC 4791 section 5.3.1) naming the calendar
/// and the components it holds.
pub(crate) fn mkcalendar(display_name: &str, components: Components) -> String {
    let mut body = String::with_capacity(400 + display_name.len());
    let _ = write!(
        body,
        r#"<?xml version="1.0" encoding="utf-8"?><c:mkcalendar xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:set><d:prop><d:displayname>{}</d:displayname><c:supported-calendar-component-set>"#,
        escape(display_name)
    );
    for (wanted, name) in [
        (components.event, "VEVENT"),
        (components.todo, "VTODO"),
        (components.journal, "VJOURNAL"),
    ] {
        if wanted {
            let _ = write!(body, r#"<c:comp name="{name}"/>"#);
        }
    }
    body.push_str("</c:supported-calendar-component-set></d:prop></d:set></c:mkcalendar>");
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml::dom::{Ns, parse};

    #[test]
    fn bodies_are_well_formed_and_escaped() {
        let sync = parse(&sync_collection("tok<&>")).unwrap();
        assert_eq!(sync.child(Ns::Dav, "sync-token").unwrap().text, "tok<&>");
        let first = parse(&sync_collection("")).unwrap();
        assert_eq!(first.child(Ns::Dav, "sync-token").unwrap().text, "");

        let hrefs = [Url::parse("https://h/cal/a%20b.ics").unwrap()];
        let get = parse(&multiget(CollectionKind::AddressBook, &hrefs)).unwrap();
        assert!(get.is(Ns::CardDav, "addressbook-multiget"));
        assert_eq!(get.child(Ns::Dav, "href").unwrap().text, "/cal/a%20b.ics");

        let make = parse(&mkcalendar("Notes & more", Components::JOURNAL)).unwrap();
        let prop = make
            .child(Ns::Dav, "set")
            .and_then(|set| set.child(Ns::Dav, "prop"))
            .unwrap();
        assert_eq!(
            prop.child(Ns::Dav, "displayname").unwrap().text,
            "Notes & more"
        );
        let comps = prop
            .child(Ns::CalDav, "supported-calendar-component-set")
            .unwrap();
        assert_eq!(comps.children.len(), 1);
        assert_eq!(comps.children[0].attribute("name"), Some("VJOURNAL"));

        for fixed in [
            PROPFIND_PRINCIPAL,
            PROPFIND_HOMES,
            PROPFIND_COLLECTIONS,
            PROPFIND_ETAG,
            PROPFIND_TOKENS,
        ] {
            assert!(parse(fixed).unwrap().is(Ns::Dav, "propfind"));
        }
    }
}
