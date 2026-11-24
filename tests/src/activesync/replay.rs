/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The replay engine: reads a session script, sends each request as the
//! device would (plain or base64 query, its headers, WBXML body) and checks
//! each answer against the expected one.
//!
//! Script lines:
//!
//! - `@device <id> <type> <user-agent> <version> plain|base64`
//! - `!deliver <subject>`: a message for the user arrives in the Inbox;
//!   `!deliver-later <seconds> <subject>` does it while the next request
//!   waits (for Ping and Sync with a heartbeat). `!deliver` and
//!   `!deliver-invite` return once the message is in the search index
//!   too (see [`settle`]).
//! - `!deliver-invite <subject>`: an invitation (iTIP REQUEST from
//!   carol@example.org) arrives in the Inbox.
//! - `!dav-put <path>`, the lines of an iCalendar or vCard, `!end`: the
//!   resource is put into the cloud at `$INBUXA_DAV_URL/<path>` (with the
//!   account's own credentials), as another client would.
//! - `!autodiscover json|pox|soap`, then `contains <text>` lines.
//! - `!setup[/<path>]`, then `contains <text>` lines: the setup page, or
//!   one of its signed-in answers (the text may be in the content type).
//! - `>>> <Command> [Param=Value ...]`, then `? Name: value` headers, then
//!   the request body as XML (or nothing).
//! - `<<< <status> [empty]`, then `? Name: value` headers expected (the value
//!   contained in the answer's), then the expected answer as XML.
//!
//! The expected XML is matched as a subset: each expected element must be
//! found among the answer's children (in any order), text compared after
//! trimming; `*` matches any text, `~text` any text containing `text`, and
//! `$name` captures the text the first time and must equal it after. `$name` in later requests is replaced by
//! the captured text.

use crate::utils::{account::Account, server::TestServer};
use activesync_proto::wbxml::{self, Content, Element};
use base64::{Engine, engine::general_purpose::STANDARD};
use jmap_client::mailbox::{self, Role};
use std::collections::HashMap;
use std::time::Duration;

struct Device {
    id: String,
    kind: String,
    agent: String,
    version: String,
    base64: bool,
}

pub async fn run(file: &str, script: &str, test: &TestServer, account: &Account) {
    let http = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(120))
        .build()
        .expect("client");
    let base = format!("https://127.0.0.1:{}", account.http_listener_port);
    let mut vars: HashMap<String, String> = HashMap::new();
    let mut device: Option<Device> = None;
    let mut later: Option<tokio::task::JoinHandle<()>> = None;
    let lines: Vec<&str> = script.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim_end();
        let at = format!("{file}:{}", i + 1);
        i += 1;
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("@device ") {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            device = Some(Device {
                id: parts[0].into(),
                kind: parts[1].into(),
                agent: parts[2].into(),
                version: parts[3].into(),
                base64: parts[4] == "base64",
            });
        } else if let Some(subject) = line.strip_prefix("!deliver ") {
            deliver(account, subject).await;
            settle(test).await;
        } else if let Some(rest) = line.strip_prefix("!deliver-later ") {
            let (secs, subject) = rest.split_once(' ').expect("seconds and subject");
            let secs: u64 = secs.parse().expect("seconds");
            let subject = subject.to_string();
            let client = account.jmap_client().await;
            let name = account.name();
            later = Some(tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(secs)).await;
                deliver_with(&client, name, &subject).await;
            }));
        } else if let Some(subject) = line.strip_prefix("!deliver-invite ") {
            deliver_invite(account, subject).await;
            settle(test).await;
        } else if let Some(path) = line.strip_prefix("!dav-put ") {
            // The lines up to `!end` are the resource, put into the cloud as
            // another client would.
            let mut text = String::new();
            while i < lines.len() && lines[i].trim() != "!end" {
                text.push_str(lines[i].trim());
                text.push_str("\r\n");
                i += 1;
            }
            i += 1;
            dav_put(&http, account, path.trim(), text).await;
        } else if let Some(path) = line.strip_prefix("!setup") {
            // GET /setup<path>, signed in except for the page itself.
            let mut wanted = Vec::new();
            while i < lines.len() && lines[i].trim_start().starts_with("contains ") {
                wanted.push(lines[i].trim_start()["contains ".len()..].to_string());
                i += 1;
            }
            let mut request = http.get(format!("{base}/setup{}", path.trim()));
            if !path.trim().is_empty() {
                request = request.basic_auth(account.name(), Some(account.secret()));
            }
            let response = request.send().await.expect("setup");
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            let body = response.text().await.expect("text");
            for text in wanted {
                assert!(
                    body.contains(&text) || content_type.contains(&text),
                    "{at}: /setup{path} lacks {text:?}:\n{body}"
                );
            }
        } else if let Some(dialect) = line.strip_prefix("!autodiscover ") {
            let mut wanted = Vec::new();
            while i < lines.len() && lines[i].trim_start().starts_with("contains ") {
                wanted.push(lines[i].trim_start()["contains ".len()..].to_string());
                i += 1;
            }
            let body = autodiscover(&http, &base, dialect.trim(), account.name()).await;
            for text in wanted {
                assert!(
                    body.contains(&text),
                    "{at}: autodiscover {dialect} lacks {text:?}:\n{body}"
                );
            }
        } else if let Some(rest) = line.strip_prefix(">>> ") {
            let device = device
                .as_ref()
                .unwrap_or_else(|| panic!("{at}: @device first"));
            let mut words = rest.split_whitespace();
            let command = words.next().expect("command").to_string();
            let params: Vec<(String, String)> = words
                .map(|word| {
                    let (k, v) = word.split_once('=').expect("Param=Value");
                    (k.to_string(), substitute(v, &vars))
                })
                .collect();
            let (headers, body) = read_block(&lines, &mut i, "<<<");
            let headers: Vec<(String, String)> = headers
                .into_iter()
                .map(|(k, v)| (k, substitute(&v, &vars)))
                .collect();
            // The expectation.
            let expect_line = lines.get(i).unwrap_or_else(|| panic!("{at}: no <<<"));
            i += 1;
            let mut expect = expect_line["<<< ".len()..].split_whitespace();
            let status: u16 = expect.next().expect("status").parse().expect("status");
            let empty = expect.next() == Some("empty");
            let (expected_headers, expected_body) = read_block(&lines, &mut i, ">>> ");

            let response = send(
                &http,
                &base,
                account,
                device,
                &command,
                &params,
                &headers,
                body.map(|xml| substitute(&xml, &vars)),
            )
            .await;
            if let Some(task) = later.take() {
                task.await.expect("delivery");
            }
            let got_status = response.status().as_u16();
            let got_headers = response.headers().clone();
            let bytes = response.bytes().await.expect("body");
            assert_eq!(got_status, status, "{at}: {command} answered {got_status}");
            for (name, value) in expected_headers {
                let got = got_headers
                    .get(name.as_str())
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default();
                assert!(
                    got.contains(&value),
                    "{at}: header {name} is {got:?}, wanted {value:?}"
                );
            }
            if empty {
                assert!(
                    bytes.is_empty(),
                    "{at}: wanted an empty answer, got {} bytes",
                    bytes.len()
                );
            }
            if let Some(expected) = expected_body {
                let expected = wbxml::from_xml(&expected).unwrap_or_else(|e| panic!("{at}: {e}"));
                let got = wbxml::decode(&bytes).unwrap_or_else(|e| panic!("{at}: answer: {e}"));
                let mut captured = vars.clone();
                assert!(
                    matches(&expected, &got, &mut captured),
                    "{at}: {command} answer doesn't match.\nExpected:\n{}\nGot:\n{}",
                    wbxml::to_xml(&expected),
                    wbxml::to_xml(&got)
                );
                vars = captured;
            }
            println!("  {at}: {command} ok");
        } else {
            panic!("{at}: can't read {line:?}");
        }
    }
}

/// `? Name: value` headers, then XML lines, up to a line starting `stop`
/// or a directive.
fn read_block(
    lines: &[&str],
    i: &mut usize,
    stop: &str,
) -> (Vec<(String, String)>, Option<String>) {
    let mut headers = Vec::new();
    let mut body = String::new();
    while *i < lines.len() {
        let line = lines[*i];
        let trimmed = line.trim_start();
        if trimmed.starts_with(stop)
            || trimmed.starts_with("<<<")
            || trimmed.starts_with('!')
            || trimmed.starts_with('@')
        {
            break;
        }
        *i += 1;
        if let Some(header) = trimmed.strip_prefix("? ") {
            let (name, value) = header.split_once(':').expect("Name: value");
            headers.push((name.trim().to_string(), value.trim().to_string()));
        } else if !trimmed.starts_with('#') {
            body.push_str(line);
            body.push('\n');
        }
    }
    let body = body.trim();
    (headers, (!body.is_empty()).then(|| body.to_string()))
}

fn substitute(text: &str, vars: &HashMap<String, String>) -> String {
    let mut out = text.to_string();
    // Longest names first, so $msg2 isn't read as $msg.
    let mut names: Vec<&String> = vars.keys().collect();
    names.sort_by_key(|name| std::cmp::Reverse(name.len()));
    for name in names {
        out = out.replace(&format!("${name}"), &vars[name]);
    }
    out
}

/// Whether `got` holds everything `expected` names.
fn matches(expected: &Element, got: &Element, vars: &mut HashMap<String, String>) -> bool {
    if expected.tag != got.tag {
        return false;
    }
    match &expected.content {
        Content::Empty => true,
        Content::Text(text) => {
            let want = text.trim();
            let have = String::from_utf8_lossy(got.as_bytes()).trim().to_string();
            if want == "*" {
                true
            } else if let Some(part) = want.strip_prefix('~') {
                have.contains(part)
            } else if let Some(name) = want.strip_prefix('$') {
                match vars.get(name) {
                    Some(value) => *value == have,
                    None => {
                        vars.insert(name.to_string(), have);
                        true
                    }
                }
            } else {
                want == have
            }
        }
        Content::Opaque(bytes) => got.as_bytes() == bytes.as_slice(),
        Content::Children(children) => {
            let mut used = vec![false; got.children().len()];
            for child in children {
                let mut found = false;
                for (index, candidate) in got.children().iter().enumerate() {
                    if used[index] {
                        continue;
                    }
                    let mut attempt = vars.clone();
                    if matches(child, candidate, &mut attempt) {
                        *vars = attempt;
                        used[index] = true;
                        found = true;
                        break;
                    }
                }
                if !found {
                    return false;
                }
            }
            true
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn send(
    http: &reqwest::Client,
    base: &str,
    account: &Account,
    device: &Device,
    command: &str,
    params: &[(String, String)],
    headers: &[(String, String)],
    body: Option<String>,
) -> reqwest::Response {
    let url = format!("{base}/Microsoft-Server-ActiveSync");
    let mut request = if command == "OPTIONS" {
        http.request(reqwest::Method::OPTIONS, &url)
    } else {
        let query = if device.base64 {
            base64_query(device, command, params)
        } else {
            let mut query = format!(
                "Cmd={command}&User={}&DeviceId={}&DeviceType={}",
                account.name(),
                device.id,
                device.kind
            );
            for (k, v) in params {
                query.push_str(&format!("&{k}={v}"));
            }
            query
        };
        let mut request = http
            .post(format!("{url}?{query}"))
            .header("Content-Type", "application/vnd.ms-sync.wbxml");
        if !device.base64 {
            request = request.header("MS-ASProtocolVersion", &device.version);
        }
        request
    };
    request = request
        .basic_auth(account.name(), Some(account.secret()))
        .header("User-Agent", &device.agent);
    for (name, value) in headers {
        request = request.header(name.as_str(), value.as_str());
    }
    if let Some(xml) = body {
        let element = wbxml::from_xml(&xml).unwrap_or_else(|e| panic!("{command} request: {e}"));
        request = request.body(wbxml::encode(&element));
    }
    request.send().await.expect("request")
}

/// The base64 query of [MS-ASHTTP] 2.2.1.1.1.1.
fn base64_query(device: &Device, command: &str, params: &[(String, String)]) -> String {
    let code = [
        ("Sync", 0u8),
        ("SendMail", 1),
        ("SmartForward", 2),
        ("SmartReply", 3),
        ("FolderSync", 9),
        ("FolderCreate", 10),
        ("FolderDelete", 11),
        ("FolderUpdate", 12),
        ("MoveItems", 13),
        ("GetItemEstimate", 14),
        ("MeetingResponse", 15),
        ("Search", 16),
        ("Settings", 17),
        ("Ping", 18),
        ("ItemOperations", 19),
        ("Provision", 20),
    ]
    .iter()
    .find(|(name, _)| *name == command)
    .map(|(_, code)| *code)
    .expect("command code");
    let version: u8 = match device.version.as_str() {
        "14.1" => 141,
        _ => 161,
    };
    let mut bytes = vec![version, code, 0x09, 0x04, device.id.len() as u8];
    bytes.extend_from_slice(device.id.as_bytes());
    bytes.push(0); // no policy key here: it goes in the header
    bytes.push(device.kind.len() as u8);
    bytes.extend_from_slice(device.kind.as_bytes());
    for (name, value) in params {
        let tag = match name.as_str() {
            "CollectionId" => 1,
            "ItemId" => 3,
            "LongId" => 4,
            _ => continue,
        };
        bytes.push(tag);
        bytes.push(value.len() as u8);
        bytes.extend_from_slice(value.as_bytes());
    }
    STANDARD.encode(bytes)
}

/// Waits until the server's due tasks are done, the search index task
/// among them.
///
/// A device's first sync of a folder (and GetItemEstimate at that point)
/// lists it with `Email/query` filtered by receipt date and sorted by it.
/// With a SQL store those come from the message cache, current as soon as
/// a message is in; with the store this suite runs on (RocksDB) they come
/// from the search index, which a background task fills a moment later, so
/// a message delivered just before the listing could be left out (a GitHub
/// run of 2026-11-24 saw two Inbox messages where three were). The replay
/// waits as a SQL-backed server never has to; the gap for servers without a
/// SQL store is recorded in docs/progress.md.
async fn settle(test: &TestServer) {
    test.wait_for_tasks_skip_not_due().await;
}

/// A plain message to the account, put into its Inbox.
async fn deliver(account: &Account, subject: &str) {
    deliver_with(&account.jmap_client().await, account.name(), subject).await;
}

async fn deliver_with(client: &jmap_client::client::Client, name: &str, subject: &str) {
    let inbox = client
        .mailbox_query(
            mailbox::query::Filter::role(Role::Inbox).into(),
            None::<Vec<_>>,
        )
        .await
        .expect("inbox")
        .take_ids()
        .pop()
        .expect("an inbox");
    let message = format!(
        "From: Carol Clark <carol@example.org>\r\nTo: {}\r\nSubject: {subject}\r\n\
         Message-ID: <{}@example.org>\r\nDate: Mon, 23 Nov 2026 10:00:00 +0000\r\n\r\n\
         Hello, this is \"{subject}\".\r\n",
        name,
        subject.replace(' ', ".")
    );
    client
        .email_import(
            message.into_bytes(),
            [inbox],
            None::<Vec<&str>>,
            Some(next_received_at()),
        )
        .await
        .expect("import");
}

/// An invitation for the account: a text part and an iTIP REQUEST.
async fn deliver_invite(account: &Account, subject: &str) {
    let client = account.jmap_client().await;
    let inbox = client
        .mailbox_query(
            mailbox::query::Filter::role(Role::Inbox).into(),
            None::<Vec<_>>,
        )
        .await
        .expect("inbox")
        .take_ids()
        .pop()
        .expect("an inbox");
    let uid = format!("{}@example.org", subject.replace(' ', "-"));
    let message = format!(
        "From: Carol Clark <carol@example.org>\r\nTo: {name}\r\nSubject: {subject}\r\n\
         Message-ID: <invite-{uid}>\r\nDate: Mon, 23 Nov 2026 10:00:00 +0000\r\n\
         MIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=\"b\"\r\n\r\n\
         --b\r\nContent-Type: text/plain\r\n\r\nYou are invited.\r\n\
         --b\r\nContent-Type: text/calendar; method=REQUEST; charset=utf-8\r\n\r\n\
         BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//Test//EN\r\nMETHOD:REQUEST\r\n\
         BEGIN:VEVENT\r\nUID:{uid}\r\nDTSTAMP:20261123T100000Z\r\n\
         DTSTART:20261202T090000Z\r\nDTEND:20261202T100000Z\r\nSUMMARY:{subject}\r\n\
         ORGANIZER;CN=Carol Clark:mailto:carol@example.org\r\n\
         ATTENDEE;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:{name}\r\nSEQUENCE:0\r\n\
         END:VEVENT\r\nEND:VCALENDAR\r\n--b--\r\n",
        name = account.name(),
    );
    client
        .email_import(
            message.into_bytes(),
            [inbox],
            None::<Vec<&str>>,
            Some(next_received_at()),
        )
        .await
        .expect("import");
}

/// Puts a resource into the cloud, as another DAV client would.
async fn dav_put(http: &reqwest::Client, account: &Account, path: &str, text: String) {
    let base = std::env::var("INBUXA_DAV_URL").expect("INBUXA_DAV_URL for !dav-put");
    let content_type = if text.contains("BEGIN:VCARD") {
        "text/vcard; charset=utf-8"
    } else {
        "text/calendar; charset=utf-8"
    };
    let response = http
        .put(format!("{}/{path}", base.trim_end_matches('/')))
        .basic_auth(account.name(), Some(account.secret()))
        .header("Content-Type", content_type)
        .body(text)
        .send()
        .await
        .expect("PUT");
    assert!(
        response.status().is_success(),
        "PUT {path}: {}",
        response.status()
    );
}

/// Receipt times a second apart in the order of delivery, so "newest
/// first" is well defined even for messages delivered in the same second.
fn next_received_at() -> i64 {
    use std::sync::atomic::{AtomicI64, Ordering};
    static LAST: AtomicI64 = AtomicI64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64);
    let mut last = LAST.load(Ordering::Relaxed);
    loop {
        let next = now.max(last + 1);
        match LAST.compare_exchange(last, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(current) => last = current,
        }
    }
}

/// Asks Autodiscover in one dialect, anonymously, and returns the answer.
async fn autodiscover(http: &reqwest::Client, base: &str, dialect: &str, email: &str) -> String {
    let response = match dialect {
        "json" => http
            .get(format!(
                "{base}/autodiscover/autodiscover.json/v1.0/{email}?Protocol=ActiveSync"
            ))
            .send()
            .await,
        "pox" => http
            .post(format!("{base}/autodiscover/autodiscover.xml"))
            .header("Content-Type", "text/xml")
            .body(format!(
                "<?xml version=\"1.0\" encoding=\"utf-8\"?><Autodiscover \
                 xmlns=\"http://schemas.microsoft.com/exchange/autodiscover/mobilesync/requestschema/2006\">\
                 <Request><EMailAddress>{email}</EMailAddress><AcceptableResponseSchema>\
                 http://schemas.microsoft.com/exchange/autodiscover/mobilesync/responseschema/2006\
                 </AcceptableResponseSchema></Request></Autodiscover>"
            ))
            .send()
            .await,
        "soap" => http
            .post(format!("{base}/autodiscover/autodiscover.svc"))
            .header("Content-Type", "text/xml; charset=utf-8")
            .body(format!(
                "<?xml version=\"1.0\" encoding=\"utf-8\"?><soap:Envelope \
                 xmlns:a=\"http://schemas.microsoft.com/exchange/2010/Autodiscover\" \
                 xmlns:soap=\"http://schemas.xmlsoap.org/soap/envelope/\"><soap:Body>\
                 <a:GetUserSettingsRequestMessage><a:Request><a:Users><a:User><a:Mailbox>{email}\
                 </a:Mailbox></a:User></a:Users><a:RequestedSettings>\
                 <a:Setting>UserDisplayName</a:Setting><a:Setting>ExternalImap4Connections</a:Setting>\
                 <a:Setting>ExternalEwsUrl</a:Setting></a:RequestedSettings></a:Request>\
                 </a:GetUserSettingsRequestMessage></soap:Body></soap:Envelope>"
            ))
            .send()
            .await,
        _ => panic!("dialect {dialect}"),
    }
    .expect("autodiscover");
    response.text().await.expect("text")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subset_matching_captures() {
        let got = wbxml::from_xml(
            r#"<FolderSync xmlns="FolderHierarchy:"><Status>1</Status><SyncKey>7</SyncKey>
               <Changes><Count>2</Count><Add><ServerId>ma</ServerId><Type>12</Type></Add>
               <Add><ServerId>mb</ServerId><Type>2</Type></Add></Changes></FolderSync>"#,
        )
        .unwrap();
        let want = wbxml::from_xml(
            r#"<FolderSync xmlns="FolderHierarchy:"><SyncKey>$k</SyncKey>
               <Changes><Add><ServerId>$inbox</ServerId><Type>2</Type></Add></Changes></FolderSync>"#,
        )
        .unwrap();
        let mut vars = HashMap::new();
        assert!(matches(&want, &got, &mut vars));
        assert_eq!(vars["inbox"], "mb");
        assert_eq!(vars["k"], "7");
        assert_eq!(substitute("x=$inbox,$k", &vars), "x=mb,7");
    }
}
