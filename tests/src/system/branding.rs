/*
 * SPDX-FileCopyrightText: 2026 Coffey Labs LLC
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Branding acceptance tests, from
//! `docs/spec/features/branding-and-templates.md`. Each check names the test
//! number or the requirement it covers. The calendar email templates and the
//! RSVP page (tests 10 to 15) went with calendars; the logo tests stay.

use crate::utils::{
    account::Account,
    server::{TestServer, TestServerBuilder},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use hyper::StatusCode;
use jmap_proto::error::set::SetErrorType;
use registry::schema::{
    prelude::{Object, ObjectInner, ObjectType, Property},
    structs::{
        CertificateManagement, DkimManagement, DnsManagement, Domain, Enterprise, Tenant, UserRoles,
    },
};
use serde_json::json;
use std::time::Duration;
use store::registry::write::{RegistryWrite, RegistryWriteResult};
use types::id::Id;

const SECRET: &str = "branding test user passphrase";
const PNG: &[u8] = include_bytes!("../../../resources/branding/email-logo.png");
const SVG_SCRIPTED: &[u8] =
    b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>";

pub async fn test(test: &mut TestServer) {
    println!("Running branding tests...");
    let admin = test.account("admin@example.org");
    let t_id = admin
        .registry_create_object(Tenant {
            name: "brand-t".to_string(),
            ..Default::default()
        })
        .await;
    let t_domain = admin.brand_domain("t-brand.example.org", Some(t_id)).await;
    let plain = admin.brand_domain("plain-brand.example.org", None).await;
    let t_admin = admin
        .create_user_account("tadmin@t-brand.example.org", SECRET, "T admin", &[], vec![])
        .await;
    admin
        .registry_update_object(
            ObjectType::Account,
            t_admin.id(),
            json!({ Property::Roles: UserRoles::Admin }),
        )
        .await;
    let png_url = data_url("image/png", PNG);

    // Acceptance test 1: nothing set, no custom logo anywhere
    let answer = get(&admin, "/logo?domain=plain-brand.example.org", None).await;
    assert_eq!(answer.status, StatusCode::NOT_FOUND, "test 1");
    answer.assert_logo_headers("test 1");

    // Acceptance test 8: logo writes (BT-3)
    for bad in [
        "javascript:alert(1)".to_string(),
        "data:text/html,<b>x</b>".to_string(),
        data_url("image/png", &[PNG, &vec![0u8; 300 * 1024]].concat()),
        data_url("image/png", &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10]),
    ] {
        admin
            .registry_update_object_expect_err(ObjectType::Domain, plain, json!({"logo": bad}))
            .await
            .assert_type(SetErrorType::InvalidProperties)
            .assert_properties(&["logo"]);
    }

    // Acceptance test 2: a server-wide PNG logo is served
    admin
        .registry_update_setting(
            Enterprise {
                logo_url: Some(png_url.clone()),
                ..Default::default()
            },
            &[Property::LogoUrl],
        )
        .await;
    let answer = get(&admin, "/logo?domain=plain-brand.example.org", None).await;
    assert_eq!(answer.status, StatusCode::OK, "test 2");
    assert_eq!(answer.header("content-type"), "image/png", "test 2");
    assert_eq!(answer.body, PNG, "test 2");
    answer.assert_logo_headers("test 2");

    // Acceptance test 3: the tenant's logo, then the domain's own
    admin
        .registry_update_object(
            ObjectType::Tenant,
            t_id,
            json!({"logo": "https://logo.example.org/t.png"}),
        )
        .await;
    let answer = get_image(&admin, "/logo?domain=t-brand.example.org").await;
    assert_eq!(answer.status, StatusCode::FOUND, "test 3");
    assert_eq!(answer.header("location"), "https://logo.example.org/t.png");
    let gif = data_url("image/gif", b"GIF89a\x01\0\x01\0\0\0\0;");
    admin
        .registry_update_object(ObjectType::Domain, t_domain, json!({"logo": gif}))
        .await;
    let answer = get(&admin, "/logo?domain=t-brand.example.org", None).await;
    assert_eq!(
        answer.header("content-type"),
        "image/gif",
        "test 3: domain wins"
    );

    // Acceptance test 4: a subdomain finds its domain, by parameter or Host
    let by_param = get(&admin, "/logo?domain=mail.t-brand.example.org", None).await;
    let by_host = get(&admin, "/logo", Some("mail.t-brand.example.org:443")).await;
    for answer in [by_param, by_host] {
        assert_eq!(answer.header("content-type"), "image/gif", "test 4");
    }

    // Acceptance test 5: a URL logo is redirected to, never fetched
    admin
        .registry_update_object(
            ObjectType::Domain,
            plain,
            json!({"logo": "https://192.0.2.1/logo.png"}),
        )
        .await;
    let started = std::time::Instant::now();
    let redirect = get_image(&admin, "/logo?domain=plain-brand.example.org").await;
    assert_eq!(redirect.status, StatusCode::FOUND, "test 5");
    assert_eq!(redirect.header("location"), "https://192.0.2.1/logo.png");
    // TEST-NET-1 never answers: a fetch would hang, a redirect is instant
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "test 5: no fetch"
    );
    // Only an image load is redirected: a link opened in the browser, a page
    // framing it, a script or a client that doesn't say isn't, so /logo
    // can't send people to the URL a domain's administrator chose
    for dest in [Some("document"), Some("iframe"), Some("empty"), None] {
        let mut request = http().get(format!(
            "{}/logo?domain=plain-brand.example.org",
            admin.base_url()
        ));
        if let Some(dest) = dest {
            request = request.header("sec-fetch-dest", dest);
        }
        let answer = Answer::read(request.send().await.unwrap()).await;
        assert_eq!(answer.status, StatusCode::NOT_FOUND, "test 5: {dest:?}");
        assert_eq!(answer.header("location"), "", "test 5: {dest:?}");
        answer.assert_url_logo_headers("test 5");
    }
    redirect.assert_url_logo_headers("test 5");
    // An http: URL stored before the fork is never redirected to
    store_directly(
        test,
        ObjectInner::Enterprise(Enterprise {
            logo_url: Some("http://logo.example.org/old.png".to_string()),
            ..Default::default()
        }),
    )
    .await;
    let answer = get_image(&admin, "/logo?domain=nowhere.example.net").await;
    assert_eq!(answer.status, StatusCode::NOT_FOUND, "test 5: http");
    assert_eq!(answer.header("location"), "", "test 5: http");
    admin
        .registry_update_setting(
            Enterprise {
                logo_url: Some(png_url.clone()),
                ..Default::default()
            },
            &[Property::LogoUrl],
        )
        .await;
    // Acceptance test 6: an unknown domain answers as a known one with no
    // logo of its own
    admin
        .registry_update_object(ObjectType::Domain, plain, json!({"logo": null}))
        .await;
    let known = get(&admin, "/logo?domain=plain-brand.example.org", None).await;
    let unknown = get(&admin, "/logo?domain=nowhere.example.net", None).await;
    assert_eq!(known.status, unknown.status, "test 6");
    assert_eq!(known.body, unknown.body, "test 6");
    assert_eq!(known.header("content-type"), unknown.header("content-type"));

    // Acceptance tests 7 and 9: an SVG is sandboxed, and a change shows on the
    // next request with no restart
    admin
        .registry_update_object(
            ObjectType::Domain,
            plain,
            json!({"logo": data_url("image/svg+xml", SVG_SCRIPTED)}),
        )
        .await;
    let answer = get(&admin, "/logo?domain=plain-brand.example.org", None).await;
    assert_eq!(answer.header("content-type"), "image/svg+xml", "test 9");
    assert_eq!(
        answer.header("content-security-policy"),
        "default-src 'none'; style-src 'unsafe-inline'; sandbox",
        "test 7"
    );

    // Acceptance test 16: a tenant administrator sets its domain's logo, but
    // nothing server-wide and not its tenant's
    t_admin
        .registry_update_object(ObjectType::Domain, t_domain, json!({"logo": png_url}))
        .await;
    for (object, id, patch) in [
        (
            ObjectType::Enterprise,
            Id::singleton(),
            json!({"logoUrl": "https://logo.example.org/x.png"}),
        ),
        (
            ObjectType::Tenant,
            t_id,
            json!({"logo": "https://logo.example.org/x.png"}),
        ),
    ] {
        let response = t_admin.registry_update(object, [(id, patch)]).await;
        let refused = response
            .0
            .pointer(&format!("/methodResponses/0/1/notUpdated/{id}"))
            .is_some()
            || response.0.pointer("/methodResponses/0/0") == Some(&json!("error"));
        assert!(refused, "test 16: {object:?} {response:?}");
    }

    // Acceptance test 19: dark-mode logos (BT-27). The server-wide dark logo
    // doesn't beat a domain's own logo; the domain's dark one does, and a
    // light page never gets a dark logo. A tenant administrator sets its
    // domain's dark logo, but not the server-wide one.
    admin
        .registry_update_object(
            ObjectType::Enterprise,
            Id::singleton(),
            json!({"logoUrlDark": "https://logo.example.org/server-dark.png"}),
        )
        .await;
    let answer = get(&admin, "/logo?domain=t-brand.example.org&theme=dark", None).await;
    assert_eq!(
        answer.header("content-type"),
        "image/png",
        "test 19: the domain's own logo"
    );
    let answer = get_image(&admin, "/logo?domain=nowhere.example.net&theme=dark").await;
    assert_eq!(answer.status, StatusCode::FOUND, "test 19");
    assert_eq!(
        answer.header("location"),
        "https://logo.example.org/server-dark.png",
        "test 19: the server-wide dark logo"
    );
    admin
        .registry_update_object_expect_err(
            ObjectType::Domain,
            t_domain,
            json!({"logoDark": "javascript:alert(1)"}),
        )
        .await
        .assert_type(SetErrorType::InvalidProperties)
        .assert_properties(&["logoDark"]);
    t_admin
        .registry_update_object(
            ObjectType::Domain,
            t_domain,
            json!({"logoDark": data_url("image/gif", b"GIF89a\x01\0\x01\0\0\0\0;")}),
        )
        .await;
    let dark = get(&admin, "/logo?domain=t-brand.example.org&theme=dark", None).await;
    assert_eq!(dark.header("content-type"), "image/gif", "test 19: dark");
    dark.assert_logo_headers("test 19");
    let light = get(&admin, "/logo?domain=t-brand.example.org", None).await;
    assert_eq!(light.header("content-type"), "image/png", "test 19: light");
    let other = get(&admin, "/logo?domain=t-brand.example.org&theme=sepia", None).await;
    assert_eq!(other.header("content-type"), "image/png", "test 19: other");
    let response = t_admin
        .registry_update(
            ObjectType::Enterprise,
            [(
                Id::singleton(),
                json!({"logoUrlDark": "https://logo.example.org/x.png"}),
            )],
        )
        .await;
    let refused = response
        .0
        .pointer(&format!(
            "/methodResponses/0/1/notUpdated/{}",
            Id::singleton()
        ))
        .is_some()
        || response.0.pointer("/methodResponses/0/0") == Some(&json!("error"));
    assert!(refused, "test 19: {response:?}");

    // Acceptance test 17: the built-in pages load /logo through an image
    // element, which follows a redirect and falls back on error (BT-26)
    for page in [include_str!("../../../resources/html-templates/login.html")] {
        assert!(page.contains("img.src = target"), "test 17");
        assert!(page.contains("img.onerror = showDefault"), "test 17");
        assert!(!page.contains("fetch(target"), "test 17");
        assert!(page.contains("params.push('theme=dark')"), "test 19");
    }

    // Clean up
    admin
        .registry_update_setting(
            Enterprise::default(),
            &[Property::LogoUrl, Property::LogoUrlDark],
        )
        .await;
    admin.reload_settings().await;
    admin.destroy_account(t_admin).await;
    for domain in [plain, t_domain] {
        admin
            .registry_destroy(ObjectType::Domain, [domain])
            .await
            .assert_destroyed(&[domain]);
    }
    admin
        .registry_destroy(ObjectType::Tenant, [t_id])
        .await
        .assert_destroyed(&[t_id]);
    test.wait_for_tasks().await;
}

/// Runs the branding tests alone:
/// `cargo test -p tests branding_tests -- --ignored`.
#[ignore]
#[tokio::test(flavor = "multi_thread")]
pub async fn branding_tests() {
    let mut test = TestServerBuilder::new("branding_tests")
        .await
        .with_default_listeners()
        .await
        .build()
        .await;
    let admin = test.create_admin_account("admin@example.org").await;
    test.insert_account(admin);
    self::test(&mut test).await;
    if test.is_reset() {
        test.temp_dir.delete();
    }
}

/// Acceptance test 18 (compat): the logos a copy of the live data holds are
/// served or skipped as before cutover.
///
/// INBUXA holds none (spec, observed 1), so this checks whatever a copy of
/// its data holds: every domain's logo is served by `/logo` or skipped
/// (BT-4). Run it with:
///
/// - `INBUXA_COMPAT_ADMIN`: `name:password` of a server-level administrator
///   in that data;
///
/// and the data itself in place of the test store: `NO_INSERT=1` and the
/// store's `TMPDIR`/`STORE` pointing at the copy, so it isn't reset.
#[ignore]
#[tokio::test(flavor = "multi_thread")]
pub async fn branding_compat() {
    let admin = std::env::var("INBUXA_COMPAT_ADMIN").expect("INBUXA_COMPAT_ADMIN");
    assert!(
        std::env::var("NO_INSERT").is_ok(),
        "NO_INSERT must be set, or the copy of inbuxa's data is wiped"
    );
    let test = TestServerBuilder::new("branding_compat")
        .await
        .with_default_listeners()
        .await
        .build_with_opts(false)
        .await;
    let (name, secret) = admin.split_once(':').expect("name:password");
    let admin = Account::new(
        Box::leak(name.to_string().into_boxed_str()),
        Box::leak(secret.to_string().into_boxed_str()),
        &[],
        "Compat admin",
        Id::from(u32::MAX),
    );

    admin.assert_authenticates("INBUXA_COMPAT_ADMIN").await;

    // Every domain's logo reads back and is served or skipped, never an error
    let domains = admin
        .jmap_method_call(
            "x:Domain/get",
            json!({"ids": null, "properties": ["name", "logo"]}),
        )
        .await;
    for domain in domains.list() {
        let name = domain["name"].as_str().unwrap();
        let answer = get(&admin, &format!("/logo?domain={name}"), None).await;
        assert!(
            matches!(
                answer.status,
                StatusCode::OK | StatusCode::FOUND | StatusCode::NOT_FOUND
            ),
            "{name}: {:?}",
            answer.status
        );
    }

    let _ = test;
}

/// Writes a setting straight into the registry, skipping `/set`'s checks,
/// as data from before the fork would be.
async fn store_directly(test: &TestServer, inner: ObjectInner) {
    let object = Object { inner, revision: 0 };
    let registry = test.server.registry();
    let old = registry
        .get(registry::types::id::ObjectId::new(
            object.object_type(),
            Id::singleton(),
        ))
        .await
        .unwrap();
    let result = match &old {
        Some(old) => registry
            .write(RegistryWrite::Update {
                object: &object,
                id: Id::singleton(),
                old_object: old,
            })
            .await
            .unwrap(),
        None => registry
            .write(RegistryWrite::Insert {
                object: &object,
                id: Some(Id::singleton()),
            })
            .await
            .unwrap(),
    };
    assert!(matches!(result, RegistryWriteResult::Success(_)));
}

fn data_url(media_type: &str, bytes: &[u8]) -> String {
    format!("data:{media_type};base64,{}", STANDARD.encode(bytes))
}

struct Answer {
    status: StatusCode,
    headers: reqwest::header::HeaderMap,
    body: Vec<u8>,
}

impl Answer {
    fn header(&self, name: &str) -> String {
        self.headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    /// BT-5: a URL logo's answer depends on who asks, so no cache keeps it.
    fn assert_url_logo_headers(&self, test: &str) {
        assert_eq!(
            self.header("cache-control"),
            "no-store, no-cache, must-revalidate",
            "{test}"
        );
        assert_eq!(self.header("vary"), "Sec-Fetch-Dest", "{test}");
        assert_eq!(self.header("x-content-type-options"), "nosniff", "{test}");
        assert_eq!(self.header("access-control-allow-origin"), "*", "{test}");
    }

    /// BT-5: every other `/logo` answer carries these.
    fn assert_logo_headers(&self, test: &str) {
        assert_eq!(
            self.header("cache-control"),
            "public, max-age=300",
            "{test}"
        );
        assert_eq!(self.header("x-content-type-options"), "nosniff", "{test}");
        assert_eq!(self.header("access-control-allow-origin"), "*", "{test}");
    }
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}

/// An anonymous GET, optionally with another `Host`.
async fn get(account: &Account, path: &str, host: Option<&str>) -> Answer {
    let mut request = http().get(format!("{}{path}", account.base_url()));
    if let Some(host) = host {
        request = request.header("host", host);
    }
    Answer::read(request.send().await.unwrap()).await
}

/// An anonymous GET as a browser's `<img>` makes it (BT-26).
async fn get_image(account: &Account, path: &str) -> Answer {
    let request = http()
        .get(format!("{}{path}", account.base_url()))
        .header("sec-fetch-dest", "image");
    Answer::read(request.send().await.unwrap()).await
}

impl Answer {
    async fn read(response: reqwest::Response) -> Answer {
        Answer {
            status: response.status(),
            headers: response.headers().clone(),
            body: response.bytes().await.unwrap().to_vec(),
        }
    }
}

impl Account {
    async fn brand_domain(&self, name: &str, tenant: Option<Id>) -> Id {
        self.registry_create_object(Domain {
            name: name.to_string(),
            is_enabled: true,
            member_tenant_id: tenant,
            certificate_management: CertificateManagement::Manual,
            dns_management: DnsManagement::Manual,
            dkim_management: DkimManagement::Manual,
            ..Default::default()
        })
        .await
    }
}
