//! The response parsers against the examples in the RFCs themselves, kept as
//! the RFCs print them (indentation, line breaks inside tags, `D:` and `C:`
//! prefixes, properties from unknown namespaces).

use reqwest::Url;

use crate::{
    CollectionKind, Components,
    discovery::{collection, homes},
    fetch::{Multiget, read_multiget},
    sync::page,
    xml::{
        dom::Ns,
        multistatus::{error_names, parse_multistatus},
    },
};

fn url(text: &str) -> Url {
    Url::parse(text).unwrap()
}

/// RFC 6578 section 3.8, "Example: Initial DAV:sync-collection REPORT":
/// three members with ETags, one property from another namespace, 404
/// propstats for a property the members lack, and an href broken across a
/// line inside its start tag.
#[test]
fn rfc6578_3_8_initial_sync() {
    let xml = r#"<?xml version="1.0" encoding="utf-8" ?>
   <D:multistatus xmlns:D="DAV:">
     <D:response>
       <D:href
   >http://webdav.example.com/home/cyrusdaboo/test.doc</D:href>
       <D:propstat>
         <D:prop>
           <D:getetag>"00001-abcd1"</D:getetag>
           <R:bigbox xmlns:R="urn:ns.example.com:boxschema">
             <R:BoxType>Box type A</R:BoxType>
           </R:bigbox>
         </D:prop>
         <D:status>HTTP/1.1 200 OK</D:status>
       </D:propstat>
     </D:response>
     <D:response>
       <D:href
   >http://webdav.example.com/home/cyrusdaboo/vcard.vcf</D:href>
       <D:propstat>
         <D:prop>
           <D:getetag>"00002-abcd1"</D:getetag>
         </D:prop>
         <D:status>HTTP/1.1 200 OK</D:status>
       </D:propstat>
       <D:propstat>
         <D:prop>
           <R:bigbox xmlns:R="urn:ns.example.com:boxschema"/>
         </D:prop>
         <D:status>HTTP/1.1 404 Not Found</D:status>
       </D:propstat>
     </D:response>
     <D:response>
       <D:href
   >http://webdav.example.com/home/cyrusdaboo/calendar.ics</D:href>
       <D:propstat>
         <D:prop>
           <D:getetag>"00003-abcd1"</D:getetag>
         </D:prop>
         <D:status>HTTP/1.1 200 OK</D:status>
       </D:propstat>
       <D:propstat>
         <D:prop>
           <R:bigbox xmlns:R="urn:ns.example.com:boxschema"/>
         </D:prop>
         <D:status>HTTP/1.1 404 Not Found</D:status>
       </D:propstat>
     </D:response>
     <D:sync-token>http://example.com/ns/sync/1234</D:sync-token>
   </D:multistatus>"#;
    let base = url("http://webdav.example.com/home/cyrusdaboo/");
    let page = page(&base, parse_multistatus(xml).unwrap()).unwrap();
    assert!(!page.truncated);
    assert_eq!(page.token, "http://example.com/ns/sync/1234");
    assert!(page.removed.is_empty());
    let changed: Vec<(&str, Option<&str>)> = page
        .changed
        .iter()
        .map(|member| (member.href.path(), member.etag.as_deref()))
        .collect();
    assert_eq!(
        changed,
        [
            ("/home/cyrusdaboo/test.doc", Some("\"00001-abcd1\"")),
            ("/home/cyrusdaboo/vcard.vcf", Some("\"00002-abcd1\"")),
            ("/home/cyrusdaboo/calendar.ics", Some("\"00003-abcd1\"")),
        ]
    );
}

/// RFC 6578 section 3.9, "Example: DAV:sync-collection Report with Token":
/// one new member, one changed, one removed (a bare 404 response).
#[test]
fn rfc6578_3_9_sync_with_token() {
    let xml = r#"<?xml version="1.0" encoding="utf-8" ?>
   <D:multistatus xmlns:D="DAV:">
     <D:response>
       <D:href
   >http://webdav.example.com/home/cyrusdaboo/file.xml</D:href>
       <D:propstat>
         <D:prop>
           <D:getetag>"00004-abcd1"</D:getetag>
         </D:prop>
         <D:status>HTTP/1.1 200 OK</D:status>
       </D:propstat>
       <D:propstat>
         <D:prop>
           <R:bigbox xmlns:R="urn:ns.example.com:boxschema"/>
         </D:prop>
         <D:status>HTTP/1.1 404 Not Found</D:status>
       </D:propstat>
     </D:response>
     <D:response>
       <D:href
   >http://webdav.example.com/home/cyrusdaboo/vcard.vcf</D:href>
       <D:propstat>
         <D:prop>
           <D:getetag>"00002-abcd2"</D:getetag>
           <R:bigbox xmlns:R="urn:ns.example.com:boxschema">
             <R:BoxType>Box type B</R:BoxType>
           </R:bigbox>
         </D:prop>
         <D:status>HTTP/1.1 200 OK</D:status>
       </D:propstat>
     </D:response>
     <D:response>
       <D:href
   >http://webdav.example.com/home/cyrusdaboo/test.doc</D:href>
       <D:status>HTTP/1.1 404 Not Found</D:status>
     </D:response>
     <D:sync-token>http://example.com/ns/sync/1238</D:sync-token>
   </D:multistatus>"#;
    let base = url("http://webdav.example.com/home/cyrusdaboo/");
    let page = page(&base, parse_multistatus(xml).unwrap()).unwrap();
    assert!(!page.truncated);
    assert_eq!(page.token, "http://example.com/ns/sync/1238");
    assert_eq!(page.changed.len(), 2);
    assert_eq!(page.changed[1].etag.as_deref(), Some("\"00002-abcd2\""));
    assert_eq!(
        page.removed,
        [url("http://webdav.example.com/home/cyrusdaboo/test.doc")]
    );
}

/// RFC 6578 section 3.10, "Example: Initial DAV:sync-collection REPORT with
/// Truncation": a 507 response for the collection with the
/// number-of-matches-within-limits condition marks the page as partial.
#[test]
fn rfc6578_3_10_truncated() {
    let xml = r#"<?xml version="1.0" encoding="utf-8" ?>
   <D:multistatus xmlns:D="DAV:">
     <D:response>
       <D:href
   >http://webdav.example.com/home/cyrusdaboo/test.doc</D:href>
       <D:propstat>
         <D:prop>
           <D:getetag>"00001-abcd1"</D:getetag>
         </D:prop>
         <D:status>HTTP/1.1 200 OK</D:status>
       </D:propstat>
     </D:response>
     <D:response>
       <D:href
   >http://webdav.example.com/home/cyrusdaboo/vcard.vcf</D:href>
       <D:propstat>
         <D:prop>
           <D:getetag>"00002-abcd1"</D:getetag>
         </D:prop>
         <D:status>HTTP/1.1 200 OK</D:status>
       </D:propstat>
     </D:response>
     <D:response>
       <D:href
   >http://webdav.example.com/home/cyrusdaboo/</D:href>
       <D:status>HTTP/1.1 507 Insufficient Storage</D:status>
       <D:error><D:number-of-matches-within-limits/></D:error>
     </D:response>
     <D:sync-token>http://example.com/ns/sync/1233</D:sync-token>
   </D:multistatus>"#;
    let base = url("http://webdav.example.com/home/cyrusdaboo/");
    let page = page(&base, parse_multistatus(xml).unwrap()).unwrap();
    assert!(page.truncated);
    assert_eq!(page.changed.len(), 2);
    assert!(page.removed.is_empty());
    assert_eq!(page.token, "http://example.com/ns/sync/1233");
}

/// RFC 6578 section 3.2: an unknown token fails the DAV:valid-sync-token
/// precondition, reported in a DAV:error body (RFC 4918 section 16).
#[test]
fn rfc6578_3_2_valid_sync_token_precondition() {
    let xml = r#"<?xml version="1.0" encoding="utf-8" ?>
   <D:error xmlns:D="DAV:"><D:valid-sync-token/></D:error>"#;
    assert!(error_names(xml, Ns::Dav, "valid-sync-token"));
    // Nextcloud (sabre/dav) adds its own elements and uses `d:`.
    let sabre = r#"<?xml version="1.0" encoding="utf-8"?>
<d:error xmlns:d="DAV:" xmlns:s="http://sabredav.org/ns">
  <s:exception>Sabre\DAV\Exception\InvalidSyncToken</s:exception>
  <s:message>Invalid or unknown sync token</s:message>
  <d:valid-sync-token/>
</d:error>"#;
    assert!(error_names(sabre, Ns::Dav, "valid-sync-token"));
    assert!(!error_names(
        "<D:error xmlns:D=\"DAV:\"/>",
        Ns::Dav,
        "valid-sync-token"
    ));
    assert!(!error_names("not xml", Ns::Dav, "valid-sync-token"));
}

/// RFC 4791 section 7.9.1, "Example: CALDAV:calendar-multiget REPORT": one
/// resource with its data, one missing.
#[test]
fn rfc4791_7_9_1_calendar_multiget() {
    let xml = "<?xml version=\"1.0\" encoding=\"utf-8\" ?>
   <D:multistatus xmlns:D=\"DAV:\"
                  xmlns:C=\"urn:ietf:params:xml:ns:caldav\">
     <D:response>
       <D:href>http://cal.example.com/bernard/work/abcd1.ics</D:href>
       <D:propstat>
         <D:prop>
           <D:getetag>\"fffff-abcd1\"</D:getetag>
           <C:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Example Corp.//CalDAV Client//EN
BEGIN:VEVENT
DTSTAMP:20060206T001102Z
DTSTART;TZID=US/Eastern:20060104T100000
DURATION:PT1H
SUMMARY:Event #1
Description:Go Steelers!
UID:74855313FA803DA593CD579A@example.com
END:VEVENT
END:VCALENDAR
</C:calendar-data>
         </D:prop>
         <D:status>HTTP/1.1 200 OK</D:status>
       </D:propstat>
     </D:response>
     <D:response>
       <D:href>http://cal.example.com/bernard/work/mtg1.ics</D:href>
       <D:status>HTTP/1.1 404 Not Found</D:status>
     </D:response>
   </D:multistatus>";
    let base = url("http://cal.example.com/bernard/work/");
    let asked = [
        url("http://cal.example.com/bernard/work/abcd1.ics"),
        url("http://cal.example.com/bernard/work/mtg1.ics"),
    ];
    let mut result = Multiget::default();
    read_multiget(&base, parse_multistatus(xml).unwrap(), &asked, &mut result).unwrap();
    assert_eq!(result.found.len(), 1);
    let found = &result.found[0];
    assert_eq!(found.href, asked[0]);
    assert_eq!(found.etag.as_deref(), Some("\"fffff-abcd1\""));
    assert!(found.data.starts_with("BEGIN:VCALENDAR\nVERSION:2.0\n"));
    assert!(found.data.contains("SUMMARY:Event #1\n"));
    assert!(found.data.ends_with("END:VCALENDAR\n"));
    assert_eq!(result.missing, [asked[1].clone()]);
}

/// RFC 6352 section 8.7.2, "Example: CARDDAV:addressbook-multiget Report":
/// relative hrefs, one vCard found, one missing (with a non-standard reason
/// phrase).
#[test]
fn rfc6352_8_7_2_addressbook_multiget() {
    let xml = "<?xml version=\"1.0\" encoding=\"utf-8\" ?>
   <D:multistatus xmlns:D=\"DAV:\"
                  xmlns:C=\"urn:ietf:params:xml:ns:carddav\">
     <D:response>
       <D:href>/home/bernard/addressbook/vcf102.vcf</D:href>
       <D:propstat>
         <D:prop>
           <D:getetag>\"23ba4d-ff11fb\"</D:getetag>
           <C:address-data>BEGIN:VCARD
VERSION:3.0
NICKNAME:me
UID:34222-232@example.com
FN:Cyrus Daboo
EMAIL:daboo@example.com
END:VCARD
</C:address-data>
         </D:prop>
         <D:status>HTTP/1.1 200 OK</D:status>
       </D:propstat>
     </D:response>
     <D:response>
       <D:href>/home/bernard/addressbook/vcf1.vcf</D:href>
       <D:status>HTTP/1.1 404 Resource Not Found</D:status>
     </D:response>
   </D:multistatus>";
    let base = url("https://addressbook.example.com/home/bernard/addressbook/");
    let asked = [
        url("https://addressbook.example.com/home/bernard/addressbook/vcf102.vcf"),
        url("https://addressbook.example.com/home/bernard/addressbook/vcf1.vcf"),
    ];
    let mut result = Multiget::default();
    read_multiget(&base, parse_multistatus(xml).unwrap(), &asked, &mut result).unwrap();
    assert_eq!(result.found.len(), 1);
    assert_eq!(result.found[0].href, asked[0]);
    assert!(result.found[0].data.contains("FN:Cyrus Daboo\n"));
    assert_eq!(result.missing, [asked[1].clone()]);
}

/// RFC 5397 section 3, the current-user-principal example response.
#[test]
fn rfc5397_3_current_user_principal() {
    let xml = r#"<?xml version="1.0" encoding="utf-8" ?>
   <D:multistatus xmlns:D="DAV:">
     <D:response>
       <D:href>/</D:href>
       <D:propstat>
         <D:prop>
           <D:current-user-principal>
             <D:href>/principals/users/cdaboo</D:href>
           </D:current-user-principal>
         </D:prop>
         <D:status>HTTP/1.1 200 OK</D:status>
       </D:propstat>
     </D:response>
   </D:multistatus>"#;
    let multistatus = parse_multistatus(xml).unwrap();
    let principal = multistatus.responses[0]
        .props
        .current_user_principal
        .as_deref();
    assert_eq!(principal, Some("/principals/users/cdaboo"));
}

/// The home-set properties as RFC 4791 section 6.2.1 and RFC 6352 section
/// 7.1.1 print them, inside a PROPFIND answer for the principal.
#[test]
fn rfc4791_6_2_1_and_rfc6352_7_1_1_home_sets() {
    let xml = r#"<?xml version="1.0" encoding="utf-8" ?>
   <D:multistatus xmlns:D="DAV:">
     <D:response>
       <D:href>/principals/users/bernard/</D:href>
       <D:propstat>
         <D:prop>
           <C:calendar-home-set xmlns:D="DAV:"
               xmlns:C="urn:ietf:params:xml:ns:caldav">
             <D:href>http://cal.example.com/bernard/calendars/</D:href>
           </C:calendar-home-set>
           <C:addressbook-home-set xmlns:D="DAV:"
              xmlns:C="urn:ietf:params:xml:ns:carddav">
             <D:href>/bernard/addresses/</D:href>
           </C:addressbook-home-set>
         </D:prop>
         <D:status>HTTP/1.1 200 OK</D:status>
       </D:propstat>
     </D:response>
   </D:multistatus>"#;
    let base = url("http://cal.example.com/principals/users/bernard/");
    let response = parse_multistatus(xml).unwrap().responses.remove(0);
    let found = homes(base.clone(), &base, response.props).unwrap();
    assert_eq!(
        found.calendar_home,
        Some(url("http://cal.example.com/bernard/calendars/"))
    );
    assert_eq!(
        found.addressbook_home,
        Some(url("http://cal.example.com/bernard/addresses/"))
    );
}

/// RFC 4791 section 5.2.3 (supported-calendar-component-set) and RFC 6352
/// section 6.2 (the addressbook resource type), in a home listing, with the
/// privilege set of RFC 3744 section 5.4.
#[test]
fn rfc4791_5_2_3_and_rfc6352_collections() {
    let xml = r#"<?xml version="1.0" encoding="utf-8" ?>
   <D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"
                  xmlns:CR="urn:ietf:params:xml:ns:carddav">
     <D:response>
       <D:href>/bernard/calendars/</D:href>
       <D:propstat>
         <D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop>
         <D:status>HTTP/1.1 200 OK</D:status>
       </D:propstat>
     </D:response>
     <D:response>
       <D:href>/bernard/calendars/work/</D:href>
       <D:propstat>
         <D:prop>
           <D:resourcetype><D:collection/><C:calendar/></D:resourcetype>
           <D:displayname>Work</D:displayname>
           <C:supported-calendar-component-set>
             <C:comp name="VEVENT"/>
             <C:comp name="VTODO"/>
           </C:supported-calendar-component-set>
           <D:current-user-privilege-set>
             <D:privilege><D:read/></D:privilege>
             <D:privilege><D:write/></D:privilege>
           </D:current-user-privilege-set>
         </D:prop>
         <D:status>HTTP/1.1 200 OK</D:status>
       </D:propstat>
       <D:propstat>
         <D:prop><D:sync-token/></D:prop>
         <D:status>HTTP/1.1 404 Not Found</D:status>
       </D:propstat>
     </D:response>
     <D:response>
       <D:href>/bernard/addresses/home/</D:href>
       <D:propstat>
         <D:prop>
           <D:resourcetype><D:collection/><CR:addressbook/></D:resourcetype>
           <D:current-user-privilege-set>
             <D:privilege><D:read/></D:privilege>
           </D:current-user-privilege-set>
         </D:prop>
         <D:status>HTTP/1.1 200 OK</D:status>
       </D:propstat>
     </D:response>
   </D:multistatus>"#;
    let base = url("http://cal.example.com/bernard/calendars/");
    let found: Vec<_> = parse_multistatus(xml)
        .unwrap()
        .responses
        .into_iter()
        .filter_map(|response| collection(&base, response).unwrap())
        .collect();
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].kind, CollectionKind::Calendar);
    assert_eq!(found[0].display_name.as_deref(), Some("Work"));
    assert_eq!(
        found[0].components,
        Components {
            event: true,
            todo: true,
            journal: false
        }
    );
    assert_eq!(found[0].sync_token, None);
    assert_eq!(found[0].writable, Some(true));
    assert_eq!(found[1].kind, CollectionKind::AddressBook);
    assert_eq!(found[1].components, Components::NONE);
    assert_eq!(found[1].writable, Some(false));
}
