# Fuzz corpora

One directory per fuzz target in `../fuzz_targets/`:

| Directory | Target | Entry point |
|---|---|---|
| `decode_framed/` | `decode_framed` | varint length-delimited frame decode (§16, §26) |
| `decode_wire/` | `decode_wire` | protobuf → core type conversion (§16) |
| `apply_transaction/` | `apply_transaction` | atomic transaction apply against a store (§12.1, §13) |
| `state_sequence/` | `state_sequence` | multi-transaction sequences with snapshot rollback checks (§12.1, §26) |

A corpus is a **coverage cache, not test data.** libFuzzer rediscovers equivalent inputs from
coverage feedback on every run; nothing here is an oracle and nothing here is required for a run to
be meaningful.

## What is canonical

`protocol/conformance-vectors/` is the authoritative input set. The CI fuzz job
(`.github/workflows/ci.yml`, "Seed fuzz corpora from conformance vectors") copies the golden and
malformed vectors into each target directory before running, so the assertions a fuzz run must
satisfy are pinned by files that are reviewed, hashed in `expected.json`, and shared with the Rust,
Swift, and Python conformance suites.

The files committed here are **optional minimized discoveries**: libFuzzer-`cmin`'d survivors of
previous runs, kept only to shorten the path back to interesting coverage during the bounded
(15 s/target) CI run. They may be regenerated, minimized further, or deleted without weakening any
guarantee — the conformance vectors and the test suites are what enforce behavior.

## Why they are kept anyway

Measured before deciding, since "a corpus is just a cache" is an argument for deleting it. The
three targets shown below were run three ways — the committed corpus replayed without mutation, the
CI seeds alone for 15 s, and both together for 15 s (`cov` = edges, `ft` = features):

| Target | Corpus replay, no mutation | CI seeds + 15 s | Corpus + seeds + 15 s | Corpus contribution |
|---|---|---|---|---|
| `decode_framed` | cov 2664 / ft 7345 | cov 3395 / ft 9595 | cov 3470 / ft 10245 | +2.2% cov, +6.8% ft |
| `decode_wire` | cov 1851 / ft 4505 | cov 2285 / ft 5339 | cov 2539 / ft 6490 | +11.1% cov, +21.6% ft |
| `apply_transaction` | cov 1397 / ft 2564 | cov 1615 / ft 3748 | cov 1745 / ft 4264 | +8.0% cov, +13.8% ft |

`state_sequence` is absent from this historical comparison because no equivalent seeds-only
measurement has been recorded for it yet.

Three things follow, and they are the reason this directory still exists:

1. **Replaying the corpus is not where its value is.** With no mutation it reaches *less* coverage
   than 15 s of fresh fuzzing from the 3–8 seed files. These blobs do not cover anything the seeds
   cannot reach; they are a head start that lets the same fixed budget reach further.
2. **The budget, not wall time, is the axis.** CI runs `-max_total_time=15` either way, so the
   corpus does not make anything slower — it buys 2–11% more edges per run in exchange for repo
   weight and diff noise.
3. **`decode_wire` contributes the most, not the least.** Its inputs must carry well-formed
   protobuf field tags before conversion code is reachable, and mutation finds that structure
   slowly; framing saturates almost immediately by comparison. Any future trimming should start
   with `decode_framed`, not with the decoders as a group.

Deleting also reclaims less than it appears: these blobs are already in history, so `size-pack`
does not shrink — only checkout size and future diffs improve.

Caveat: one sample per cell on one machine. The `decode_wire` and `apply_transaction` gaps are
well outside run-to-run noise; the 2.2% on `decode_framed` is not. Re-measure with the commands
below before acting on these numbers.

## What does not belong here

- **Crash reproducers.** libFuzzer writes those to `../artifacts/`, which is gitignored on purpose.
  A crash is not archived as a corpus blob: promote it to a named vector under
  `protocol/conformance-vectors/` (or `malformed/`) plus a regression test, so all three languages
  assert on it.
- **Flat files directly under `corpus/`.** No target consumes them; `../.gitignore` excludes
  anything outside the four target directories for exactly that reason.

## Working with the corpora

From the repository root, the helper script copies the committed corpus and canonical fixtures to a
temporary directory, replays them, then fuzzes every target for 15 seconds without dirtying the
working tree. Pass a different per-target duration, or `0` for replay only:

```bash
./scripts/run-fuzz.sh
./scripts/run-fuzz.sh 60
./scripts/run-fuzz.sh 0
```

For direct work on one corpus:

```bash
cd server-rust/fuzz

# Replay the committed corpus without mutation (what CI does first).
cargo fuzz run decode_framed corpus/decode_framed -- -runs=0

# Bounded exploratory run.
cargo fuzz run decode_framed corpus/decode_framed -- -max_total_time=60

# Minimize before committing anything new.
cargo fuzz cmin decode_framed corpus/decode_framed
```

Requires a nightly toolchain and `cargo install cargo-fuzz --locked`.
