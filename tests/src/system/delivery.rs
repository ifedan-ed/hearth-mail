/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 *
 * Modified by Coffey Labs in 2026 for INBUXA.
 */

use crate::utils::{
    account::Account, imap::AssertResult, jmap::JmapUtils, server::TestServer, smtp::SmtpConnection,
};
use common::{Server, auth::BuildAccessToken};
use email::{
    cache::{MessageCacheFetch, email::MessageCacheAccess},
    mailbox::{INBOX_ID, JUNK_ID, SENT_ID},
    message::metadata::MessageMetadata,
};
use jmap::blob::download::BlobDownload;
use jmap_proto::error::set::SetErrorType;
use registry::{
    schema::{
        enums::StorageQuota,
        prelude::{ObjectType, Property},
        structs::{
            EmailAlias, Expression, MailingList, MtaExtensions, SpamTag, SpamTagScore,
            SpamTrainingSample,
        },
    },
    types::{EnumImpl, datetime::UTCDateTime, float::Float, list::List, map::Map},
};
use serde_json::json;
use std::time::Duration;
use store::{
    ValueKey,
    roaring::RoaringBitmap,
    write::{AlignedBytes, Archive, now},
};
use types::{
    blob::{BlobClass, BlobId},
    collection::Collection,
    field::EmailField,
    id::Id,
    keyword::{ITIP_KEYWORD, Keyword},
};
use utils::chained_bytes::ChainedBytes;

pub async fn test(test: &mut TestServer) {
    println!("Running Email delivery tests...");
    let admin = test.account("admin@example.org");

    // Prepare tests
    admin
        .registry_create_object(SpamTag::Score(SpamTagScore {
            score: Float::new(1000.0),
            tag: "GTUBE_TEST".to_string(),
        }))
        .await;
    admin
        .registry_update_setting(
            MtaExtensions {
                expn: Expression {
                    else_: "true".to_string(),
                    ..Default::default()
                },
                vrfy: Expression {
                    else_: "true".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            },
            &[Property::Expn, Property::Vrfy],
        )
        .await;
    admin.reload_settings().await;

    // Create a domain name and a test account
    let john = test
        .create_user_account(
            "admin@example.org",
            "jdoe@example.org",
            "this is a very strong password",
            &["john.doe@example.org"],
            "jdoe@example.org",
        )
        .await;
    let jane = test
        .create_user_account(
            "admin@example.org",
            "jane.smith@example.org",
            "this is a very strong password",
            &[],
            "jane.smith@example.org",
        )
        .await;
    let bill = test
        .create_user_account(
            "admin@example.org",
            "bill@example.org",
            "this is a very strong password",
            &[],
            "bill@example.org",
        )
        .await;
    admin
        .registry_update_object(
            ObjectType::Account,
            john.id(),
            json!({
                Property::Quotas: {StorageQuota::MaxMaskedAddresses.as_str(): 2}
            }),
        )
        .await;

    // Create a mailing list
    let domain_id = admin.find_or_create_domain("example.org").await;
    let list_id = admin
        .registry_create_object(MailingList {
            name: "members".to_string(),
            recipients: Map::new(vec![
                "jdoe@example.org".to_string(),
                "jane.smith@example.org".to_string(),
                "bill@example.org".to_string(),
            ]),
            aliases: List::from_iter([EmailAlias {
                name: "corporate".to_string(),
                domain_id,
                enabled: true,
                ..Default::default()
            }]),
            domain_id,
            ..Default::default()
        })
        .await;

    // Delivering to individuals
    let mut lmtp = SmtpConnection::connect().await;
    lmtp.ingest(
        "bill@example.org",
        &["jdoe@example.org"],
        concat!(
            "From: bill@example.org\r\n",
            "To: jdoe@example.org\r\n",
            "Subject: TPS Report\r\n",
            "\r\n",
            "I'm going to need those TPS reports ASAP. ",
            "So, if you could do that, that'd be great."
        ),
    )
    .await;

    let john_cache = test
        .server
        .get_cached_messages(john.id().document_id())
        .await
        .unwrap();

    assert_eq!(john_cache.emails.items.len(), 1);
    assert_eq!(john_cache.in_mailbox(INBOX_ID).count(), 1);
    assert_eq!(john_cache.in_mailbox(JUNK_ID).count(), 0);

    // Make sure there are no spam training samples
    admin
        .registry_destroy_all(ObjectType::SpamTrainingSample)
        .await;
    assert!(
        admin
            .registry_query(
                ObjectType::SpamTrainingSample,
                Vec::<(&str, &str)>::new(),
                Vec::<&str>::new(),
            )
            .await
            .ids()
            .next()
            .is_none()
    );

    // Masked email tests
    john.registry_create_many(
        ObjectType::MaskedEmail,
        [json!({
            Property::EmailDomain: "invalid.org"
        })],
    )
    .await
    .not_created(0)
    .to_set_error()
    .assert_type(SetErrorType::Forbidden)
    .assert_description_contains("The specified domain is not valid for this account.");

    let response = john
        .registry_create_many(
            ObjectType::MaskedEmail,
            [json!({
                Property::EmailDomain: "example.org",
                Property::EmailPrefix: "secretive",
                Property::ExpiresAt: UTCDateTime::from_timestamp((now() + 1) as i64)
            })],
        )
        .await;
    let masked = response.created(0);
    let masked_prefix_id = masked.object_id();
    let masked_prefix_email = masked.text_field("email").to_string();
    assert!(
        masked_prefix_email.starts_with("secretive")
            && masked_prefix_email.ends_with("@example.org"),
        "Unexpected masked email: {masked_prefix_email}"
    );

    let response = john
        .registry_create_many(
            ObjectType::MaskedEmail,
            [json!({
                Property::EmailDomain: "example.org",
            })],
        )
        .await;
    let masked = response.created(0);
    let masked_random_id = masked.object_id();
    let masked_random_email = masked.text_field("email").to_string();
    // inbuxa: ME-13: the fork's addresses never contain a '.' in the local
    // part, so they can't be mistaken for upstream's
    assert!(
        !masked_random_email.split('@').next().unwrap().contains('.')
            && masked_random_email.ends_with("@example.org"),
        "Unexpected masked email: {masked_random_email}"
    );

    john.registry_create_many(
        ObjectType::MaskedEmail,
        [json!({
            Property::EmailDomain: "example.org",
        })],
    )
    .await
    .not_created(0)
    .to_set_error()
    .assert_type(SetErrorType::OverQuota);

    // Test spam filtering using masked email
    lmtp.ingest(
        "bill@example.org",
        &[masked_prefix_email.as_str()],
        concat!(
            "From: bill@example.org\r\n",
            "To: john.doe@example.org\r\n",
            "Subject: XJS*C4JDBQADN1.NSBN3*2IDNEN*GTUBE-STANDARD-ANTI-UBE-TEST-EMAIL*C.34X\r\n",
            "\r\n",
            "--- Forwarded Message ---\r\n\r\n ",
            "I'm going to need those TPS reports ASAP. ",
            "So, if you could do that, that'd be great."
        ),
    )
    .await;
    let john_cache = test
        .server
        .get_cached_messages(john.id().document_id())
        .await
        .unwrap();
    let inbox_ids = john_cache
        .in_mailbox(INBOX_ID)
        .map(|e| e.document_id)
        .collect::<RoaringBitmap>();
    let junk_ids = john_cache
        .in_mailbox(JUNK_ID)
        .map(|e| e.document_id)
        .collect::<RoaringBitmap>();
    assert_eq!(john_cache.emails.items.len(), 2);
    assert_eq!(inbox_ids.len(), 1);
    assert_eq!(junk_ids.len(), 1);
    assert_message_headers_contains(
        &test.server,
        john.id().document_id(),
        junk_ids.min().unwrap(),
        "X-Spam-Status: Yes",
    )
    .await;
    assert_eq!(john.spam_training_samples().await, vec![]);

    // Hearth Mail: the contacts spam override, using masked email. The
    // contacts live in Nextcloud or Hearth and are asked over HTTP
    // (common::contacts_lookup); here a stand-in answers "known" for
    // dmarc-bill@example.org only, and the server is switched to it.
    // The lookup is asked with John's own address (jdoe@), not the masked
    // one the mail reached
    let lookup_url = contacts_lookup_stub("jdoe@example.org", "dmarc-bill@example.org").await;
    let mut core = (*test.server.inner.shared_core.load_full()).clone();
    core.spam.contacts_lookup = Some(std::sync::Arc::new(
        common::contacts_lookup::ContactsLookup::new(lookup_url, "stub-secret".into()).unwrap(),
    ));
    test.server
        .inner
        .shared_core
        .store(std::sync::Arc::new(core));
    lmtp.ingest(
        "dmarc-bill@example.org",
        &[masked_random_email.as_str()],
        concat!(
            "From: dmarc-bill@example.org\r\n",
            "To: john.doe@example.org\r\n",
            "Subject: XJS*C4JDBQADN1.NSBN3*2IDNEN*GTUBE-STANDARD-ANTI-UBE-TEST-EMAIL*C.34X\r\n",
            "\r\n",
            "--- Forwarded Message ---\r\n\r\n ",
            "I'm going to need those TPS reports ASAP. ",
            "So, if you could do that, that'd be great."
        ),
    )
    .await;
    let john_cache = test
        .server
        .get_cached_messages(john.id().document_id())
        .await
        .unwrap();
    let inbox_ids = john_cache
        .in_mailbox(INBOX_ID)
        .map(|e| e.document_id)
        .collect::<RoaringBitmap>();
    let junk_ids = john_cache
        .in_mailbox(JUNK_ID)
        .map(|e| e.document_id)
        .collect::<RoaringBitmap>();
    assert_eq!(john_cache.emails.items.len(), 3);
    assert_eq!(inbox_ids.len(), 2);
    assert_eq!(junk_ids.len(), 1);
    assert_message_headers_contains(
        &test.server,
        john.id().document_id(),
        inbox_ids.max().unwrap(),
        "X-Spam-Status: No, reason=card-exists",
    )
    .await;
    let samples = john.spam_training_samples().await;
    assert_eq!(samples.iter().filter(|x| !x.1.is_spam).count(), 1);
    assert_eq!(samples.iter().filter(|x| x.1.is_spam).count(), 0);
    // The lookup off again, so the next override is what decides
    let mut core = (*test.server.inner.shared_core.load_full()).clone();
    core.spam.contacts_lookup = None;
    test.server
        .inner
        .shared_core
        .store(std::sync::Arc::new(core));

    // Test trusted reply override
    john.jmap_client()
        .await
        .email_import(
            concat!(
                "From: john.doe@example.org\r\n",
                "To: dmarc-bill@example.org\r\n",
                "Message-ID: <trusted@message-id.example.org>\r\n",
                "Subject: XJS*C4JDBQADN1.NSBN3*2IDNEN*GTUBE-STANDARD-ANTI-UBE-TEST-EMAIL*C.34X\r\n",
                "\r\n",
                "This is a trusted reply."
            )
            .as_bytes()
            .to_vec(),
            vec![Id::from(SENT_ID).to_string()],
            None::<Vec<String>>,
            None,
        )
        .await
        .unwrap()
        .take_id();
    assert_eq!(
        test.server
            .get_cached_messages(john.id().document_id())
            .await
            .unwrap()
            .emails
            .items
            .len(),
        4
    );
    lmtp.ingest(
        "dmarc-bill@example.org",
        &["john.doe@example.org"],
        concat!(
            "From: dmarc-bill@example.org\r\n",
            "To: john.doe@example.org\r\n",
            "Message-ID: <other@message-id.example.org>\r\n",
            "References: <trusted@message-id.example.org>\r\n",
            "Subject: XJS*C4JDBQADN1.NSBN3*2IDNEN*GTUBE-STANDARD-ANTI-UBE-TEST-EMAIL*C.34X\r\n",
            "\r\n",
            "--- Forwarded Message ---\r\n\r\n ",
            "I'm going to need those TPS reports ASAP. ",
            "So, if you could do that, that'd be great."
        ),
    )
    .await;
    let john_cache = test
        .server
        .get_cached_messages(john.id().document_id())
        .await
        .unwrap();
    let inbox_ids = john_cache
        .in_mailbox(INBOX_ID)
        .map(|e| e.document_id)
        .collect::<RoaringBitmap>();
    let junk_ids = john_cache
        .in_mailbox(JUNK_ID)
        .map(|e| e.document_id)
        .collect::<RoaringBitmap>();
    assert_eq!(john_cache.emails.items.len(), 5);
    assert_eq!(inbox_ids.len(), 3);
    assert_eq!(junk_ids.len(), 1);
    assert_message_headers_contains(
        &test.server,
        john.id().document_id(),
        inbox_ids.max().unwrap(),
        "X-Spam-Status: No, reason=trusted-reply",
    )
    .await;
    let samples = john.spam_training_samples().await;
    assert_eq!(samples.iter().filter(|x| !x.1.is_spam).count(), 2);
    assert_eq!(samples.iter().filter(|x| x.1.is_spam).count(), 0);

    // EXPN and VRFY
    lmtp.expn("members@example.org", 2)
        .await
        .assert_contains("jdoe@example.org")
        .assert_contains("jane.smith@example.org")
        .assert_contains("bill@example.org");
    lmtp.expn("non_existant@example.org", 5).await;
    lmtp.expn("jdoe@example.org", 5).await;
    lmtp.vrfy("jdoe@example.org", 2).await;
    lmtp.vrfy("members@example.org", 5).await;
    lmtp.vrfy("non_existant@example.org", 5).await;
    lmtp.vrfy(masked_random_email.as_str(), 2).await;
    lmtp.vrfy(masked_prefix_email.as_str(), 5).await; // Should have expired

    // Delivering to a mailing list
    lmtp.ingest(
        "bill@example.org",
        &["members@example.org"],
        concat!(
            "From: bill@example.org\r\n",
            "To: members@example.org\r\n",
            "Subject: WFH policy\r\n",
            "\r\n",
            "We need the entire staff back in the office, ",
            "TPS reports cannot be filed properly from home."
        ),
    )
    .await;

    tokio::time::sleep(Duration::from_millis(200)).await;

    for (account, num_messages) in [(&john, 6), (&jane, 1), (&bill, 1)] {
        assert_eq!(
            test.server
                .get_cached_messages(account.id().document_id())
                .await
                .unwrap()
                .emails
                .items
                .len(),
            num_messages,
            "for {}",
            account.id_string()
        );
    }

    // Removing members from the mailing list and chunked ingest
    admin
        .registry_update_object(
            ObjectType::MailingList,
            list_id,
            json!({
                "recipients/jdoe@example.org": false
            }),
        )
        .await;
    lmtp.ingest_chunked(
        "bill@example.org",
        &["members@example.org"],
        concat!(
            "From: bill@example.org\r\n",
            "To: members@example.org\r\n",
            "Subject: WFH policy (reminder)\r\n",
            "\r\n",
            "This is a reminder that we need the entire staff back in the office, ",
            "TPS reports cannot be filed properly from home."
        ),
        10,
    )
    .await;

    for (account, num_messages) in [(&john, 6), (&jane, 2), (&bill, 2)] {
        assert_eq!(
            test.server
                .get_cached_messages(account.id().document_id())
                .await
                .unwrap()
                .emails
                .items
                .len(),
            num_messages,
            "for {}",
            account.id_string()
        );
    }

    // Deduplication of recipients
    lmtp.ingest(
        "bill@example.org",
        &[
            "members@example.org",
            "jdoe@example.org",
            "john.doe@example.org",
            "jane.smith@example.org",
            "bill@example.org",
        ],
        concat!(
            "From: bill@example.org\r\n",
            "Bcc: Undisclosed recipients;\r\n",
            "Subject: Holidays\r\n",
            "\r\n",
            "Remember to file your TPS reports before ",
            "going on holidays."
        ),
    )
    .await;

    // Make sure blobs are properly linked
    test.blob_expire_all().await;

    for (account, num_messages) in [(&john, 7), (&jane, 3), (&bill, 3)] {
        let account_id = account.id().document_id();
        let cache = test.server.get_cached_messages(account_id).await.unwrap();
        assert_eq!(
            cache.emails.items.len(),
            num_messages,
            "for {}",
            account.id_string()
        );
        let access_token = test.server.access_token(account_id).await.unwrap().build();

        for document_id in cache.in_mailbox(INBOX_ID).map(|e| e.document_id) {
            let metadata = message_metadata(&test.server, account_id, document_id).await;
            let partial_message = test
                .server
                .blob_store()
                .get_blob(metadata.blob_hash.0.as_ref(), 0..usize::MAX)
                .await
                .unwrap()
                .unwrap();

            assert_ne!(metadata.blob_body_offset, 0);
            let expected_full_message = String::from_utf8(
                ChainedBytes::new(metadata.raw_headers.as_ref())
                    .with_last(
                        partial_message
                            .get(metadata.blob_body_offset as usize..)
                            .unwrap_or_default(),
                    )
                    .to_bytes(),
            )
            .unwrap();
            assert!(
                expected_full_message.contains("Delivered-To:")
                    && expected_full_message.contains("Subject:"),
                "for {account_id}: {expected_full_message}"
            );
            let full_message = String::from_utf8(
                test.server
                    .blob_download(
                        &BlobId {
                            hash: metadata.blob_hash,
                            class: BlobClass::Linked {
                                account_id,
                                collection: Collection::Email.into(),
                                document_id,
                            },
                            section: None,
                        },
                        &access_token,
                    )
                    .await
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(full_message, expected_full_message, "for {account_id}");
        }
    }

    // Sub-addressed local members must resolve when the list is expanded
    admin
        .registry_create_object(MailingList {
            name: "tps-reports".to_string(),
            recipients: Map::new(vec!["bill+tps@example.org".to_string()]),
            domain_id,
            ..Default::default()
        })
        .await;

    let bill_messages = inbox_count(&test.server, &bill).await;

    lmtp.ingest(
        "john.doe@example.org",
        &["tps-reports@example.org"],
        concat!(
            "From: john.doe@example.org\r\n",
            "To: tps-reports@example.org\r\n",
            "Subject: Cover sheet\r\n",
            "\r\n",
            "Did you get the memo about the new cover sheet?"
        ),
    )
    .await;

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(
        inbox_count(&test.server, &bill).await,
        bill_messages + 1,
        "sub-addressed mailing list member was not delivered"
    );

    // Lists nested within lists must be expanded recursively
    admin
        .registry_create_object(MailingList {
            name: "engineering".to_string(),
            recipients: Map::new(vec!["jane.smith@example.org".to_string()]),
            domain_id,
            ..Default::default()
        })
        .await;
    admin
        .registry_create_object(MailingList {
            name: "all-staff".to_string(),
            recipients: Map::new(vec![
                "engineering@example.org".to_string(),
                "bill@example.org".to_string(),
            ]),
            domain_id,
            ..Default::default()
        })
        .await;

    let jane_messages = inbox_count(&test.server, &jane).await;
    let bill_messages = inbox_count(&test.server, &bill).await;

    lmtp.ingest(
        "john.doe@example.org",
        &["all-staff@example.org"],
        concat!(
            "From: john.doe@example.org\r\n",
            "To: all-staff@example.org\r\n",
            "Subject: Company picnic\r\n",
            "\r\n",
            "Bring your own stapler."
        ),
    )
    .await;

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(
        inbox_count(&test.server, &jane).await,
        jane_messages + 1,
        "nested mailing list member was not delivered"
    );
    assert_eq!(
        inbox_count(&test.server, &bill).await,
        bill_messages + 1,
        "direct mailing list member was not delivered"
    );

    // Lists that reference each other must terminate and deliver exactly once
    admin
        .registry_create_object(MailingList {
            name: "ouroboros-head".to_string(),
            recipients: Map::new(vec![
                "ouroboros-tail@example.org".to_string(),
                "jdoe@example.org".to_string(),
                "bill@example.org".to_string(),
            ]),
            domain_id,
            ..Default::default()
        })
        .await;
    admin
        .registry_create_object(MailingList {
            name: "ouroboros-tail".to_string(),
            recipients: Map::new(vec![
                "ouroboros-head@example.org".to_string(),
                "jane.smith@example.org".to_string(),
                "bill@example.org".to_string(),
            ]),
            domain_id,
            ..Default::default()
        })
        .await;

    let john_messages = inbox_count(&test.server, &john).await;
    let jane_messages = inbox_count(&test.server, &jane).await;
    let bill_messages = inbox_count(&test.server, &bill).await;

    lmtp.ingest(
        "john.doe@example.org",
        &["ouroboros-head@example.org"],
        concat!(
            "From: john.doe@example.org\r\n",
            "To: ouroboros-head@example.org\r\n",
            "Subject: Going in circles\r\n",
            "\r\n",
            "Please advise."
        ),
    )
    .await;

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(
        inbox_count(&test.server, &john).await,
        john_messages + 1,
        "cyclic mailing list did not deliver exactly once"
    );
    assert_eq!(
        inbox_count(&test.server, &jane).await,
        jane_messages + 1,
        "cyclic mailing list did not deliver exactly once"
    );
    assert_eq!(
        inbox_count(&test.server, &bill).await,
        bill_messages + 1,
        "member shared by two nested lists was delivered more than once"
    );

    // Hearth Mail: a delivered invitation carries the $itip keyword, and
    // only the invitation does
    // (a new session: the one above is at its message limit)
    let mut lmtp = SmtpConnection::connect().await;
    let before = test
        .server
        .get_cached_messages(jane.id().document_id())
        .await
        .unwrap()
        .emails
        .items
        .len();
    for (subject, content_type) in [
        (
            "Invitation: planning",
            "text/calendar; charset=utf-8; method=REQUEST",
        ),
        ("Not an invitation", "text/plain; charset=utf-8"),
    ] {
        lmtp.ingest(
            "bill@example.org",
            &["jane.smith@example.org"],
            &format!(
                "From: bill@example.org\r\nTo: jane.smith@example.org\r\nSubject: {subject}\r\n\
                 Content-Type: {content_type}\r\n\r\nBEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\n\
                 END:VCALENDAR\r\n"
            ),
        )
        .await;
    }
    let jane_cache = test
        .server
        .get_cached_messages(jane.id().document_id())
        .await
        .unwrap();
    assert_eq!(jane_cache.emails.items.len(), before + 2);
    assert_eq!(
        jane_cache
            .with_keyword(&Keyword::Other(ITIP_KEYWORD.into()))
            .count(),
        1,
        "only the invitation is tagged $itip"
    );

    // Remove test data
    john.registry_destroy(
        ObjectType::MaskedEmail,
        [masked_prefix_id, masked_random_id],
    )
    .await
    .assert_destroyed(&[masked_prefix_id, masked_random_id]);
    for account in [&john, &jane, &bill] {
        test.destroy_all_mailboxes(account).await;
    }
    admin.registry_destroy_all(ObjectType::MailingList).await;
    admin
        .registry_destroy_all(ObjectType::SpamTrainingSample)
        .await;
    admin.registry_destroy_all(ObjectType::SpamTag).await;
    test.assert_is_empty().await;

    for account in [john, jane, bill] {
        admin.destroy_account(account).await;
    }

    test.cleanup().await;
}

impl Account {
    pub async fn spam_training_sample_ids(&self) -> Vec<Id> {
        self.registry_query_ids(
            ObjectType::SpamTrainingSample,
            Vec::<(&str, &str)>::new(),
            Vec::<&str>::new(),
        )
        .await
    }

    pub async fn spam_training_samples(&self) -> Vec<(Id, SpamTrainingSample)> {
        self.registry_get_all().await
    }
}

async fn assert_message_headers_contains(
    server: &Server,
    account_id: u32,
    document_id: u32,
    value: &str,
) {
    let headers = message_headers(server, account_id, document_id).await;
    assert!(
        headers.contains(value),
        "Expected message headers to contain {:?}, got {:?}",
        value,
        headers
    );
}

async fn message_headers(server: &Server, account_id: u32, document_id: u32) -> String {
    std::str::from_utf8(
        message_metadata(server, account_id, document_id)
            .await
            .raw_headers
            .as_ref(),
    )
    .unwrap()
    .to_string()
}

async fn message_metadata(server: &Server, account_id: u32, document_id: u32) -> MessageMetadata {
    server
        .store()
        .get_value::<Archive<AlignedBytes>>(ValueKey::property(
            account_id,
            Collection::Email,
            document_id,
            EmailField::Metadata,
        ))
        .await
        .unwrap()
        .unwrap()
        .deserialize::<MessageMetadata>()
        .unwrap()
}

async fn inbox_count(server: &Server, account: &Account) -> usize {
    server
        .get_cached_messages(account.id().document_id())
        .await
        .unwrap()
        .emails
        .items
        .len()
}

/// Hearth Mail: a stand-in for the contacts lookup (Tidings or Hearth):
/// answers `{"known": true}` when the request asks about `known_sender` for
/// the mailbox `account`, and `false` otherwise. Returns its URL.
async fn contacts_lookup_stub(account: &'static str, known_sender: &'static str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/known", listener.local_addr().unwrap());
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).to_lowercase();
                let known = request.contains("authorization: bearer stub-secret")
                    && request.contains(&format!("\"account\":\"{account}\""))
                    && request.contains(&format!("\"sender\":\"{known_sender}\""));
                let body = format!("{{\"known\": {known}}}");
                let reply = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(reply.as_bytes()).await;
            });
        }
    });
    url
}
