/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The Calendar class both ways, with [MS-ASCAL] section 4's examples.

use super::*;
use crate::items::{
    fixtures::{self, application_data, context},
    timezone::{TimeZoneRules, rules_for_zone},
};
use crate::request::Version;
use chrono::NaiveDate;

const V14: Version = Version::V14_1;
const V16: Version = Version::V16_1;

/// The text of `tag` in `data`.
fn get(data: &Element, tag: crate::wbxml::Tag) -> Option<&str> {
    data.child_str(tag)
}

/// The `Timezone` of `data` as rules.
fn blob(data: &Element) -> TimeZoneRules {
    TimeZoneRules::from_base64(get(data, calendar::TIMEZONE).unwrap()).unwrap()
}

/// A `Timezone` element's XML for `zone` in mid-2026.
fn timezone_xml(zone: chrono_tz::Tz) -> String {
    let at = NaiveDate::from_ymd_opt(2026, 7, 1)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap();
    format!(
        "<calendar:Timezone>{}</calendar:Timezone>",
        rules_for_zone(zone, at).to_base64()
    )
}

fn add(xml: &str, version: Version) -> String {
    to_ical(
        &application_data(xml),
        None,
        Omitted::Cleared,
        &context(version),
    )
    .unwrap()
}

/// [MS-ASCAL] 4.1, the lunch meeting: every element lands in the event,
/// and comes back as sent.
#[test]
fn spec_4_1_one_off_meeting() {
    let sent = application_data(fixtures::ASCAL_4_1_CHANGE);
    let ctx = context(V14);
    let text = to_ical(&sent, None, Omitted::Cleared, &ctx).unwrap();
    for line in [
        "DTSTART;TZID=America/Los_Angeles:20081010T120000",
        "DTEND;TZID=America/Los_Angeles:20081010T133000",
        "SUMMARY:Lunch meeting",
        "LOCATION:Cafeteria A",
        "ORGANIZER;CN=\"Anat Kerry\":mailto:anat@contoso.com",
        "TRANSP:OPAQUE",
        "X-MICROSOFT-CDO-BUSYSTATUS:OOF",
        "TRIGGER:-PT15M",
        "DTSTAMP:20081002T231357Z",
        "TZID:America/Los_Angeles",
    ] {
        assert!(text.contains(line), "{line} missing from\n{text}");
    }
    // An RTF body is not converted (the one DESCRIPTION is the alarm's),
    // and normal sensitivity needs no CLASS.
    assert_eq!(text.matches("DESCRIPTION").count(), 1);
    assert!(!text.contains("CLASS"));
    assert_eq!(
        uid(&text).unwrap().as_deref(),
        get(&sent, calendar::UID).map(str::trim)
    );

    let back = from_ical(&text, &ctx).unwrap();
    for tag in [
        calendar::START_TIME,
        calendar::END_TIME,
        calendar::SUBJECT,
        calendar::LOCATION,
        calendar::UID,
        calendar::ORGANIZER_NAME,
        calendar::ORGANIZER_EMAIL,
        calendar::BUSY_STATUS,
        calendar::REMINDER,
        calendar::SENSITIVITY,
        calendar::ALL_DAY_EVENT,
        calendar::MEETING_STATUS,
        calendar::DT_STAMP,
    ] {
        assert_eq!(get(&back, tag), get(&sent, tag).map(str::trim), "{tag:?}");
    }
    assert!(blob(&back).same_rules(&blob(&sent)));
    assert_eq!(back.child(calendar::RECURRENCE), None);
}

/// [MS-ASCAL] 4.1, the monthly team meeting: the second Monday until July.
#[test]
fn spec_4_1_monthly_meeting() {
    let sent = application_data(fixtures::ASCAL_4_1_MONTHLY);
    let text = to_ical(&sent, None, Omitted::Cleared, &context(V14)).unwrap();
    assert!(
        text.contains("RRULE:FREQ=MONTHLY;UNTIL=20090713T190000Z;BYDAY=2MO"),
        "{text}"
    );
    let back = from_ical(&text, &context(V14)).unwrap();
    let recurrence = back.child(calendar::RECURRENCE).unwrap();
    let expected = sent.child(calendar::RECURRENCE).unwrap();
    for tag in [
        calendar::TYPE,
        calendar::INTERVAL,
        calendar::UNTIL,
        calendar::WEEK_OF_MONTH,
        calendar::DAY_OF_WEEK,
    ] {
        assert_eq!(get(recurrence, tag), get(expected, tag), "{tag:?}");
    }
    assert_eq!(get(recurrence, calendar::CALENDAR_TYPE), Some("0"));
}

/// [MS-ASCAL] 4.2: three Fridays, the second deleted, in a zone named
/// "(GMT-08:00) Pacific Time (US & C" that only its rules identify.
#[test]
fn spec_4_2_recurring_with_a_deleted_exception() {
    let sent = application_data(fixtures::ASCAL_4_2);
    let text = to_ical(&sent, None, Omitted::Cleared, &context(V14)).unwrap();
    assert!(
        text.contains("RRULE:FREQ=WEEKLY;COUNT=3;BYDAY=FR"),
        "{text}"
    );
    assert!(
        text.contains("EXDATE;TZID=America/Los_Angeles:20090424T100000"),
        "{text}"
    );
    assert!(text.contains("mailto:anat@contoso.com\r\n"));

    let back = from_ical(&text, &context(V14)).unwrap();
    let exceptions = back.child(calendar::EXCEPTIONS).unwrap();
    let exception = &exceptions.children()[0];
    assert_eq!(exceptions.children().len(), 1);
    assert_eq!(get(exception, calendar::DELETED), Some("1"));
    assert_eq!(
        get(exception, calendar::EXCEPTION_START_TIME),
        Some("20090424T170000Z")
    );
    let recurrence = back.child(calendar::RECURRENCE).unwrap();
    assert_eq!(get(recurrence, calendar::OCCURRENCES), Some("3"));
    assert_eq!(get(recurrence, calendar::DAY_OF_WEEK), Some("32"));
    assert_eq!(get(recurrence, calendar::FIRST_DAY_OF_WEEK), Some("1"));
    assert_eq!(get(&back, calendar::RESPONSE_TYPE), None);
}

/// [MS-ASCAL] 4.3: a meeting the account organizes; the attendee's
/// answer arrives as a change.
#[test]
fn spec_4_3_attendee_status() {
    let anat = vec!["anat@contoso.com".to_string()];
    let ctx = Context {
        addresses: &anat,
        ..context(V14)
    };
    let text = to_ical(
        &application_data(fixtures::ASCAL_4_3_NEW),
        None,
        Omitted::Cleared,
        &ctx,
    )
    .unwrap();
    assert!(text.contains("CN=\"Chris Gray\""), "{text}");
    assert!(text.contains("PARTSTAT=NEEDS-ACTION"));
    // 2005's Pacific rules: first Sunday of April to last Sunday of October.
    assert!(
        text.contains("DTSTART;TZID=America/Los_Angeles:20051103T150000"),
        "{text}"
    );
    let back = from_ical(&text, &ctx).unwrap();
    assert_eq!(get(&back, calendar::MEETING_STATUS), Some("1"));
    assert_eq!(get(&back, calendar::RESPONSE_TYPE), Some("1"));
    assert_eq!(get(&back, calendar::ORGANIZER_NAME), Some("Anat Kerry"));
    let attendee = &back.child(calendar::ATTENDEES).unwrap().children()[0];
    assert_eq!(
        get(attendee, calendar::EMAIL),
        Some("chris@fourthcoffee.com")
    );
    assert_eq!(get(attendee, calendar::ATTENDEE_STATUS), Some("5"));
    assert_eq!(get(attendee, calendar::ATTENDEE_TYPE), Some("1"));

    let accepted = application_data(fixtures::ASCAL_4_3_ACCEPTED);
    let text = to_ical(&accepted, Some(&text), Omitted::Cleared, &ctx).unwrap();
    assert!(text.contains("PARTSTAT=ACCEPTED"), "{text}");
    assert_eq!(text.matches("ATTENDEE").count(), 1);
    let back = from_ical(&text, &ctx).unwrap();
    let attendee = &back.child(calendar::ATTENDEES).unwrap().children()[0];
    assert_eq!(get(attendee, calendar::ATTENDEE_STATUS), Some("3"));
    // The example's body has a size but no text: there is no description.
    let body = back.child(airsyncbase::BODY).unwrap();
    assert_eq!(body.child(airsyncbase::DATA), None);
}

/// One series of each recurrence type survives both ways at the calendar
/// level, in 14.1 and 16.1.
#[test]
fn recurrences_of_every_type() {
    let patterns = [
        "<calendar:Type>0</calendar:Type><calendar:Interval>3</calendar:Interval><calendar:Occurrences>10</calendar:Occurrences>",
        "<calendar:Type>1</calendar:Type><calendar:Interval>2</calendar:Interval><calendar:DayOfWeek>42</calendar:DayOfWeek><calendar:FirstDayOfWeek>0</calendar:FirstDayOfWeek>",
        "<calendar:Type>2</calendar:Type><calendar:Interval>1</calendar:Interval><calendar:DayOfMonth>24</calendar:DayOfMonth><calendar:Until>20271224T090000Z</calendar:Until>",
        "<calendar:Type>3</calendar:Type><calendar:Interval>1</calendar:Interval><calendar:WeekOfMonth>4</calendar:WeekOfMonth><calendar:DayOfWeek>4</calendar:DayOfWeek>",
        "<calendar:Type>5</calendar:Type><calendar:Interval>1</calendar:Interval><calendar:DayOfMonth>24</calendar:DayOfMonth><calendar:MonthOfYear>11</calendar:MonthOfYear>",
        "<calendar:Type>6</calendar:Type><calendar:Interval>1</calendar:Interval><calendar:WeekOfMonth>5</calendar:WeekOfMonth><calendar:DayOfWeek>4</calendar:DayOfWeek><calendar:MonthOfYear>11</calendar:MonthOfYear>",
    ];
    for version in [V14, V16] {
        for pattern in patterns {
            let xml = format!(
                "{}<calendar:StartTime>20261124T090000Z</calendar:StartTime>\
                 <calendar:EndTime>20261124T100000Z</calendar:EndTime>\
                 <calendar:Subject>Series</calendar:Subject>\
                 <calendar:Recurrence>{pattern}</calendar:Recurrence>",
                timezone_xml(chrono_tz::Europe::Berlin)
            );
            let sent = application_data(&xml);
            let text = to_ical(&sent, None, Omitted::Cleared, &context(version)).unwrap();
            let back = from_ical(&text, &context(version)).unwrap();
            let read = |data: &Element| {
                Pattern::read(
                    data.child(calendar::RECURRENCE).unwrap(),
                    &recurrence::CALENDAR,
                )
                .unwrap()
            };
            assert!(read(&back).same_as(&read(&sent)), "{pattern}\n{text}");
            // Sending the same pattern back leaves the RRULE line alone.
            let again = to_ical(&back, Some(&text), Omitted::Cleared, &context(version)).unwrap();
            let rule = |text: &str| {
                text.lines()
                    .find(|l| l.starts_with("RRULE"))
                    .map(str::to_string)
            };
            assert_eq!(rule(&again), rule(&text));
        }
    }
}

/// An RRULE with no ActiveSync pattern reaches the device as the plainest
/// pattern, and survives the device sending that back.
#[test]
fn rules_without_a_pattern_survive_a_round_trip() {
    let stored = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:r\r\n\
        DTSTAMP:20261101T000000Z\r\nDTSTART:20261102T090000Z\r\nDTEND:20261102T100000Z\r\n\
        RRULE:FREQ=MONTHLY;BYMONTHDAY=2,16\r\nSUMMARY:Twice a month\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let back = from_ical(stored, &context(V14)).unwrap();
    let recurrence = back.child(calendar::RECURRENCE).unwrap();
    assert_eq!(get(recurrence, calendar::TYPE), Some("2"));
    assert_eq!(get(recurrence, calendar::DAY_OF_MONTH), Some("2"));
    let text = to_ical(&back, Some(stored), Omitted::Cleared, &context(V14)).unwrap();
    assert!(
        text.contains("RRULE:FREQ=MONTHLY;BYMONTHDAY=2,16"),
        "{text}"
    );
}

/// A changed instance becomes an override that inherits what the
/// exception leaves out; deleting it turns it into an `EXDATE`.
#[test]
fn exceptions_in_14_1() {
    let series = fixtures::ASCAL_4_2.replace("<calendar:Occurrences>3</calendar:Occurrences>", "");
    let moved = series.replace(
        "<calendar:Deleted>1</calendar:Deleted>",
        "<calendar:StartTime>20090424T200000Z</calendar:StartTime>\
         <calendar:EndTime>20090424T210000Z</calendar:EndTime>\
         <calendar:Subject>Moved</calendar:Subject>\
         <calendar:Reminder/>",
    );
    let text = add(&moved, V14);
    assert!(
        text.contains("RECURRENCE-ID;TZID=America/Los_Angeles:20090424T100000"),
        "{text}"
    );
    assert!(text.contains("DTSTART;TZID=America/Los_Angeles:20090424T130000"));
    assert!(text.contains("SUMMARY:Moved"));
    assert_eq!(text.matches("LOCATION:My office").count(), 2);
    assert_eq!(text.matches("BEGIN:VALARM").count(), 1);
    assert!(!text.contains("EXDATE"));

    let back = from_ical(&text, &context(V14)).unwrap();
    let exception = &back.child(calendar::EXCEPTIONS).unwrap().children()[0];
    assert_eq!(
        get(exception, calendar::EXCEPTION_START_TIME),
        Some("20090424T170000Z")
    );
    assert_eq!(
        get(exception, calendar::START_TIME),
        Some("20090424T200000Z")
    );
    assert_eq!(get(exception, calendar::SUBJECT), Some("Moved"));
    assert_eq!(get(exception, calendar::LOCATION), None);
    // The series has a reminder, the exception has none: sent empty.
    assert_eq!(
        exception
            .child(calendar::REMINDER)
            .map(|r| r.as_bytes().len()),
        Some(0)
    );
    assert_eq!(get(exception, calendar::DELETED), None);

    let deleted = to_ical(
        &application_data(&series),
        Some(&text),
        Omitted::Cleared,
        &context(V14),
    )
    .unwrap();
    assert!(!deleted.contains("RECURRENCE-ID"), "{deleted}");
    assert!(deleted.contains("EXDATE;TZID=America/Los_Angeles:20090424T100000"));
}

/// 16.1 changes and deletes instances one at a time, and a series whose
/// times change loses its exceptions.
#[test]
fn instances_in_16_1() {
    let ctx = context(V16);
    let series = add(
        &format!(
            "{}<calendar:StartTime>20261124T090000Z</calendar:StartTime>\
             <calendar:EndTime>20261124T100000Z</calendar:EndTime>\
             <calendar:Subject>Standup</calendar:Subject>\
             <airsyncbase:Location><airsyncbase:DisplayName>Room 1</airsyncbase:DisplayName></airsyncbase:Location>\
             <calendar:Recurrence><calendar:Type>0</calendar:Type><calendar:Interval>1</calendar:Interval></calendar:Recurrence>",
            timezone_xml(chrono_tz::Europe::Berlin)
        ),
        V16,
    );
    let change = application_data(
        "<calendar:Subject>Long standup</calendar:Subject>\
         <calendar:EndTime>20261125T110000Z</calendar:EndTime>",
    );
    let text = change_instance(&series, "20261125T090000Z", &change, &ctx).unwrap();
    assert!(
        text.contains("RECURRENCE-ID;TZID=Europe/Berlin:20261125T100000"),
        "{text}"
    );
    assert!(text.contains("DTEND;TZID=Europe/Berlin:20261125T120000"));
    assert_eq!(text.matches("LOCATION:Room 1").count(), 2);
    let text = delete_instance(&text, "2026-11-26T09:00:00.000Z", &ctx).unwrap();
    assert!(
        text.contains("EXDATE;TZID=Europe/Berlin:20261126T100000"),
        "{text}"
    );

    let back = from_ical(&text, &ctx).unwrap();
    let exceptions = back.child(calendar::EXCEPTIONS).unwrap().children();
    assert_eq!(exceptions.len(), 2);
    assert_eq!(
        get(&exceptions[0], airsyncbase::INSTANCE_ID),
        Some("20261125T090000Z")
    );
    assert_eq!(get(&exceptions[0], calendar::SUBJECT), Some("Long standup"));
    assert_eq!(get(&exceptions[0], calendar::EXCEPTION_START_TIME), None);
    assert_eq!(get(&exceptions[1], calendar::DELETED), Some("1"));
    assert_eq!(
        back.child(airsyncbase::LOCATION)
            .and_then(|l| l.child_str(airsyncbase::DISPLAY_NAME)),
        Some("Room 1")
    );

    // Moving the series drops its exceptions ([MS-ASCAL] 2.2.2.22).
    let moved = application_data(
        "<calendar:StartTime>20261124T080000Z</calendar:StartTime>\
         <calendar:EndTime>20261124T090000Z</calendar:EndTime>",
    );
    let text = to_ical(&moved, Some(&text), Omitted::Kept, &ctx).unwrap();
    assert!(!text.contains("RECURRENCE-ID"), "{text}");
    assert!(!text.contains("EXDATE"));
    assert!(text.contains("SUMMARY:Standup"));
    assert!(text.contains("SEQUENCE:1"));
    assert!(change_instance("garbage", "20261125T090000Z", &change, &ctx).is_err());
    assert!(delete_instance(&series, "yesterday", &ctx).is_err());
}

/// All-day events: dates in iCalendar; midnights in the event's zone in
/// 14.1, bare dates without a zone in 16.1 ([MS-ASCAL] 2.2.2.1).
#[test]
fn all_day_events() {
    let text = add(
        &format!(
            "{}<calendar:AllDayEvent>1</calendar:AllDayEvent>\
             <calendar:StartTime>20261123T230000Z</calendar:StartTime>\
             <calendar:EndTime>20261124T230000Z</calendar:EndTime>\
             <calendar:Subject>Holiday</calendar:Subject>\
             <calendar:Recurrence><calendar:Type>1</calendar:Type><calendar:DayOfWeek>2</calendar:DayOfWeek>\
             <calendar:Until>20261228T230000Z</calendar:Until></calendar:Recurrence>",
            timezone_xml(chrono_tz::Europe::Berlin)
        ),
        V14,
    );
    assert!(text.contains("DTSTART;VALUE=DATE:20261124"), "{text}");
    assert!(text.contains("DTEND;VALUE=DATE:20261125"));
    assert!(text.contains("UNTIL=20261229;"), "{text}");
    let back = from_ical(&text, &context(V14)).unwrap();
    assert_eq!(get(&back, calendar::START_TIME), Some("20261123T230000Z"));
    assert_eq!(get(&back, calendar::END_TIME), Some("20261124T230000Z"));
    assert_eq!(get(&back, calendar::ALL_DAY_EVENT), Some("1"));
    let november = dates::parse("20261124T000000Z").unwrap();
    assert!(blob(&back).same_rules(&rules_for_zone(chrono_tz::Europe::Berlin, november)));
    let recurrence = back.child(calendar::RECURRENCE).unwrap();
    assert_eq!(get(recurrence, calendar::UNTIL), Some("20261228T230000Z"));

    let text = add(
        "<calendar:AllDayEvent>1</calendar:AllDayEvent>\
         <calendar:StartTime>20261124T000000Z</calendar:StartTime>\
         <calendar:EndTime>20261126T000000Z</calendar:EndTime>\
         <calendar:Subject>Trip</calendar:Subject>",
        V16,
    );
    assert!(text.contains("DTSTART;VALUE=DATE:20261124"), "{text}");
    assert!(text.contains("DTEND;VALUE=DATE:20261126"));
    assert!(!text.contains("VTIMEZONE"));
    let back = from_ical(&text, &context(V16)).unwrap();
    assert_eq!(back.child(calendar::TIMEZONE), None);
    assert_eq!(get(&back, calendar::START_TIME), Some("20261124T000000Z"));
    assert_eq!(get(&back, calendar::END_TIME), Some("20261126T000000Z"));
}

/// The `Timezone` blob both ways for the zones the plan names: the event
/// gets the IANA zone, and the device gets a blob with the same rules.
#[test]
fn timezone_round_trips() {
    for zone in [
        chrono_tz::UTC,
        chrono_tz::Europe::Berlin,
        chrono_tz::America::New_York,
        chrono_tz::Africa::Lagos,
        chrono_tz::Australia::Sydney,
    ] {
        for start in ["20260115T100000Z", "20260715T100000Z"] {
            let text = add(
                &format!(
                    "{}<calendar:StartTime>{start}</calendar:StartTime>\
                     <calendar:EndTime>{start}</calendar:EndTime><calendar:Subject>Zone</calendar:Subject>",
                    timezone_xml(zone)
                ),
                V14,
            );
            if zone == chrono_tz::UTC {
                assert!(text.contains(&format!("DTSTART:{start}")), "{text}");
                assert!(!text.contains("VTIMEZONE"));
            } else {
                assert!(
                    text.contains(&format!("DTSTART;TZID={}:", zone.name())),
                    "{text}"
                );
                assert!(text.contains(&format!("TZID:{}", zone.name())), "{text}");
            }
            let back = from_ical(&text, &context(V14)).unwrap();
            assert_eq!(get(&back, calendar::START_TIME), Some(start));
            let at = dates::parse(start).unwrap();
            assert!(blob(&back).same_rules(&rules_for_zone(zone, at)), "{zone}");
        }
    }
}

/// A blob matching no IANA zone gets its own VTIMEZONE, and the device
/// gets the same rules back.
#[test]
fn custom_zones_get_their_own_vtimezone() {
    let berlin = rules_for_zone(
        chrono_tz::Europe::Berlin,
        dates::parse("20260101T000000Z").unwrap(),
    );
    let odd = TimeZoneRules {
        bias: -17,
        standard_name: "Mars Standard Time".into(),
        daylight_name: "Mars Daylight Time".into(),
        ..berlin
    };
    let text = add(
        &format!(
            "<calendar:Timezone>{}</calendar:Timezone>\
             <calendar:StartTime>20260715T100000Z</calendar:StartTime>\
             <calendar:EndTime>20260715T110000Z</calendar:EndTime>",
            odd.to_base64()
        ),
        V14,
    );
    assert!(text.contains("TZID:Mars Standard Time"), "{text}");
    assert!(
        text.contains("DTSTART;TZID=\"Mars Standard Time\":"),
        "{text}"
    );
    let back = from_ical(&text, &context(V14)).unwrap();
    assert_eq!(get(&back, calendar::START_TIME), Some("20260715T100000Z"));
    assert!(blob(&back).same_rules(&odd));
}

/// Nextcloud's properties, parameters and alarm settings survive an edit
/// on a phone, in 16.1 (only what is sent) and in 14.1 (everything sent).
#[test]
fn unknown_properties_survive_an_update() {
    let stored = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Nextcloud//EN\r\nX-WR-CALNAME:Work\r\n\
        BEGIN:VEVENT\r\nUID:keep-1\r\nDTSTAMP:20261101T000000Z\r\n\
        DTSTART;TZID=Europe/Berlin:20261124T100000\r\nDTEND;TZID=Europe/Berlin:20261124T110000\r\n\
        SUMMARY;LANGUAGE=de:Besprechung\r\nLOCATION:Raum 1\r\nCOLOR:red\r\n\
        X-NEXTCLOUD-THING;X-PARAM=1:kept\r\nURL:https://example.com/x\r\nCLASS:PUBLIC\r\n\
        ATTACH;FMTTYPE=text/plain:https://example.com/a.txt\r\n\
        BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT10M\r\nX-ALARM-KEEP:yes\r\nEND:VALARM\r\n\
        END:VEVENT\r\nBEGIN:VTODO\r\nUID:other\r\nSUMMARY:Not mine\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
    let check = |text: &str| {
        for line in [
            "X-WR-CALNAME:Work",
            "SUMMARY;LANGUAGE=de:Meeting",
            "LOCATION:Raum 1",
            "COLOR:red",
            "X-NEXTCLOUD-THING;X-PARAM=1:kept",
            "URL:https://example.com/x",
            "CLASS:PUBLIC",
            "ATTACH;FMTTYPE=text/plain:https://example.com/a.txt",
            "X-ALARM-KEEP:yes",
            "TRIGGER:-PT10M",
            "DTSTART;TZID=Europe/Berlin:20261124T100000",
            "SUMMARY:Not mine",
            "UID:keep-1",
        ] {
            assert!(text.contains(line), "{line} missing from\n{text}");
        }
        assert!(!text.contains("SEQUENCE"));
    };
    let subject_only = application_data("<calendar:Subject>Meeting</calendar:Subject>");
    check(&to_ical(&subject_only, Some(stored), Omitted::Kept, &context(V16)).unwrap());

    let mut full = from_ical(stored, &context(V14)).unwrap();
    if let crate::wbxml::Content::Children(children) = &mut full.content {
        for child in children
            .iter_mut()
            .filter(|child| child.tag == calendar::SUBJECT)
        {
            *child = Element::text(calendar::SUBJECT, "Meeting");
        }
    }
    check(&to_ical(&full, Some(stored), Omitted::Cleared, &context(V14)).unwrap());
}

/// What a 14.1 `Change` leaves out goes, unless it is ghosted.
#[test]
fn omitted_elements_follow_supported() {
    let stored = add(
        &format!(
            "{}<calendar:StartTime>20261124T090000Z</calendar:StartTime>\
             <calendar:EndTime>20261124T100000Z</calendar:EndTime>\
             <calendar:Subject>Plan</calendar:Subject><calendar:Location>Here</calendar:Location>\
             <calendar:Reminder>5</calendar:Reminder>",
            timezone_xml(chrono_tz::Europe::Berlin)
        ),
        V14,
    );
    let change = application_data("<calendar:Subject>Plan B</calendar:Subject>");
    let managed = [calendar::SUBJECT, calendar::LOCATION];
    let text = to_ical(
        &change,
        Some(&stored),
        Omitted::Supported(&managed),
        &context(V14),
    )
    .unwrap();
    assert!(!text.contains("LOCATION"), "{text}");
    assert!(text.contains("TRIGGER:-PT5M"));
    let text = to_ical(&change, Some(&stored), Omitted::Cleared, &context(V14)).unwrap();
    assert!(!text.contains("VALARM"));
    // Times are never cleared by leaving them out.
    assert!(text.contains("DTSTART;TZID=Europe/Berlin:20261124T100000"));
}

/// The smaller mappings: sensitivity, busy status, categories, reminders,
/// counter proposals and replies.
#[test]
fn property_mappings() {
    let xml = format!(
        "{}<calendar:StartTime>20261124T090000Z</calendar:StartTime>\
         <calendar:EndTime>20261124T100000Z</calendar:EndTime>\
         <calendar:Sensitivity>3</calendar:Sensitivity><calendar:BusyStatus>0</calendar:BusyStatus>\
         <calendar:Categories><calendar:Category>Red</calendar:Category><calendar:Category>Blue, too</calendar:Category></calendar:Categories>\
         <calendar:Reminder>0</calendar:Reminder><calendar:DisallowNewTimeProposal>1</calendar:DisallowNewTimeProposal>\
         <calendar:ResponseRequested>0</calendar:ResponseRequested>\
         <calendar:Attendees><calendar:Attendee><calendar:Email>\"Bo\" &lt;bo@example.com&gt;</calendar:Email>\
         <calendar:Name>Bo</calendar:Name><calendar:AttendeeType>3</calendar:AttendeeType></calendar:Attendee></calendar:Attendees>\
         <airsyncbase:Body><airsyncbase:Type>1</airsyncbase:Type><airsyncbase:Data>Agenda:\r\n1. Coffee</airsyncbase:Data></airsyncbase:Body>",
        timezone_xml(chrono_tz::Europe::Berlin)
    );
    let text = add(&xml, V14);
    // Long lines are folded at 75 octets; the checks read them whole.
    let unfolded = text.replace("\r\n ", "");
    for line in [
        "CLASS:CONFIDENTIAL",
        "TRANSP:TRANSPARENT",
        "CATEGORIES:Red,Blue\\, too",
        "TRIGGER:PT0S",
        "X-MICROSOFT-DISALLOW-COUNTER:TRUE",
        "RSVP=FALSE",
        "CUTYPE=RESOURCE",
        "mailto:bo@example.com",
        "DESCRIPTION:Agenda:\\n1. Coffee",
        // The account organizes the meetings it makes.
        "ORGANIZER;CN=\"Me Myself\":mailto:me@example.com",
    ] {
        assert!(unfolded.contains(line), "{line} missing from\n{text}");
    }
    let back = from_ical(&text, &context(V14)).unwrap();
    assert_eq!(get(&back, calendar::SENSITIVITY), Some("3"));
    assert_eq!(get(&back, calendar::BUSY_STATUS), Some("0"));
    assert_eq!(get(&back, calendar::REMINDER), Some("0"));
    assert_eq!(get(&back, calendar::DISALLOW_NEW_TIME_PROPOSAL), Some("1"));
    assert_eq!(get(&back, calendar::RESPONSE_REQUESTED), Some("0"));
    assert_eq!(get(&back, calendar::MEETING_STATUS), Some("1"));
    let categories: Vec<_> = back
        .child(calendar::CATEGORIES)
        .unwrap()
        .children()
        .iter()
        .filter_map(Element::as_str)
        .collect();
    assert_eq!(categories, ["Red", "Blue, too"]);
    let body = back.child(airsyncbase::BODY).unwrap();
    assert_eq!(
        body.child_str(airsyncbase::DATA),
        Some("Agenda:\r\n1. Coffee")
    );

    // "Working elsewhere" is only 16.1's.
    let elsewhere = add(
        &xml.replace("<calendar:BusyStatus>0", "<calendar:BusyStatus>4"),
        V14,
    );
    assert!(elsewhere.contains("X-MICROSOFT-CDO-BUSYSTATUS:WORKINGELSEWHERE"));
    assert_eq!(
        get(
            &from_ical(&elsewhere, &context(V14)).unwrap(),
            calendar::BUSY_STATUS
        ),
        Some("2")
    );
    assert_eq!(
        get(
            &from_ical(&elsewhere, &context(V16)).unwrap(),
            calendar::BUSY_STATUS
        ),
        Some("4")
    );
}

/// 16.1: the server makes the `UID` and keeps `ClientUid`; `Location` is
/// AirSyncBase's, with coordinates.
#[test]
fn version_16_identity_and_location() {
    let text = add(
        "<calendar:StartTime>20261124T090000Z</calendar:StartTime>\
         <calendar:EndTime>20261124T100000Z</calendar:EndTime>\
         <calendar:UID>device-made</calendar:UID><calendar:ClientUid>client-1</calendar:ClientUid>\
         <airsyncbase:Location><airsyncbase:DisplayName>Brandenburger Tor</airsyncbase:DisplayName>\
         <airsyncbase:Latitude>52.5163</airsyncbase:Latitude><airsyncbase:Longitude>13.3777</airsyncbase:Longitude></airsyncbase:Location>",
        V16,
    );
    let made = uid(&text).unwrap().unwrap();
    assert_ne!(made, "device-made");
    assert_eq!(made.len(), 36);
    assert_eq!(client_uid(&text).unwrap().as_deref(), Some("client-1"));
    assert!(text.contains("GEO:52.5163;13.3777"), "{text}");
    let back = from_ical(&text, &context(V16)).unwrap();
    let location = back.child(airsyncbase::LOCATION).unwrap();
    assert_eq!(
        location.child_str(airsyncbase::DISPLAY_NAME),
        Some("Brandenburger Tor")
    );
    assert_eq!(location.child_str(airsyncbase::LATITUDE), Some("52.5163"));
    assert_eq!(back.child(calendar::LOCATION), None);
    // 14.1 takes the device's UID.
    let text = add(
        "<calendar:StartTime>20261124T090000Z</calendar:StartTime>\
         <calendar:EndTime>20261124T100000Z</calendar:EndTime><calendar:UID>device-made</calendar:UID>",
        V14,
    );
    assert_eq!(uid(&text).unwrap().as_deref(), Some("device-made"));
}

/// [MS-ASCAL] 3.2.4.4: an event added without times. "Now" is 18:03.
#[test]
fn missing_times_are_filled_in() {
    let text = add("<calendar:Subject>Now</calendar:Subject>", V14);
    assert!(text.contains("DTSTART:20261123T183000Z"), "{text}");
    assert!(text.contains("DTEND:20261123T190000Z"));
    let text = add("<calendar:EndTime>20261123T200000Z</calendar:EndTime>", V14);
    assert!(text.contains("DTSTART:20261123T183000Z"), "{text}");
    let text = add(
        "<calendar:StartTime>20261123T120000Z</calendar:StartTime>",
        V14,
    );
    assert!(text.contains("DTEND:20261123T190000Z"), "{text}");
    for xml in [
        "<calendar:EndTime>20261123T100000Z</calendar:EndTime>",
        "<calendar:StartTime>20261124T120000Z</calendar:StartTime>",
    ] {
        assert!(
            to_ical(
                &application_data(xml),
                None,
                Omitted::Cleared,
                &context(V14)
            )
            .is_err(),
            "{xml}"
        );
    }
}

/// Bad input gives an error, never a panic.
#[test]
fn garbage_is_refused() {
    let ctx = context(V14);
    let stored = add(
        "<calendar:StartTime>20261123T090000Z</calendar:StartTime>",
        V14,
    );
    for xml in [
        "<calendar:Timezone>!!!</calendar:Timezone>",
        "<calendar:Timezone>AAAA</calendar:Timezone>",
        "<calendar:EndTime>tomorrow</calendar:EndTime>",
        "<calendar:Recurrence><calendar:Type>9</calendar:Type></calendar:Recurrence>",
        "<calendar:Attendees><calendar:Attendee><calendar:Name>No mail</calendar:Name></calendar:Attendee></calendar:Attendees>",
        "<calendar:Sensitivity>7</calendar:Sensitivity>",
        "<calendar:BusyStatus>x</calendar:BusyStatus>",
        "<calendar:AllDayEvent>2</calendar:AllDayEvent>",
        "<calendar:MeetingStatus>2</calendar:MeetingStatus>",
        "<calendar:Exceptions><calendar:Exception><calendar:Subject>?</calendar:Subject></calendar:Exception></calendar:Exceptions>",
        "<airsyncbase:Body><airsyncbase:Data>no type</airsyncbase:Data></airsyncbase:Body>",
    ] {
        let data = application_data(&format!(
            "<calendar:StartTime>20261123T090000Z</calendar:StartTime>{xml}"
        ));
        assert!(
            to_ical(&data, Some(&stored), Omitted::Cleared, &ctx).is_err(),
            "{xml}"
        );
    }
    let data = application_data("<calendar:Subject>x</calendar:Subject>");
    for text in [
        "",
        "garbage",
        "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n",
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:no start\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nDTSTART:2026\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nDTSTART:20260230T100000Z\r\nEND:VEVENT\r\n",
        "BEGIN:VCARD\r\nFN:x\r\nEND:VCARD\r\n",
    ] {
        assert!(from_ical(text, &ctx).is_err(), "{text:?}");
        assert!(
            to_ical(&data, Some(text), Omitted::Cleared, &ctx).is_err(),
            "{text:?}"
        );
    }
    // Odd but readable events do not fail: an RRULE nobody can expand, a
    // zone nobody knows, an end before the start.
    let odd = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:o\r\nDTSTART;TZID=Nowhere/City:20261124T100000\r\n\
        DTEND;TZID=Nowhere/City:20261124T090000\r\nRRULE:FREQ=SECONDLY;BYMONTH=2;BYMONTHDAY=31\r\n\
        EXDATE:garbage\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let back = from_ical(odd, &ctx).unwrap();
    assert_eq!(
        get(&back, calendar::START_TIME),
        get(&back, calendar::END_TIME)
    );
}
