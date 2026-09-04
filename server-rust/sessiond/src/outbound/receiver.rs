//! Policy-free access to one subscriber's bounded UI and resource lanes.

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{LogicalChannelClass, OutboundItem, OutboundRecvError, SubscriberState};
use crate::session::lock_or_recover;

/// A policy-free handle for receiving outbound items streamed to one connection.
///
/// The connection writer owns the sole logical-channel scheduler. This receiver only reports
/// readiness and pops the explicit UI or resource lane selected by that scheduler.
#[derive(Debug)]
pub struct OutboundReceiver {
    notify_rx: mpsc::Receiver<()>,
    pub(super) state: Arc<Mutex<SubscriberState>>,
    disconnect: CancellationToken,
}

impl OutboundReceiver {
    pub(super) fn new(
        notify_rx: mpsc::Receiver<()>,
        state: Arc<Mutex<SubscriberState>>,
        disconnect: CancellationToken,
    ) -> Self {
        Self {
            notify_rx,
            state,
            disconnect,
        }
    }

    /// Waits for one item from the explicitly selected UI or resource lane.
    ///
    /// This is the authoritative lag signal: overflow returns
    /// [`OutboundRecvError::Lagged`]. Scheduling policy stays with the caller.
    pub async fn recv_class(
        &mut self,
        class: LogicalChannelClass,
    ) -> Result<OutboundItem, OutboundRecvError> {
        loop {
            if let Some(item) = self.try_recv_class(class)? {
                return Ok(item);
            }

            match self.notify_rx.recv().await {
                Some(()) => {}
                None => {
                    if let Some(item) = self.try_recv_class(class)? {
                        return Ok(item);
                    }
                    return Err(OutboundRecvError::Closed);
                }
            }
        }
    }

    /// Non-blocking poll of one explicitly selected UI or resource lane.
    pub fn try_recv_class(
        &mut self,
        class: LogicalChannelClass,
    ) -> Result<Option<OutboundItem>, OutboundRecvError> {
        let mut guard = lock_or_recover(&self.state);
        if let Some(reason) = &guard.stale_reason {
            return Err(OutboundRecvError::Lagged(reason.clone()));
        }
        if let Some(item) = guard.pop_class(class) {
            drop(guard);
            let _ = self.notify_rx.try_recv();
            return Ok(Some(item));
        }
        if guard.is_closed {
            return Err(OutboundRecvError::Closed);
        }
        Ok(None)
    }

    /// Returns `true` when `class` has a frame that [`Self::pop_class`] can emit.
    #[must_use]
    pub(crate) fn class_ready(&self, class: LogicalChannelClass) -> bool {
        lock_or_recover(&self.state).class_ready(class)
    }

    /// Pops the explicit lane selected by the connection writer's scheduler.
    pub(crate) fn pop_class(
        &mut self,
        class: LogicalChannelClass,
    ) -> Result<Option<OutboundItem>, OutboundRecvError> {
        self.try_recv_class(class)
    }

    /// Waits until UI or resource work is queued, or the subscriber terminates.
    pub(crate) async fn wait_for_work(&mut self) -> Result<(), OutboundRecvError> {
        loop {
            {
                let guard = lock_or_recover(&self.state);
                if let Some(reason) = &guard.stale_reason {
                    return Err(OutboundRecvError::Lagged(reason.clone()));
                }
                if guard.has_scheduled_work() {
                    return Ok(());
                }
                if guard.is_closed {
                    return Err(OutboundRecvError::Closed);
                }
            }

            match self.notify_rx.recv().await {
                Some(()) => {}
                None => {
                    let guard = lock_or_recover(&self.state);
                    if let Some(reason) = &guard.stale_reason {
                        return Err(OutboundRecvError::Lagged(reason.clone()));
                    }
                    if guard.has_scheduled_work() {
                        return Ok(());
                    }
                    return Err(OutboundRecvError::Closed);
                }
            }
        }
    }

    /// Token cancelled when this subscriber overflows or the hub closes.
    ///
    /// Connection writes race this against socket writes so backpressure cannot delay a required
    /// resync after overflow (§20.2).
    #[must_use]
    pub fn disconnect_token(&self) -> &CancellationToken {
        &self.disconnect
    }

    /// Current terminal state, if the queue has overflowed or closed.
    #[must_use]
    pub fn termination(&self) -> Option<OutboundRecvError> {
        let guard = lock_or_recover(&self.state);
        if let Some(reason) = &guard.stale_reason {
            Some(OutboundRecvError::Lagged(reason.clone()))
        } else if guard.is_closed {
            Some(OutboundRecvError::Closed)
        } else {
            None
        }
    }

    /// Returns `true` if the underlying queue has closed.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        lock_or_recover(&self.state).is_closed
    }
}

/// Releases the queue as soon as the connection ends, rather than at the next publish (§20.2).
impl Drop for OutboundReceiver {
    fn drop(&mut self) {
        let mut guard = lock_or_recover(&self.state);
        guard.is_closed = true;
        guard.clear();
    }
}
