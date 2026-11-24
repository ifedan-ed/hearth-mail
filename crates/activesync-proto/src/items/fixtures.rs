/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Test fixtures copied from the specifications, each naming its section,
//! and the context the tests convert with.

use super::{BodyPreference, Context};
use crate::request::Version;
use chrono::{DateTime, Utc};

/// The Pacific Time blob of [MS-ASDTYPE] 3.6.6, line breaks as printed.
pub(crate) const PACIFIC_BLOB: &str =
    "4AEAACgARwBNAFQALQAwADgAOgAwADAAKQAgAFAAYQBjAGkAZgBpAGMAIABUAGkAbQBlACAAKABVA
      FMAIAAmACAAQwAAAAsAAAABAAIAAAAAAAAAAAAAACgARwBNAFQALQAwADgAOgAwADAAKQAgAFAAYQ
      BjAGkAZgBpAGMAIABUAGkAbQBlACAAKABVAFMAIAAmACAAQwAAAAMAAAACAAIAAAAAAAAAxP///w=
      =";

/// The account the tests convert for.
pub(crate) const ME: &str = "me@example.com";

/// A fixed "now": 2026-11-23 18:03 UTC, as in [MS-ASCAL] 3.2.4.4's
/// example of a request at 18:03.
pub(crate) fn now() -> DateTime<Utc> {
    DateTime::from_timestamp(1_795_456_980, 0).unwrap_or_default()
}

/// The addresses of [`ME`].
pub(crate) fn addresses() -> &'static [String] {
    static ADDRESSES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    ADDRESSES.get_or_init(|| vec![ME.to_string()])
}

/// A context for `version` in Berlin, plain-text bodies.
pub(crate) fn context(version: Version) -> Context<'static> {
    Context {
        version,
        zone: chrono_tz::Europe::Berlin,
        now: now(),
        addresses: addresses(),
        name: Some("Me Myself"),
        body: BodyPreference::default(),
    }
}

/// Wraps `ApplicationData` children copied from a specification example
/// in an element that declares the namespaces the examples use.
pub(crate) fn application_data(children: &str) -> crate::wbxml::Element {
    let xml = format!(
        "<ApplicationData xmlns=\"AirSync:\" xmlns:calendar=\"Calendar:\" \
         xmlns:airsyncbase=\"AirSyncBase:\" xmlns:contacts=\"Contacts:\" \
         xmlns:contacts2=\"Contacts2:\" xmlns:tasks=\"Tasks:\" xmlns:notes=\"Notes:\" \
         xmlns:email=\"Email:\" xmlns:email2=\"Email2:\" xmlns:A=\"AirSyncBase:\" \
         xmlns:B=\"POOMCONTACTS:\">{children}</ApplicationData>"
    );
    match crate::wbxml::from_xml(&xml) {
        Ok(element) => element,
        Err(error) => panic!("fixture is not ActiveSync XML: {error}"),
    }
}

/// [MS-ASCAL] 4.1, the `Change` of `1:12`: a one-off lunch meeting in
/// Pacific Time. Lines the PDF wrapped (the `Timezone` and `UID` values)
/// are joined.
pub(crate) const ASCAL_4_1_CHANGE: &str = r#"
  <calendar:Timezone>4AEAAFAAYQBjAGkAZgBpAGMAIABTAHQAYQBuAGQAYQByAGQAIABUAGkAbQBlAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAsAAAABAAIAAAAAAAAAAAAAAFAAYQBjAGkAZgBpAGMAIABEAGEAeQBsAGkAZwBoAHQAIABUAGkAbQBlAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAMAAAACAAIAAAAAAAAAxP///w==</calendar:Timezone>
  <calendar:DtStamp>20081002T231357Z</calendar:DtStamp>
  <calendar:StartTime>20081010T190000Z</calendar:StartTime>
  <calendar:Subject>Lunch meeting</calendar:Subject>
  <calendar:UID>040000008200E00074C5B7101A82E008000000001027EAEDA124C901000000000000000010000000C58EA426C0CFF24AB3125200707153B1</calendar:UID>
  <calendar:OrganizerName>Anat Kerry</calendar:OrganizerName>
  <calendar:OrganizerEmail>anat@contoso.com</calendar:OrganizerEmail>
  <calendar:Location>Cafeteria A</calendar:Location>
  <calendar:EndTime>20081010T203000Z</calendar:EndTime>
  <airsyncbase:Body>
    <airsyncbase:Type>3</airsyncbase:Type>
    <airsyncbase:EstimatedDataSize>5669</airsyncbase:EstimatedDataSize>
    <airsyncbase:Truncated>1</airsyncbase:Truncated>
  </airsyncbase:Body>
  <calendar:Sensitivity>0</calendar:Sensitivity>
  <calendar:BusyStatus>3</calendar:BusyStatus>
  <calendar:AllDayEvent>0</calendar:AllDayEvent>
  <calendar:Reminder>15</calendar:Reminder>
  <calendar:MeetingStatus>0</calendar:MeetingStatus>
  <airsyncbase:NativeBodyType>3</airsyncbase:NativeBodyType>
"#;

/// [MS-ASCAL] 4.1, the `Add` of `1:14`: a team meeting on the second
/// Monday of every month until July 2009.
pub(crate) const ASCAL_4_1_MONTHLY: &str = r#"
  <calendar:Timezone>4AEAAFAAYQBjAGkAZgBpAGMAIABTAHQAYQBuAGQAYQByAGQAIABUAGkAbQBlAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAsAAAABAAIAAAAAAAAAAAAAAFAAYQBjAGkAZgBpAGMAIABEAGEAeQBsAGkAZwBoAHQAIABUAGkAbQBlAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAMAAAACAAIAAAAAAAAAxP///w==</calendar:Timezone>
  <calendar:DtStamp>20081002T231639Z</calendar:DtStamp>
  <calendar:StartTime>20081013T190000Z</calendar:StartTime>
  <calendar:Subject>Team Meeting</calendar:Subject>
  <calendar:UID>040000008200E00074C5B7101A82E0080000000060043DFCA924C9010000000000000000100000097F14EF755AC454BA30EFA7B1B315E43</calendar:UID>
  <calendar:OrganizerName>Anat Kerry</calendar:OrganizerName>
  <calendar:OrganizerEmail>anat@contoso.com</calendar:OrganizerEmail>
  <calendar:Location>My office</calendar:Location>
  <calendar:EndTime>20081013T193000Z</calendar:EndTime>
  <calendar:Recurrence>
    <calendar:Type>3</calendar:Type>
    <calendar:Interval>1</calendar:Interval>
    <calendar:Until>20090713T190000Z</calendar:Until>
    <calendar:WeekOfMonth>2</calendar:WeekOfMonth>
    <calendar:DayOfWeek>2</calendar:DayOfWeek>
  </calendar:Recurrence>
  <airsyncbase:Body>
    <airsyncbase:Type>3</airsyncbase:Type>
    <airsyncbase:EstimatedDataSize>5769</airsyncbase:EstimatedDataSize>
    <airsyncbase:Truncated>1</airsyncbase:Truncated>
  </airsyncbase:Body>
  <calendar:Sensitivity>0</calendar:Sensitivity>
  <calendar:BusyStatus>2</calendar:BusyStatus>
  <calendar:AllDayEvent>0</calendar:AllDayEvent>
  <calendar:Reminder>15</calendar:Reminder>
  <calendar:MeetingStatus>0</calendar:MeetingStatus>
  <airsyncbase:NativeBodyType>3</airsyncbase:NativeBodyType>
"#;

/// [MS-ASCAL] 4.2: a weekly appointment on Fridays, three times, the
/// second deleted. The trailing space of the `OrganizerEmail` is the
/// specification's.
pub(crate) const ASCAL_4_2: &str = r#"
  <calendar:Timezone>4AEAACgARwBNAFQALQAwADgAOgAwADAAKQAgAFAAYQBjAGkAZgBpAGMAIABUAGkAbQBlACAAKABVAFMAIAAmACAAQwAAAAsAAAABAAIAAAAAAAAAAAAAACgARwBNAFQALQAwADgAOgAwADAAKQAgAFAAYQBjAGkAZgBpAGMAIABUAGkAbQBlACAAKABVAFMAIAAmACAAQwAAAAMAAAACAAIAAAAAAAAAxP///w==</calendar:Timezone>
  <calendar:DtStamp>20090415T165811Z</calendar:DtStamp>
  <calendar:StartTime>20090417T170000Z</calendar:StartTime>
  <calendar:Subject>Recurring appointment test</calendar:Subject>
  <calendar:UID>040000008200E00074C5B7101A82E00800000000B0CD1F52EBBDC901000000000000000010000000B05E442FCB2CA443BF3D99B51A729FE6</calendar:UID>
  <calendar:OrganizerName>Anat Kerry</calendar:OrganizerName>
  <calendar:OrganizerEmail>anat@contoso.com </calendar:OrganizerEmail>
  <calendar:Location>My office</calendar:Location>
  <calendar:EndTime>20090417T180000Z</calendar:EndTime>
  <calendar:Recurrence>
    <calendar:Type>1</calendar:Type>
    <calendar:Interval>1</calendar:Interval>
    <calendar:Occurrences>3</calendar:Occurrences>
    <calendar:DayOfWeek>32</calendar:DayOfWeek>
  </calendar:Recurrence>
  <airsyncbase:Body>
    <airsyncbase:Type>3</airsyncbase:Type>
    <airsyncbase:EstimatedDataSize>238</airsyncbase:EstimatedDataSize>
    <airsyncbase:Truncated>1</airsyncbase:Truncated>
  </airsyncbase:Body>
  <calendar:Sensitivity>0</calendar:Sensitivity>
  <calendar:BusyStatus>2</calendar:BusyStatus>
  <calendar:AllDayEvent>0</calendar:AllDayEvent>
  <calendar:Reminder>15</calendar:Reminder>
  <calendar:Exceptions>
    <calendar:Exception>
      <calendar:Deleted>1</calendar:Deleted>
      <calendar:ExceptionStartTime>20090424T170000Z</calendar:ExceptionStartTime>
    </calendar:Exception>
  </calendar:Exceptions>
  <calendar:MeetingStatus>0</calendar:MeetingStatus>
  <airsyncbase:NativeBodyType>3</airsyncbase:NativeBodyType>
  <calendar:ResponseRequested>1</calendar:ResponseRequested>
  <calendar:ResponseType>1</calendar:ResponseType>
"#;

/// [MS-ASCAL] 4.3, the first response: a meeting with one attendee who
/// has not answered (`AttendeeStatus` 0). The specification misspells one
/// tag `AteendeeType`, which is not well-formed XML; it is spelled right
/// here. The line breaks inside `Email` and `OrganizerName` are its own.
pub(crate) const ASCAL_4_3_NEW: &str = r#"
  <calendar:Timezone>4AEAAFAAYQBjAGkAZgBpAGMAIABTAHQAYQBuAGQAYQByAGQAIABUAGkAbQBlAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAoAAAAFAAIAAAAAAAAAAAAAAFAAYQBjAGkAZgBpAGMAIABEAGEAeQBsAGkAZwBoAHQAIABUAGkAbQBlAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAQAAAABAAIAAAAAAAAAxP///w==</calendar:Timezone>
  <calendar:DtStamp>20051103T010509Z</calendar:DtStamp>
  <calendar:StartTime>20051103T230000Z</calendar:StartTime>
  <calendar:Subject>test meeting</calendar:Subject>
  <calendar:UID>040000008200E00074C5B7101A82E00800000000B0FD68A212E0C5010000000000000000100000008C46B9A4960AF340871367CEC57B4543</calendar:UID>
  <calendar:Attendees>
    <calendar:Attendee>
      <calendar:Email>chris@fourthcoffee.com
      </calendar:Email>
      <calendar:Name>Chris Gray</calendar:Name>
      <calendar:AttendeeStatus>0</calendar:AttendeeStatus>
      <calendar:AttendeeType>1</calendar:AttendeeType>
    </calendar:Attendee>
  </calendar:Attendees>
  <calendar:OrganizerName>Anat Kerry
  </calendar:OrganizerName>
  <calendar:OrganizerEmail>anat@contoso.com</calendar:OrganizerEmail>
  <calendar:Location>34/1123</calendar:Location>
  <calendar:EndTime>20051104T000000Z</calendar:EndTime>
  <airsyncbase:Body>
    <airsyncbase:Type>1</airsyncbase:Type>
    <airsyncbase:EstimatedDataSize>28
    </airsyncbase:EstimatedDataSize>
  </airsyncbase:Body>
  <calendar:Sensitivity>0</calendar:Sensitivity>
  <calendar:BusyStatus>2</calendar:BusyStatus>
  <calendar:AllDayEvent>0</calendar:AllDayEvent>
  <calendar:Reminder>15</calendar:Reminder>
  <calendar:MeetingStatus>1</calendar:MeetingStatus>
"#;

/// [MS-ASCAL] 4.3, the second response: the attendee accepted
/// (`AttendeeStatus` 3).
pub(crate) const ASCAL_4_3_ACCEPTED: &str = r#"
  <calendar:Timezone>4AEAAFAAYQBjAGkAZgBpAGMAIABTAHQAYQBuAGQAYQByAGQAIABUAGkAbQBlAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAoAAAAFAAIAAAAAAAAAAAAAAFAAYQBjAGkAZgBpAGMAIABEAGEAeQBsAGkAZwBoAHQAIABUAGkAbQBlAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAQAAAABAAIAAAAAAAAAxP///w==
  </calendar:Timezone>
  <calendar:DtStamp>20051103T013759Z</calendar:DtStamp>
  <calendar:StartTime>20051103T230000Z</calendar:StartTime>
  <calendar:Subject>test meeting</calendar:Subject>
  <calendar:UID>040000008200E00074C5B7101A82E00800000000B0FD68A212E0C5010000000000000000100000008C46B9A4960AF340871367CEC57B4543</calendar:UID>
  <calendar:Attendees>
    <calendar:Attendee>
      <calendar:Email>chris@fourthcoffee.com
      </calendar:Email>
      <calendar:Name>Chris Gray</calendar:Name>
      <calendar:AttendeeStatus>3</calendar:AttendeeStatus>
      <calendar:AttendeeType>1</calendar:AttendeeType>
    </calendar:Attendee>
  </calendar:Attendees>
  <calendar:OrganizerName>Anat Kerry
  </calendar:OrganizerName>
  <calendar:OrganizerEmail>anat@contoso.com</calendar:OrganizerEmail>
  <calendar:Location>34/1123</calendar:Location>
  <calendar:EndTime>20051104T000000Z</calendar:EndTime>
  <airsyncbase:Body>
    <airsyncbase:Type>1</airsyncbase:Type>
    <airsyncbase:EstimatedDataSize>28</airsyncbase:EstimatedDataSize>
  </airsyncbase:Body>
  <calendar:Sensitivity>0</calendar:Sensitivity>
  <calendar:BusyStatus>2</calendar:BusyStatus>
  <calendar:AllDayEvent>0</calendar:AllDayEvent>
  <calendar:Reminder>15</calendar:Reminder>
  <calendar:MeetingStatus>1</calendar:MeetingStatus>
"#;
