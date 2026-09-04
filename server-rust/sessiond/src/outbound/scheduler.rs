//! Logical channel scheduler (§18.2, §19.2).
//!
//! Selects the next outbound logical class from a deterministic 24-slot weighted cycle.
//! Empty lanes are skipped without consuming a write; FIFO order is preserved inside each
//! lane; the cursor is retained between selections.
//!
//! Logical classification is independent of protobuf/Core semantics: SSH and TCP serialize
//! selected frames onto one byte stream, while a future QUIC binding may map the same
//! classes to independent streams without changing Core messages.

use srui_protocol::{srui_message, SruiMessage};

/// Logical traffic class used by the outbound scheduler (§19.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogicalChannelClass {
    /// Handshake, resume, `SERVER EVENT_ACK`, and future wire-error envelopes.
    Control,
    /// Semantic user events.
    Input,
    /// Committed UI transactions.
    Ui,
    /// Interactive PTY bytes that should preempt bulk terminal output.
    TerminalHigh,
    /// Bulk terminal output.
    TerminalNormal,
    /// Resource metadata and chunks (images, attachments).
    Resource,
}

impl LogicalChannelClass {
    /// Every logical class, in declaration order.
    pub const ALL: [Self; 6] = [
        Self::Control,
        Self::Input,
        Self::Ui,
        Self::TerminalHigh,
        Self::TerminalNormal,
        Self::Resource,
    ];
}

include!("logical_channel_policy.generated.rs");

/// Maps a server-originated envelope to its logical class (§19.2).
///
/// `SERVER EVENT_ACK` is always control-class and is never coalesced or merged. Client-originated
/// and unknown envelopes return `None` so they cannot be admitted into an outbound lane.
#[must_use]
pub fn logical_class_for_server_envelope(message: &SruiMessage) -> Option<LogicalChannelClass> {
    match &message.msg {
        Some(srui_message::Msg::ServerWelcome(_))
        | Some(srui_message::Msg::ServerResumeOk(_))
        | Some(srui_message::Msg::ServerResyncRequired(_))
        | Some(srui_message::Msg::ServerEventAck(_)) => Some(LogicalChannelClass::Control),
        Some(srui_message::Msg::Transaction(_)) => Some(LogicalChannelClass::Ui),
        Some(srui_message::Msg::ResourceMetadata(_))
        | Some(srui_message::Msg::ResourceChunk(_)) => Some(LogicalChannelClass::Resource),
        Some(srui_message::Msg::ClientHello(_))
        | Some(srui_message::Msg::ClientResume(_))
        | Some(srui_message::Msg::Event(_))
        | None => None,
    }
}

/// Weighted round-robin selector over [`SERVICE_CYCLE`].
#[derive(Debug, Clone)]
pub struct LogicalChannelScheduler {
    cursor: usize,
}

impl Default for LogicalChannelScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl LogicalChannelScheduler {
    /// Creates a scheduler whose next selection starts at slot 0 of [`SERVICE_CYCLE`].
    #[must_use]
    pub fn new() -> Self {
        Self { cursor: 0 }
    }

    /// Selects the next ready class, skipping empty lanes without consuming a write.
    ///
    /// The cursor advances past every inspected slot, including skipped empty lanes, so
    /// fairness continues across calls. Returns `None` when no lane is ready; the cursor
    /// then sits where it started after a full empty scan.
    pub fn select_next(
        &mut self,
        ready: impl Fn(LogicalChannelClass) -> bool,
    ) -> Option<LogicalChannelClass> {
        for _ in 0..SERVICE_CYCLE.len() {
            let class = SERVICE_CYCLE[self.cursor];
            self.cursor = (self.cursor + 1) % SERVICE_CYCLE.len();
            if ready(class) {
                return Some(class);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use srui_protocol::{ResourceChunk, ResourceMetadata, ServerEventAck, Transaction};
    use std::collections::{HashMap, VecDeque};

    const RESOURCE_TOKEN_SIZE: usize = 16 * 1024;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct Token {
        class: LogicalChannelClass,
        id: u32,
    }

    #[derive(Debug, Default)]
    struct Lanes {
        queues: HashMap<LogicalChannelClass, VecDeque<Token>>,
    }

    impl Lanes {
        fn push(&mut self, token: Token) {
            self.queues.entry(token.class).or_default().push_back(token);
        }

        fn ready(&self, class: LogicalChannelClass) -> bool {
            self.queues
                .get(&class)
                .is_some_and(|queue| !queue.is_empty())
        }

        fn pop(&mut self, class: LogicalChannelClass) -> Token {
            self.queues
                .get_mut(&class)
                .expect("lane exists")
                .pop_front()
                .expect("lane ready")
        }

        fn replenish_all(&mut self, next_id: &mut u32) {
            for class in LogicalChannelClass::ALL {
                if !self.ready(class) {
                    self.push(Token {
                        class,
                        id: *next_id,
                    });
                    *next_id += 1;
                }
            }
        }
    }

    fn envelope(msg: srui_message::Msg) -> SruiMessage {
        SruiMessage { msg: Some(msg) }
    }

    #[test]
    fn saturated_cycle_matches_service_sequence_and_fifo() {
        let mut lanes = Lanes::default();
        for class in LogicalChannelClass::ALL {
            for seq in 0..8u32 {
                lanes.push(Token { class, id: seq });
            }
        }

        let mut scheduler = LogicalChannelScheduler::new();
        let mut last_id = [0u32; 6];
        for expected in SERVICE_CYCLE {
            let class = scheduler
                .select_next(|c| lanes.ready(c))
                .expect("every lane is backlogged");
            assert_eq!(class, expected);
            let token = lanes.pop(class);
            assert_eq!(token.class, class);
            let idx = LogicalChannelClass::ALL
                .iter()
                .position(|c| *c == class)
                .unwrap();
            assert_eq!(token.id, last_id[idx], "{class:?} must stay FIFO");
            last_id[idx] += 1;
        }
    }

    #[test]
    fn empty_lanes_are_skipped_without_consuming_a_write() {
        let mut lanes = Lanes::default();
        lanes.push(Token {
            class: LogicalChannelClass::Resource,
            id: 1,
        });
        lanes.push(Token {
            class: LogicalChannelClass::Resource,
            id: 2,
        });

        let mut scheduler = LogicalChannelScheduler::new();
        let first = scheduler
            .select_next(|c| lanes.ready(c))
            .expect("resource ready");
        assert_eq!(first, LogicalChannelClass::Resource);
        assert_eq!(lanes.pop(first).id, 1);

        let second = scheduler
            .select_next(|c| lanes.ready(c))
            .expect("resource still ready");
        assert_eq!(second, LogicalChannelClass::Resource);
        assert_eq!(lanes.pop(second).id, 2);

        assert!(scheduler.select_next(|c| lanes.ready(c)).is_none());
    }

    #[test]
    fn continuous_saturation_respects_documented_service_gaps() {
        let mut lanes = Lanes::default();
        let mut next_id = 0u32;
        lanes.replenish_all(&mut next_id);

        let mut scheduler = LogicalChannelScheduler::new();
        let mut last_index: HashMap<LogicalChannelClass, usize> = HashMap::new();
        let mut dispatched = Vec::new();

        for index in 0..(SERVICE_CYCLE.len() * 6) {
            lanes.replenish_all(&mut next_id);
            let class = scheduler
                .select_next(|c| lanes.ready(c))
                .expect("saturated");
            let token = lanes.pop(class);
            assert_eq!(token.class, class);
            if let Some(prev) = last_index.insert(class, index) {
                let gap = index - prev;
                assert!(
                    gap <= class.max_service_gap(),
                    "{class:?} gap {gap} exceeds bound {}",
                    class.max_service_gap()
                );
            }
            dispatched.push(class);
        }

        assert_eq!(&dispatched[..SERVICE_CYCLE.len()], &SERVICE_CYCLE);
    }

    #[test]
    fn resource_backlog_yields_to_newly_ready_control_input_and_ui() {
        let mut lanes = Lanes::default();
        for id in 0..300u32 {
            let mut payload = vec![0u8; RESOURCE_TOKEN_SIZE];
            payload[0] = (id % 256) as u8;
            let _ = payload;
            lanes.push(Token {
                class: LogicalChannelClass::Resource,
                id,
            });
        }

        let mut scheduler = LogicalChannelScheduler::new();
        let first = scheduler
            .select_next(|c| lanes.ready(c))
            .expect("resource ready");
        assert_eq!(first, LogicalChannelClass::Resource);
        assert_eq!(lanes.pop(first).id, 0);

        lanes.push(Token {
            class: LogicalChannelClass::Control,
            id: 1000,
        });
        lanes.push(Token {
            class: LogicalChannelClass::Input,
            id: 1001,
        });
        lanes.push(Token {
            class: LogicalChannelClass::Ui,
            id: 1002,
        });

        let mut seen_control = false;
        let mut seen_input = false;
        let mut seen_ui = false;
        for _ in 0..SERVICE_CYCLE.len() {
            let class = scheduler
                .select_next(|c| lanes.ready(c))
                .expect("probes or remaining resource");
            let token = lanes.pop(class);
            match token.class {
                LogicalChannelClass::Resource => panic!(
                    "no second resource token may precede the control/input/UI probes, got {token:?}"
                ),
                LogicalChannelClass::Control => {
                    assert_eq!(token.id, 1000);
                    seen_control = true;
                }
                LogicalChannelClass::Input => {
                    assert_eq!(token.id, 1001);
                    seen_input = true;
                }
                LogicalChannelClass::Ui => {
                    assert_eq!(token.id, 1002);
                    seen_ui = true;
                }
                _ => {}
            }
            if seen_control && seen_input && seen_ui {
                return;
            }
        }
        panic!("did not observe control, input, and UI probes before wrapping the cycle");
    }

    #[test]
    fn two_control_tokens_stay_separate_and_fifo() {
        let mut lanes = Lanes::default();
        lanes.push(Token {
            class: LogicalChannelClass::Control,
            id: 1,
        });
        lanes.push(Token {
            class: LogicalChannelClass::Control,
            id: 2,
        });

        let mut scheduler = LogicalChannelScheduler::new();
        let first = scheduler.select_next(|c| lanes.ready(c)).unwrap();
        assert_eq!(lanes.pop(first).id, 1);
        let second = scheduler.select_next(|c| lanes.ready(c)).unwrap();
        assert_eq!(lanes.pop(second).id, 2);
        assert_eq!(first, LogicalChannelClass::Control);
        assert_eq!(second, LogicalChannelClass::Control);
    }

    #[test]
    fn server_event_ack_maps_only_to_control() {
        let ack = envelope(srui_message::Msg::ServerEventAck(ServerEventAck {
            event_id: b"evt-1".to_vec(),
            last_processed_event_seq: 1,
            ..Default::default()
        }));
        assert_eq!(
            logical_class_for_server_envelope(&ack),
            Some(LogicalChannelClass::Control)
        );
        for class in LogicalChannelClass::ALL {
            if class != LogicalChannelClass::Control {
                assert_ne!(logical_class_for_server_envelope(&ack), Some(class));
            }
        }

        assert_eq!(
            logical_class_for_server_envelope(&envelope(srui_message::Msg::Transaction(
                Transaction::default()
            ))),
            Some(LogicalChannelClass::Ui)
        );
        assert_eq!(
            logical_class_for_server_envelope(&envelope(srui_message::Msg::ResourceMetadata(
                ResourceMetadata::default()
            ))),
            Some(LogicalChannelClass::Resource)
        );
        assert_eq!(
            logical_class_for_server_envelope(&envelope(srui_message::Msg::ResourceChunk(
                ResourceChunk::default()
            ))),
            Some(LogicalChannelClass::Resource)
        );

        for unschedulable in [
            SruiMessage::default(),
            envelope(srui_message::Msg::ClientHello(Default::default())),
            envelope(srui_message::Msg::ClientResume(Default::default())),
            envelope(srui_message::Msg::Event(Default::default())),
        ] {
            assert_eq!(
                logical_class_for_server_envelope(&unschedulable),
                None,
                "client or unknown envelopes must not enter a server outbound lane"
            );
        }
    }
}
