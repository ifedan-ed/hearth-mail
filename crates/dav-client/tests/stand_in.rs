//! The client against a stand-in server that checks each request (method,
//! Depth, conditions, body) and answers the way Nextcloud (sabre/dav) does:
//! `d:` prefixes, extra namespaces, relative hrefs, its error bodies.

mod support;

use std::time::Duration;

use dav_client::{
    CollectionKind, Components, Condition, Credentials, DavClient, DavError, Limits, LoginFlow,
    LoginPoll, Url,
};
use support::{Answer, Recorded, StandIn};

const NS: &str = r#"xmlns:d="DAV:" xmlns:s="http://sabredav.org/ns" xmlns:cal="urn:ietf:params:xml:ns:caldav" xmlns:cs="http://calendarserver.org/ns/" xmlns:oc="http://owncloud.org/ns" xmlns:nc="http://nextcloud.org/ns" xmlns:card="urn:ietf:params:xml:ns:carddav" xmlns:x1="http://apple.com/ns/ical/""#;

fn client(server: &StandIn) -> DavClient {
    client_with(server, Limits::default())
}

fn client_with(server: &StandIn, limits: Limits) -> DavClient {
    DavClient::new(
        &server.url("/remote.php/dav"),
        Credentials {
            user: "ann".into(),
            password: "app-pass word".into(),
        },
        limits,
    )
    .unwrap()
}

fn url(server: &StandIn, path: &str) -> Url {
    Url::parse(&server.url(path)).unwrap()
}

/// What every request to the DAV root must carry.
fn assert_signed_in(request: &Recorded) {
    // base64("ann:app-pass word")
    assert_eq!(
        request.header("authorization"),
        Some("Basic YW5uOmFwcC1wYXNzIHdvcmQ=")
    );
    assert!(
        request
            .header("user-agent")
            .unwrap()
            .starts_with("dav-client/")
    );
}

#[tokio::test]
async fn discovery_asks_the_root_then_the_principal() {
    let server = StandIn::start(|request| match request.path.as_str() {
        "/remote.php/dav" => Answer::multistatus(&format!(
            r#"<?xml version="1.0"?>
<d:multistatus {NS}><d:response><d:href>/remote.php/dav/</d:href><d:propstat><d:prop><d:current-user-principal><d:href>/remote.php/dav/principals/users/ann/</d:href></d:current-user-principal></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat><d:propstat><d:prop><cal:calendar-home-set/><card:addressbook-home-set/></d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat></d:response></d:multistatus>"#
        )),
        "/remote.php/dav/principals/users/ann/" => Answer::multistatus(&format!(
            r#"<?xml version="1.0"?>
<d:multistatus {NS}><d:response><d:href>/remote.php/dav/principals/users/ann/</d:href><d:propstat><d:prop><cal:calendar-home-set><d:href>/remote.php/dav/calendars/ann/</d:href></cal:calendar-home-set><card:addressbook-home-set><d:href>/remote.php/dav/addressbooks/users/ann/</d:href></card:addressbook-home-set></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#
        )),
        _ => Answer::new(404, ""),
    })
    .await;
    let homes = client(&server).discover().await.unwrap();
    assert_eq!(
        homes.principal,
        url(&server, "/remote.php/dav/principals/users/ann/")
    );
    assert_eq!(
        homes.calendar_home,
        Some(url(&server, "/remote.php/dav/calendars/ann/"))
    );
    assert_eq!(
        homes.addressbook_home,
        Some(url(&server, "/remote.php/dav/addressbooks/users/ann/"))
    );

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert_eq!(request.method, "PROPFIND");
        assert_eq!(request.header("depth"), Some("0"));
        assert_signed_in(request);
    }
    assert!(requests[0].body.contains("current-user-principal"));
    assert!(requests[1].body.contains("calendar-home-set"));
    assert!(requests[1].body.contains("addressbook-home-set"));
}

#[tokio::test]
async fn discovery_takes_one_request_when_the_root_has_the_homes() {
    let server = StandIn::start(|_| {
        Answer::multistatus(&format!(
            r#"<d:multistatus {NS}><d:response><d:href>/dav/</d:href><d:propstat><d:prop><d:current-user-principal><d:href>/dav/principals/ann/</d:href></d:current-user-principal><cal:calendar-home-set><d:href>/dav/cal/ann/</d:href></cal:calendar-home-set></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#
        ))
    })
    .await;
    let homes = client(&server).discover().await.unwrap();
    assert_eq!(homes.calendar_home, Some(url(&server, "/dav/cal/ann/")));
    assert_eq!(homes.addressbook_home, None);
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn collections_are_listed_like_nextcloud_shows_them() {
    let server = StandIn::start(|_| {
        Answer::multistatus(&format!(
            r##"<?xml version="1.0"?>
<d:multistatus {NS}>
 <d:response><d:href>/remote.php/dav/calendars/ann/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
 <d:response><d:href>/remote.php/dav/calendars/ann/personal/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><cal:calendar/></d:resourcetype><d:displayname>Personal</d:displayname><cal:supported-calendar-component-set><cal:comp name="VEVENT"/><cal:comp name="VTODO"/></cal:supported-calendar-component-set><cs:getctag>http://sabre.io/ns/sync/3</cs:getctag><d:sync-token>http://sabre.io/ns/sync/3</d:sync-token><x1:calendar-color>#0082c9</x1:calendar-color><d:current-user-privilege-set><d:privilege><d:write/></d:privilege><d:privilege><d:read/></d:privilege></d:current-user-privilege-set></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
 <d:response><d:href>/remote.php/dav/calendars/ann/contact_birthdays/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><cal:calendar/></d:resourcetype><d:displayname>Contact birthdays</d:displayname><cal:supported-calendar-component-set><cal:comp name="VEVENT"/></cal:supported-calendar-component-set><d:current-user-privilege-set><d:privilege><d:read/></d:privilege></d:current-user-privilege-set></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat><d:propstat><d:prop><x1:calendar-color/></d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat></d:response>
 <d:response><d:href>/remote.php/dav/calendars/ann/inbox/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><cal:schedule-inbox/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
 <d:response><d:href>/remote.php/dav/calendars/ann/trashbin/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><nc:trash-bin/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
</d:multistatus>"##
        ))
    })
    .await;
    let home = url(&server, "/remote.php/dav/calendars/ann/");
    let collections = client(&server).list_collections(&home).await.unwrap();
    assert_eq!(collections.len(), 2);
    let personal = &collections[0];
    assert_eq!(
        personal.href,
        url(&server, "/remote.php/dav/calendars/ann/personal/")
    );
    assert_eq!(personal.kind, CollectionKind::Calendar);
    assert_eq!(personal.display_name.as_deref(), Some("Personal"));
    assert!(personal.components.event && personal.components.todo);
    assert!(!personal.components.journal);
    assert_eq!(
        personal.sync_token.as_deref(),
        Some("http://sabre.io/ns/sync/3")
    );
    assert_eq!(personal.ctag.as_deref(), Some("http://sabre.io/ns/sync/3"));
    assert_eq!(personal.color.as_deref(), Some("#0082c9"));
    assert_eq!(personal.writable, Some(true));
    assert_eq!(collections[1].writable, Some(false));
    assert_eq!(collections[1].color, None);

    let request = &server.requests()[0];
    assert_eq!(request.method, "PROPFIND");
    assert_eq!(request.header("depth"), Some("1"));
    for property in [
        "resourcetype",
        "displayname",
        "supported-calendar-component-set",
        "getctag",
        "sync-token",
        "calendar-color",
        "current-user-privilege-set",
    ] {
        assert!(request.body.contains(property), "asks for {property}");
    }
}

/// A sync page answering `members` (href, etag) and `removed`, with
/// `token`, truncated when `more`.
fn sync_page(members: &[(&str, &str)], removed: &[&str], token: &str, more: bool) -> Answer {
    let mut body = format!("<?xml version=\"1.0\"?>\n<d:multistatus {NS}>");
    for (href, etag) in members {
        body.push_str(&format!(
            "<d:response><d:href>/remote.php/dav/calendars/ann/personal/{href}</d:href><d:propstat><d:prop><d:getetag>&quot;{etag}&quot;</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"
        ));
    }
    for href in removed {
        body.push_str(&format!(
            "<d:response><d:href>/remote.php/dav/calendars/ann/personal/{href}</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>"
        ));
    }
    if more {
        body.push_str("<d:response><d:href>/remote.php/dav/calendars/ann/personal/</d:href><d:status>HTTP/1.1 507 Insufficient Storage</d:status><d:error><d:number-of-matches-within-limits/></d:error></d:response>");
    }
    body.push_str(&format!(
        "<d:sync-token>{token}</d:sync-token></d:multistatus>"
    ));
    Answer::multistatus(&body)
}

/// The token a sync-collection request carries.
fn sent_token(request: &Recorded) -> String {
    let body = &request.body;
    match (body.find("<d:sync-token>"), body.find("</d:sync-token>")) {
        (Some(start), Some(end)) => body[start + 14..end].to_owned(),
        _ => String::new(),
    }
}

#[tokio::test]
async fn sync_follows_truncated_pages_and_merges_them() {
    let server = StandIn::start(|request| match sent_token(request).as_str() {
        "" => sync_page(&[("a.ics", "1"), ("b.ics", "1")], &[], "tok/1", true),
        "tok/1" => sync_page(
            &[("c.ics", "1"), ("a.ics", "2")],
            &["b.ics"],
            "tok/2",
            false,
        ),
        "tok/2" => sync_page(&[], &["c.ics"], "tok/3", false),
        _ => Answer::new(500, ""),
    })
    .await;
    let dav = client(&server);
    let collection = url(&server, "/remote.php/dav/calendars/ann/personal/");
    let first = dav.sync_collection(&collection, "").await.unwrap();
    assert_eq!(first.token, "tok/2");
    let mut changed: Vec<(String, Option<String>)> = first
        .changed
        .iter()
        .map(|member| (member.href.path().to_owned(), member.etag.clone()))
        .collect();
    changed.sort();
    assert_eq!(
        changed,
        [
            (
                "/remote.php/dav/calendars/ann/personal/a.ics".to_owned(),
                Some("\"2\"".to_owned())
            ),
            (
                "/remote.php/dav/calendars/ann/personal/c.ics".to_owned(),
                Some("\"1\"".to_owned())
            ),
        ]
    );
    assert_eq!(
        first.removed,
        [url(&server, "/remote.php/dav/calendars/ann/personal/b.ics")]
    );

    let next = dav
        .sync_collection(&collection, &first.token)
        .await
        .unwrap();
    assert!(next.changed.is_empty());
    assert_eq!(next.removed.len(), 1);
    assert_eq!(next.token, "tok/3");

    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    for request in &requests {
        assert_eq!(request.method, "REPORT");
        assert_eq!(request.header("depth"), Some("0"));
        assert!(request.body.contains("<d:sync-level>1</d:sync-level>"));
        assert!(request.body.contains("<d:getetag/>"));
    }
    assert!(requests[0].body.contains("<d:sync-token/>"));
}

#[tokio::test]
async fn sync_gives_up_on_a_server_that_never_finishes() {
    let server = StandIn::start(|_| sync_page(&[("a.ics", "1")], &[], "same", true)).await;
    let limits = Limits {
        max_sync_pages: 3,
        ..Limits::default()
    };
    let collection = url(&server, "/remote.php/dav/calendars/ann/personal/");
    let err = client_with(&server, limits)
        .sync_collection(&collection, "")
        .await
        .unwrap_err();
    assert!(matches!(err, DavError::Protocol(_)), "{err:?}");
    assert_eq!(server.requests().len(), 3);
}

#[tokio::test]
async fn an_unknown_sync_token_is_its_own_error() {
    let server = StandIn::start(|_| {
        Answer::new(
            403,
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:error xmlns:d="DAV:" xmlns:s="http://sabredav.org/ns">
  <s:exception>Sabre\DAV\Exception\InvalidSyncToken</s:exception>
  <s:message>Invalid or unknown sync token</s:message>
  <d:valid-sync-token/>
</d:error>"#,
        )
        .header("Content-Type", "application/xml; charset=utf-8")
    })
    .await;
    let collection = url(&server, "/remote.php/dav/calendars/ann/personal/");
    let err = client(&server)
        .sync_collection(&collection, "http://sabre.io/ns/sync/999")
        .await
        .unwrap_err();
    assert_eq!(err, DavError::InvalidSyncToken);

    // A 403 without the precondition is a plain status.
    let server = StandIn::start(|_| Answer::new(403, "")).await;
    let collection = url(&server, "/remote.php/dav/calendars/ann/personal/");
    let err = client(&server)
        .sync_collection(&collection, "x")
        .await
        .unwrap_err();
    assert_eq!(err, DavError::Status(403));
}

/// Answers a multiget from the hrefs in its body: every href but
/// `gone.ics` is found.
fn answer_multiget(request: &Recorded) -> Answer {
    let mut body = format!("<?xml version=\"1.0\"?>\n<d:multistatus {NS}>");
    let mut rest = request.body.as_str();
    while let Some(start) = rest.find("<d:href>") {
        let end = rest.find("</d:href>").unwrap();
        let href = &rest[start + 8..end];
        rest = &rest[end + 9..];
        if href.ends_with("gone.ics") {
            body.push_str(&format!(
                "<d:response><d:href>{href}</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>"
            ));
        } else {
            body.push_str(&format!(
                "<d:response><d:href>{href}</d:href><d:propstat><d:prop><d:getetag>&quot;e-{href}&quot;</d:getetag><cal:calendar-data>BEGIN:VCALENDAR&#13;\nUID:{href}&#13;\nEND:VCALENDAR&#13;\n</cal:calendar-data></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"
            ));
        }
    }
    body.push_str("</d:multistatus>");
    Answer::multistatus(&body)
}

#[tokio::test]
async fn multiget_asks_in_batches() {
    let server = StandIn::start(answer_multiget).await;
    let limits = Limits {
        multiget_batch: 2,
        ..Limits::default()
    };
    let dav = client_with(&server, limits);
    let collection = url(&server, "/remote.php/dav/calendars/ann/personal/");
    let hrefs: Vec<Url> = ["a.ics", "b%20c.ics", "gone.ics"]
        .iter()
        .map(|name| collection.join(name).unwrap())
        .collect();
    let result = dav
        .multiget(&collection, CollectionKind::Calendar, &hrefs)
        .await
        .unwrap();
    assert_eq!(result.found.len(), 2);
    assert_eq!(result.missing, [hrefs[2].clone()]);
    let first = &result.found[0];
    assert_eq!(first.href, hrefs[0]);
    assert_eq!(
        first.etag.as_deref(),
        Some("\"e-/remote.php/dav/calendars/ann/personal/a.ics\"")
    );
    // CRLF as Nextcloud sends it (`&#13;` before each line feed).
    assert_eq!(
        first.data,
        "BEGIN:VCALENDAR\r\nUID:/remote.php/dav/calendars/ann/personal/a.ics\r\nEND:VCALENDAR\r\n"
    );
    assert_eq!(result.found[1].href, hrefs[1]);

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert_eq!(request.method, "REPORT");
        assert_eq!(request.header("depth"), Some("1"));
        assert!(request.body.contains("calendar-multiget"));
        assert!(request.body.contains("<c:calendar-data/>"));
    }
    assert_eq!(requests[0].body.matches("<d:href>").count(), 2);
    assert_eq!(requests[1].body.matches("<d:href>").count(), 1);
    assert!(requests[0].body.contains("/personal/b%20c.ics"));
}

#[tokio::test]
async fn address_books_use_addressbook_multiget() {
    let server = StandIn::start(|_| {
        Answer::multistatus(&format!(
            r#"<d:multistatus {NS}><d:response><d:href>/remote.php/dav/addressbooks/users/ann/contacts/x.vcf</d:href><d:propstat><d:prop><d:getetag>"v1"</d:getetag><card:address-data>BEGIN:VCARD
END:VCARD</card:address-data></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#
        ))
    })
    .await;
    let book = url(&server, "/remote.php/dav/addressbooks/users/ann/contacts/");
    let hrefs = [book.join("x.vcf").unwrap()];
    let result = client(&server)
        .multiget(&book, CollectionKind::AddressBook, &hrefs)
        .await
        .unwrap();
    assert_eq!(result.found[0].data, "BEGIN:VCARD\nEND:VCARD");
    let request = &server.requests()[0];
    assert!(request.body.contains("addressbook-multiget"));
    assert!(request.body.contains("<c:address-data/>"));
}

#[tokio::test]
async fn get_returns_data_and_etag() {
    let server = StandIn::start(|request| {
        assert_eq!(request.method, "GET");
        Answer::new(200, "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n").header("ETag", "\"g1\"")
    })
    .await;
    let href = url(&server, "/remote.php/dav/calendars/ann/personal/a.ics");
    let resource = client(&server).get(&href).await.unwrap();
    assert_eq!(resource.etag.as_deref(), Some("\"g1\""));
    assert_eq!(resource.data, "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n");
}

#[tokio::test]
async fn put_creates_and_updates_under_conditions() {
    let server = StandIn::start(|request| match request.method.as_str() {
        "PUT" if request.header("if-none-match") == Some("*") => {
            Answer::new(201, "").header("ETag", "\"new\"")
        }
        "PUT" if request.header("if-match") == Some("\"new\"") => Answer::new(204, ""),
        "PUT" => Answer::new(412, ""),
        // Nextcloud leaves the ETag out when it changed what was stored.
        "PROPFIND" => Answer::multistatus(&format!(
            r#"<d:multistatus {NS}><d:response><d:href>/remote.php/dav/calendars/ann/personal/a.ics</d:href><d:propstat><d:prop><d:getetag>"after"</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#
        )),
        _ => Answer::new(405, ""),
    })
    .await;
    let dav = client(&server);
    let href = url(&server, "/remote.php/dav/calendars/ann/personal/a.ics");
    let ical = "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n";
    let etag = dav
        .put(
            &href,
            CollectionKind::Calendar,
            ical.into(),
            Condition::Create,
        )
        .await
        .unwrap();
    assert_eq!(etag, "\"new\"");
    let etag = dav
        .put(
            &href,
            CollectionKind::Calendar,
            ical.into(),
            Condition::Update(&etag),
        )
        .await
        .unwrap();
    assert_eq!(etag, "\"after\"");
    let stale = dav
        .put(
            &href,
            CollectionKind::Calendar,
            ical.into(),
            Condition::Update("\"old\""),
        )
        .await
        .unwrap_err();
    assert_eq!(stale, DavError::PreconditionFailed);

    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    assert_eq!(
        requests[0].header("content-type"),
        Some("text/calendar; charset=utf-8")
    );
    assert_eq!(requests[0].body, ical);
    assert_eq!(requests[0].header("if-match"), None);
    assert_eq!(requests[1].header("if-none-match"), None);
    assert_eq!(requests[2].method, "PROPFIND");
    assert_eq!(requests[2].header("depth"), Some("0"));
    assert!(requests[2].body.contains("getetag"));
}

#[tokio::test]
async fn delete_sends_if_match() {
    let server = StandIn::start(|request| match request.header("if-match") {
        Some("\"e1\"") => Answer::new(204, ""),
        Some(_) => Answer::new(412, ""),
        None => Answer::new(404, ""),
    })
    .await;
    let dav = client(&server);
    let href = url(
        &server,
        "/remote.php/dav/addressbooks/users/ann/contacts/x.vcf",
    );
    dav.delete(&href, Some("\"e1\"")).await.unwrap();
    assert_eq!(
        dav.delete(&href, Some("\"e0\"")).await.unwrap_err(),
        DavError::PreconditionFailed
    );
    assert_eq!(
        dav.delete(&href, None).await.unwrap_err(),
        DavError::NotFound
    );
    assert!(server.requests().iter().all(|r| r.method == "DELETE"));
}

#[tokio::test]
async fn mkcalendar_names_the_components() {
    let server = StandIn::start(|_| Answer::new(201, "")).await;
    let href = url(&server, "/remote.php/dav/calendars/ann/notes/");
    client(&server)
        .mkcalendar(&href, "Notes", Components::JOURNAL)
        .await
        .unwrap();
    let request = &server.requests()[0];
    assert_eq!(request.method, "MKCALENDAR");
    assert_eq!(request.path, "/remote.php/dav/calendars/ann/notes/");
    assert!(
        request
            .body
            .contains("<d:displayname>Notes</d:displayname>")
    );
    assert!(request.body.contains(r#"<c:comp name="VJOURNAL"/>"#));
    assert!(!request.body.contains("VEVENT"));
}

#[tokio::test]
async fn change_markers_read_token_and_ctag() {
    let server = StandIn::start(|_| {
        Answer::multistatus(&format!(
            r#"<d:multistatus {NS}><d:response><d:href>/remote.php/dav/calendars/ann/personal/</d:href><d:propstat><d:prop><d:sync-token>http://sabre.io/ns/sync/7</d:sync-token><cs:getctag>http://sabre.io/ns/sync/7</cs:getctag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#
        ))
    })
    .await;
    let collection = url(&server, "/remote.php/dav/calendars/ann/personal/");
    let markers = client(&server).change_markers(&collection).await.unwrap();
    assert_eq!(
        markers.sync_token.as_deref(),
        Some("http://sabre.io/ns/sync/7")
    );
    assert_eq!(markers.ctag.as_deref(), Some("http://sabre.io/ns/sync/7"));
    assert_eq!(server.requests()[0].header("depth"), Some("0"));
}

#[tokio::test]
async fn failures_have_their_own_errors() {
    let server = StandIn::start(|request| match request.path.as_str() {
        "/remote.php/dav/denied" => Answer::new(401, ""),
        "/remote.php/dav/big" => Answer::new(200, &"x".repeat(2048)),
        "/remote.php/dav/slow" => Answer {
            delay: Duration::from_secs(3),
            ..Answer::new(200, "")
        },
        "/remote.php/dav/broken" => Answer::multistatus("<d:multistatus xmlns:d=\"DAV:\">"),
        _ => Answer::new(500, ""),
    })
    .await;
    let limits = Limits {
        max_body_bytes: 1024,
        timeout: Duration::from_millis(500),
        ..Limits::default()
    };
    let dav = client_with(&server, limits);
    let at = |path: &str| url(&server, path);
    assert_eq!(
        dav.get(&at("/remote.php/dav/denied")).await.unwrap_err(),
        DavError::AuthRefused
    );
    assert_eq!(
        dav.get(&at("/remote.php/dav/big")).await.unwrap_err(),
        DavError::TooLarge { limit: 1024 }
    );
    assert_eq!(
        dav.get(&at("/remote.php/dav/slow")).await.unwrap_err(),
        DavError::Timeout
    );
    assert_eq!(
        dav.get(&at("/remote.php/dav/other")).await.unwrap_err(),
        DavError::Status(500)
    );
    let broken = dav
        .change_markers(&at("/remote.php/dav/broken"))
        .await
        .unwrap_err();
    assert!(matches!(broken, DavError::Protocol(_)), "{broken:?}");

    // Nothing listens on port 9 of the loopback address.
    let refused = DavClient::new(
        "http://127.0.0.1:9/dav",
        Credentials {
            user: "a".into(),
            password: "b".into(),
        },
        Limits::default(),
    )
    .unwrap()
    .discover()
    .await
    .unwrap_err();
    assert!(matches!(refused, DavError::Network(_)), "{refused:?}");

    assert!(matches!(
        DavClient::new(
            "ftp://x/",
            Credentials {
                user: "a".into(),
                password: "b".into()
            },
            Limits::default()
        ),
        Err(DavError::InvalidUrl(_))
    ));
}

#[tokio::test]
async fn login_flow_starts_and_polls() {
    let server = StandIn::start(|request| match request.path.as_str() {
        "/nc/index.php/login/v2" => Answer::new(
            200,
            r#"{"poll":{"token":"t0k+/=","endpoint":"https://cloud.example/login/v2/poll"},"login":"https://cloud.example/login/v2/flow/abc"}"#,
        )
        .header("Content-Type", "application/json"),
        "/poll" if request.body == "token=t0k%2B%2F%3D" => Answer::new(
            200,
            r#"{"server":"https://cloud.example","loginName":"ann","appPassword":"secret-app"}"#,
        ),
        _ => Answer::new(404, "[]"),
    })
    .await;
    let flow = LoginFlow::new(&Limits::default()).unwrap();
    let start = flow.start(&url(&server, "/nc")).await.unwrap();
    assert_eq!(start.poll_token, "t0k+/=");
    assert_eq!(
        start.poll_endpoint.as_str(),
        "https://cloud.example/login/v2/poll"
    );
    assert_eq!(
        start.login.as_str(),
        "https://cloud.example/login/v2/flow/abc"
    );

    let pending = flow.poll(&url(&server, "/poll"), "other").await.unwrap();
    assert_eq!(pending, LoginPoll::Pending);
    let LoginPoll::Approved(login) = flow.poll(&url(&server, "/poll"), "t0k+/=").await.unwrap()
    else {
        panic!("not approved");
    };
    assert_eq!(login.login_name, "ann");
    assert_eq!(login.app_password, "secret-app");
    assert_eq!(login.server, "https://cloud.example");
    assert!(!format!("{login:?}").contains("secret-app"));

    let requests = server.requests();
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].header("authorization"), None);
    assert_eq!(
        requests[1].header("content-type"),
        Some("application/x-www-form-urlencoded")
    );
}
