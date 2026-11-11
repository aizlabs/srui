# Suite 9 — Security limits (§32 item 9)

**Status:** `active`  
**Spec sections:** §26, §20.1, §27, §32.9

Run this suite alone:

```bash
scripts/run-conformance --suite 9
```

## Fixtures

Code-driven suite: no shared fixtures. This suite asserts behaviour against the registry tables `build.rs` and `generate_swift_registry.py` already generate, rather than introducing a second copy of the registry.

Malformed shortcuts (PX-016-G01): `malformed_shortcut_over_length.bin`, `malformed_shortcut_unknown_token.bin` and `malformed_shortcut_duplicate_modifier.bin` (listed in `expected.json`) are refused on decode by Rust (`shortcut_test`) and by both Swift decode paths (`DecoderTests`, `ShortcutStoreTests`), with the same reason; the 32-byte bound is checked before parsing. Both parsers split on the `+` byte and assert the shared table `shortcut_grammar_cases.json` row for row.

Hierarchical model limits (PX-018-G01): `max_model_tree_depth` (default 256 levels) and the shape rules for `tree_depth` / `descendant_count` are refused identically by the Rust store and the Swift replica. Negative vectors live with suite 1 and replay through both decode/apply paths: `76_model_tree_depth_over_limit_rejected.json` and `77_model_tree_depth_over_default_limit_rejected.json` (depth over the limit), `78_model_tree_descendant_count_past_item_count_rejected.json` (`descendant_count` past `item_count`), `79_model_tree_depth_jump_rejected.json` (a depth jump over one) `80_model_tree_child_outside_parent_span_rejected.json` (a child outside its parent's span), `85_model_tree_deeper_item_after_leaf_rejected.json` (a child under a leaf), `86_model_tree_unreachable_depth_across_gap_rejected.json` (a depth no forest reaches across an uncached gap) and `88_model_tree_descendants_below_the_deepest_level_rejected.json` (descendants below the deepest level). `model_tree_test` / `ModelTreeStoreTests` assert the limit at and past its bound and the shared table `model_tree_shape_cases.json` (323 rows) row for row, every verdict confirmed by the true-forest oracle in `protocol/tests/test_model_tree_shape_cases.py`; `conformance_security_limits_test` pins the boundary on its own.

## Runners

**Rust**

```bash
cargo test --manifest-path server-rust/Cargo.toml -p srui-sessiond --test security_limits_test
cargo test --manifest-path server-rust/Cargo.toml -p srui-protocol --test framing_decoder_matrix_test
cargo test --manifest-path server-rust/Cargo.toml -p srui-semantic-tree --test conformance_security_limits_test
cargo test --manifest-path server-rust/Cargo.toml -p srui-semantic-tree --test shortcut_test
cargo test --manifest-path server-rust/Cargo.toml -p srui-semantic-tree --test model_tree_test
cargo test --manifest-path server-rust/Cargo.toml -p srui-sdk --test tree_model_test
```

**Swift**

```bash
swift test --package-path client-macos --filter FramingDecoderMatrixTests
swift test --package-path client-macos --filter DecoderTests
swift test --package-path client-macos --filter SSHTransportPostureTests
swift test --package-path client-macos --filter TransactionRateLimiterTests
swift test --package-path client-macos --filter SecurityLimitsConformanceTests
swift test --package-path client-macos --filter ShortcutStoreTests
swift test --package-path client-macos --filter ModelTreeStoreTests
```
