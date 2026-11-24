/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! ActiveSync end to end: replays of iPhone Mail, Outlook mobile and an
//! Android Exchange client against a test server, over HTTPS, as the
//! devices talk (docs/activesync.md, "Tests").
//!
//! The sessions are in tests/resources/activesync/*.txt, written as XML and
//! sent as WBXML. They are built from the specifications' examples and each
//! client's documented behaviour (every file names its sources), not
//! captured from devices: none were available. Real devices are unproven
//! until someone runs them (docs/progress.md).

pub mod replay;

use crate::utils::server::TestServerBuilder;
use registry::schema::enums::Permission;

#[tokio::test(flavor = "multi_thread")]
pub async fn activesync_tests() {
    let mut test = TestServerBuilder::new("activesync_tests")
        .await
        .with_default_listeners()
        .await
        .build()
        .await;
    let admin = test.create_admin_account("admin@example.com").await;
    for (name, secret, description) in [
        ("ann@example.com", "ann secret + extra safety", "Ann Archer"),
        ("bob@example.com", "bob secret + extra safety", "Bob Baker"),
    ] {
        let account = admin
            .create_user_account(
                name,
                secret,
                description,
                &[],
                vec![Permission::UnlimitedRequests, Permission::UnlimitedUploads],
            )
            .await;
        test.insert_account(account);
    }

    for (file, user) in [
        // In this order: each session finds what the one before left.
        ("iphone.txt", "ann@example.com"),
        ("android.txt", "ann@example.com"),
        ("outlook.txt", "bob@example.com"),
    ] {
        let account = test.account(user);
        let script = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("resources/activesync")
                .join(file),
        )
        .unwrap_or_else(|err| panic!("{file}: {err}"));
        println!("Replaying {file} as {user}...");
        replay::run(file, &script, &test, account).await;
    }
    // The DAV bridge against a throwaway Nextcloud, when one is given
    // (tools/activesync/test-bridge-nextcloud.sh sets INBUXA_DAV_URL and
    // makes the same users there).
    if std::env::var("INBUXA_DAV_URL").is_ok() {
        let script = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("resources/activesync/nextcloud.txt"),
        )
        .expect("nextcloud.txt");
        println!("Replaying nextcloud.txt as ann@example.com...");
        replay::run(
            "nextcloud.txt",
            &script,
            &test,
            test.account("ann@example.com"),
        )
        .await;
    } else {
        println!("INBUXA_DAV_URL not set: the DAV bridge replay is skipped");
    }

    // The devices' state goes with its prefix; mail with the mailboxes.
    test.server
        .in_memory_store()
        .key_delete_prefix(&[common::KV_ACTIVESYNC])
        .await
        .expect("state removed");
    for user in ["ann@example.com", "bob@example.com"] {
        test.destroy_all_mailboxes(test.account(user)).await;
    }
    if test.is_reset() {
        test.temp_dir.delete();
    }
}
