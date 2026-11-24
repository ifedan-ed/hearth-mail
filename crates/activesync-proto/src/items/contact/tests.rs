/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The Contact class both ways, with [MS-ASCNTC] section 4's example.

use super::*;
use crate::items::fixtures::{application_data, context};
use crate::request::Version;

fn get(data: &Element, tag: Tag) -> Option<&str> {
    data.child_str(tag)
}

/// [MS-ASCNTC] 4: the contact the server sends, in the example's own
/// prefixes (`A:` AirSyncBase, `B:` POOMCONTACTS). The example cuts the
/// picture short with "..."; the cut is dropped so the base64 is whole.
/// The line break inside `Email1Address` is the example's.
const SPEC_4: &str = r#"
  <A:Body>
    <A:Type>3</A:Type>
    <A:EstimatedDataSize>5500</A:EstimatedDataSize>
    <A:Truncated>1</A:Truncated>
  </A:Body>
  <B:WebPage>http://www.contoso.com/</B:WebPage>
  <B:BusinessAddressCountry>United States of America</B:BusinessAddressCountry>
  <B:Email1Address>"Anat Kerry (anat@contoso.com)"
&lt;anat@contoso.com&gt;</B:Email1Address>
  <B:BusinessFaxNumber>(206) 555-0100</B:BusinessFaxNumber>
  <B:FileAs>Kerry, Anat</B:FileAs>
  <B:FirstName>Anat</B:FirstName>
  <B:HomePhoneNumber>(206) 555-0101</B:HomePhoneNumber>
  <B:BusinessAddressCity>Redmond</B:BusinessAddressCity>
  <B:MiddleName>M.</B:MiddleName>
  <B:MobilePhoneNumber>(206) 555-0102</B:MobilePhoneNumber>
  <B:CompanyName>Contoso, Ltd.</B:CompanyName>
  <B:BusinessAddressPostalCode>10021</B:BusinessAddressPostalCode>
  <B:LastName>Kerry</B:LastName>
  <B:BusinessAddressState>WA</B:BusinessAddressState>
  <B:BusinessAddressStreet>234 Main St.</B:BusinessAddressStreet>
  <B:BusinessPhoneNumber>(206) 555-0103</B:BusinessPhoneNumber>
  <B:JobTitle>Development Manager</B:JobTitle>
  <B:Picture>/9j/4AAQSkZJRgABAQEAYABgAAD/</B:Picture>
  <A:NativeBodyType>3</A:NativeBodyType>"#;

#[test]
fn spec_4_contact_both_ways() {
    let ctx = context(Version::V14_1);
    let sent = application_data(SPEC_4);
    let text = to_vcard(&sent, None, Omitted::Cleared, &ctx).unwrap();
    for line in [
        "VERSION:3.0",
        "N:Kerry;Anat;M.;;",
        "FN:Anat M. Kerry",
        "ORG:Contoso\\, Ltd.",
        "TITLE:Development Manager",
        "EMAIL:anat@contoso.com",
        "TEL;TYPE=WORK,FAX:(206) 555-0100",
        "TEL;TYPE=HOME,VOICE:(206) 555-0101",
        "TEL;TYPE=CELL:(206) 555-0102",
        "TEL;TYPE=WORK,VOICE:(206) 555-0103",
        "ADR;TYPE=WORK:;;234 Main St.;Redmond;WA;10021;United States of America",
        "URL:http://www.contoso.com/",
        "PHOTO;ENCODING=b;TYPE=JPEG:/9j/4AAQSkZJRgABAQEAYABgAAD/",
        "REV:20261123T180300Z",
    ] {
        assert!(text.contains(line), "{line} missing from\n{text}");
    }
    // An RTF note is not converted.
    assert!(!text.contains("NOTE"));
    assert!(uid(&text).unwrap().is_some());

    let back = from_vcard(&text, &ctx).unwrap();
    for tag in [
        contacts::WEB_PAGE,
        contacts::BUSINESS_ADDRESS_COUNTRY,
        contacts::BUSINESS_FAX_NUMBER,
        contacts::FILE_AS,
        contacts::FIRST_NAME,
        contacts::HOME_PHONE_NUMBER,
        contacts::BUSINESS_ADDRESS_CITY,
        contacts::MIDDLE_NAME,
        contacts::MOBILE_PHONE_NUMBER,
        contacts::COMPANY_NAME,
        contacts::BUSINESS_ADDRESS_POSTAL_CODE,
        contacts::LAST_NAME,
        contacts::BUSINESS_ADDRESS_STATE,
        contacts::BUSINESS_ADDRESS_STREET,
        contacts::BUSINESS_PHONE_NUMBER,
        contacts::JOB_TITLE,
        contacts::PICTURE,
    ] {
        assert_eq!(get(&back, tag), get(&sent, tag), "{tag:?}");
    }
    // The address alone: the display form is the device's to make.
    assert_eq!(
        get(&back, contacts::EMAIL1_ADDRESS),
        Some("anat@contoso.com")
    );
    // Sending it all back changes nothing (calcard only normalises the
    // order and case of parameters when it reads the card again).
    let again = to_vcard(&back, Some(&text), Omitted::Cleared, &ctx).unwrap();
    assert_eq!(from_vcard(&again, &ctx).unwrap(), back);
    assert_eq!(again.lines().count(), text.lines().count());
}

/// Every phone slot, the second email and IM addresses, the other two
/// addresses, dates, relations and categories, both ways.
#[test]
fn every_field_round_trips() {
    let ctx = context(Version::V16_1);
    let phones = [
        (contacts::BUSINESS_PHONE_NUMBER, "1"),
        (contacts::BUSINESS2_PHONE_NUMBER, "2"),
        (contacts::HOME_PHONE_NUMBER, "3"),
        (contacts::HOME2_PHONE_NUMBER, "4"),
        (contacts::MOBILE_PHONE_NUMBER, "5"),
        (contacts::BUSINESS_FAX_NUMBER, "6"),
        (contacts::HOME_FAX_NUMBER, "7"),
        (contacts::PAGER_NUMBER, "8"),
        (contacts::CAR_PHONE_NUMBER, "9"),
        (contacts::RADIO_PHONE_NUMBER, "10"),
        (contacts::ASSISTANT_PHONE_NUMBER, "11"),
        (contacts2::COMPANY_MAIN_PHONE, "12"),
        (contacts2::MMS, "13"),
    ];
    let mut xml = String::new();
    for (tag, number) in phones {
        let prefix = if tag.page() == contacts2::MMS.page() {
            "contacts2"
        } else {
            "contacts"
        };
        xml.push_str(&format!(
            "<{prefix}:{0}>{number}</{prefix}:{0}>",
            tag.name()
        ));
    }
    xml.push_str(
        "<contacts:FirstName>Ada</contacts:FirstName><contacts:LastName>Lovelace</contacts:LastName>\
         <contacts:Title>Countess</contacts:Title><contacts:Suffix>FRS</contacts:Suffix>\
         <contacts:Department>Analytics</contacts:Department><contacts:CompanyName>Engines</contacts:CompanyName>\
         <contacts:Email1Address>ada@example.com</contacts:Email1Address>\
         <contacts:Email2Address>countess@example.org</contacts:Email2Address>\
         <contacts2:IMAddress>xmpp:ada@chat.example</contacts2:IMAddress>\
         <contacts2:IMAddress2>ada.lovelace</contacts2:IMAddress2>\
         <contacts2:NickName>Enchantress</contacts2:NickName>\
         <contacts:HomeAddressStreet>12 St James's Square\nFlat 2</contacts:HomeAddressStreet>\
         <contacts:HomeAddressCity>London</contacts:HomeAddressCity>\
         <contacts:OtherAddressCity>Ockham</contacts:OtherAddressCity>\
         <contacts:Birthday>1815-12-09T23:00:00.000Z</contacts:Birthday>\
         <contacts:Anniversary>1835-07-08T11:59:00.000Z</contacts:Anniversary>\
         <contacts:Spouse>William King</contacts:Spouse>\
         <contacts:AssistantName>Charles</contacts:AssistantName>\
         <contacts:Children><contacts:Child>Byron</contacts:Child><contacts:Child>Annabella</contacts:Child></contacts:Children>\
         <contacts:Categories><contacts:Category>Science</contacts:Category></contacts:Categories>\
         <contacts:OfficeLocation>Room 1</contacts:OfficeLocation>\
         <contacts2:ManagerName>Babbage</contacts2:ManagerName>\
         <contacts2:CustomerId>C-1</contacts2:CustomerId><contacts2:GovernmentId>G-1</contacts2:GovernmentId>\
         <contacts2:AccountName>A-1</contacts2:AccountName>\
         <contacts:YomiFirstName>Eida</contacts:YomiFirstName>\
         <airsyncbase:Body><airsyncbase:Type>1</airsyncbase:Type><airsyncbase:Data>First programmer.</airsyncbase:Data></airsyncbase:Body>",
    );
    let sent = application_data(&xml);
    let text = to_vcard(&sent, None, Omitted::Cleared, &ctx).unwrap();
    for line in [
        "BDAY:1815-12-10",
        "X-ANNIVERSARY:1835-07-08",
        "RELATED;VALUE=TEXT;TYPE=spouse:William King",
        "RELATED;VALUE=TEXT;TYPE=agent:Charles",
        "IMPP:xmpp:ada@chat.example",
        "IMPP:im:ada.lovelace",
        "NOTE:First programmer.",
        "FN:Countess Ada Lovelace FRS",
        "ADR;TYPE=HOME:;;12 St James's Square\\nFlat 2;London;;;",
        "ADR:;;;Ockham;;;",
    ] {
        assert!(text.contains(line), "{line} missing from\n{text}");
    }
    let back = from_vcard(&text, &ctx).unwrap();
    for child in sent.children() {
        if child.tag == airsyncbase::BODY {
            continue;
        }
        let expected = match child.tag {
            tag if tag == contacts::BIRTHDAY => Some("1815-12-10T11:59:00.000Z"),
            tag if tag == contacts2::IM_ADDRESS => Some("ada@chat.example"),
            _ => child.as_str(),
        };
        let found = back.child(child.tag);
        match expected {
            Some(expected) => assert_eq!(
                found.and_then(Element::as_str),
                Some(expected),
                "{:?}",
                child.tag
            ),
            None => assert_eq!(
                found.map(|f| f.children().len()),
                Some(child.children().len()),
                "{:?}",
                child.tag
            ),
        }
    }
    assert_eq!(
        back.child(airsyncbase::BODY)
            .and_then(|b| b.child_str(airsyncbase::DATA)),
        Some("First programmer.")
    );
    let again = to_vcard(&back, Some(&text), Omitted::Cleared, &ctx).unwrap();
    assert_eq!(from_vcard(&again, &ctx).unwrap(), back);
    assert_eq!(again.lines().count(), text.lines().count());
}

/// A 4.0 card stays 4.0, with its dates, photo and relations read.
#[test]
fn version_4_cards() {
    let ctx = context(Version::V14_1);
    let stored = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:urn:uuid:4fbe8971\r\nFN:Grace Hopper\r\n\
        N:Hopper;Grace;;;\r\nBDAY:19061209\r\nANNIVERSARY:19300615\r\n\
        PHOTO:data:image/png;base64,iVBORw0KGgo=\r\nRELATED;TYPE=spouse;VALUE=text:Vincent\r\n\
        EMAIL;TYPE=work:grace@navy.example\r\nGENDER:F\r\nEND:VCARD\r\n";
    let back = from_vcard(stored, &ctx).unwrap();
    assert_eq!(
        get(&back, contacts::BIRTHDAY),
        Some("1906-12-09T11:59:00.000Z")
    );
    assert_eq!(
        get(&back, contacts::ANNIVERSARY),
        Some("1930-06-15T11:59:00.000Z")
    );
    assert_eq!(get(&back, contacts::PICTURE), Some("iVBORw0KGgo="));
    assert_eq!(get(&back, contacts::SPOUSE), Some("Vincent"));
    assert_eq!(get(&back, contacts::FILE_AS), Some("Hopper, Grace"));
    let change = application_data(
        "<contacts:FirstName>Grace</contacts:FirstName><contacts:LastName>Hopper</contacts:LastName>\
         <contacts:Birthday>1906-12-09T11:59:00.000Z</contacts:Birthday>\
         <contacts:JobTitle>Rear Admiral</contacts:JobTitle>",
    );
    let text = to_vcard(&change, Some(stored), Omitted::Kept, &ctx).unwrap();
    for line in [
        "VERSION:4.0",
        "UID:urn:uuid:4fbe8971",
        "BDAY:19061209",
        "TITLE:Rear Admiral",
        "GENDER:F",
        "FN:Grace Hopper",
        "PHOTO:data:image/png;base64\\,iVBORw0KGgo=",
    ] {
        assert!(text.contains(line), "{line} missing from\n{text}");
    }
}

/// Nextcloud's extra lines survive an edit on a phone: a fourth email,
/// Apple's grouped labels, unknown properties and parameters.
#[test]
fn unknown_properties_survive_an_update() {
    let ctx = context(Version::V14_1);
    let stored = "BEGIN:VCARD\r\nVERSION:3.0\r\nPRODID:-//Sabre//Sabre VObject 4.5.6//EN\r\n\
        UID:nc-1\r\nFN:Custom Display\r\nN:Doe;Jane;;;\r\n\
        EMAIL;TYPE=WORK:jane@work.example\r\nEMAIL;TYPE=HOME:jane@home.example\r\n\
        EMAIL:three@example.com\r\nEMAIL;X-CUSTOM=yes:four@example.com\r\n\
        item1.TEL;TYPE=pref:+49 30 1\r\nitem1.X-ABLabel:Office\r\n\
        X-NEXTCLOUD-THING:kept\r\nCLIENTPIDMAP:1;urn:uuid:x\r\nEND:VCARD\r\n";
    let mut sent = from_vcard(stored, &ctx).unwrap();
    if let crate::wbxml::Content::Children(children) = &mut sent.content {
        for child in children.iter_mut() {
            if child.tag == contacts::EMAIL2_ADDRESS {
                *child = Element::text(contacts::EMAIL2_ADDRESS, "jane@new.example");
            }
        }
    }
    let text = to_vcard(&sent, Some(stored), Omitted::Cleared, &ctx).unwrap();
    for line in [
        "EMAIL;TYPE=WORK:jane@work.example",
        "EMAIL;TYPE=HOME:jane@new.example",
        "EMAIL;X-CUSTOM=yes:four@example.com",
        "item1.TEL;TYPE=pref:+49 30 1",
        "item1.X-ABLabel:Office",
        "X-NEXTCLOUD-THING:kept",
        "CLIENTPIDMAP:1;urn:uuid:x",
        "FN:Custom Display",
        "UID:nc-1",
    ] {
        assert!(text.contains(line), "{line} missing from\n{text}");
    }
}

/// What a 14.1 `Change` leaves out goes when the device manages it, and
/// stays when it is ghosted; pictures left out always stay.
#[test]
fn omitted_elements_follow_supported() {
    let ctx = context(Version::V14_1);
    let stored = to_vcard(&application_data(SPEC_4), None, Omitted::Cleared, &ctx).unwrap();
    let change = application_data("<contacts:FirstName>Ana</contacts:FirstName>");
    let managed = [contacts::FIRST_NAME, contacts::MOBILE_PHONE_NUMBER];
    let text = to_vcard(&change, Some(&stored), Omitted::Supported(&managed), &ctx).unwrap();
    assert!(!text.contains("(206) 555-0102"), "{text}");
    assert!(text.contains("TITLE:Development Manager"));
    assert!(text.contains("N:Kerry;Ana;M.;;"));
    assert!(text.contains("FN:Ana M. Kerry"));
    assert!(text.contains("PHOTO"));
    let text = to_vcard(&change, Some(&stored), Omitted::Cleared, &ctx).unwrap();
    assert!(!text.contains("TITLE"));
    assert!(!text.contains("ADR"));
    assert!(text.contains("N:;Ana;;;"), "{text}");
    assert!(text.contains("PHOTO"));
    let no_picture = application_data("<contacts:Picture/>");
    let text = to_vcard(&no_picture, Some(&stored), Omitted::Kept, &ctx).unwrap();
    assert!(!text.contains("PHOTO"));
}

#[test]
fn garbage_is_refused() {
    let ctx = context(Version::V14_1);
    let big = format!(
        "<contacts:Picture>{}</contacts:Picture>",
        "A".repeat(MAX_PICTURE + 4)
    );
    for xml in [
        big.as_str(),
        "<contacts:Picture>not base64!</contacts:Picture>",
        "<contacts:Birthday>someday</contacts:Birthday>",
        "<airsyncbase:Body><airsyncbase:Type>7</airsyncbase:Type></airsyncbase:Body>",
    ] {
        assert!(
            to_vcard(&application_data(xml), None, Omitted::Cleared, &ctx).is_err(),
            "{xml}"
        );
    }
    for text in ["", "garbage", "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n"] {
        assert!(from_vcard(text, &ctx).is_err(), "{text}");
        assert!(uid(text).is_err());
    }
    // A card with odd values reads without failing.
    let odd = "BEGIN:VCARD\r\nVERSION:3.0\r\nBDAY:--0512\r\nPHOTO;VALUE=uri:http://x/p.jpg\r\nN:;;;;\r\nEND:VCARD\r\n";
    let back = from_vcard(odd, &ctx).unwrap();
    assert_eq!(back.child(contacts::BIRTHDAY), None);
    assert_eq!(back.child(contacts::PICTURE), None);
}
