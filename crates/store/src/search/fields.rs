/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::search::*;

impl SearchableField for EmailSearchField {
    fn index() -> SearchIndex {
        SearchIndex::Email
    }

    fn primary_keys() -> &'static [SearchField] {
        &[SearchField::AccountId, SearchField::DocumentId]
    }

    fn all_fields() -> &'static [SearchField] {
        &[
            SearchField::Email(EmailSearchField::From),
            SearchField::Email(EmailSearchField::To),
            SearchField::Email(EmailSearchField::Cc),
            SearchField::Email(EmailSearchField::Bcc),
            SearchField::Email(EmailSearchField::Subject),
            SearchField::Email(EmailSearchField::Body),
            SearchField::Email(EmailSearchField::Attachment),
            SearchField::Email(EmailSearchField::ReceivedAt),
            SearchField::Email(EmailSearchField::SentAt),
            SearchField::Email(EmailSearchField::Size),
            SearchField::Email(EmailSearchField::HasAttachment),
            SearchField::Email(EmailSearchField::Headers),
        ]
    }

    fn is_indexed(&self) -> bool {
        #[cfg(not(feature = "test_mode"))]
        {
            matches!(
                self,
                EmailSearchField::From
                    | EmailSearchField::To
                    | EmailSearchField::Subject
                    | EmailSearchField::ReceivedAt
                    | EmailSearchField::SentAt
                    | EmailSearchField::Size
                    | EmailSearchField::HasAttachment,
            )
        }

        #[cfg(feature = "test_mode")]
        {
            matches!(
                self,
                EmailSearchField::From
                    | EmailSearchField::To
                    | EmailSearchField::Subject
                    | EmailSearchField::ReceivedAt
                    | EmailSearchField::SentAt
                    | EmailSearchField::Size
                    | EmailSearchField::HasAttachment
                    | EmailSearchField::Bcc
                    | EmailSearchField::Cc
            )
        }
    }

    fn is_text(&self) -> bool {
        matches!(
            self,
            EmailSearchField::From
                | EmailSearchField::To
                | EmailSearchField::Cc
                | EmailSearchField::Bcc
                | EmailSearchField::Subject
                | EmailSearchField::Body
                | EmailSearchField::Attachment,
        )
    }
}

impl SearchableField for TracingSearchField {
    fn index() -> SearchIndex {
        SearchIndex::Tracing
    }

    fn primary_keys() -> &'static [SearchField] {
        &[SearchField::Id]
    }

    fn all_fields() -> &'static [SearchField] {
        &[
            SearchField::Tracing(TracingSearchField::EventType),
            SearchField::Tracing(TracingSearchField::QueueId),
            SearchField::Tracing(TracingSearchField::Keywords),
        ]
    }

    fn is_indexed(&self) -> bool {
        matches!(
            self,
            TracingSearchField::QueueId | TracingSearchField::EventType
        )
    }

    fn is_text(&self) -> bool {
        matches!(self, TracingSearchField::Keywords)
    }
}

impl SearchField {
    pub(crate) fn is_indexed(&self) -> bool {
        match self {
            SearchField::Email(field) => field.is_indexed(),
            SearchField::Tracing(field) => field.is_indexed(),
            SearchField::AccountId | SearchField::DocumentId | SearchField::Id => false,
        }
    }

    pub(crate) fn is_text(&self) -> bool {
        match self {
            SearchField::Email(field) => field.is_text(),
            SearchField::Tracing(field) => field.is_text(),
            SearchField::AccountId | SearchField::DocumentId | SearchField::Id => false,
        }
    }

    pub(crate) fn is_json(&self) -> bool {
        matches!(self, SearchField::Email(EmailSearchField::Headers))
    }
}

impl SearchIndex {
    pub fn all_fields(&self) -> &[SearchField] {
        match self {
            SearchIndex::Email => EmailSearchField::all_fields(),
            SearchIndex::Tracing => TracingSearchField::all_fields(),
            SearchIndex::InMemory => unreachable!(),
        }
    }

    pub fn primary_keys(&self) -> &'static [SearchField] {
        match self {
            SearchIndex::Email => EmailSearchField::primary_keys(),
            SearchIndex::Tracing => TracingSearchField::primary_keys(),
            SearchIndex::InMemory => unreachable!(),
        }
    }
}
