/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The Task class both ways, with [MS-ASTASK] section 4's examples.

use super::*;
use crate::items::fixtures::{application_data, context};
use crate::request::Version;
use crate::wbxml::codepages::airsyncbase;

/// The examples' device is in Pacific Time (`DueDate` 13:00 is
/// `UtcDueDate` 20:00 in September).
fn pacific() -> Context<'static> {
    Context {
        zone: chrono_tz::America::Los_Angeles,
        ..context(Version::V14_1)
    }
}

fn get(data: &Element, tag: Tag) -> Option<&str> {
    data.child_str(tag)
}

/// [MS-ASTASK] 4.1: the task a device adds.
const SPEC_4_1: &str = r#"
  <airsyncbase:Body>
    <airsyncbase:Type>2</airsyncbase:Type>
    <airsyncbase:Data>&lt;strong&gt;Must&lt;/strong&gt; complete TPS reports using
the new cover sheet.</airsyncbase:Data>
  </airsyncbase:Body>
  <tasks:Subject>TPS Reports for August 2009</tasks:Subject>
  <tasks:Importance>2</tasks:Importance>
  <tasks:Categories>
    <tasks:Category>Business</tasks:Category>
    <tasks:Category>Reports</tasks:Category>
  </tasks:Categories>
  <tasks:Complete>0</tasks:Complete>
  <tasks:DueDate>2009-09-03T13:00:00.000Z</tasks:DueDate>
  <tasks:UtcDueDate>2009-09-03T20:00:00.000Z</tasks:UtcDueDate>
  <tasks:ReminderSet>1</tasks:ReminderSet>
  <tasks:ReminderTime>2009-09-02T09:00:00.000Z</tasks:ReminderTime>
  <tasks:Sensitivity>1</tasks:Sensitivity>
  <tasks:StartDate>2009-09-03T09:00:00.000Z</tasks:StartDate>
  <tasks:UtcStartDate>2009-09-03T16:00:00.000Z</tasks:UtcStartDate>"#;

/// [MS-ASTASK] 4.2: the `Change` the server sends for `19:1`, a task due
/// on a day.
const SPEC_4_2: &str = r#"
  <airsyncbase:Body>
    <airsyncbase:Type>3</airsyncbase:Type>
    <airsyncbase:EstimatedDataSize>5731</airsyncbase:EstimatedDataSize>
    <airsyncbase:Truncated>1</airsyncbase:Truncated>
  </airsyncbase:Body>
  <tasks:Subject>Finish Q4 sales roll-up</tasks:Subject>
  <tasks:Importance>1</tasks:Importance>
  <tasks:UtcStartDate>2008-10-02T07:00:00.000Z</tasks:UtcStartDate>
  <tasks:StartDate>2008-10-02T00:00:00.000Z</tasks:StartDate>
  <tasks:UtcDueDate>2008-10-10T07:00:00.000Z</tasks:UtcDueDate>
  <tasks:DueDate>2008-10-10T00:00:00.000Z</tasks:DueDate>
  <tasks:Complete>0</tasks:Complete>
  <tasks:Sensitivity>0</tasks:Sensitivity>
  <tasks:ReminderTime>2008-10-10T19:30:00.000Z</tasks:ReminderTime>
  <tasks:ReminderSet>1</tasks:ReminderSet>"#;

#[test]
fn spec_4_1_a_task_with_times() {
    let ctx = pacific();
    let text = to_ical(&application_data(SPEC_4_1), None, Omitted::Cleared, &ctx).unwrap();
    for line in [
        "BEGIN:VTODO",
        "SUMMARY:TPS Reports for August 2009",
        "DESCRIPTION:Must complete TPS reports using\\nthe new cover sheet.",
        "X-ALT-DESC;FMTTYPE=text/html:<strong>Must</strong>",
        "PRIORITY:1",
        "CATEGORIES:Business,Reports",
        "DUE:20090903T200000Z",
        "DTSTART:20090903T160000Z",
        "CLASS:PRIVATE",
        "TRIGGER;VALUE=DATE-TIME:20090902T090000Z",
    ] {
        assert!(text.contains(line), "{line} missing from\n{text}");
    }
    assert!(!text.contains("STATUS"));
    let back = from_ical(&text, &ctx).unwrap();
    let sent = application_data(SPEC_4_1);
    for tag in [
        tasks::SUBJECT,
        tasks::IMPORTANCE,
        tasks::DUE_DATE,
        tasks::UTC_DUE_DATE,
        tasks::START_DATE,
        tasks::UTC_START_DATE,
        tasks::REMINDER_SET,
        tasks::REMINDER_TIME,
        tasks::COMPLETE,
    ] {
        assert_eq!(get(&back, tag), get(&sent, tag), "{tag:?}");
    }
    // Personal and private are both CLASS:PRIVATE.
    assert_eq!(get(&back, tasks::SENSITIVITY), Some("2"));
    let categories = back.child(tasks::CATEGORIES).unwrap();
    assert_eq!(categories.children().len(), 2);
}

#[test]
fn spec_4_2_a_task_due_on_a_day() {
    let ctx = pacific();
    let text = to_ical(&application_data(SPEC_4_2), None, Omitted::Cleared, &ctx).unwrap();
    for line in [
        "DTSTART;VALUE=DATE:20081002",
        "DUE;VALUE=DATE:20081010",
        "TRIGGER;VALUE=DATE-TIME:20081010T193000Z",
    ] {
        assert!(text.contains(line), "{line} missing from\n{text}");
    }
    // Normal importance and sensitivity need no lines; RTF is not converted.
    assert!(!text.contains("PRIORITY"));
    assert!(!text.contains("CLASS"));
    assert_eq!(text.matches("DESCRIPTION").count(), 1, "only the alarm's");
    let back = from_ical(&text, &ctx).unwrap();
    let sent = application_data(SPEC_4_2);
    for tag in [
        tasks::UTC_START_DATE,
        tasks::START_DATE,
        tasks::UTC_DUE_DATE,
        tasks::DUE_DATE,
        tasks::IMPORTANCE,
        tasks::SENSITIVITY,
        tasks::REMINDER_TIME,
        tasks::REMINDER_SET,
        tasks::COMPLETE,
    ] {
        assert_eq!(get(&back, tag), get(&sent, tag), "{tag:?}");
    }
    let body = back.child(airsyncbase::BODY).unwrap();
    assert_eq!(body.child(airsyncbase::DATA), None);
}

#[test]
fn completion_both_ways() {
    let ctx = context(Version::V16_1);
    let done = application_data(
        "<tasks:Subject>Done</tasks:Subject><tasks:Complete>1</tasks:Complete>\
         <tasks:DateCompleted>2026-11-20T10:00:00.000Z</tasks:DateCompleted>",
    );
    let text = to_ical(&done, None, Omitted::Cleared, &ctx).unwrap();
    for line in [
        "STATUS:COMPLETED",
        "COMPLETED:20261120T100000Z",
        "PERCENT-COMPLETE:100",
    ] {
        assert!(text.contains(line), "{line} missing from\n{text}");
    }
    let back = from_ical(&text, &ctx).unwrap();
    assert_eq!(get(&back, tasks::COMPLETE), Some("1"));
    assert_eq!(
        get(&back, tasks::DATE_COMPLETED),
        Some("2026-11-20T10:00:00.000Z")
    );

    let undone =
        application_data("<tasks:Subject>Done</tasks:Subject><tasks:Complete>0</tasks:Complete>");
    let text = to_ical(&undone, Some(&text), Omitted::Cleared, &ctx).unwrap();
    assert!(text.contains("STATUS:NEEDS-ACTION"), "{text}");
    assert!(!text.contains("COMPLETED:"));
    assert!(!text.contains("PERCENT-COMPLETE"));
    assert_eq!(
        get(&from_ical(&text, &ctx).unwrap(), tasks::COMPLETE),
        Some("0")
    );
}

#[test]
fn recurrence_with_regenerate() {
    let ctx = context(Version::V14_1);
    let sent = application_data(
        "<tasks:Subject>Water the plants</tasks:Subject>\
         <tasks:StartDate>2026-11-23T00:00:00.000Z</tasks:StartDate>\
         <tasks:UtcStartDate>2026-11-22T23:00:00.000Z</tasks:UtcStartDate>\
         <tasks:Recurrence><tasks:Type>1</tasks:Type><tasks:Start>2026-11-23T00:00:00.000Z</tasks:Start>\
         <tasks:Until>2026-12-28T00:00:00.000Z</tasks:Until><tasks:Interval>1</tasks:Interval>\
         <tasks:DayOfWeek>2</tasks:DayOfWeek><tasks:Regenerate>1</tasks:Regenerate>\
         <tasks:DeadOccur>0</tasks:DeadOccur></tasks:Recurrence>",
    );
    let text = to_ical(&sent, None, Omitted::Cleared, &ctx).unwrap();
    assert!(text.contains("DTSTART;VALUE=DATE:20261123"), "{text}");
    assert!(
        text.contains("RRULE:FREQ=WEEKLY;UNTIL=20261228;BYDAY=MO"),
        "{text}"
    );
    assert!(text.contains("X-ACTIVESYNC-REGENERATE:1"));
    assert!(!text.contains("DEADOCCUR"));
    let back = from_ical(&text, &ctx).unwrap();
    let recurrence = back.child(tasks::RECURRENCE).unwrap();
    assert_eq!(get(recurrence, tasks::TYPE), Some("1"));
    assert_eq!(get(recurrence, tasks::DAY_OF_WEEK), Some("2"));
    assert_eq!(
        get(recurrence, tasks::START),
        Some("2026-11-23T00:00:00.000Z")
    );
    assert_eq!(
        get(recurrence, tasks::UNTIL),
        Some("2026-12-28T00:00:00.000Z")
    );
    assert_eq!(get(recurrence, tasks::REGENERATE), Some("1"));
    assert_eq!(get(recurrence, tasks::FIRST_DAY_OF_WEEK), Some("1"));
    // Sending it back changes nothing in the rule.
    let again = to_ical(&back, Some(&text), Omitted::Cleared, &ctx).unwrap();
    assert!(again.contains("RRULE:FREQ=WEEKLY;UNTIL=20261228;BYDAY=MO"));

    let without_start =
        application_data("<tasks:Recurrence><tasks:Type>0</tasks:Type></tasks:Recurrence>");
    assert!(to_ical(&without_start, None, Omitted::Cleared, &ctx).is_err());
}

/// Importance and reminders left out of a change stay ([MS-ASTASK]
/// 3.2.5.3.1, 3.2.5.3.2); Nextcloud's own fields stay too.
#[test]
fn updates_keep_what_they_do_not_carry() {
    let ctx = context(Version::V14_1);
    let stored = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VTODO\r\nUID:t1\r\n\
        SUMMARY:Old\r\nPRIORITY:2\r\nSTATUS:IN-PROCESS\r\nPERCENT-COMPLETE:50\r\n\
        RELATED-TO:parent-task\r\nX-NC-SORT:3\r\nDUE;VALUE=DATE:20261130\r\n\
        BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER;VALUE=DATE-TIME:20261129T080000Z\r\nEND:VALARM\r\n\
        END:VTODO\r\nEND:VCALENDAR\r\n";
    let change = application_data(
        "<tasks:Subject>New</tasks:Subject><tasks:Complete>0</tasks:Complete>\
         <tasks:DueDate>2026-11-30T00:00:00.000Z</tasks:DueDate>\
         <tasks:UtcDueDate>2026-11-29T23:00:00.000Z</tasks:UtcDueDate>",
    );
    let text = to_ical(&change, Some(stored), Omitted::Cleared, &ctx).unwrap();
    for line in [
        "SUMMARY:New",
        "PRIORITY:2",
        "STATUS:IN-PROCESS",
        "PERCENT-COMPLETE:50",
        "RELATED-TO:parent-task",
        "X-NC-SORT:3",
        "DUE;VALUE=DATE:20261130",
        "TRIGGER;VALUE=DATE-TIME:20261129T080000Z",
        "UID:t1",
    ] {
        assert!(text.contains(line), "{line} missing from\n{text}");
    }
    let back = from_ical(&text, &ctx).unwrap();
    assert_eq!(get(&back, tasks::IMPORTANCE), Some("2"));
    assert_eq!(get(&back, tasks::REMINDER_SET), Some("1"));
    let off = application_data("<tasks:ReminderSet>0</tasks:ReminderSet>");
    let text = to_ical(&off, Some(&text), Omitted::Cleared, &ctx).unwrap();
    assert!(!text.contains("VALARM"));
    assert!(!text.contains("SUMMARY"), "a cleared subject goes");
}

#[test]
fn garbage_is_refused() {
    let ctx = context(Version::V14_1);
    for xml in [
        "<tasks:Complete>2</tasks:Complete>",
        "<tasks:Sensitivity>9</tasks:Sensitivity>",
        "<tasks:ReminderSet>yes</tasks:ReminderSet>",
        "<tasks:UtcDueDate>soon</tasks:UtcDueDate>",
        "<tasks:Recurrence><tasks:Type>1</tasks:Type><tasks:Start>2026-11-23T00:00:00.000Z</tasks:Start>\
         <tasks:Occurrences>300</tasks:Occurrences></tasks:Recurrence>",
    ] {
        assert!(
            to_ical(&application_data(xml), None, Omitted::Cleared, &ctx).is_err(),
            "{xml}"
        );
    }
    for text in [
        "",
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
    ] {
        assert!(from_ical(text, &ctx).is_err());
    }
}
