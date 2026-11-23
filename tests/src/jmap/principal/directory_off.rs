/*
 * SPDX-FileCopyrightText: 2026 Coffey Labs LLC
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use crate::utils::{
    account::Account,
    jmap::{JmapResponse, JmapUtils},
    server::TestServer,
};
use registry::schema::{prelude::Property, structs::Sharing};
use serde_json::{Value, json};

/// Runs before anything shares between these accounts, so that "already has
/// to do with" means the account itself and its groups.
pub async fn test(test: &TestServer) {
    println!("Running directory queries off tests...");
    let admin = test.account("admin");
    let john = test.account("jdoe@example.com");
    let jane = test.account("jane.smith@example.com");
    let john_id = john.id_string();
    let jane_id = jane.id_string();

    set_directory_queries(test, false).await;

    // A regular user holds every jmapPrincipal* permission and still can't list anyone
    assert_eq!(
        john.jmap_method_call("Principal/query", json!({ "accountId": john_id }))
            .await
            .method_response()
            .typ(),
        "forbidden"
    );

    // Principal/get answers for the account itself and nobody else, asked by id or for everyone
    let response = john
        .jmap_method_call(
            "Principal/get",
            json!({ "accountId": john_id, "ids": [john_id, jane_id], "properties": ["id"] }),
        )
        .await;
    let found = ids(&response.method_response()["list"]);
    assert_eq!(found, [john_id], "{response:?}");
    assert_eq!(
        response.method_response()["notFound"],
        json!([jane_id]),
        "{response:?}"
    );
    let response = john
        .jmap_method_call(
            "Principal/get",
            json!({ "accountId": john_id, "ids": null, "properties": ["id"] }),
        )
        .await;
    let found = ids(&response.method_response()["list"]);
    assert!(found.contains(&john_id), "{response:?}");
    assert!(!found.contains(&jane_id), "{response:?}");

    // Nobody new can be shared with, and an account that exists is refused the
    // same way as one that doesn't, so a grant can't be used to look anyone up
    let robert = test.account("robert@example.com");
    let robert_id = robert.id_string();
    let inbox = first_id(john, "Mailbox/get").await;
    for grantee in [jane_id, "zzzzzz"] {
        let response = share(
            john,
            "Mailbox",
            &inbox,
            json!({ grantee: { "mayReadItems": true } }),
        )
        .await;
        assert_eq!(
            response.not_updated(&inbox)["type"],
            "invalidForeignKey",
            "Mailbox {grantee}: {response:?}"
        );
    }

    // Those already shared with stay usable. Jane and Robert get John's
    // folder while the setting is on.
    set_directory_queries(test, true).await;
    let response = john
        .jmap_method_call(
            "Mailbox/set",
            json!({
                "accountId": john_id,
                "create": { "i0": {
                    "name": "Directory off",
                    "shareWith": {
                        jane_id: { "mayReadItems": true, "mayAddItems": true },
                        robert_id: { "mayReadItems": true },
                    }
                }}
            }),
        )
        .await;
    let folder = response.created(0).id().to_string();
    set_directory_queries(test, false).await;

    // John can name them, though he still can't list anyone or see the rest
    let response = john
        .jmap_method_call(
            "Principal/get",
            json!({
                "accountId": john_id,
                "ids": [jane_id, robert_id, test.account("bill@example.com").id_string()],
                "properties": ["id"]
            }),
        )
        .await;
    let found = ids(&response.method_response()["list"]);
    assert!(
        found.contains(&jane_id) && found.contains(&robert_id),
        "{response:?}"
    );
    assert_eq!(
        response.method_response()["notFound"],
        json!([test.account("bill@example.com").id_string()]),
        "{response:?}"
    );

    // He can change what Jane has, and share something else with her
    let response = share(
        john,
        "Mailbox",
        &folder,
        json!({
            jane_id: { "mayReadItems": true, "mayAddItems": true, "maySetSeen": true },
            robert_id: { "mayReadItems": true },
        }),
    )
    .await;
    assert!(response.updated(&folder).is_null(), "{response:?}");
    let response = share(
        john,
        "Mailbox",
        &inbox,
        json!({ jane_id: { "mayReadItems": true } }),
    )
    .await;
    assert!(response.updated(&inbox).is_null(), "{response:?}");

    // Jane knows John now, but Robert isn't hers to see: she can't manage
    // the folder they're both on
    let response = jane
        .jmap_method_call(
            "Principal/get",
            json!({ "accountId": jane_id, "ids": [john_id, robert_id], "properties": ["id"] }),
        )
        .await;
    assert_eq!(
        ids(&response.method_response()["list"]),
        [john_id],
        "{response:?}"
    );

    // Sharing can be taken away, and then Jane can't be named anymore
    let response = share(john, "Mailbox", &inbox, Value::Null).await;
    assert!(response.updated(&inbox).is_null(), "{response:?}");
    john.jmap_method_call(
        "Mailbox/set",
        json!({ "accountId": john_id, "destroy": [folder], "onDestroyRemoveEmails": true }),
    )
    .await;
    let response = john
        .jmap_method_call(
            "Principal/get",
            json!({ "accountId": john_id, "ids": [jane_id], "properties": ["id"] }),
        )
        .await;
    assert_eq!(
        response.method_response()["notFound"],
        json!([jane_id]),
        "{response:?}"
    );

    // Whoever can list accounts anyway isn't held back
    let response = admin
        .jmap_method_calls(json!([
            ["Principal/query", { "accountId": admin.id_string() }, "q"],
            ["Principal/get", {
                "accountId": admin.id_string(),
                "#ids": { "resultOf": "q", "name": "Principal/query", "path": "/ids" },
                "properties": ["id"]
            }, "g"]
        ]))
        .await;
    let found = ids(&response.response_at(1)["list"]);
    assert!(
        found.contains(&john_id) && found.contains(&jane_id),
        "{response:?}"
    );

    // Back on, a regular user can look people up again
    set_directory_queries(test, true).await;
    let response = john
        .jmap_method_calls(json!([
            ["Principal/query", { "accountId": john_id }, "q"],
            ["Principal/get", {
                "accountId": john_id,
                "#ids": { "resultOf": "q", "name": "Principal/query", "path": "/ids" },
                "properties": ["id"]
            }, "g"]
        ]))
        .await;
    assert!(
        ids(&response.response_at(1)["list"]).contains(&jane_id),
        "{response:?}"
    );

    // Looking up who's shared with made these
    for account in [john, jane] {
        test.destroy_all_mailboxes(account).await;
    }
    test.assert_is_empty().await;
}

async fn first_id(account: &Account, method: &str) -> String {
    account
        .jmap_method_call(
            method,
            json!({ "accountId": account.id_string(), "ids": null, "properties": ["id"] }),
        )
        .await
        .method_response()["list"][0]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{method} returned nothing"))
        .to_string()
}

async fn share(account: &Account, object: &str, id: &str, share_with: Value) -> JmapResponse {
    account
        .jmap_method_call(
            &format!("{object}/set"),
            json!({
                "accountId": account.id_string(),
                "update": { id: { "shareWith": share_with } }
            }),
        )
        .await
}

fn ids(list: &Value) -> Vec<&str> {
    list.as_array()
        .unwrap_or_else(|| panic!("Expected a list: {list:?}"))
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect()
}

pub async fn set_directory_queries(test: &TestServer, allow: bool) {
    let admin = test.account("admin");
    admin
        .registry_update_setting(
            Sharing {
                allow_directory_queries: allow,
                ..Default::default()
            },
            &[Property::AllowDirectoryQueries],
        )
        .await;
    admin.reload_settings().await;
}
