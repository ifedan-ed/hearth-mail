/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Waiting for something to change, for Ping and for a Sync with `Wait` or
//! `HeartbeatInterval`: the server's push manager tells about every mail
//! and mailbox change of the account, the same events JMAP's EventSource
//! gets, so a delivery ends the wait at once.

use crate::{Ctx, error::Result};
use common::ipc::PushNotification;
use std::time::{Duration, Instant};
use types::type_state::DataType;
use utils::map::bitmap::Bitmap;

/// Something changed, or the time is up.
#[derive(Debug, PartialEq, Eq)]
pub enum Woke {
    Changed,
    TimedOut,
}

/// A subscription to the account's mail changes.
pub struct Watch {
    rx: tokio::sync::mpsc::Receiver<PushNotification>,
}

impl Ctx<'_> {
    /// Starts listening before the caller checks for changes, so nothing
    /// that happens in between is missed.
    pub async fn watch(&self) -> Result<Watch> {
        let mut types = Bitmap::<DataType>::new();
        types.insert(DataType::Email);
        types.insert(DataType::EmailDelivery);
        types.insert(DataType::Mailbox);
        Ok(Watch {
            rx: self
                .server
                .subscribe_push_manager(self.token, types)
                .await?,
        })
    }
}

impl Watch {
    /// Waits until the account's mail changes or `until` passes. A burst
    /// of notifications (a delivery touches several types) counts once.
    pub async fn wait(&mut self, until: Instant) -> Woke {
        let now = Instant::now();
        if until <= now {
            return Woke::TimedOut;
        }
        match tokio::time::timeout(until - now, self.rx.recv()).await {
            Ok(Some(_)) => {
                // Let the rest of the burst arrive and drop it.
                tokio::time::sleep(Duration::from_millis(250)).await;
                while self.rx.try_recv().is_ok() {}
                Woke::Changed
            }
            // The push manager went away (shutdown): as a timeout.
            Ok(None) | Err(_) => Woke::TimedOut,
        }
    }
}
