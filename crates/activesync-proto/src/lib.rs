/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Exchange ActiveSync's wire format, with no server code, so it can be
//! tested on its own: the WBXML codec and code pages ([`wbxml`]), the
//! request line ([`request`]), and the conversions between ActiveSync items
//! and iCalendar and vCard ([`items`]). The plan is docs/activesync.md.

pub mod items;
pub mod request;
pub mod wbxml;
