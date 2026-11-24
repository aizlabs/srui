//! The processes that serve this session, which no action may target (D1
//! §27 server security controls; D2 T32; PX-030).
//!
//! The chain is derived on the server, from its own process tree and its own
//! connections, and never from a name, PID or path a client supplies:
//!
//! * **srtop itself**, `std::process::id()`: the daemon that holds the
//!   session (it embeds the SRUI session runtime, so it is the session
//!   daemon too);
//! * **its ancestors**, walked through `/proc/<pid>/stat`'s parent field up
//!   to the namespace's root (a parent of 0);
//! * **each connected client's transport**: the process at the other end of
//!   the accepted Unix socket, by the kernel-reported peer PID
//!   (`SO_PEERCRED`, which the kernel translates into this PID namespace) —
//!   the `srui-ssh-bridge` the SSH subsystem started;
//! * **its ancestors**: the SSH session processes serving that client
//!   (`sshd: user@notty` and the privileged monitor above it) and upwards.
//!
//! It is read afresh at every decision (Prepare, Confirm, every
//! revalidation), so a client that connects or disconnects in between is
//! accounted for. It is consulted only for keys from this host's live
//! `/proc` source ([`crate::procfs::live_source_id`]): a PID names a process
//! only within the source and namespace it was read in, and a fixture's PID
//! 4101 is no process of this host. For those keys it **fails closed**: when
//! any part of the chain cannot be read — this process's own record, an
//! ancestor's before the root is reached, a connected peer whose PID the
//! kernel did not report or whose record is gone — every target is refused
//! with [`Refusal::SupportChainUnknown`] until it can be read again.
//!
//! Protection is by PID number within this namespace. A member that is alive
//! holds its PID, so no other process can carry it; a member that exits
//! leaves the chain at the next read. This list is never the only boundary:
//! the gate's owner, identity and freshness checks apply first.

use crate::actions::{Refusal, SupportChain, SupportRole};
use crate::procfs::{parse_stat, read_bounded, DEFAULT_PROC_ROOT};
use crate::source::{Observed, ProcessKey, SourceId};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The longest ancestry walked before the chain is declared unreadable: far
/// deeper than any real process tree, and a bound on a corrupted one.
pub const MAX_ANCESTRY: usize = 64;

/// The live support chain of this srtop process (PX-030).
#[derive(Debug)]
pub struct ServerSupportChain {
    own: u32,
    source: SourceId,
    proc_root: PathBuf,
    /// Connected transports: connection number → the peer PID the kernel
    /// reported, `None` when it reported none.
    transports: Mutex<BTreeMap<u64, Option<u32>>>,
    next: AtomicU64,
}

impl Default for ServerSupportChain {
    fn default() -> Self {
        Self::new()
    }
}

/// One connected client's transport, registered with the chain while its
/// connection lasts. Dropping it unregisters the transport.
#[derive(Debug)]
pub struct Transport {
    chain: Arc<ServerSupportChain>,
    id: u64,
}

impl Drop for Transport {
    fn drop(&mut self) {
        self.chain
            .transports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.id);
    }
}

impl ServerSupportChain {
    /// This process's chain, read from the host's `/proc`, for keys from the
    /// live source.
    pub fn new() -> Self {
        Self::with(
            std::process::id(),
            crate::procfs::live_source_id(),
            Path::new(DEFAULT_PROC_ROOT),
        )
    }

    /// The chain of process `own`, read from `proc_root`, for keys from
    /// `source`.
    pub fn with(own: u32, source: SourceId, proc_root: &Path) -> Self {
        Self {
            own,
            source,
            proc_root: proc_root.to_path_buf(),
            transports: Mutex::new(BTreeMap::new()),
            next: AtomicU64::new(0),
        }
    }

    /// Registers a connected client's transport, the socket peer whose PID
    /// the kernel reported as `peer` (or did not), for as long as the
    /// returned guard lives.
    pub fn connected(self: &Arc<Self>, peer: Option<u32>) -> Transport {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.transports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, peer);
        Transport {
            chain: Arc::clone(self),
            id,
        }
    }

    /// How many transports are registered.
    pub fn transports(&self) -> usize {
        self.transports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Every member of the chain now, with its role, or why it cannot be
    /// read.
    pub fn members(&self) -> Result<Vec<(u32, SupportRole)>, String> {
        let mut members = vec![(self.own, SupportRole::ThisServer)];
        for ancestor in self.ancestry(self.own)? {
            members.push((ancestor, SupportRole::ServerAncestor));
        }
        let peers: Vec<Option<u32>> = self
            .transports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .copied()
            .collect();
        for peer in peers {
            let peer = peer.ok_or_else(|| {
                "the kernel did not report a connected client's transport process".to_string()
            })?;
            members.push((peer, SupportRole::ClientTransport));
            for ancestor in self.ancestry(peer)? {
                members.push((ancestor, SupportRole::TransportAncestor));
            }
        }
        Ok(members)
    }

    /// The parents of `pid`, nearest first, up to the namespace's root.
    fn ancestry(&self, pid: u32) -> Result<Vec<u32>, String> {
        let mut ancestry = Vec::new();
        let mut current = pid;
        loop {
            let path = self.proc_root.join(format!("{current}/stat"));
            let bytes = read_bounded(&path)
                .map_err(|error| format!("PID {current}'s record cannot be read: {error}"))?;
            let stat = parse_stat(current, &bytes)
                .ok_or_else(|| format!("PID {current}'s record does not parse"))?;
            let Observed::Known(parent) = stat.parent else {
                return Err(format!("PID {current}'s parent is not known"));
            };
            if parent == 0 {
                return Ok(ancestry);
            }
            if ancestry.len() >= MAX_ANCESTRY {
                return Err(format!(
                    "more than {MAX_ANCESTRY} ancestors above PID {pid}"
                ));
            }
            ancestry.push(parent);
            current = parent;
        }
    }
}

impl SupportChain for ServerSupportChain {
    fn protects(&self, key: &ProcessKey, pid: u32) -> Result<(), Refusal> {
        if key.source != self.source {
            // Not a process of this host: its PID names nothing here.
            return Ok(());
        }
        let members = self.members().map_err(Refusal::SupportChainUnknown)?;
        match members.into_iter().find(|(member, _)| *member == pid) {
            Some((pid, role)) => Err(Refusal::SupportChain { pid, role }),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{BootId, CreationToken, HostId, PidNamespaceId};

    struct Root(PathBuf);

    impl Root {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("srtop-px030-support-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        /// `pid`'s stat with `parent`.
        fn process(&self, pid: u32, parent: u32) {
            let directory = self.0.join(pid.to_string());
            std::fs::create_dir_all(&directory).unwrap();
            let line = format!(
                "{pid} (p{pid}) S {parent} 1 1 0 -1 0 0 0 0 0 1 1 0 0 20 0 1 0 100 4096 10\n"
            );
            let mut file = std::fs::File::create(directory.join("stat")).unwrap();
            std::io::Write::write_all(&mut file, line.as_bytes()).unwrap();
        }
    }

    impl Drop for Root {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn key(source: &str, pid: u32) -> ProcessKey {
        ProcessKey {
            source: SourceId(source.into()),
            host: Observed::Known(HostId("h".into())),
            boot: Observed::Known(BootId("b".into())),
            pid_namespace: Observed::Known(PidNamespaceId(1)),
            pid: Observed::Known(pid),
            creation: CreationToken::LinuxBootTicks(100),
            thread: None,
        }
    }

    #[test]
    fn the_chain_is_this_server_its_ancestors_and_each_transport_with_its_ancestors() {
        // init(1) -> sshd(10) -> monitor(20) -> session(30) -> bridge(40);
        // init(1) -> srtop(50); and an unrelated worker(60) under session.
        let root = Root::new("chain");
        for (pid, parent) in [
            (1, 0),
            (10, 1),
            (20, 10),
            (30, 20),
            (40, 30),
            (50, 1),
            (60, 30),
        ] {
            root.process(pid, parent);
        }
        let chain = Arc::new(ServerSupportChain::with(
            50,
            SourceId("live".into()),
            &root.0,
        ));
        let refused = |pid| chain.protects(&key("live", pid), pid);
        assert_eq!(
            refused(50),
            Err(Refusal::SupportChain {
                pid: 50,
                role: SupportRole::ThisServer
            })
        );
        assert_eq!(
            refused(1),
            Err(Refusal::SupportChain {
                pid: 1,
                role: SupportRole::ServerAncestor
            })
        );
        // No client is connected yet: the SSH processes are not the chain's.
        assert_eq!(refused(40), Ok(()));
        let transport = chain.connected(Some(40));
        assert_eq!(chain.transports(), 1);
        assert_eq!(
            refused(40),
            Err(Refusal::SupportChain {
                pid: 40,
                role: SupportRole::ClientTransport
            })
        );
        for pid in [30, 20, 10] {
            assert!(
                matches!(
                    refused(pid),
                    Err(Refusal::SupportChain {
                        role: SupportRole::TransportAncestor,
                        ..
                    })
                ),
                "{pid}"
            );
        }
        assert_eq!(refused(60), Ok(()), "a sibling is not an ancestor");
        // Another source's PID 50 is no process of this host.
        assert_eq!(chain.protects(&key("fixture", 50), 50), Ok(()));
        // The transport disconnects: the SSH processes leave the chain.
        drop(transport);
        assert_eq!(chain.transports(), 0);
        assert_eq!(refused(40), Ok(()));
    }

    #[test]
    fn a_chain_that_cannot_be_read_refuses_every_live_target() {
        let root = Root::new("unreadable");
        for (pid, parent) in [(1, 0), (50, 1), (60, 1)] {
            root.process(pid, parent);
        }
        // Own record missing.
        let chain = Arc::new(ServerSupportChain::with(
            51,
            SourceId("live".into()),
            &root.0,
        ));
        assert!(matches!(
            chain.protects(&key("live", 60), 60),
            Err(Refusal::SupportChainUnknown(_))
        ));
        // A peer the kernel did not name, and a peer whose record is gone.
        let chain = Arc::new(ServerSupportChain::with(
            50,
            SourceId("live".into()),
            &root.0,
        ));
        assert_eq!(chain.protects(&key("live", 60), 60), Ok(()));
        let unnamed = chain.connected(None);
        assert!(matches!(
            chain.protects(&key("live", 60), 60),
            Err(Refusal::SupportChainUnknown(_))
        ));
        drop(unnamed);
        let gone = chain.connected(Some(77));
        assert!(matches!(
            chain.protects(&key("live", 60), 60),
            Err(Refusal::SupportChainUnknown(_))
        ));
        drop(gone);
        assert_eq!(chain.protects(&key("live", 60), 60), Ok(()));
        // A fixture's key is not judged by this host's chain at all.
        let lost = chain.connected(None);
        assert_eq!(chain.protects(&key("fixture", 60), 60), Ok(()));
        drop(lost);
        // A cycle is bounded and refused, not walked forever.
        root.process(70, 71);
        root.process(71, 70);
        let cyclic = Arc::new(ServerSupportChain::with(
            70,
            SourceId("live".into()),
            &root.0,
        ));
        assert!(matches!(
            cyclic.members(),
            Err(why) if why.contains("more than")
        ));
    }
}
