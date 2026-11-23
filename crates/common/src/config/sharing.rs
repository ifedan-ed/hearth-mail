/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 *
 * Modified by Coffey Labs in 2026 for INBUXA.
 * Modified for Hearth Mail in 2026.
 */

//! Sharing limits for shared mailboxes and the directory.
//!
//! These two settings used to live in the groupware configuration next to
//! the calendar, contact and WebDAV limits. Hearth Mail keeps only mail, so
//! they are all that is left of it: how many accounts one mailbox can be
//! shared with, and whether people may search the directory for others to
//! share with.

use registry::schema::structs::Sharing;
use store::registry::bootstrap::Bootstrap;

#[derive(Debug, Clone, Default)]
pub struct SharingConfig {
    /// Most grants one shared item (a mailbox) may carry.
    pub max_shares_per_item: usize,
    /// Whether accounts may look up other principals to share with.
    pub allow_directory_query: bool,
}

impl SharingConfig {
    pub async fn parse(bp: &mut Bootstrap) -> Self {
        let share = bp.setting_infallible::<Sharing>().await;
        SharingConfig {
            max_shares_per_item: share.max_shares as usize,
            allow_directory_query: share.allow_directory_queries,
        }
    }
}
