/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 *
 * Modified by Coffey Labs in 2026 for INBUXA.
 */

pub mod serialize;
pub mod status;

use self::serialize::serialize_hex;
use crate::{
    error::method::MethodErrorWrapper,
    method::{
        changes::ChangesResponse,
        copy::{CopyBlobResponse, CopyResponse},
        get::GetResponse,
        import::ImportEmailResponse,
        lookup::BlobLookupResponse,
        parse::ParseResponse,
        query::QueryResponse,
        query_changes::QueryChangesResponse,
        search_snippet::GetSearchSnippetResponse,
        set::SetResponse,
        upload::BlobUploadResponse,
        validate::ValidateSieveScriptResponse,
    },
    object::{
        AnyId, blob::Blob, email::Email, email_submission::EmailSubmission, identity::Identity,
        mailbox::Mailbox, principal::Principal, push_subscription::PushSubscription, quota::Quota,
        registry::Registry, share_notification::ShareNotification, sieve::Sieve, thread::Thread,
        vacation_response::VacationResponse,
    },
    request::{Call, method::MethodName},
};
use jmap_tools::{Null, Value};
use std::collections::HashMap;

#[derive(Debug, serde::Serialize)]
#[serde(untagged)]
pub enum ResponseMethod<'x> {
    Get(GetResponseMethod),
    Set(SetResponseMethod),
    Changes(ChangesResponseMethod),
    Copy(CopyResponseMethod),
    ImportEmail(ImportEmailResponse),
    Parse(ParseResponseMethod),
    QueryChanges(QueryChangesResponse),
    Query(QueryResponse),
    SearchSnippet(GetSearchSnippetResponse),
    ValidateScript(ValidateSieveScriptResponse),
    LookupBlob(BlobLookupResponse),
    UploadBlob(BlobUploadResponse),
    Echo(Value<'x, Null, Null>),
    Error(MethodErrorWrapper),
}

#[derive(Debug, serde::Serialize)]
#[serde(untagged)]
pub enum GetResponseMethod {
    Email(GetResponse<Email>),
    Mailbox(GetResponse<Mailbox>),
    Thread(GetResponse<Thread>),
    Identity(GetResponse<Identity>),
    EmailSubmission(GetResponse<EmailSubmission>),
    PushSubscription(GetResponse<PushSubscription>),
    Sieve(GetResponse<Sieve>),
    VacationResponse(GetResponse<VacationResponse>),
    Principal(GetResponse<Principal>),
    Quota(GetResponse<Quota>),
    Blob(GetResponse<Blob>),
    ShareNotification(GetResponse<ShareNotification>),
    Registry(GetResponse<Registry>),
    MaskedEmail(GetResponse<crate::object::fastmail_masked_email::FastmailMaskedEmail>),
    DeletedAccount(GetResponse<crate::object::inbuxa_deleted_account::DeletedAccount>),
    AiLimits(GetResponse<crate::object::inbuxa_ai_limits::AiLimits>),
    LogSettings(GetResponse<crate::object::inbuxa_log_settings::LogSettings>),
    DlpSettings(GetResponse<crate::object::inbuxa_dlp_settings::DlpSettings>),
    DataInventory(GetResponse<crate::object::inbuxa_data_inventory::DataInventory>),
    InventorySnapshot(GetResponse<crate::object::inbuxa_inventory_snapshot::InventorySnapshot>),
    AuditEvent(GetResponse<crate::object::inbuxa_audit::AuditEvent>),
    AuditSettings(GetResponse<crate::object::inbuxa_audit::AuditSettings>),
    AccountLock(GetResponse<crate::object::inbuxa_account_lock::AccountLock>),
    LegalHold(GetResponse<crate::object::inbuxa_legal_hold::LegalHold>),
    MailRule(GetResponse<crate::object::inbuxa_mail_rule::MailRule>),
    Migration(GetResponse<crate::object::inbuxa_migration::Migration>),
    StoreCheck(GetResponse<crate::object::inbuxa_store_check::StoreCheck>),
    StoreSnapshot(GetResponse<crate::object::inbuxa_store_snapshot::StoreSnapshot>),
    BlobStoreChange(GetResponse<crate::object::inbuxa_blob_store_change::BlobStoreChange>),
    Relay(GetResponse<crate::object::inbuxa_relay::Relay>),
    RelayMessage(GetResponse<crate::object::inbuxa_relay_message::RelayMessage>),
    SecurityAcceptance(GetResponse<crate::object::inbuxa_security_acceptance::SecurityAcceptance>),
    // inbuxa: passkeys
    Passkey(GetResponse<crate::object::inbuxa_passkey::Passkey>),
    DeliverabilityReport(
        GetResponse<crate::object::inbuxa_deliverability_report::DeliverabilityReport>,
    ),
    DeliverabilitySettings(
        GetResponse<crate::object::inbuxa_deliverability_settings::DeliverabilitySettings>,
    ),
    ReportExport(GetResponse<crate::object::inbuxa_report_export::ReportExport>),
    ScheduledReport(GetResponse<crate::object::inbuxa_scheduled_report::ScheduledReport>),
    ScheduledReportSettings(
        GetResponse<crate::object::inbuxa_scheduled_report_settings::ScheduledReportSettings>,
    ),
    Journal(GetResponse<crate::object::inbuxa_journal::Journal>),
    JournalEntry(GetResponse<crate::object::inbuxa_journal_entry::JournalEntry>),
    HeldMessage(GetResponse<crate::object::inbuxa_held_message::HeldMessage>),
    HoldExport(GetResponse<crate::object::inbuxa_hold_export::HoldExport>),
    ProtocolPolicy(GetResponse<crate::object::inbuxa_protocol_policy::ProtocolPolicy>),
    TenantProtocolPolicy(
        GetResponse<crate::object::inbuxa_tenant_protocol_policy::TenantProtocolPolicy>,
    ),
    SharingPolicy(GetResponse<crate::object::inbuxa_sharing_policy::SharingPolicy>),
    FrontEnds(GetResponse<crate::object::inbuxa_front_ends::FrontEnds>),
}

#[derive(Debug, serde::Serialize)]
#[serde(untagged)]
pub enum SetResponseMethod {
    Email(Box<SetResponse<Email>>),
    Mailbox(Box<SetResponse<Mailbox>>),
    Identity(Box<SetResponse<Identity>>),
    EmailSubmission(Box<SetResponse<EmailSubmission>>),
    PushSubscription(Box<SetResponse<PushSubscription>>),
    Sieve(Box<SetResponse<Sieve>>),
    VacationResponse(Box<SetResponse<VacationResponse>>),
    ShareNotification(Box<SetResponse<ShareNotification>>),
    Registry(Box<SetResponse<Registry>>),
    MaskedEmail(Box<SetResponse<crate::object::fastmail_masked_email::FastmailMaskedEmail>>),
    DeletedAccount(Box<SetResponse<crate::object::inbuxa_deleted_account::DeletedAccount>>),
    AiLimits(Box<SetResponse<crate::object::inbuxa_ai_limits::AiLimits>>),
    LogSettings(Box<SetResponse<crate::object::inbuxa_log_settings::LogSettings>>),
    DlpSettings(Box<SetResponse<crate::object::inbuxa_dlp_settings::DlpSettings>>),
    AuditSettings(Box<SetResponse<crate::object::inbuxa_audit::AuditSettings>>),
    AuditExport(Box<SetResponse<crate::object::inbuxa_audit::AuditExport>>),
    AuditVerification(Box<SetResponse<crate::object::inbuxa_audit::AuditVerification>>),
    AccountLock(Box<SetResponse<crate::object::inbuxa_account_lock::AccountLock>>),
    LegalHold(Box<SetResponse<crate::object::inbuxa_legal_hold::LegalHold>>),
    MailRule(Box<SetResponse<crate::object::inbuxa_mail_rule::MailRule>>),
    Migration(Box<SetResponse<crate::object::inbuxa_migration::Migration>>),
    StoreCheck(Box<SetResponse<crate::object::inbuxa_store_check::StoreCheck>>),
    StoreSnapshot(Box<SetResponse<crate::object::inbuxa_store_snapshot::StoreSnapshot>>),
    BlobStoreChange(Box<SetResponse<crate::object::inbuxa_blob_store_change::BlobStoreChange>>),
    Relay(Box<SetResponse<crate::object::inbuxa_relay::Relay>>),
    RelayMessage(Box<SetResponse<crate::object::inbuxa_relay_message::RelayMessage>>),
    SecurityAcceptance(
        Box<SetResponse<crate::object::inbuxa_security_acceptance::SecurityAcceptance>>,
    ),
    // inbuxa: passkeys
    Passkey(Box<SetResponse<crate::object::inbuxa_passkey::Passkey>>),
    DeliverabilityReport(
        Box<SetResponse<crate::object::inbuxa_deliverability_report::DeliverabilityReport>>,
    ),
    DeliverabilitySettings(
        Box<SetResponse<crate::object::inbuxa_deliverability_settings::DeliverabilitySettings>>,
    ),
    ReportExport(Box<SetResponse<crate::object::inbuxa_report_export::ReportExport>>),
    ScheduledReport(Box<SetResponse<crate::object::inbuxa_scheduled_report::ScheduledReport>>),
    ScheduledReportSettings(
        Box<SetResponse<crate::object::inbuxa_scheduled_report_settings::ScheduledReportSettings>>,
    ),
    Journal(Box<SetResponse<crate::object::inbuxa_journal::Journal>>),
    JournalExport(Box<SetResponse<crate::object::inbuxa_journal_entry::JournalExport>>),
    JournalVerification(Box<SetResponse<crate::object::inbuxa_journal_entry::JournalVerification>>),
    HeldMessage(Box<SetResponse<crate::object::inbuxa_held_message::HeldMessage>>),
    HoldExport(Box<SetResponse<crate::object::inbuxa_hold_export::HoldExport>>),
    Explanation(Box<SetResponse<crate::object::inbuxa_explanation::Explanation>>),
    ProtocolPolicy(Box<SetResponse<crate::object::inbuxa_protocol_policy::ProtocolPolicy>>),
    TenantProtocolPolicy(
        Box<SetResponse<crate::object::inbuxa_tenant_protocol_policy::TenantProtocolPolicy>>,
    ),
    SharingPolicy(Box<SetResponse<crate::object::inbuxa_sharing_policy::SharingPolicy>>),
    FrontEnds(Box<SetResponse<crate::object::inbuxa_front_ends::FrontEnds>>),
}

#[derive(Debug, serde::Serialize)]
#[serde(untagged)]
pub enum ChangesResponseMethod {
    Email(Box<ChangesResponse<Email>>),
    Mailbox(Box<ChangesResponse<Mailbox>>),
    Thread(Box<ChangesResponse<Thread>>),
    Identity(Box<ChangesResponse<Identity>>),
    EmailSubmission(Box<ChangesResponse<EmailSubmission>>),
    Quota(Box<ChangesResponse<Quota>>),
    ShareNotification(Box<ChangesResponse<ShareNotification>>),
    // inbuxa: x:MaskedEmail/changes
    Registry(Box<ChangesResponse<crate::object::registry::Registry>>),
}

#[derive(Debug, serde::Serialize)]
#[serde(untagged)]
pub enum CopyResponseMethod {
    Email(CopyResponse<Email>),
    Blob(CopyBlobResponse),
}

#[derive(Debug, serde::Serialize)]
#[serde(untagged)]
pub enum ParseResponseMethod {
    Email(ParseResponse<Email>),
}

#[derive(Debug, serde::Serialize)]
pub struct Response<'x> {
    #[serde(rename = "methodResponses")]
    pub method_responses: Vec<Call<ResponseMethod<'x>>>,

    #[serde(rename = "sessionState")]
    #[serde(serialize_with = "serialize_hex")]
    pub session_state: u32,

    #[serde(rename = "createdIds")]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub created_ids: HashMap<String, AnyId>,
}

impl<'x> Response<'x> {
    pub fn new(session_state: u32, created_ids: HashMap<String, AnyId>, capacity: usize) -> Self {
        Response {
            session_state,
            created_ids,
            method_responses: Vec::with_capacity(capacity),
        }
    }

    pub fn push_response(
        &mut self,
        id: String,
        name: MethodName,
        method: impl Into<ResponseMethod<'x>>,
    ) {
        self.method_responses.push(Call {
            id,
            method: method.into(),
            name,
        });
    }

    pub fn push_error(&mut self, id: String, err: impl Into<MethodErrorWrapper>) {
        self.method_responses.push(Call {
            id,
            method: ResponseMethod::Error(err.into()),
            name: MethodName::error(),
        });
    }

    pub fn push_created_id(&mut self, create_id: String, id: impl Into<AnyId>) {
        self.created_ids.insert(create_id, id.into());
    }
}

impl From<trc::Error> for ResponseMethod<'_> {
    fn from(error: trc::Error) -> Self {
        ResponseMethod::Error(error.into())
    }
}

impl<'x, T: Into<ResponseMethod<'x>>> From<trc::Result<T>> for ResponseMethod<'x> {
    fn from(result: trc::Result<T>) -> Self {
        match result {
            Ok(value) => value.into(),
            Err(error) => error.into(),
        }
    }
}

impl<'x> From<GetResponse<Email>> for ResponseMethod<'x> {
    fn from(value: GetResponse<Email>) -> Self {
        ResponseMethod::Get(GetResponseMethod::Email(value))
    }
}

impl<'x> From<GetResponse<Mailbox>> for ResponseMethod<'x> {
    fn from(value: GetResponse<Mailbox>) -> Self {
        ResponseMethod::Get(GetResponseMethod::Mailbox(value))
    }
}

impl<'x> From<GetResponse<Thread>> for ResponseMethod<'x> {
    fn from(value: GetResponse<Thread>) -> Self {
        ResponseMethod::Get(GetResponseMethod::Thread(value))
    }
}

impl<'x> From<GetResponse<Identity>> for ResponseMethod<'x> {
    fn from(value: GetResponse<Identity>) -> Self {
        ResponseMethod::Get(GetResponseMethod::Identity(value))
    }
}

impl<'x> From<GetResponse<EmailSubmission>> for ResponseMethod<'x> {
    fn from(value: GetResponse<EmailSubmission>) -> Self {
        ResponseMethod::Get(GetResponseMethod::EmailSubmission(value))
    }
}

impl<'x> From<GetResponse<PushSubscription>> for ResponseMethod<'x> {
    fn from(value: GetResponse<PushSubscription>) -> Self {
        ResponseMethod::Get(GetResponseMethod::PushSubscription(value))
    }
}

impl<'x> From<GetResponse<Sieve>> for ResponseMethod<'x> {
    fn from(value: GetResponse<Sieve>) -> Self {
        ResponseMethod::Get(GetResponseMethod::Sieve(value))
    }
}

// inbuxa: Fastmail's MaskedEmail
impl<'x> From<GetResponse<crate::object::fastmail_masked_email::FastmailMaskedEmail>>
    for ResponseMethod<'x>
{
    fn from(value: GetResponse<crate::object::fastmail_masked_email::FastmailMaskedEmail>) -> Self {
        ResponseMethod::Get(GetResponseMethod::MaskedEmail(value))
    }
}

impl<'x> From<SetResponse<crate::object::fastmail_masked_email::FastmailMaskedEmail>>
    for ResponseMethod<'x>
{
    fn from(value: SetResponse<crate::object::fastmail_masked_email::FastmailMaskedEmail>) -> Self {
        ResponseMethod::Set(SetResponseMethod::MaskedEmail(Box::new(value)))
    }
}

// inbuxa: AI call limits
impl<'x> From<GetResponse<crate::object::inbuxa_protocol_policy::ProtocolPolicy>>
    for ResponseMethod<'x>
{
    fn from(value: GetResponse<crate::object::inbuxa_protocol_policy::ProtocolPolicy>) -> Self {
        ResponseMethod::Get(GetResponseMethod::ProtocolPolicy(value))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_protocol_policy::ProtocolPolicy>>
    for ResponseMethod<'x>
{
    fn from(value: SetResponse<crate::object::inbuxa_protocol_policy::ProtocolPolicy>) -> Self {
        ResponseMethod::Set(SetResponseMethod::ProtocolPolicy(Box::new(value)))
    }
}

impl<'x> From<GetResponse<crate::object::inbuxa_tenant_protocol_policy::TenantProtocolPolicy>>
    for ResponseMethod<'x>
{
    fn from(
        value: GetResponse<crate::object::inbuxa_tenant_protocol_policy::TenantProtocolPolicy>,
    ) -> Self {
        ResponseMethod::Get(GetResponseMethod::TenantProtocolPolicy(value))
    }
}

impl<'x> From<GetResponse<crate::object::inbuxa_sharing_policy::SharingPolicy>>
    for ResponseMethod<'x>
{
    fn from(value: GetResponse<crate::object::inbuxa_sharing_policy::SharingPolicy>) -> Self {
        ResponseMethod::Get(GetResponseMethod::SharingPolicy(value))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_tenant_protocol_policy::TenantProtocolPolicy>>
    for ResponseMethod<'x>
{
    fn from(
        value: SetResponse<crate::object::inbuxa_tenant_protocol_policy::TenantProtocolPolicy>,
    ) -> Self {
        ResponseMethod::Set(SetResponseMethod::TenantProtocolPolicy(Box::new(value)))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_sharing_policy::SharingPolicy>>
    for ResponseMethod<'x>
{
    fn from(value: SetResponse<crate::object::inbuxa_sharing_policy::SharingPolicy>) -> Self {
        ResponseMethod::Set(SetResponseMethod::SharingPolicy(Box::new(value)))
    }
}

impl<'x> From<GetResponse<crate::object::inbuxa_front_ends::FrontEnds>> for ResponseMethod<'x> {
    fn from(value: GetResponse<crate::object::inbuxa_front_ends::FrontEnds>) -> Self {
        ResponseMethod::Get(GetResponseMethod::FrontEnds(value))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_front_ends::FrontEnds>> for ResponseMethod<'x> {
    fn from(value: SetResponse<crate::object::inbuxa_front_ends::FrontEnds>) -> Self {
        ResponseMethod::Set(SetResponseMethod::FrontEnds(Box::new(value)))
    }
}

impl<'x> From<GetResponse<crate::object::inbuxa_ai_limits::AiLimits>> for ResponseMethod<'x> {
    fn from(value: GetResponse<crate::object::inbuxa_ai_limits::AiLimits>) -> Self {
        ResponseMethod::Get(GetResponseMethod::AiLimits(value))
    }
}

impl<'x> From<GetResponse<crate::object::inbuxa_log_settings::LogSettings>> for ResponseMethod<'x> {
    fn from(value: GetResponse<crate::object::inbuxa_log_settings::LogSettings>) -> Self {
        ResponseMethod::Get(GetResponseMethod::LogSettings(value))
    }
}

impl<'x> From<GetResponse<crate::object::inbuxa_dlp_settings::DlpSettings>> for ResponseMethod<'x> {
    fn from(value: GetResponse<crate::object::inbuxa_dlp_settings::DlpSettings>) -> Self {
        ResponseMethod::Get(GetResponseMethod::DlpSettings(value))
    }
}

impl<'x> From<GetResponse<crate::object::inbuxa_data_inventory::DataInventory>>
    for ResponseMethod<'x>
{
    fn from(value: GetResponse<crate::object::inbuxa_data_inventory::DataInventory>) -> Self {
        ResponseMethod::Get(GetResponseMethod::DataInventory(value))
    }
}

impl<'x> From<GetResponse<crate::object::inbuxa_inventory_snapshot::InventorySnapshot>>
    for ResponseMethod<'x>
{
    fn from(
        value: GetResponse<crate::object::inbuxa_inventory_snapshot::InventorySnapshot>,
    ) -> Self {
        ResponseMethod::Get(GetResponseMethod::InventorySnapshot(value))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_ai_limits::AiLimits>> for ResponseMethod<'x> {
    fn from(value: SetResponse<crate::object::inbuxa_ai_limits::AiLimits>) -> Self {
        ResponseMethod::Set(SetResponseMethod::AiLimits(Box::new(value)))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_log_settings::LogSettings>> for ResponseMethod<'x> {
    fn from(value: SetResponse<crate::object::inbuxa_log_settings::LogSettings>) -> Self {
        ResponseMethod::Set(SetResponseMethod::LogSettings(Box::new(value)))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_dlp_settings::DlpSettings>> for ResponseMethod<'x> {
    fn from(value: SetResponse<crate::object::inbuxa_dlp_settings::DlpSettings>) -> Self {
        ResponseMethod::Set(SetResponseMethod::DlpSettings(Box::new(value)))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_explanation::Explanation>> for ResponseMethod<'x> {
    fn from(value: SetResponse<crate::object::inbuxa_explanation::Explanation>) -> Self {
        ResponseMethod::Set(SetResponseMethod::Explanation(Box::new(value)))
    }
}

// inbuxa: deleted accounts (UD-17)
impl<'x> From<GetResponse<crate::object::inbuxa_deleted_account::DeletedAccount>>
    for ResponseMethod<'x>
{
    fn from(value: GetResponse<crate::object::inbuxa_deleted_account::DeletedAccount>) -> Self {
        ResponseMethod::Get(GetResponseMethod::DeletedAccount(value))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_deleted_account::DeletedAccount>>
    for ResponseMethod<'x>
{
    fn from(value: SetResponse<crate::object::inbuxa_deleted_account::DeletedAccount>) -> Self {
        ResponseMethod::Set(SetResponseMethod::DeletedAccount(Box::new(value)))
    }
}

impl<'x> From<GetResponse<VacationResponse>> for ResponseMethod<'x> {
    fn from(value: GetResponse<VacationResponse>) -> Self {
        ResponseMethod::Get(GetResponseMethod::VacationResponse(value))
    }
}

impl<'x> From<GetResponse<Principal>> for ResponseMethod<'x> {
    fn from(value: GetResponse<Principal>) -> Self {
        ResponseMethod::Get(GetResponseMethod::Principal(value))
    }
}

impl<'x> From<GetResponse<Quota>> for ResponseMethod<'x> {
    fn from(value: GetResponse<Quota>) -> Self {
        ResponseMethod::Get(GetResponseMethod::Quota(value))
    }
}

impl<'x> From<GetResponse<Blob>> for ResponseMethod<'x> {
    fn from(value: GetResponse<Blob>) -> Self {
        ResponseMethod::Get(GetResponseMethod::Blob(value))
    }
}

impl<'x> From<GetResponse<Registry>> for ResponseMethod<'x> {
    fn from(value: GetResponse<Registry>) -> Self {
        ResponseMethod::Get(GetResponseMethod::Registry(value))
    }
}

impl<'x> From<SetResponse<Email>> for ResponseMethod<'x> {
    fn from(value: SetResponse<Email>) -> Self {
        ResponseMethod::Set(SetResponseMethod::Email(Box::new(value)))
    }
}

impl<'x> From<SetResponse<Mailbox>> for ResponseMethod<'x> {
    fn from(value: SetResponse<Mailbox>) -> Self {
        ResponseMethod::Set(SetResponseMethod::Mailbox(Box::new(value)))
    }
}

impl<'x> From<SetResponse<Identity>> for ResponseMethod<'x> {
    fn from(value: SetResponse<Identity>) -> Self {
        ResponseMethod::Set(SetResponseMethod::Identity(Box::new(value)))
    }
}

impl<'x> From<SetResponse<EmailSubmission>> for ResponseMethod<'x> {
    fn from(value: SetResponse<EmailSubmission>) -> Self {
        ResponseMethod::Set(SetResponseMethod::EmailSubmission(Box::new(value)))
    }
}

impl<'x> From<SetResponse<PushSubscription>> for ResponseMethod<'x> {
    fn from(value: SetResponse<PushSubscription>) -> Self {
        ResponseMethod::Set(SetResponseMethod::PushSubscription(Box::new(value)))
    }
}

impl<'x> From<SetResponse<Sieve>> for ResponseMethod<'x> {
    fn from(value: SetResponse<Sieve>) -> Self {
        ResponseMethod::Set(SetResponseMethod::Sieve(Box::new(value)))
    }
}

impl<'x> From<SetResponse<VacationResponse>> for ResponseMethod<'x> {
    fn from(value: SetResponse<VacationResponse>) -> Self {
        ResponseMethod::Set(SetResponseMethod::VacationResponse(Box::new(value)))
    }
}

impl<'x> From<SetResponse<Registry>> for ResponseMethod<'x> {
    fn from(value: SetResponse<Registry>) -> Self {
        ResponseMethod::Set(SetResponseMethod::Registry(Box::new(value)))
    }
}

impl<'x> From<ChangesResponse<Email>> for ResponseMethod<'x> {
    fn from(value: ChangesResponse<Email>) -> Self {
        ResponseMethod::Changes(ChangesResponseMethod::Email(Box::new(value)))
    }
}

impl<'x> From<ChangesResponse<Mailbox>> for ResponseMethod<'x> {
    fn from(value: ChangesResponse<Mailbox>) -> Self {
        ResponseMethod::Changes(ChangesResponseMethod::Mailbox(Box::new(value)))
    }
}

impl<'x> From<ChangesResponse<Thread>> for ResponseMethod<'x> {
    fn from(value: ChangesResponse<Thread>) -> Self {
        ResponseMethod::Changes(ChangesResponseMethod::Thread(Box::new(value)))
    }
}

impl<'x> From<ChangesResponse<Identity>> for ResponseMethod<'x> {
    fn from(value: ChangesResponse<Identity>) -> Self {
        ResponseMethod::Changes(ChangesResponseMethod::Identity(Box::new(value)))
    }
}

impl<'x> From<ChangesResponse<EmailSubmission>> for ResponseMethod<'x> {
    fn from(value: ChangesResponse<EmailSubmission>) -> Self {
        ResponseMethod::Changes(ChangesResponseMethod::EmailSubmission(Box::new(value)))
    }
}

impl<'x> From<ChangesResponse<Quota>> for ResponseMethod<'x> {
    fn from(value: ChangesResponse<Quota>) -> Self {
        ResponseMethod::Changes(ChangesResponseMethod::Quota(Box::new(value)))
    }
}

impl<'x> From<CopyResponse<Email>> for ResponseMethod<'x> {
    fn from(value: CopyResponse<Email>) -> Self {
        ResponseMethod::Copy(CopyResponseMethod::Email(value))
    }
}

impl<'x> From<CopyBlobResponse> for ResponseMethod<'x> {
    fn from(value: CopyBlobResponse) -> Self {
        ResponseMethod::Copy(CopyResponseMethod::Blob(value))
    }
}

impl<'x> From<ImportEmailResponse> for ResponseMethod<'x> {
    fn from(value: ImportEmailResponse) -> Self {
        ResponseMethod::ImportEmail(value)
    }
}

impl<'x> From<ParseResponse<Email>> for ResponseMethod<'x> {
    fn from(value: ParseResponse<Email>) -> Self {
        ResponseMethod::Parse(ParseResponseMethod::Email(value))
    }
}

impl<'x> From<QueryChangesResponse> for ResponseMethod<'x> {
    fn from(value: QueryChangesResponse) -> Self {
        ResponseMethod::QueryChanges(value)
    }
}

impl<'x> From<QueryResponse> for ResponseMethod<'x> {
    fn from(value: QueryResponse) -> Self {
        ResponseMethod::Query(value)
    }
}

impl<'x> From<GetSearchSnippetResponse> for ResponseMethod<'x> {
    fn from(value: GetSearchSnippetResponse) -> Self {
        ResponseMethod::SearchSnippet(value)
    }
}

impl<'x> From<ValidateSieveScriptResponse> for ResponseMethod<'x> {
    fn from(value: ValidateSieveScriptResponse) -> Self {
        ResponseMethod::ValidateScript(value)
    }
}

impl<'x> From<BlobLookupResponse> for ResponseMethod<'x> {
    fn from(value: BlobLookupResponse) -> Self {
        ResponseMethod::LookupBlob(value)
    }
}

impl<'x> From<BlobUploadResponse> for ResponseMethod<'x> {
    fn from(value: BlobUploadResponse) -> Self {
        ResponseMethod::UploadBlob(value)
    }
}

impl<'x> From<Value<'x, Null, Null>> for ResponseMethod<'x> {
    fn from(value: Value<'x, Null, Null>) -> Self {
        ResponseMethod::Echo(value)
    }
}

impl<'x> From<MethodErrorWrapper> for ResponseMethod<'x> {
    fn from(value: MethodErrorWrapper) -> Self {
        ResponseMethod::Error(value)
    }
}

impl From<ChangesResponse<ShareNotification>> for ResponseMethod<'_> {
    fn from(response: ChangesResponse<ShareNotification>) -> Self {
        ResponseMethod::Changes(ChangesResponseMethod::ShareNotification(Box::new(response)))
    }
}

impl From<SetResponse<ShareNotification>> for ResponseMethod<'_> {
    fn from(response: SetResponse<ShareNotification>) -> Self {
        ResponseMethod::Set(SetResponseMethod::ShareNotification(Box::new(response)))
    }
}

impl From<GetResponse<ShareNotification>> for ResponseMethod<'_> {
    fn from(response: GetResponse<ShareNotification>) -> Self {
        ResponseMethod::Get(GetResponseMethod::ShareNotification(response))
    }
}

// inbuxa: the audit log
impl<'x> From<GetResponse<crate::object::inbuxa_audit::AuditEvent>> for ResponseMethod<'x> {
    fn from(value: GetResponse<crate::object::inbuxa_audit::AuditEvent>) -> Self {
        ResponseMethod::Get(GetResponseMethod::AuditEvent(value))
    }
}

impl<'x> From<GetResponse<crate::object::inbuxa_audit::AuditSettings>> for ResponseMethod<'x> {
    fn from(value: GetResponse<crate::object::inbuxa_audit::AuditSettings>) -> Self {
        ResponseMethod::Get(GetResponseMethod::AuditSettings(value))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_audit::AuditSettings>> for ResponseMethod<'x> {
    fn from(value: SetResponse<crate::object::inbuxa_audit::AuditSettings>) -> Self {
        ResponseMethod::Set(SetResponseMethod::AuditSettings(Box::new(value)))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_audit::AuditExport>> for ResponseMethod<'x> {
    fn from(value: SetResponse<crate::object::inbuxa_audit::AuditExport>) -> Self {
        ResponseMethod::Set(SetResponseMethod::AuditExport(Box::new(value)))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_audit::AuditVerification>> for ResponseMethod<'x> {
    fn from(value: SetResponse<crate::object::inbuxa_audit::AuditVerification>) -> Self {
        ResponseMethod::Set(SetResponseMethod::AuditVerification(Box::new(value)))
    }
}

// inbuxa: account lock with delegation
impl<'x> From<GetResponse<crate::object::inbuxa_account_lock::AccountLock>> for ResponseMethod<'x> {
    fn from(value: GetResponse<crate::object::inbuxa_account_lock::AccountLock>) -> Self {
        ResponseMethod::Get(GetResponseMethod::AccountLock(value))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_account_lock::AccountLock>> for ResponseMethod<'x> {
    fn from(value: SetResponse<crate::object::inbuxa_account_lock::AccountLock>) -> Self {
        ResponseMethod::Set(SetResponseMethod::AccountLock(Box::new(value)))
    }
}

// inbuxa: legal hold
impl<'x> From<GetResponse<crate::object::inbuxa_held_message::HeldMessage>> for ResponseMethod<'x> {
    fn from(value: GetResponse<crate::object::inbuxa_held_message::HeldMessage>) -> Self {
        ResponseMethod::Get(GetResponseMethod::HeldMessage(value))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_held_message::HeldMessage>> for ResponseMethod<'x> {
    fn from(value: SetResponse<crate::object::inbuxa_held_message::HeldMessage>) -> Self {
        ResponseMethod::Set(SetResponseMethod::HeldMessage(Box::new(value)))
    }
}

// inbuxa: the deliverability check
impl<'x> From<GetResponse<crate::object::inbuxa_deliverability_report::DeliverabilityReport>>
    for ResponseMethod<'x>
{
    fn from(
        value: GetResponse<crate::object::inbuxa_deliverability_report::DeliverabilityReport>,
    ) -> Self {
        ResponseMethod::Get(GetResponseMethod::DeliverabilityReport(value))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_deliverability_report::DeliverabilityReport>>
    for ResponseMethod<'x>
{
    fn from(
        value: SetResponse<crate::object::inbuxa_deliverability_report::DeliverabilityReport>,
    ) -> Self {
        ResponseMethod::Set(SetResponseMethod::DeliverabilityReport(Box::new(value)))
    }
}

impl<'x> From<GetResponse<crate::object::inbuxa_deliverability_settings::DeliverabilitySettings>>
    for ResponseMethod<'x>
{
    fn from(
        value: GetResponse<crate::object::inbuxa_deliverability_settings::DeliverabilitySettings>,
    ) -> Self {
        ResponseMethod::Get(GetResponseMethod::DeliverabilitySettings(value))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_deliverability_settings::DeliverabilitySettings>>
    for ResponseMethod<'x>
{
    fn from(
        value: SetResponse<crate::object::inbuxa_deliverability_settings::DeliverabilitySettings>,
    ) -> Self {
        ResponseMethod::Set(SetResponseMethod::DeliverabilitySettings(Box::new(value)))
    }
}
impl<'x> From<GetResponse<crate::object::inbuxa_report_export::ReportExport>>
    for ResponseMethod<'x>
{
    fn from(value: GetResponse<crate::object::inbuxa_report_export::ReportExport>) -> Self {
        ResponseMethod::Get(GetResponseMethod::ReportExport(value))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_report_export::ReportExport>>
    for ResponseMethod<'x>
{
    fn from(value: SetResponse<crate::object::inbuxa_report_export::ReportExport>) -> Self {
        ResponseMethod::Set(SetResponseMethod::ReportExport(Box::new(value)))
    }
}
impl<'x> From<GetResponse<crate::object::inbuxa_scheduled_report::ScheduledReport>>
    for ResponseMethod<'x>
{
    fn from(value: GetResponse<crate::object::inbuxa_scheduled_report::ScheduledReport>) -> Self {
        ResponseMethod::Get(GetResponseMethod::ScheduledReport(value))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_scheduled_report::ScheduledReport>>
    for ResponseMethod<'x>
{
    fn from(value: SetResponse<crate::object::inbuxa_scheduled_report::ScheduledReport>) -> Self {
        ResponseMethod::Set(SetResponseMethod::ScheduledReport(Box::new(value)))
    }
}
impl<'x> From<GetResponse<crate::object::inbuxa_scheduled_report_settings::ScheduledReportSettings>>
    for ResponseMethod<'x>
{
    fn from(
        value: GetResponse<
            crate::object::inbuxa_scheduled_report_settings::ScheduledReportSettings,
        >,
    ) -> Self {
        ResponseMethod::Get(GetResponseMethod::ScheduledReportSettings(value))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_scheduled_report_settings::ScheduledReportSettings>>
    for ResponseMethod<'x>
{
    fn from(
        value: SetResponse<
            crate::object::inbuxa_scheduled_report_settings::ScheduledReportSettings,
        >,
    ) -> Self {
        ResponseMethod::Set(SetResponseMethod::ScheduledReportSettings(Box::new(value)))
    }
}

// inbuxa: accepted security to-do items
impl<'x> From<GetResponse<crate::object::inbuxa_security_acceptance::SecurityAcceptance>>
    for ResponseMethod<'x>
{
    fn from(
        value: GetResponse<crate::object::inbuxa_security_acceptance::SecurityAcceptance>,
    ) -> Self {
        ResponseMethod::Get(GetResponseMethod::SecurityAcceptance(value))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_security_acceptance::SecurityAcceptance>>
    for ResponseMethod<'x>
{
    fn from(
        value: SetResponse<crate::object::inbuxa_security_acceptance::SecurityAcceptance>,
    ) -> Self {
        ResponseMethod::Set(SetResponseMethod::SecurityAcceptance(Box::new(value)))
    }
}

impl<'x> From<GetResponse<crate::object::inbuxa_mail_rule::MailRule>> for ResponseMethod<'x> {
    fn from(value: GetResponse<crate::object::inbuxa_mail_rule::MailRule>) -> Self {
        ResponseMethod::Get(GetResponseMethod::MailRule(value))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_mail_rule::MailRule>> for ResponseMethod<'x> {
    fn from(value: SetResponse<crate::object::inbuxa_mail_rule::MailRule>) -> Self {
        ResponseMethod::Set(SetResponseMethod::MailRule(Box::new(value)))
    }
}

// inbuxa: migrations into this server
impl<'x> From<GetResponse<crate::object::inbuxa_migration::Migration>> for ResponseMethod<'x> {
    fn from(value: GetResponse<crate::object::inbuxa_migration::Migration>) -> Self {
        ResponseMethod::Get(GetResponseMethod::Migration(value))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_migration::Migration>> for ResponseMethod<'x> {
    fn from(value: SetResponse<crate::object::inbuxa_migration::Migration>) -> Self {
        ResponseMethod::Set(SetResponseMethod::Migration(Box::new(value)))
    }
}

// inbuxa: checking the store's blobs
impl<'x> From<GetResponse<crate::object::inbuxa_store_check::StoreCheck>> for ResponseMethod<'x> {
    fn from(value: GetResponse<crate::object::inbuxa_store_check::StoreCheck>) -> Self {
        ResponseMethod::Get(GetResponseMethod::StoreCheck(value))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_store_check::StoreCheck>> for ResponseMethod<'x> {
    fn from(value: SetResponse<crate::object::inbuxa_store_check::StoreCheck>) -> Self {
        ResponseMethod::Set(SetResponseMethod::StoreCheck(Box::new(value)))
    }
}

// inbuxa: a snapshot of the store
impl<'x> From<GetResponse<crate::object::inbuxa_store_snapshot::StoreSnapshot>>
    for ResponseMethod<'x>
{
    fn from(value: GetResponse<crate::object::inbuxa_store_snapshot::StoreSnapshot>) -> Self {
        ResponseMethod::Get(GetResponseMethod::StoreSnapshot(value))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_store_snapshot::StoreSnapshot>>
    for ResponseMethod<'x>
{
    fn from(value: SetResponse<crate::object::inbuxa_store_snapshot::StoreSnapshot>) -> Self {
        ResponseMethod::Set(SetResponseMethod::StoreSnapshot(Box::new(value)))
    }
}

// inbuxa: a change of blob store
impl<'x> From<GetResponse<crate::object::inbuxa_blob_store_change::BlobStoreChange>>
    for ResponseMethod<'x>
{
    fn from(value: GetResponse<crate::object::inbuxa_blob_store_change::BlobStoreChange>) -> Self {
        ResponseMethod::Get(GetResponseMethod::BlobStoreChange(value))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_blob_store_change::BlobStoreChange>>
    for ResponseMethod<'x>
{
    fn from(value: SetResponse<crate::object::inbuxa_blob_store_change::BlobStoreChange>) -> Self {
        ResponseMethod::Set(SetResponseMethod::BlobStoreChange(Box::new(value)))
    }
}

impl<'x> From<GetResponse<crate::object::inbuxa_relay::Relay>> for ResponseMethod<'x> {
    fn from(value: GetResponse<crate::object::inbuxa_relay::Relay>) -> Self {
        ResponseMethod::Get(GetResponseMethod::Relay(value))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_relay::Relay>> for ResponseMethod<'x> {
    fn from(value: SetResponse<crate::object::inbuxa_relay::Relay>) -> Self {
        ResponseMethod::Set(SetResponseMethod::Relay(Box::new(value)))
    }
}

impl<'x> From<GetResponse<crate::object::inbuxa_relay_message::RelayMessage>>
    for ResponseMethod<'x>
{
    fn from(value: GetResponse<crate::object::inbuxa_relay_message::RelayMessage>) -> Self {
        ResponseMethod::Get(GetResponseMethod::RelayMessage(value))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_relay_message::RelayMessage>>
    for ResponseMethod<'x>
{
    fn from(value: SetResponse<crate::object::inbuxa_relay_message::RelayMessage>) -> Self {
        ResponseMethod::Set(SetResponseMethod::RelayMessage(Box::new(value)))
    }
}

// inbuxa: journaling
impl<'x> From<GetResponse<crate::object::inbuxa_journal_entry::JournalEntry>>
    for ResponseMethod<'x>
{
    fn from(value: GetResponse<crate::object::inbuxa_journal_entry::JournalEntry>) -> Self {
        ResponseMethod::Get(GetResponseMethod::JournalEntry(value))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_journal_entry::JournalExport>>
    for ResponseMethod<'x>
{
    fn from(value: SetResponse<crate::object::inbuxa_journal_entry::JournalExport>) -> Self {
        ResponseMethod::Set(SetResponseMethod::JournalExport(Box::new(value)))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_journal_entry::JournalVerification>>
    for ResponseMethod<'x>
{
    fn from(value: SetResponse<crate::object::inbuxa_journal_entry::JournalVerification>) -> Self {
        ResponseMethod::Set(SetResponseMethod::JournalVerification(Box::new(value)))
    }
}

impl<'x> From<GetResponse<crate::object::inbuxa_journal::Journal>> for ResponseMethod<'x> {
    fn from(value: GetResponse<crate::object::inbuxa_journal::Journal>) -> Self {
        ResponseMethod::Get(GetResponseMethod::Journal(value))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_journal::Journal>> for ResponseMethod<'x> {
    fn from(value: SetResponse<crate::object::inbuxa_journal::Journal>) -> Self {
        ResponseMethod::Set(SetResponseMethod::Journal(Box::new(value)))
    }
}

impl<'x> From<GetResponse<crate::object::inbuxa_legal_hold::LegalHold>> for ResponseMethod<'x> {
    fn from(value: GetResponse<crate::object::inbuxa_legal_hold::LegalHold>) -> Self {
        ResponseMethod::Get(GetResponseMethod::LegalHold(value))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_legal_hold::LegalHold>> for ResponseMethod<'x> {
    fn from(value: SetResponse<crate::object::inbuxa_legal_hold::LegalHold>) -> Self {
        ResponseMethod::Set(SetResponseMethod::LegalHold(Box::new(value)))
    }
}

// inbuxa: legal hold exports
impl<'x> From<GetResponse<crate::object::inbuxa_hold_export::HoldExport>> for ResponseMethod<'x> {
    fn from(value: GetResponse<crate::object::inbuxa_hold_export::HoldExport>) -> Self {
        ResponseMethod::Get(GetResponseMethod::HoldExport(value))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_hold_export::HoldExport>> for ResponseMethod<'x> {
    fn from(value: SetResponse<crate::object::inbuxa_hold_export::HoldExport>) -> Self {
        ResponseMethod::Set(SetResponseMethod::HoldExport(Box::new(value)))
    }
}

// inbuxa: passkeys
impl<'x> From<GetResponse<crate::object::inbuxa_passkey::Passkey>> for ResponseMethod<'x> {
    fn from(value: GetResponse<crate::object::inbuxa_passkey::Passkey>) -> Self {
        ResponseMethod::Get(GetResponseMethod::Passkey(value))
    }
}

impl<'x> From<SetResponse<crate::object::inbuxa_passkey::Passkey>> for ResponseMethod<'x> {
    fn from(value: SetResponse<crate::object::inbuxa_passkey::Passkey>) -> Self {
        ResponseMethod::Set(SetResponseMethod::Passkey(Box::new(value)))
    }
}
