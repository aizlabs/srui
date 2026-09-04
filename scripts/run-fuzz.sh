#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: ./scripts/run-fuzz.sh [seconds-per-target]

Replay the committed and canonical seed corpora, then fuzz all four targets for
15 seconds each by default. Pass 0 to replay the corpus without mutation. The
maximum duration is 86400 seconds (24 hours) per target.

Examples:
  ./scripts/run-fuzz.sh
  ./scripts/run-fuzz.sh 60
  ./scripts/run-fuzz.sh 0
EOF
}

if [[ $# -gt 1 ]]; then
  usage >&2
  exit 2
fi

duration="${1:-${FUZZ_SECONDS:-15}}"
case "$duration" in
  ''|*[!0-9]*)
    echo "seconds-per-target must be a non-negative integer, got: $duration" >&2
    usage >&2
    exit 2
    ;;
esac

# Normalize without shell arithmetic first: Bash 3.2 silently wraps oversized integers.
while [[ ${#duration} -gt 1 && "${duration:0:1}" == "0" ]]; do
  duration="${duration:1}"
done
max_duration=86400
if [[ ${#duration} -gt ${#max_duration} ]] \
  || { [[ ${#duration} -eq ${#max_duration} ]] && [[ "$duration" > "$max_duration" ]]; }; then
  echo "seconds-per-target must not exceed $max_duration, got: $duration" >&2
  exit 2
fi
# The bounded value is safe to interpret explicitly as decimal.
duration="$((10#$duration))"

if ! cargo +nightly fuzz --version >/dev/null 2>&1; then
  cat >&2 <<'EOF'
cargo-fuzz with a nightly Rust toolchain is required.

Install it with:
  rustup toolchain install nightly
  cargo install cargo-fuzz --locked
EOF
  exit 1
fi

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/.." && pwd)"
vectors="$repo_root/protocol/conformance-vectors"
fuzz_dir="$repo_root/server-rust/fuzz"
targets=(decode_framed decode_wire apply_transaction state_sequence)

scratch="$(mktemp -d "${TMPDIR:-/tmp}/srui-fuzz.XXXXXX")"
cleanup() {
  rm -rf -- "$scratch"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

corpus="$scratch/corpus"
for target in "${targets[@]}"; do
  mkdir -p "$corpus/$target"
  cp -R "$fuzz_dir/corpus/$target/." "$corpus/$target/"
done

cp "$vectors/golden_framed_message.bin" "$corpus/decode_framed/"
cp "$vectors/malformed_overlong_varint.bin" "$corpus/decode_framed/"
cp "$vectors/malformed_truncated_frame.bin" "$corpus/decode_framed/"
cp "$vectors/golden_node_record.bin" "$corpus/decode_wire/"
cp "$vectors/golden_transaction.bin" "$corpus/decode_wire/"
cp "$vectors/malformed/"*.bin "$corpus/decode_wire/"
cp "$vectors/golden_transaction.bin" "$corpus/apply_transaction/"
cp "$vectors/malformed/"*.bin "$corpus/apply_transaction/"
cp "$vectors/golden_transaction.bin" "$corpus/state_sequence/"

cd "$fuzz_dir"
for target in "${targets[@]}"; do
  echo "==> Replaying $target corpus"
  cargo +nightly fuzz run "$target" "$corpus/$target" -- -runs=0

  if [[ "$duration" -gt 0 ]]; then
    echo "==> Fuzzing $target for ${duration}s"
    cargo +nightly fuzz run "$target" "$corpus/$target" -- -max_total_time="$duration"
  fi
done

echo "All fuzz targets completed without a crash."
