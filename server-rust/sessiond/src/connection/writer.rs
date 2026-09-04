//! Active-session outbound scheduling and delivery.

use tokio::io::AsyncWrite;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use tokio_util::codec::FramedWrite;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::outbound::{
    logical_class_for_server_envelope, LogicalChannelClass, LogicalChannelScheduler, OutboundItem,
    OutboundReceiver, OutboundRecvError,
};
use crate::session::SessionError;
use srui_protocol::{srui_message, ServerEventAck, SruiCodec, SruiMessage};

use super::{send_message_with_read_state, ConnectionError};

pub(super) async fn write_loop<W>(
    framed_write: FramedWrite<W, SruiCodec>,
    outbound: OutboundReceiver,
    control_rx: mpsc::Receiver<ServerEventAck>,
    shutdown: CancellationToken,
    session_cancel: CancellationToken,
    read_finished: CancellationToken,
) -> Result<(), ConnectionError>
where
    W: AsyncWrite + Unpin,
{
    Writer::new(
        framed_write,
        outbound,
        control_rx,
        shutdown,
        session_cancel,
        read_finished,
    )
    .run()
    .await
}

/// Owns the active connection's sole scheduler and every piece of outbound writer state.
struct Writer<W> {
    framed_write: FramedWrite<W, SruiCodec>,
    outbound: OutboundReceiver,
    control_rx: mpsc::Receiver<ServerEventAck>,
    scheduler: LogicalChannelScheduler,
    pending_ack: Option<ServerEventAck>,
    control_closed: bool,
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
        shutdown: CancellationToken,
        session_cancel: CancellationToken,
        read_finished: CancellationToken,
    ) -> Self {
        Self {
            framed_write,
            outbound,
            control_rx,
            scheduler: LogicalChannelScheduler::new(),
            pending_ack: None,
            control_closed: false,
            outbound_idle: false,
            shutdown,
            session_cancel,
            read_finished,
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

            // A closed control channel means the completed reader cannot produce more
            // acknowledgements. Once its bounded queue is empty, finish without streaming
            // unrelated low-priority output forever to a client that ended its input side.
            if self.control_closed && self.pending_ack.is_none() {
                return Ok(());
            }

            let read_finished = self.read_finished.is_cancelled();
            let control_ready = self.pending_ack.is_some();
            let outbound = &self.outbound;
            let class = self.scheduler.select_next(|class| match class {
                LogicalChannelClass::Control => control_ready,
                LogicalChannelClass::Input
                | LogicalChannelClass::TerminalHigh
                | LogicalChannelClass::TerminalNormal => false,
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
                    LogicalChannelClass::Input
                    | LogicalChannelClass::TerminalHigh
                    | LogicalChannelClass::TerminalNormal => continue,
                };
                debug_assert_eq!(logical_class_for_server_envelope(&envelope), Some(class));
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
