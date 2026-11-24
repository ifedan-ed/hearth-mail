/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Provision ([MS-ASPROV], [MS-ASCMD] 2.2.1.14): the device asks for the
//! server's policy, gets it with a temporary key, acknowledges it and gets
//! the final key (MS-ASPROV 4.1).
//!
//! The policy asks nothing of the device (no PIN, no encryption, every
//! feature allowed), and keys are handed out but never demanded (no HTTP
//! 449): Hearth Mail leaves device rules to the people who own the devices
//! (docs/decisions.md). Remote wipe is not offered; a device acknowledging
//! one is answered with success.

use crate::{Ctx, Reply, error::Result};
use activesync_proto::wbxml::{
    Element, Tag,
    codepages::{provision as p, settings},
};

/// The only policy type of 14.1 and 16.1.
const POLICY_TYPE: &str = "MS-EAS-Provisioning-WBXML";

/// Provision status codes ([MS-ASPROV] 2.2.2.54.1, 2.2.2.54.2).
mod status {
    pub const OK: &str = "1";
    pub const PROTOCOL: &str = "2";
    pub const UNKNOWN_POLICY: &str = "2";
    pub const BAD_KEY: &str = "5";
}

pub async fn provision(ctx: &Ctx<'_>, request: Element) -> Result<Reply> {
    let mut device = ctx.device().await?;
    let mut answer = Element::new(p::PROVISION).with(Element::text(p::STATUS, status::OK));

    if let Some(info) = request
        .child(settings::DEVICE_INFORMATION)
        .and_then(|info| info.child(settings::SET))
    {
        device.info = super::settings::device_info(info);
        answer.push(
            Element::new(settings::DEVICE_INFORMATION)
                .with(Element::text(settings::STATUS, status::OK)),
        );
    }

    if request.child(p::REMOTE_WIPE).is_some() {
        answer.push(Element::new(p::REMOTE_WIPE).with(Element::text(p::STATUS, status::OK)));
        return Ok(Reply::Wbxml(answer));
    }

    let Some(policy) = request
        .child(p::POLICIES)
        .and_then(|policies| policies.child(p::POLICY))
    else {
        return Ok(Reply::Wbxml(
            Element::new(p::PROVISION).with(Element::text(p::STATUS, status::PROTOCOL)),
        ));
    };
    let kind = policy.child_str(p::POLICY_TYPE).unwrap_or_default().trim();
    let mut answer_policy =
        Element::new(p::POLICY).with(Element::text(p::POLICY_TYPE, POLICY_TYPE));
    if kind != POLICY_TYPE {
        answer_policy.push(Element::text(p::STATUS, status::UNKNOWN_POLICY));
    } else if let Some(key) = policy.child_u64(p::POLICY_KEY) {
        // Phase 3: the device acknowledges the temporary key.
        if device.policy_key == Some(key as u32) {
            let final_key = new_key();
            device.policy_key = Some(final_key);
            answer_policy.push(Element::text(p::STATUS, status::OK));
            answer_policy.push(Element::text(p::POLICY_KEY, final_key.to_string()));
        } else {
            answer_policy.push(Element::text(p::STATUS, status::BAD_KEY));
        }
    } else {
        // Phase 2: hand over the policy with a temporary key.
        let temporary = new_key();
        device.policy_key = Some(temporary);
        answer_policy.push(Element::text(p::STATUS, status::OK));
        answer_policy.push(Element::text(p::POLICY_KEY, temporary.to_string()));
        answer_policy.push(Element::new(p::DATA).with(provision_doc()));
    }
    ctx.save_device(&device).await?;
    answer.push(Element::new(p::POLICIES).with(answer_policy));
    Ok(Reply::Wbxml(answer))
}

/// A policy key: any non-zero number (0 means "no policy yet").
fn new_key() -> u32 {
    rand::random::<u32>().max(1)
}

/// The policy: nothing required, everything allowed ([MS-ASPROV] 2.2.2.27
/// lists the settings; unlimited sizes are left empty, as 4.1.2 shows).
fn provision_doc() -> Element {
    let settings: [(Tag, &str); 36] = [
        (p::DEVICE_PASSWORD_ENABLED, "0"),
        (p::ALPHANUMERIC_DEVICE_PASSWORD_REQUIRED, "0"),
        (p::PASSWORD_RECOVERY_ENABLED, "0"),
        (p::REQUIRE_STORAGE_CARD_ENCRYPTION, "0"),
        (p::ATTACHMENTS_ENABLED, "1"),
        (p::MIN_DEVICE_PASSWORD_LENGTH, ""),
        (p::MAX_INACTIVITY_TIME_DEVICE_LOCK, ""),
        (p::MAX_DEVICE_PASSWORD_FAILED_ATTEMPTS, ""),
        (p::MAX_ATTACHMENT_SIZE, ""),
        (p::ALLOW_SIMPLE_DEVICE_PASSWORD, "1"),
        (p::DEVICE_PASSWORD_EXPIRATION, ""),
        (p::DEVICE_PASSWORD_HISTORY, "0"),
        (p::ALLOW_STORAGE_CARD, "1"),
        (p::ALLOW_CAMERA, "1"),
        (p::REQUIRE_DEVICE_ENCRYPTION, "0"),
        (p::ALLOW_UNSIGNED_APPLICATIONS, "1"),
        (p::ALLOW_UNSIGNED_INSTALLATION_PACKAGES, "1"),
        (p::MIN_DEVICE_PASSWORD_COMPLEX_CHARACTERS, "1"),
        (p::ALLOW_WI_FI, "1"),
        (p::ALLOW_TEXT_MESSAGING, "1"),
        (p::ALLOW_POPIMAP_EMAIL, "1"),
        (p::ALLOW_BLUETOOTH, "2"),
        (p::ALLOW_IR_DA, "1"),
        (p::REQUIRE_MANUAL_SYNC_WHEN_ROAMING, "0"),
        (p::ALLOW_DESKTOP_SYNC, "1"),
        (p::MAX_CALENDAR_AGE_FILTER, "0"),
        (p::ALLOW_HTML_EMAIL, "1"),
        (p::MAX_EMAIL_AGE_FILTER, "0"),
        (p::MAX_EMAIL_BODY_TRUNCATION_SIZE, "-1"),
        (p::MAX_EMAIL_HTML_BODY_TRUNCATION_SIZE, "-1"),
        (p::REQUIRE_SIGNED_SMIME_MESSAGES, "0"),
        (p::REQUIRE_ENCRYPTED_SMIME_MESSAGES, "0"),
        (p::ALLOW_SMIME_SOFT_CERTS, "1"),
        (p::ALLOW_BROWSER, "1"),
        (p::ALLOW_CONSUMER_EMAIL, "1"),
        (p::ALLOW_INTERNET_SHARING, "1"),
    ];
    let mut doc = Element::new(p::EAS_PROVISION_DOC);
    for (tag, value) in settings {
        doc.push(if value.is_empty() {
            Element::new(tag)
        } else {
            Element::text(tag, value)
        });
    }
    doc
}
