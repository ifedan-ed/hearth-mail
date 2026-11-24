//! End to end against a real Nextcloud: discovery, MKCALENDAR, events,
//! tasks, journals and contacts written, synced, updated and deleted under
//! ETag conditions, and Login Flow v2 started.
//!
//! Ignored by default: it needs a throwaway Nextcloud, which
//! tools/activesync/test-dav-nextcloud.sh starts before running it with
//! `--ignored` and these variables:
//! - `DAV_TEST_URL`: the DAV root (`http://<host>/remote.php/dav`),
//! - `DAV_TEST_USER`, `DAV_TEST_PASSWORD`: an account there,
//! - `DAV_TEST_SERVER`: the Nextcloud's web address, for Login Flow v2.
//!
//! Never point it at a Nextcloud with real data: it creates and deletes
//! calendars.

use std::time::{SystemTime, UNIX_EPOCH};

use dav_client::{
    Collection, CollectionKind, Components, Condition, Credentials, DavClient, DavError, Limits,
    LoginFlow, LoginPoll, SyncChanges, Url,
};

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"))
}

/// A suffix that keeps names from a previous run apart.
fn unique() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{nanos:x}")
}

fn event(uid: &str, summary: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Hearth//dav-client test//EN\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nDTSTAMP:20261123T120000Z\r\nDTSTART:20261201T090000Z\r\nDTEND:20261201T100000Z\r\nSUMMARY:{summary}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

fn task(uid: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Hearth//dav-client test//EN\r\nBEGIN:VTODO\r\nUID:{uid}\r\nDTSTAMP:20261123T120000Z\r\nSUMMARY:Buy milk\r\nSTATUS:NEEDS-ACTION\r\nEND:VTODO\r\nEND:VCALENDAR\r\n"
    )
}

fn journal(uid: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Hearth//dav-client test//EN\r\nBEGIN:VJOURNAL\r\nUID:{uid}\r\nDTSTAMP:20261123T120000Z\r\nSUMMARY:A note\r\nDESCRIPTION:Written by the dav-client test.\r\nEND:VJOURNAL\r\nEND:VCALENDAR\r\n"
    )
}

fn vcard(uid: &str, name: &str) -> String {
    format!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:{uid}\r\nFN:{name}\r\nN:{name};;;;\r\nEMAIL;TYPE=INTERNET:ada@example.com\r\nEND:VCARD\r\n"
    )
}

fn hrefs(changes: &SyncChanges) -> Vec<&Url> {
    changes.changed.iter().map(|member| &member.href).collect()
}

fn find<'a>(collections: &'a [Collection], href: &Url) -> &'a Collection {
    collections
        .iter()
        .find(|collection| collection.href == *href)
        .unwrap_or_else(|| panic!("{href} is not listed"))
}

#[tokio::test]
#[ignore = "needs a throwaway Nextcloud; run tools/activesync/test-dav-nextcloud.sh"]
async fn nextcloud_end_to_end() {
    let dav = DavClient::new(
        &env("DAV_TEST_URL"),
        Credentials {
            user: env("DAV_TEST_USER"),
            password: env("DAV_TEST_PASSWORD"),
        },
        Limits::default(),
    )
    .unwrap();

    // Discovery.
    let homes = dav.discover().await.unwrap();
    println!("principal {}", homes.principal);
    let calendar_home = homes.calendar_home.expect("a calendar home");
    let addressbook_home = homes.addressbook_home.expect("an address book home");
    println!("calendar home {calendar_home}, address book home {addressbook_home}");

    // A calendar for events and tasks, and a Notes calendar for journals.
    let suffix = unique();
    let calendar = calendar_home.join(&format!("dav-test-{suffix}/")).unwrap();
    let notes = calendar_home.join(&format!("notes-{suffix}/")).unwrap();
    dav.mkcalendar(
        &calendar,
        "dav-client test",
        Components {
            event: true,
            todo: true,
            journal: false,
        },
    )
    .await
    .unwrap();
    dav.mkcalendar(&notes, "Notes", Components::JOURNAL)
        .await
        .unwrap();
    let calendars = dav.list_collections(&calendar_home).await.unwrap();
    for listed in &calendars {
        println!(
            "calendar {} {:?} {:?} writable {:?}",
            listed.href, listed.display_name, listed.components, listed.writable
        );
    }
    let listed = find(&calendars, &calendar);
    assert_eq!(listed.kind, CollectionKind::Calendar);
    assert_eq!(listed.display_name.as_deref(), Some("dav-client test"));
    assert!(listed.components.event && listed.components.todo && !listed.components.journal);
    assert_eq!(listed.writable, Some(true));
    assert!(listed.sync_token.is_some());
    let listed_notes = find(&calendars, &notes);
    assert_eq!(listed_notes.components, Components::JOURNAL);

    // Initial sync of the empty calendar.
    let empty = dav.sync_collection(&calendar, "").await.unwrap();
    assert!(empty.changed.is_empty() && empty.removed.is_empty());

    // Create an event and a task; a second create of the same href fails.
    let event_href = calendar.join(&format!("event-{suffix}.ics")).unwrap();
    let task_href = calendar.join(&format!("task-{suffix}.ics")).unwrap();
    let event_etag = dav
        .put(
            &event_href,
            CollectionKind::Calendar,
            event(&format!("event-{suffix}"), "Planning"),
            Condition::Create,
        )
        .await
        .unwrap();
    let task_etag = dav
        .put(
            &task_href,
            CollectionKind::Calendar,
            task(&format!("task-{suffix}")),
            Condition::Create,
        )
        .await
        .unwrap();
    let again = dav
        .put(
            &event_href,
            CollectionKind::Calendar,
            event(&format!("event-{suffix}"), "Again"),
            Condition::Create,
        )
        .await
        .unwrap_err();
    assert_eq!(again, DavError::PreconditionFailed);

    // Sync from the first token sees both, with the ETags the PUTs gave.
    let created = dav.sync_collection(&calendar, &empty.token).await.unwrap();
    assert_eq!(created.changed.len(), 2, "{created:?}");
    for member in &created.changed {
        let expected = if member.href == event_href {
            &event_etag
        } else {
            assert_eq!(member.href, task_href);
            &task_etag
        };
        assert_eq!(member.etag.as_ref(), Some(expected));
    }

    // Read both back in one multiget, and the event with GET.
    let fetched = dav
        .multiget(
            &calendar,
            CollectionKind::Calendar,
            &[event_href.clone(), task_href.clone()],
        )
        .await
        .unwrap();
    assert!(fetched.missing.is_empty());
    assert_eq!(fetched.found.len(), 2);
    for resource in &fetched.found {
        assert!(resource.data.contains(&format!(
            "UID:{}",
            if resource.href == event_href {
                format!("event-{suffix}")
            } else {
                format!("task-{suffix}")
            }
        )));
    }
    let got = dav.get(&event_href).await.unwrap();
    assert!(got.data.contains("SUMMARY:Planning"));
    assert_eq!(got.etag.as_ref(), Some(&event_etag));

    // Update with the current ETag; a stale ETag is refused.
    let updated_etag = dav
        .put(
            &event_href,
            CollectionKind::Calendar,
            event(&format!("event-{suffix}"), "Planning, moved"),
            Condition::Update(&event_etag),
        )
        .await
        .unwrap();
    assert_ne!(updated_etag, event_etag);
    let stale = dav
        .put(
            &event_href,
            CollectionKind::Calendar,
            event(&format!("event-{suffix}"), "Lost update"),
            Condition::Update(&event_etag),
        )
        .await
        .unwrap_err();
    assert_eq!(stale, DavError::PreconditionFailed);
    assert!(
        dav.get(&event_href)
            .await
            .unwrap()
            .data
            .contains("SUMMARY:Planning, moved")
    );

    // Delete the task: a stale ETag is refused, the current one works.
    let refused = dav.delete(&task_href, Some("\"stale\"")).await.unwrap_err();
    assert_eq!(refused, DavError::PreconditionFailed);
    dav.delete(&task_href, Some(&task_etag)).await.unwrap();
    assert_eq!(dav.get(&task_href).await.unwrap_err(), DavError::NotFound);

    // Sync from the second token: the event changed, the task removed.
    let changes = dav
        .sync_collection(&calendar, &created.token)
        .await
        .unwrap();
    assert_eq!(hrefs(&changes), [&event_href]);
    assert_eq!(changes.changed[0].etag.as_ref(), Some(&updated_etag));
    assert_eq!(changes.removed, std::slice::from_ref(&task_href));
    let quiet = dav
        .sync_collection(&calendar, &changes.token)
        .await
        .unwrap();
    assert!(quiet.changed.is_empty() && quiet.removed.is_empty());
    let markers = dav.change_markers(&calendar).await.unwrap();
    assert_eq!(markers.sync_token.as_deref(), Some(changes.token.as_str()));

    // A token the server never issued is its own error. (Nextcloud reads
    // a sabre token from the future as "no changes", so this one is not in
    // its form at all.)
    let invalid = dav
        .sync_collection(&calendar, "urn:dav-client:not-a-token")
        .await
        .unwrap_err();
    assert_eq!(invalid, DavError::InvalidSyncToken);

    // A journal in the Notes calendar.
    let journal_href = notes.join(&format!("note-{suffix}.ics")).unwrap();
    let journal_etag = dav
        .put(
            &journal_href,
            CollectionKind::Calendar,
            journal(&format!("note-{suffix}")),
            Condition::Create,
        )
        .await
        .unwrap();
    let noted = dav.sync_collection(&notes, "").await.unwrap();
    assert_eq!(hrefs(&noted), [&journal_href]);
    let note = dav.get(&journal_href).await.unwrap();
    assert!(note.data.contains("BEGIN:VJOURNAL"));
    dav.delete(&journal_href, Some(&journal_etag))
        .await
        .unwrap();

    // A contact in the default address book.
    let books = dav.list_collections(&addressbook_home).await.unwrap();
    for book in &books {
        println!("address book {} {:?}", book.href, book.display_name);
    }
    let book = books
        .iter()
        .find(|book| book.kind == CollectionKind::AddressBook && book.writable != Some(false))
        .expect("a writable address book");
    let before = dav.sync_collection(&book.href, "").await.unwrap();
    let card_href = book.href.join(&format!("ada-{suffix}.vcf")).unwrap();
    let card_etag = dav
        .put(
            &card_href,
            CollectionKind::AddressBook,
            vcard(&format!("ada-{suffix}"), "Ada Lovelace"),
            Condition::Create,
        )
        .await
        .unwrap();
    let added = dav
        .sync_collection(&book.href, &before.token)
        .await
        .unwrap();
    assert_eq!(hrefs(&added), [&card_href]);
    let cards = dav
        .multiget(
            &book.href,
            CollectionKind::AddressBook,
            std::slice::from_ref(&card_href),
        )
        .await
        .unwrap();
    assert!(cards.found[0].data.contains("FN:Ada Lovelace"));
    let card_etag = dav
        .put(
            &card_href,
            CollectionKind::AddressBook,
            vcard(&format!("ada-{suffix}"), "Ada King"),
            Condition::Update(&card_etag),
        )
        .await
        .unwrap();
    dav.delete(&card_href, Some(&card_etag)).await.unwrap();
    let removed = dav.sync_collection(&book.href, &added.token).await.unwrap();
    assert_eq!(removed.removed, std::slice::from_ref(&card_href));

    // Clean up the calendars this test made.
    dav.delete(&calendar, None).await.unwrap();
    dav.delete(&notes, None).await.unwrap();

    // Login Flow v2: started, and pending until someone approves it.
    let flow = LoginFlow::new(&Limits::default()).unwrap();
    let server = Url::parse(&env("DAV_TEST_SERVER")).unwrap();
    let start = flow.start(&server).await.unwrap();
    println!("login flow page {}", start.login);
    assert!(start.login.path().contains("/login/v2/flow"));
    let poll = flow
        .poll(&start.poll_endpoint, &start.poll_token)
        .await
        .unwrap();
    assert_eq!(poll, LoginPoll::Pending);
}
