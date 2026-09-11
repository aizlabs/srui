//! SRUI Core State-Machine Conformance Test Runner (§32 item 1, §4).
//!
//! Implements: §6.2 (identity scoping), §12.1 (atomic transactions/revisions), §13 (operations),
//! §26 (safety limits), §32.1 (core state-machine suite).
//!
//! Loads and executes declarative conformance vectors from
//! `protocol/conformance-vectors/suites/01-core-state-machine/vectors/*.json`, validating atomic
//! transactions, identity invariants, safety limits, model operations, and rollback behavior.
//!
//! The semantic-not-paint invariant (§32 item 3) is a separate, independently runnable suite in
//! `conformance_semantic_not_paint_test.rs`.

mod common;

use common::fixture_replay::replay_fixture;

// ==============================================================================
// Integration Test Cases
// ==============================================================================

#[test]
fn test_all_state_machine_conformance_fixtures() {
    // Count-pinned by protocol/conformance-vectors/suites/manifest.json.
    let fixture_paths = common::suite_vectors(1);
    println!(
        "Replaying {} state-machine conformance fixtures...",
        fixture_paths.len()
    );

    for path in &fixture_paths {
        println!("  -> Replaying fixture: {:?}", path.file_name().unwrap());
        replay_fixture(path);
    }
}
