/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 *
 * Modified by Coffey Labs in 2026 for INBUXA.
 */

use crate::{
    Server,
    auth::AccountCache,
    storage::{ObjectQuota, TenantQuota},
};
use registry::{
    schema::enums::{StorageQuota, TenantStorageQuota},
    types::EnumImpl,
};
use std::sync::Arc;
use store::{
    ValueKey,
    write::{BatchBuilder, ValueClass},
};
use trc::AddContext;

// inbuxa: the disk quota of the account a request writes into, with the bytes
// the request has already added. A request commits all its items at once, so
// each item is checked together with the ones before it, not against the
// stored usage alone.
pub struct QuotaBudget {
    account: Arc<AccountCache>,
    reserved: u64,
}

impl QuotaBudget {
    pub fn new(account: Arc<AccountCache>) -> Self {
        Self {
            account,
            reserved: 0,
        }
    }
}

impl Server {
    pub async fn get_used_quota_account(&self, account_id: u32) -> trc::Result<i64> {
        self.core
            .storage
            .data
            .get_counter(ValueKey {
                account_id,
                collection: 0,
                document_id: 0,
                class: ValueClass::Quota,
            })
            .await
            .add_context(|err| err.caused_by(trc::location!()).account_id(account_id))
    }

    // inbuxa: MT-20: a batch's tenant storage changes go to the tenant of the
    // account whose data changed, not the writer's: a sharee or delegate in
    // another tenant, or a server administrator with none, would otherwise
    // move the wrong counter. A deleted account still kept (UD-15) is its
    // kept record's tenant's. Run before every write that carries them.
    pub async fn resolve_tenant_quota(&self, batch: &mut BatchBuilder) -> trc::Result<()> {
        let accounts = batch.tenant_quota_accounts();
        if accounts.is_empty() {
            return Ok(());
        }
        let mut tenants = Vec::with_capacity(accounts.len());
        for account_id in accounts {
            let tenant_id = match self.try_account(account_id).await? {
                Some(account) => account.id_tenant,
                None => inbuxa_features::undelete::data::kept_account(
                    &self.core.storage.data,
                    account_id,
                )
                .await?
                .and_then(|kept| kept.member_tenant_id)
                .map(|tenant_id| tenant_id as u32),
            };
            tenants.push((account_id, tenant_id));
        }
        batch.retarget_tenant_quota(|account_id| {
            tenants
                .iter()
                .find(|(id, _)| *id == account_id)
                .and_then(|(_, tenant_id)| *tenant_id)
        });
        Ok(())
    }

    // inbuxa: MT-20: storage used by all a tenant's members together
    pub async fn get_used_quota_tenant(&self, tenant_id: u32) -> trc::Result<i64> {
        inbuxa_features::tenancy::quota::used(&self.core.storage.data, tenant_id).await
    }

    pub async fn has_available_quota(
        &self,
        account: &AccountCache,
        item_size: u64,
    ) -> trc::Result<()> {
        if account.quota_disk != 0 {
            let used_quota = self.get_used_quota_account(account.id).await?.max(0) as u64;

            if used_quota + item_size > account.quota_disk {
                return Err(trc::LimitEvent::Quota
                    .into_err()
                    .ctx(trc::Key::Limit, account.quota_disk)
                    .ctx(trc::Key::Size, used_quota));
            }
        }

        // inbuxa: MT-19: the tenant's limit applies too, whichever is reached first
        if let Some(tenant_id) = account.id_tenant {
            let tenant = self.tenant(tenant_id).await?;
            if tenant.quota_disk != 0 {
                let used_quota = self.get_used_quota_tenant(tenant_id).await?.max(0) as u64;

                if used_quota + item_size > tenant.quota_disk {
                    return Err(trc::LimitEvent::TenantQuota
                        .into_err()
                        .ctx(trc::Key::Id, tenant_id)
                        .ctx(trc::Key::Limit, tenant.quota_disk)
                        .ctx(trc::Key::Size, used_quota));
                }
            }
        }

        Ok(())
    }

    // inbuxa: the disk space an account can really use, for reporting it to
    // clients (JMAP Quota, IMAP QUOTA, DAV quota properties). Returns what
    // the account uses and, when any limit applies, the limit it will
    // actually meet: its own quota or its tenant's remaining space,
    // whichever comes first (MT-19). Without this a 50 GB account in a 5 GB
    // tenant shows 45 GB free while its mail is refused.
    pub async fn effective_disk_quota(
        &self,
        account: &AccountCache,
    ) -> trc::Result<(u64, Option<u64>)> {
        let used = self.get_used_quota_account(account.id).await?.max(0) as u64;
        let tenant = match account.id_tenant {
            Some(tenant_id) => {
                let tenant = self.tenant(tenant_id).await?;
                if tenant.quota_disk > 0 {
                    let tenant_used = self.get_used_quota_tenant(tenant_id).await?.max(0) as u64;
                    Some((tenant.quota_disk, tenant_used))
                } else {
                    None
                }
            }
            None => None,
        };
        Ok((used, effective_limit(used, account.quota_disk, tenant)))
    }

    // inbuxa: checks an item against the account's quota and, when it fits,
    // counts it towards the rest of the request
    pub async fn reserve_quota(&self, budget: &mut QuotaBudget, item_size: u64) -> trc::Result<()> {
        if item_size > 0 {
            self.has_available_quota(&budget.account, budget.reserved + item_size)
                .await?;
            budget.reserved += item_size;
        }
        Ok(())
    }

    #[inline(always)]
    pub fn object_quota(&self, user_quotas: Option<&ObjectQuota>, object: StorageQuota) -> u32 {
        user_quotas.unwrap_or(&self.core.email.max_objects).0[object as usize]
    }
}

impl ObjectQuota {
    #[inline(always)]
    pub fn set(&mut self, item: StorageQuota, max: u32) {
        self.0[item as usize] = max;
    }

    #[inline(always)]
    pub fn get(&self, item: StorageQuota) -> u32 {
        self.0[item as usize]
    }
}

impl TenantQuota {
    #[inline(always)]
    pub fn set(&mut self, item: TenantStorageQuota, max: u32) {
        self.0[item as usize] = max;
    }

    #[inline(always)]
    pub fn get(&self, item: TenantStorageQuota) -> u32 {
        self.0[item as usize]
    }
}

impl Default for ObjectQuota {
    fn default() -> Self {
        Self([u32::MAX; StorageQuota::COUNT - 1])
    }
}

impl Default for TenantQuota {
    fn default() -> Self {
        Self([u32::MAX; TenantStorageQuota::COUNT - 1])
    }
}

/// The disk limit an account will meet, from what it uses (`used`), its
/// own quota (`account_quota`, 0 for none) and its tenant's quota and use,
/// when the tenant has one.
///
/// The account's own quota is reported as it is, also when the account is
/// already over it (a quota lowered below the use, or mail stored before
/// the quota was set): clients show "43 MB of 40 MB", not a limit that
/// moves up with the use. Hearth Mail's end-to-end run found the old
/// arithmetic (used plus the space left, which is zero when over) reporting
/// the use itself as the limit. The tenant's limit counts as the use plus
/// the tenant's space left, so a tenant running out shows on every
/// account in it; the lower of the two applies.
pub fn effective_limit(used: u64, account_quota: u64, tenant: Option<(u64, u64)>) -> Option<u64> {
    let own = (account_quota > 0).then_some(account_quota);
    let tenant =
        tenant.map(|(quota, tenant_used)| used.saturating_add(quota.saturating_sub(tenant_used)));
    match (own, tenant) {
        (Some(own), Some(tenant)) => Some(own.min(tenant)),
        (own, tenant) => own.or(tenant),
    }
}

#[cfg(test)]
mod tests {
    use super::effective_limit;

    #[test]
    fn no_limit() {
        assert_eq!(effective_limit(10, 0, None), None);
    }

    #[test]
    fn own_quota_under_and_over() {
        assert_eq!(effective_limit(10, 40, None), Some(40));
        // Over the quota: still the quota, never the use.
        assert_eq!(effective_limit(43, 40, None), Some(40));
    }

    #[test]
    fn tenant_space_left_applies_when_lower() {
        // 50 GB account in a tenant with 5 of 100 left: 10 + 5.
        assert_eq!(effective_limit(10, 50, Some((100, 95))), Some(15));
        // Plenty left in the tenant: the account's own quota.
        assert_eq!(effective_limit(10, 50, Some((1000, 100))), Some(50));
        // No own quota: the tenant's.
        assert_eq!(effective_limit(10, 0, Some((100, 95))), Some(15));
        // Tenant already over: nothing more fits.
        assert_eq!(effective_limit(10, 50, Some((100, 120))), Some(10));
    }
}
