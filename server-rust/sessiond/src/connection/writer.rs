//! Active-session outbound scheduling and delivery.

use std::collections::VecDeque;

use tokio::io::AsyncWrite;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::watch;
use tokio_util::codec::FramedWrite;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::outbound::{
    server_envelope_matches_class, LogicalChannelClass, LogicalChannelScheduler, OutboundItem,
    OutboundReceiver, OutboundRecvError,
};
use crate::session::SessionError;
use srui_protocol::{srui_message, ServerEventAck, SruiCodec, SruiMessage};

use super::{send_message_with_read_state, ConnectionError, TerminalLanes, WriterCancel};

pub(super) async fn write_loop<W>(
    framed_write: FramedWrite<W, SruiCodec>,
    outbound: OutboundReceiver,
    control_rx: mpsc::Receiver<ServerEventAck>,
    terminal_lanes: TerminalLanes,
    cancel: WriterCancel,
) -> Result<(), ConnectionError>
where
    W: AsyncWrite + Unpin,
{
    Writer::new(framed_write, outbound, control_rx, terminal_lanes, cancel)
        .run()
        .await
}

/// Owns the active connection's sole scheduler and every piece of outbound writer state.
struct Writer<W> {
    framed_write: FramedWrite<W, SruiCodec>,
    outbound: OutboundReceiver,
    control_rx: mpsc::Receiver<ServerEventAck>,
    terminal_high_rx: mpsc::Receiver<SruiMessage>,
    terminal_normal_rx: mpsc::Receiver<SruiMessage>,
    catch_up: VecDeque<(LogicalChannelClass, SruiMessage)>,
    catch_up_released: watch::Sender<bool>,
    scheduler: LogicalChannelScheduler,
    pending_ack: Option<ServerEventAck>,
    pending_terminal_high: Option<SruiMessage>,
    pending_terminal_normal: Option<SruiMessage>,
    control_closed: bool,
    terminal_high_closed: bool,
    terminal_normal_closed: bool,
    outbound_idle: bool,
    shutdown: CancellationToken,
    session_cancel: CancellationToken,
    read_finished: CancellationToken,
}

impl<W> Writer<W>
where
    W: AsyncWrite + Unpin,
{
    fn new(
        framed_write: FramedWrite<W, SruiCodec>,
        outbound: OutboundReceiver,
        control_rx: mpsc::Receiver<ServerEventAck>,
        terminal_lanes: TerminalLanes,
        cancel: WriterCancel,
    ) -> Self {
        Self {
            framed_write,
            outbound,
            control_rx,
            terminal_high_rx: terminal_lanes.high_rx,
            terminal_normal_rx: terminal_lanes.normal_rx,
            catch_up: terminal_lanes.catch_up.into(),
            catch_up_released: terminal_lanes.catch_up_released,
            scheduler: LogicalChannelScheduler::new(),
            pending_ack: None,
            pending_terminal_high: None,
            pending_terminal_normal: None,
            control_closed: false,
            terminal_high_closed: false,
            terminal_normal_closed: false,
            outbound_idle: false,
            shutdown: cancel.shutdown,
            session_cancel: cancel.session_cancel,
            read_finished: cancel.read_finished,
        }
    }

    async fn run(mut self) -> Result<(), ConnectionError> {
        loop {
            if self.session_cancel.is_cancelled() || self.shutdown.is_cancelled() {
                return Ok(());
            }
            if !self.read_finished.is_cancelled() {
                match self.outbound.termination() {
                    Some(OutboundRecvError::Lagged(reason)) => {
                        warn!(%reason, "Client outbound queue overflowed; closing connection to force resync");
                        return Err(ConnectionError::Session(SessionError::LaggedResyncRequired));
                    }
                    Some(OutboundRecvError::Closed) | None => {}
                }
            }

            if self.pending_ack.is_none() && !self.control_closed {
                match self.control_rx.try_recv() {
                    Ok(ack) => self.pending_ack = Some(ack),
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => self.control_closed = true,
                }
            }
            if self.pending_terminal_high.is_none() && !self.terminal_high_closed {
                match self.terminal_high_rx.try_recv() {
                    Ok(msg) => self.pending_terminal_high = Some(msg),
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => self.terminal_high_closed = true,
                }
            }
            if self.pending_terminal_normal.is_none() && !self.terminal_normal_closed {
                match self.terminal_normal_rx.try_recv() {
                    Ok(msg) => self.pending_terminal_normal = Some(msg),
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => self.terminal_normal_closed = true,
                }
            }

            // A closed control channel means the completed reader cannot produce more
            // acknowledgements. Once its bounded queue is empty, finish without streaming
            // unrelated low-priority output forever to a client that ended its input side.
            if self.control_closed && self.pending_ack.is_none() {
                return Ok(());
            }

            let read_finished = self.read_finished.is_cancelled();
            let control_ready = self.pending_ack.is_some();
            let catching_up = !self.catch_up.is_empty();
            let catch_up_high = self
                .catch_up
                .front()
                .is_some_and(|(class, _)| *class == LogicalChannelClass::TerminalHigh);
            let catch_up_normal = self
                .catch_up
                .front()
                .is_some_and(|(class, _)| *class == LogicalChannelClass::TerminalNormal);
            let terminal_high_ready = self.pending_terminal_high.is_some();
            let terminal_normal_ready = self.pending_terminal_normal.is_some();
            let outbound = &self.outbound;
            let class = self.scheduler.select_next(|class| match class {
                LogicalChannelClass::Control => control_ready,
                LogicalChannelClass::Input => false,
                LogicalChannelClass::TerminalHigh => {
                    !read_finished && (catch_up_high || (!catching_up && terminal_high_ready))
                }
                LogicalChannelClass::TerminalNormal => {
                    !read_finished && (catch_up_normal || (!catching_up && terminal_normal_ready))
                }
                LogicalChannelClass::Ui => {
                    !read_finished && outbound.class_ready(LogicalChannelClass::Ui)
                }
                LogicalChannelClass::Resource => {
                    !read_finished && outbound.class_ready(LogicalChannelClass::Resource)
                }
            });

            if let Some(class) = class {
                let envelope = match class {
                    LogicalChannelClass::Control => SruiMessage {
                        msg: Some(srui_message::Msg::ServerEventAck(
                            self.pending_ack
                                .take()
                                .expect("control selected only when an acknowledgement is pending"),
                        )),
                    },
                    LogicalChannelClass::Ui | LogicalChannelClass::Resource => {
                        match self.outbound.pop_class(class) {
                            Ok(Some(item)) => outbound_item_to_message(item),
                            Ok(None) => continue,
                            Err(OutboundRecvError::Lagged(reason)) => {
                                warn!(%reason, "Client outbound queue overflowed; closing connection to force resync");
                                return Err(ConnectionError::Session(
                                    SessionError::LaggedResyncRequired,
                                ));
                            }
                            Err(OutboundRecvError::Closed) => {
                                self.outbound_idle = true;
                                continue;
                            }
                        }
                    }
                    LogicalChannelClass::TerminalHigh => self
                        .take_catch_up(LogicalChannelClass::TerminalHigh)
                        .or_else(|| self.pending_terminal_high.take())
                        .expect("terminalHigh selected only when a live/resync frame is pending"),
                    LogicalChannelClass::TerminalNormal => self
                        .take_catch_up(LogicalChannelClass::TerminalNormal)
                        .or_else(|| self.pending_terminal_normal.take())
                        .expect("terminalNormal selected only when a replay frame is pending"),
                    LogicalChannelClass::Input => continue,
                };
                debug_assert!(
                    server_envelope_matches_class(&envelope, class),
                    "envelope {envelope:?} is not legal on {class:?}"
                );
                if !send_message_with_read_state(
                    &mut self.framed_write,
                    envelope,
                    class,
                    &self.shutdown,
                    &self.outbound,
                    Some(&self.read_finished),
                )
                .await?
                {
                    return Ok(());
                }
                if self.catch_up.is_empty() {
                    let _ = self.catch_up_released.send(true);
                }

                // A send may complete without yielding while the socket remains writable. Yield
                // after every frame so the sibling read future can settle input and enqueue ACKs.
                tokio::task::yield_now().await;
                continue;
            }

            if !self.read_finished.is_cancelled()
                && self.pending_ack.is_none()
                && (self.outbound_idle || self.outbound.is_closed())
                && !self.outbound.class_ready(LogicalChannelClass::Ui)
                && !self.outbound.class_ready(LogicalChannelClass::Resource)
            {
                // Hub close ends the writer even while the read future is still attached; the
                // connection-local token then cancels the reader (§20.2).
                return Ok(());
            }

            let disconnect = self.outbound.disconnect_token().clone();
            tokio::select! {
                biased;
                _ = self.session_cancel.cancelled() => return Ok(()),
                _ = self.shutdown.cancelled() => return Ok(()),
                _ = self.read_finished.cancelled() => {}
                _ = disconnect.cancelled(), if !self.outbound_idle && !self.read_finished.is_cancelled() => match self.outbound.termination() {
                    Some(OutboundRecvError::Lagged(reason)) => {
                        warn!(%reason, "Client outbound queue overflowed; closing connection to force resync");
                        return Err(ConnectionError::Session(SessionError::LaggedResyncRequired));
                    }
                    Some(OutboundRecvError::Closed) => {
                        self.outbound_idle = !self.outbound.class_ready(LogicalChannelClass::Ui)
                            && !self.outbound.class_ready(LogicalChannelClass::Resource);
                    }
                    None => {
                        warn!("Client outbound subscriber disconnected during wait; closing connection to force resync");
                        return Err(ConnectionError::Session(SessionError::LaggedResyncRequired));
                    }
                },
                ack = self.control_rx.recv(), if self.pending_ack.is_none() && !self.control_closed => {
                    match ack {
                        Some(ack) => self.pending_ack = Some(ack),
                        None => self.control_closed = true,
                    }
                }
                msg = self.terminal_high_rx.recv(), if self.pending_terminal_high.is_none() && !self.terminal_high_closed && !self.read_finished.is_cancelled() => {
                    match msg {
                        Some(msg) => self.pending_terminal_high = Some(msg),
                        None => self.terminal_high_closed = true,
                    }
                }
                msg = self.terminal_normal_rx.recv(), if self.pending_terminal_normal.is_none() && !self.terminal_normal_closed && !self.read_finished.is_cancelled() => {
                    match msg {
                        Some(msg) => self.pending_terminal_normal = Some(msg),
                        None => self.terminal_normal_closed = true,
                    }
                }
                result = self.outbound.wait_for_work(), if !self.outbound_idle && !self.read_finished.is_cancelled() => {
                    match result {
                        Ok(()) => tokio::task::yield_now().await,
                        Err(OutboundRecvError::Lagged(reason)) => {
                            warn!(%reason, "Client outbound queue overflowed; closing connection to force resync");
                            return Err(ConnectionError::Session(SessionError::LaggedResyncRequired));
                        }
                        Err(OutboundRecvError::Closed) => {
                            self.outbound_idle = true;
                        }
                    }
                }
            }
        }
    }

    fn take_catch_up(&mut self, class: LogicalChannelClass) -> Option<SruiMessage> {
        if self
            .catch_up
            .front()
            .is_some_and(|(front, _)| *front == class)
        {
            let (_, envelope) = self.catch_up.pop_front().expect("front checked");
            Some(envelope)
        } else {
            None
        }
    }
}

fn outbound_item_to_message(item: OutboundItem) -> SruiMessage {
    match item {
        OutboundItem::Transaction(tx) => SruiMessage {
            msg: Some(srui_message::Msg::Transaction(tx)),
        },
        OutboundItem::ResourceMetadata(meta) => SruiMessage {
            msg: Some(srui_message::Msg::ResourceMetadata(meta)),
        },
        OutboundItem::ResourceChunk(chunk) => SruiMessage {
            msg: Some(srui_message::Msg::ResourceChunk(chunk)),
        },
    }
}
