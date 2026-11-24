/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Mail commands run as JMAP requests handed straight to the server's JMAP
//! handler, with the device's access token: the same permissions, shared
//! mailboxes, quotas, undelete, legal holds and push as every other client,
//! and nothing ActiveSync has to keep in step (docs/activesync.md, "Mail:
//! the JMAP methods, in process").
//!
//! A command builds one request (several method calls, joined with
//! back-references where one needs another's result), so each ActiveSync
//! command costs one pass through the JMAP layer. Requests and answers are
//! JSON values; the cost of building and reading them is small next to
//! the store reads behind them.

use crate::{
    Ctx,
    error::{Error, Result},
};
use common::{Server, auth::AccessToken};
use http_proto::HttpSessionData;
use jmap::api::request::RequestHandler;
use jmap_proto::request::Request;
use serde_json::{Value, json};
use types::id::Id;

/// The capabilities every request asks for.
pub const USING: [&str; 4] = [
    "urn:ietf:params:jmap:core",
    "urn:ietf:params:jmap:mail",
    "urn:ietf:params:jmap:submission",
    "urn:ietf:params:jmap:vacationresponse",
];

/// One method call: name, arguments and its call id.
pub struct Call {
    pub name: &'static str,
    pub args: Value,
    pub id: &'static str,
}

impl Call {
    pub fn new(name: &'static str, id: &'static str, args: Value) -> Self {
        Call { name, args, id }
    }
}

/// The answers of one request, by call id.
pub struct Answers(Vec<(String, Value, String)>);

impl Answers {
    /// The arguments of the answer to call `id`, or the JMAP error it got.
    pub fn get(&self, id: &str) -> Result<&Value> {
        let (name, args, _) = self
            .0
            .iter()
            .find(|(_, _, call_id)| call_id == id)
            .ok_or_else(|| Error::Internal(format!("no JMAP answer for {id}")))?;
        if name == "error" {
            Err(Error::Jmap(format!(
                "{}: {}",
                args["type"].as_str().unwrap_or("error"),
                args["description"].as_str().unwrap_or_default()
            )))
        } else {
            Ok(args)
        }
    }

    /// The answer to `id` taken out, to keep without copying.
    pub fn take(&mut self, id: &str) -> Result<Value> {
        self.get(id)?;
        let index = self
            .0
            .iter()
            .position(|(_, _, call_id)| call_id == id)
            .ok_or_else(|| Error::Internal(format!("no JMAP answer for {id}")))?;
        Ok(self.0.swap_remove(index).1)
    }
}

impl Ctx<'_> {
    /// The account id as JMAP writes it.
    pub fn jmap_account(&self) -> String {
        Id::from(self.account_id()).to_string()
    }

    /// Runs `calls` as one JMAP request with the device's access token.
    /// `accountId` is filled in on every call that doesn't name one.
    pub async fn jmap(&self, calls: Vec<Call>) -> Result<Answers> {
        self.jmap_with(&USING, calls).await
    }

    /// [`Ctx::jmap`] with other capabilities in `using`.
    pub async fn jmap_with(&self, using: &[&str], calls: Vec<Call>) -> Result<Answers> {
        run(self.server, self.token, self.session, using, calls).await
    }
}

/// Runs `calls` as one JMAP request for `token`'s account, with the
/// capabilities in `using`; `accountId` is filled in on every call that
/// doesn't name one.
pub async fn run(
    server: &Server,
    token: &AccessToken,
    session: &HttpSessionData,
    using: &[&str],
    calls: Vec<Call>,
) -> Result<Answers> {
    let account = Id::from(token.account_id()).to_string();
    let method_calls: Vec<Value> = calls
        .into_iter()
        .map(|mut call| {
            if let Value::Object(args) = &mut call.args {
                args.entry("accountId")
                    .or_insert_with(|| Value::String(account.clone()));
            }
            json!([call.name, call.args, call.id])
        })
        .collect();
    let bytes = serde_json::to_vec(&json!({ "using": using, "methodCalls": method_calls }))
        .map_err(|err| Error::Internal(err.to_string()))?;
    let request = Request::parse(
        &bytes,
        server.core.jmap.request_max_calls,
        // An internal request is as large as the device's message at
        // most, which the HTTP layer already capped.
        usize::MAX,
    )?;
    let response = server.handle_jmap_request(request, token, session).await;
    let value = serde_json::to_value(&response).map_err(|err| Error::Internal(err.to_string()))?;
    let Value::Object(mut object) = value else {
        return Err(Error::Internal("JMAP answered something else".into()));
    };
    let Some(Value::Array(responses)) = object.remove("methodResponses") else {
        return Err(Error::Internal(
            "JMAP answered without methodResponses".into(),
        ));
    };
    Ok(Answers(
        responses
            .into_iter()
            .filter_map(|response| match response {
                Value::Array(mut parts) if parts.len() == 3 => {
                    let id = parts.pop()?.as_str()?.to_string();
                    let args = parts.pop()?;
                    let name = parts.pop()?.as_str()?.to_string();
                    Some((name, args, id))
                }
                _ => None,
            })
            .collect(),
    ))
}
