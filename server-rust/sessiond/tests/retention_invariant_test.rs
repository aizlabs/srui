//! Retention invariants for long-lived per-client state (§15, §20.2, §26).
//!
//! These tests differ in kind from the rest of the suite. Elsewhere a test drives one operation
//! and asserts its outcome — accepted, rejected, this many entries. That shape cannot see an
//! unbounded *value* behind a bounded entry count: a table capped at 256 entries stays at 256
//! entries no matter how large each key is, so every count assertion keeps passing while memory
//! grows without limit.
//!
//! What follows asserts a budget over an operation *sequence* instead: after each of thousands of
//! handshakes with distinct, maximal identifiers, the bytes retained on behalf of clients must
//! still fit [`MAX_RETAINED_CLIENT_STATE_BYTES`]. Each test also asserts that the sequence really
//! did saturate the table it targets, so a change that quietly stops retaining anything cannot
//! make the invariant pass vacuously.

use srui_protocol::{ClientHello, ClientResume};
use srui_sessiond::{
    Session, SessionError, MAX_CLIENT_INSTANCE_ID_BYTES, MAX_RETAINED_CLIENT_STATE_BYTES,
};

/// Deterministic sequence driver. A seeded LCG keeps a "random" operation order reproducible: a
/// failing CI run names the seed, and replaying it reproduces the exact sequence.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
}

/// A distinct identifier of exactly the maximum accepted length — the worst case the cap allows.
fn max_length_id(nonce: u64) -> Vec<u8> {
    let mut id = nonce.to_be_bytes().to_vec();
    id.resize(MAX_CLIENT_INSTANCE_ID_BYTES, 0xAB);
    id
}

/// Identifier size a hostile-but-well-formed peer can present.
///
/// The wire permits anything up to the 16 MiB frame ceiling; 64 KiB is a thousand times the
/// accepted cap, which is enough to blow the budget by three orders of magnitude while keeping
/// the test fast. Sizing this at `cap + 1` would *not* work: 256 entries of 65 bytes still fits
/// the budget, so the test would pass against the very bug it exists to catch.
const HOSTILE_ID_BYTES: usize = 64 * 1024;

/// An identifier far past the cap, of the size an unbounded table would actually retain.
fn hostile_id(nonce: u64) -> Vec<u8> {
    let mut id = nonce.to_be_bytes().to_vec();
    id.resize(HOSTILE_ID_BYTES, 0xCD);
    id
}

fn hello(client_instance_id: Vec<u8>) -> ClientHello {
    ClientHello {
        core_version: "0.5.0".to_string(),
        profiles: vec!["org.srui.standard-widgets/1".to_string()],
        limits: None,
        client_instance_id,
        client_metadata: Default::default(),
        known_resource_hashes: vec![],
    }
}

fn resume(session_id: &str, client_instance_id: Vec<u8>) -> ClientResume {
    ClientResume {
        session_id: session_id.to_string(),
        client_instance_id,
        last_applied_revision: 0,
        last_acked_event_seq: 0,
        terminal_stream_offsets: Default::default(),
        limits: None,
        known_resource_hashes: vec![],
        pending_text_edits: vec![],
    }
}

/// Thousands of distinct clients, each presenting the largest identifier the handshake accepts,
/// must leave retained bytes inside a budget derived from the caps rather than from the traffic.
#[test]
fn retained_client_state_stays_within_budget_across_a_handshake_sequence() {
    const SEED: u64 = 0x5121_9AC7_0B3D_1E55;
    const OPERATIONS: usize = 3_000;

    let session = Session::new("retention-invariant");
    let session_id = session.session_id();
    let mut rng = Lcg(SEED);
    let mut accepted = 0usize;
    let mut offered_bytes = 0usize;

    for step in 0..OPERATIONS {
        let nonce = step as u64;
        // Whether a given handshake is accepted or refused is deliberately not asserted here —
        // that belongs to the unit tests for the identifier cap. This test asserts only that the
        // *outcome of the sequence* stays inside the budget, so it keeps failing for the right
        // reason if the cap is ever removed, rather than failing on a refusal that no longer
        // happens.
        let id = if rng.next().is_multiple_of(2) {
            max_length_id(nonce)
        } else {
            hostile_id(nonce)
        };
        offered_bytes += id.len();

        let handshake_accepted = if rng.next().is_multiple_of(2) {
            session.bootstrap_fresh_client(&hello(id)).is_ok()
        } else {
            // A resume naming an incarnation this session never issued still records the client.
            !matches!(
                session.bootstrap_resume(&resume(&session_id, id)),
                Err(SessionError::InvalidInput(_))
            )
        };
        if handshake_accepted {
            accepted += 1;
        }

        // The invariant is asserted after *every* operation, not once at the end: a table that
        // overshoots its bound mid-sequence and is later trimmed would otherwise pass.
        let retained = session
            .retained_client_state_bytes()
            .expect("retained byte accounting must remain available");
        assert!(
            retained <= MAX_RETAINED_CLIENT_STATE_BYTES,
            "seed {SEED:#x} step {step}: retained {retained} bytes exceeds the \
             {MAX_RETAINED_CLIENT_STATE_BYTES} byte budget"
        );
    }

    // Not vacuous: clients were actually retained, and the sequence offered far more bytes than
    // the budget, so the assertion above was load-bearing rather than trivially satisfied.
    let retained = session
        .retained_client_state_bytes()
        .expect("retained byte accounting must remain available");
    assert!(accepted > 0, "the sequence must complete some handshakes");
    assert!(
        retained > 0,
        "the sequence must actually retain client state, or the invariant proves nothing"
    );
    assert!(
        offered_bytes > MAX_RETAINED_CLIENT_STATE_BYTES,
        "offered {offered_bytes} bytes against a {MAX_RETAINED_CLIENT_STATE_BYTES} byte budget: \
         the sequence is too short to put the cap under test"
    );
}

/// The same property stated as growth: doubling the number of distinct clients must not change
/// retained bytes once the table is saturated.
#[test]
fn retained_client_state_does_not_grow_with_client_count() {
    let session = Session::new("retention-growth");

    // Half the clients present an identifier at the cap, half present one past it. A daemon that
    // retains what it should refuse grows with the second half; a daemon that bounds retention
    // plateaus on the first.
    let saturate = |from: u64, to: u64| {
        for nonce in from..to {
            let _ = session.bootstrap_fresh_client(&hello(max_length_id(nonce)));
            let _ = session.bootstrap_fresh_client(&hello(hostile_id(nonce)));
        }
        session
            .retained_client_state_bytes()
            .expect("retained byte accounting must remain available")
    };

    let after_1k = saturate(0, 1_000);
    let after_2k = saturate(1_000, 2_000);

    assert_eq!(
        after_1k, after_2k,
        "retained bytes must plateau at the cap, not track the number of clients seen"
    );
    assert!(after_1k <= MAX_RETAINED_CLIENT_STATE_BYTES);
    assert!(after_1k > 0);
}
