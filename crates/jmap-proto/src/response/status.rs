/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::object::email::{EmailProperty, EmailValue};
use crate::types::state::State;
use jmap_tools::Value;
use types::{id::Id, type_state::DataType};
use utils::map::vec_map::VecMap;

#[derive(serde::Serialize, Debug)]
#[serde(tag = "@type")]
pub enum PushObject {
    StateChange {
        changed: VecMap<Id, VecMap<DataType, State>>,
    },
    EmailPush {
        #[serde(rename = "accountId")]
        account_id: Id,
        emails: Vec<Value<'static, EmailProperty, EmailValue>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        state: Option<State>,
    },
}
