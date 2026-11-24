/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Meeting requests and replies, with [MS-ASEMAIL] 4.1.5's examples.

use super::*;
use crate::items::{
    fixtures::{ME, context},
    timezone::TimeZoneRules,
};
use crate::request::Version;

/// The invitation behind [MS-ASEMAIL] 4.1.5.1's meeting request: Cafe,
/// 20 February 2009 9:30 to 10:30 Central Time, a 15-minute reminder.
const ONE_OFF: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nMETHOD:REQUEST\r\n\
    BEGIN:VEVENT\r\nUID:one-off-1\r\nDTSTAMP:20090219T083515Z\r\n\
    DTSTART;TZID=America/Chicago:20090220T093000\r\nDTEND;TZID=America/Chicago:20090220T103000\r\n\
    SUMMARY:Example Meeting Request\r\nLOCATION:Cafe\r\n\
    ORGANIZER;CN=Device User2:mailto:someone2@example.com\r\n\
    ATTENDEE;CN=Device User;RSVP=TRUE;PARTSTAT=NEEDS-ACTION;ROLE=REQ-PARTICIPANT:mailto:me@example.com\r\n\
    BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

/// The invitation behind 4.1.5.2's: the third Tuesday of every month at
/// 15:00 Central Time until 29 December 2009, sent again (sequence 1).
const MONTHLY: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:test\r\nMETHOD:REQUEST\r\n\
    BEGIN:VEVENT\r\nUID:monthly-1\r\nSEQUENCE:1\r\nDTSTAMP:20090219T084719Z\r\n\
    DTSTART;TZID=America/Chicago:20090317T150000\r\nDTEND;TZID=America/Chicago:20090317T160000\r\n\
    RRULE:FREQ=MONTHLY;UNTIL=20091229T210000Z;BYDAY=3TU\r\nSUMMARY:Monthly Meeting\r\n\
    LOCATION:My Office\r\nORGANIZER;CN=Device User2:mailto:someone2@example.com\r\n\
    ATTENDEE;RSVP=TRUE:mailto:me@example.com\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

/// The Central Time blob of both examples.
const CENTRAL: &str = "aAEAACgARwBNAFQALQAwADYAOgAwADAAKQAgAEMAZQBuAHQAcgBhAGwAIABUAGkAbQBlACAAKABVAFMAIAAmACAAQwAAAAsAAAABAAIAAAAAAAAAAAAAACgARwBNAFQALQAwADYAOgAwADAAKQAgAEMAZQBuAHQAcgBhAGwAIABUAGkAbQBlACAAKABVAFMAIAAmACAAQwAAAAMAAAACAAIAAAAAAAAAxP///w==";

fn get(data: &Element, tag: crate::wbxml::Tag) -> Option<&str> {
    data.child_str(tag)
}

#[test]
fn spec_4_1_5_1_one_off_request() {
    let request = meeting_request(ONE_OFF, &context(Version::V14_1)).unwrap();
    for (tag, value) in [
        (email::ALL_DAY_EVENT, "0"),
        (email::START_TIME, "2009-02-20T15:30:00.000Z"),
        (email::DT_STAMP, "2009-02-19T08:35:15.000Z"),
        (email::END_TIME, "2009-02-20T16:30:00.000Z"),
        (email::INSTANCE_TYPE, "0"),
        (email::LOCATION, "Cafe"),
        (email::ORGANIZER, "\"Device User2\" <someone2@example.com>"),
        (email::REMINDER, "900"),
        (email::RESPONSE_REQUESTED, "1"),
        (email::SENSITIVITY, "0"),
        (email::BUSY_STATUS, "2"),
        (email2::MEETING_MESSAGE_TYPE, "1"),
    ] {
        assert_eq!(get(&request, tag), Some(value), "{tag:?}");
    }
    let zone = TimeZoneRules::from_base64(get(&request, email::TIME_ZONE).unwrap()).unwrap();
    assert!(zone.same_rules(&TimeZoneRules::from_base64(CENTRAL).unwrap()));
    let id = STANDARD
        .decode(get(&request, email::GLOBAL_OBJ_ID).unwrap())
        .unwrap();
    assert_eq!(&id[..4], &[4, 0, 0, 0]);
    assert!(id.windows(9).any(|w| w == b"one-off-1"));
    assert_eq!(request.child(email::RECURRENCES), None);
    assert_eq!(
        message_class(ONE_OFF).unwrap(),
        "IPM.Schedule.Meeting.Request"
    );
}

#[test]
fn spec_4_1_5_2_recurring_request() {
    let request = meeting_request(MONTHLY, &context(Version::V14_1)).unwrap();
    assert_eq!(
        get(&request, email::START_TIME),
        Some("2009-03-17T20:00:00.000Z")
    );
    assert_eq!(get(&request, email::INSTANCE_TYPE), Some("1"));
    assert_eq!(get(&request, email2::MEETING_MESSAGE_TYPE), Some("2"));
    let recurrence = request
        .child(email::RECURRENCES)
        .and_then(|list| list.child(email::RECURRENCE))
        .unwrap();
    for (tag, value) in [
        (email::TYPE, "3"),
        (email::INTERVAL, "1"),
        (email::UNTIL, "20091229T210000Z"),
        (email::WEEK_OF_MONTH, "3"),
        (email::DAY_OF_WEEK, "4"),
    ] {
        assert_eq!(get(recurrence, tag), Some(value), "{tag:?}");
    }
}

#[test]
fn version_16_requests_name_the_uid() {
    let request = meeting_request(ONE_OFF, &context(Version::V16_1)).unwrap();
    assert_eq!(get(&request, calendar_tags::UID), Some("one-off-1"));
    assert_eq!(request.child(email::GLOBAL_OBJ_ID), None);
    let location = request.child(airsyncbase::LOCATION).unwrap();
    assert_eq!(location.child_str(airsyncbase::DISPLAY_NAME), Some("Cafe"));
}

#[test]
fn outlook_ids_are_kept() {
    let uid = "040000008200E00074C5B7101A82E00800000000B0CD1F52EBBDC901000000000000000010000000B05E442FCB2CA443BF3D99B51A729FE6";
    let id = global_object_id(uid, None);
    assert_eq!(id.len(), uid.len() / 2);
    let date = chrono::NaiveDate::from_ymd_opt(2004, 3, 9).unwrap();
    let instance = global_object_id(uid, Some(date));
    assert_eq!(&instance[16..20], &[0x07, 0xD4, 3, 9]);
    let vcal = global_object_id("abc", None);
    assert_eq!(
        u32::from_le_bytes([vcal[36], vcal[37], vcal[38], vcal[39]]),
        16
    );
    assert_eq!(&vcal[40..48], b"vCal-Uid");
    assert_eq!(vcal.len(), 40 + 16);
}

#[test]
fn replies_answer_the_organizer() {
    let ctx = context(Version::V14_1);
    let text = reply(
        ONE_OFF,
        Response::Accepted,
        None,
        Some("See you there"),
        &ctx,
    )
    .unwrap();
    // Long lines are folded at 75 octets; the checks read them whole.
    let unfolded = text.replace("\r\n ", "");
    for line in [
        "METHOD:REPLY",
        "UID:one-off-1",
        "ORGANIZER;CN=\"Device User2\":mailto:someone2@example.com",
        "PARTSTAT=ACCEPTED",
        "mailto:me@example.com",
        "DTSTAMP:20261123T180300Z",
        "COMMENT:See you there",
        "DTSTART;TZID=America/Chicago:20090220T093000",
        "TZID:America/Chicago",
    ] {
        assert!(unfolded.contains(line), "{line} missing from\n{text}");
    }
    assert!(!text.contains("RSVP"));
    assert!(!text.contains("VALARM"));
    assert_eq!(
        message_class(&text).unwrap(),
        "IPM.Schedule.Meeting.Resp.Pos"
    );

    // One instance of a series: its RECURRENCE-ID and the sequence.
    let text = reply(
        MONTHLY,
        Response::Declined,
        Some("2009-04-21T20:00:00.000Z"),
        None,
        &ctx,
    )
    .unwrap();
    for line in [
        "RECURRENCE-ID;TZID=America/Chicago:20090421T150000",
        "SEQUENCE:1",
        "PARTSTAT=DECLINED",
    ] {
        assert!(text.contains(line), "{line} missing from\n{text}");
    }
    // The zone's observances have yearly rules; the answer has none.
    let answer = text.split("BEGIN:VEVENT").nth(1).unwrap();
    assert!(!answer.contains("RRULE"), "{text}");
    assert_eq!(
        message_class(&text).unwrap(),
        "IPM.Schedule.Meeting.Resp.Neg"
    );

    // An account invited through a list answers as itself.
    let list = ONE_OFF.replace("mailto:me@example.com", "mailto:team@example.com");
    let text = reply(&list, Response::Tentative, None, None, &ctx).unwrap();
    assert!(
        text.contains(&format!(
            "ATTENDEE;CN=\"Me Myself\";PARTSTAT=TENTATIVE:mailto:{ME}"
        )),
        "{text}"
    );
}

#[test]
fn responses_update_the_calendar_copy() {
    let ctx = context(Version::V14_1);
    let text = respond(ONE_OFF, Response::Tentative, None, &ctx).unwrap();
    assert!(!text.contains("METHOD"), "{text}");
    assert!(text.contains("PARTSTAT=TENTATIVE"));
    assert!(text.contains("BEGIN:VALARM"));
    assert_eq!(text.matches("ATTENDEE").count(), 1);
    let text = respond(MONTHLY, Response::Accepted, Some("20090421T200000Z"), &ctx).unwrap();
    assert!(
        text.contains("RECURRENCE-ID;TZID=America/Chicago:20090421T150000"),
        "{text}"
    );
    assert_eq!(text.matches("PARTSTAT=ACCEPTED").count(), 1);
    assert_eq!(text.matches("BEGIN:VEVENT").count(), 2);
}

#[test]
fn cancellations_and_garbage() {
    let ctx = context(Version::V14_1);
    let cancel = ONE_OFF.replace("METHOD:REQUEST", "METHOD:CANCEL");
    assert_eq!(
        message_class(&cancel).unwrap(),
        "IPM.Schedule.Meeting.Canceled"
    );
    assert!(meeting_request(&cancel, &ctx).is_ok());
    for text in ["", "garbage", "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n"] {
        assert!(meeting_request(text, &ctx).is_err());
        assert!(reply(text, Response::Accepted, None, None, &ctx).is_err());
        assert!(respond(text, Response::Accepted, None, &ctx).is_err());
    }
    let no_organizer = ONE_OFF.replace(
        "ORGANIZER;CN=Device User2:mailto:someone2@example.com\r\n",
        "",
    );
    assert!(reply(&no_organizer, Response::Accepted, None, None, &ctx).is_err());
    assert!(reply(MONTHLY, Response::Accepted, Some("not a time"), None, &ctx).is_err());
    assert_eq!(Response::from_user_response(4), None);
    assert_eq!(Response::from_user_response(2), Some(Response::Tentative));
}
