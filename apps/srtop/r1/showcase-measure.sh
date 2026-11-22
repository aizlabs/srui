#!/usr/bin/env bash
# PX-027: the R1 showcase measurements against the Linux devbox (D1 §§31-32). Starts the box that
# apps/srtop/devbox/devbox.sh manages, records the exact environment, and runs `srtop-demo showcase`
# (a release build) four ways, each through a userspace TCP relay in front of the box's SSH port that
# counts the encrypted stream while the probe counts the SRUI payload:
#
#   compare  srtop's SRUI session against htop over `ssh -tt` (htop pinned in the image), alternately,
#            under the same process workloads (idle, busy, churn), at the same sampling interval
#            (srtop --refresh-interval-ms against htop -d, each read back from what the side
#            itself reports), the same warm-up and window, on a fixed terminal size;
#   rtt      the SRUI session at 0/100/300/600 ms RTT, added by the relay;
#   idle     an unchanged fake source (--fake-source --smoke-fixture): no app transaction after the
#            snapshot for the whole window, then srtop's own SIGUSR1 canary is the next one;
#   scale    the SRUI session against srtop's synthetic source at 1,000 and 10,000 rows.
#
# It audits every file it wrote with privacy-audit.sh and stops the box.
#
#   apps/srtop/r1/showcase-measure.sh            the published matrix (about 35 minutes)
#   apps/srtop/r1/showcase-measure.sh --quick    two runs of each, shorter windows (about 6 minutes)
#   SRUI_SHOWCASE_EVIDENCE_DIR=DIR ...           keep the evidence in DIR
#   SRTOP_SHOWCASE_KEEP_BOX=1 ...                leave the box running afterwards
#
# The image must hold the revision under test, exactly as live-checks.sh requires. Only the box's
# own state directory ($SRTOP_DEVBOX_STATE) is used for keys and known_hosts; ~/.ssh is never read
# for trust or written (ssh runs with -F /dev/null). Workers in the box are PX-025's bounded ones
# and fixed-lifetime sleeps; every run waits for them to be gone. Never root, never sudo.
set -euo pipefail

R1_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "$R1_DIR/../../.." && pwd)
DEVBOX_DIR=$REPO_ROOT/apps/srtop/devbox
STATE_DIR=${SRTOP_DEVBOX_STATE:-$HOME/.codex/srtop-devbox}
NAME=${SRTOP_DEVBOX_NAME:-srtop-devbox}
IMAGE=${SRTOP_DEVBOX_IMAGE:-srtop-devbox:latest}
PORT=${SRTOP_DEVBOX_PORT:-2222}
REV=${SRTOP_DEVBOX_REV:-HEAD}
IMAGE_INPUTS=(
  apps/srtop/Cargo.toml apps/srtop/Cargo.lock apps/srtop/src apps/srtop/demo/Cargo.toml apps/srtop/demo/Cargo.lock apps/srtop/demo/src
  server-rust protocol
  apps/srtop/devbox/Dockerfile apps/srtop/devbox/entrypoint.sh apps/srtop/devbox/sshd_config
)

# The published settings. The interval is srtop's default and a whole number of htop's tenths.
INTERVAL_MS=1000
COLUMNS_FIXED=120
LINES_FIXED=40
RUNS=7 WARMUP_MS=3000 DURATION_MS=15000
RTT_RUNS=5 RTT_WARMUP_MS=1000 RTT_DURATION_MS=5000
SCALE_RUNS=5
IDLE_MS=10000
MODE_NAME=published
case "${1:-}" in
  "") ;;
  --quick)
    MODE_NAME=quick RUNS=2 WARMUP_MS=1000 DURATION_MS=5000 RTT_RUNS=2 RTT_DURATION_MS=3000 SCALE_RUNS=2 IDLE_MS=5000
    ;;
  -h|--help) sed -n '2,26p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
  *) echo "showcase-measure: unknown option: $1" >&2; exit 2 ;;
esac

die() { echo "showcase-measure: $*" >&2; exit 1; }

started=""
cleanup() {
  local status=$?
  set +e
  if [ -n "$started" ] && [ -z "${SRTOP_SHOWCASE_KEEP_BOX:-}" ]; then
    "$DEVBOX_DIR/devbox.sh" down
  fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

[ "$(uname -s)" = Darwin ] || die "the measuring side is the macOS host; run this on a Mac"
[ "$(id -u)" -ne 0 ] || die "run as your own user, never as root"
docker=$(command -v docker) || die "docker is required (Docker Desktop must be running)"
docker info >/dev/null 2>&1 || die "docker is not running"

revision=$(git -C "$REPO_ROOT" rev-parse --verify --quiet "$REV^{commit}") || die "no such revision: $REV"
rebuild="SRTOP_DEVBOX_REV=$REV apps/srtop/devbox/devbox.sh build"
git -C "$REPO_ROOT" diff --quiet "$revision" -- "${IMAGE_INPUTS[@]}" \
  || die "the working tree changes what goes into the image since $REV; commit it, then: $rebuild"
if docker image inspect "$IMAGE" >/dev/null 2>&1; then
  label=$(docker image inspect -f '{{index .Config.Labels "org.srui.revision"}}' "$IMAGE" 2>/dev/null || true)
  [ -n "$label" ] || die "$IMAGE has no org.srui.revision label; rebuild it: $rebuild"
  git -C "$REPO_ROOT" cat-file -e "$label^{commit}" 2>/dev/null \
    || die "$IMAGE was built from $label, which this clone does not have; rebuild it: $rebuild"
  git -C "$REPO_ROOT" diff --quiet "$label" "$revision" -- "${IMAGE_INPUTS[@]}" \
    || die "$IMAGE was built from $label, which differs from $revision in what goes into the image; rebuild it: $rebuild"
fi

evidence=${SRUI_SHOWCASE_EVIDENCE_DIR:-}
[ -n "$evidence" ] || evidence=$(mktemp -d "${TMPDIR:-/tmp}/srui-showcase.XXXXXX")
mkdir -p "$evidence/ssh-logs"

# The measuring tool, a release build, like everything it measures.
cargo build --release --locked --manifest-path "$REPO_ROOT/apps/srtop/demo/Cargo.toml" --bin srtop-demo
demo=$REPO_ROOT/apps/srtop/demo/target/release/srtop-demo
[ -x "$demo" ] || die "no srtop-demo at $demo"

box_up() { # SRTOP_ARGS
  started=1
  SRTOP_ARGS=$1 SRTOP_DEVBOX_REV=$REV "$DEVBOX_DIR/devbox.sh" up >/dev/null
  docker exec "$NAME" sh -c 'echo "$SRTOP_ARGS"' >"$evidence/.args"
  [ "$(cat "$evidence/.args")" = "$1" ] || die "the box runs srtop with $(cat "$evidence/.args"), not $1"
  rm -f "$evidence/.args"
  echo "showcase-measure: box up with srtop $1"
}

showcase() { # MATRIX OUT ARGS...
  local matrix=$1 out=$2
  shift 2
  "$demo" showcase "$matrix" --docker "$docker" --container "$NAME" --port "$PORT" \
    --key "$STATE_DIR/id_ed25519" --known-hosts "$STATE_DIR/known_hosts" \
    --logs "$evidence/ssh-logs" --out "$evidence/$out" --interval-ms "$INTERVAL_MS" \
    --columns "$COLUMNS_FIXED" --lines "$LINES_FIXED" "$@"
}

box_up "--live-source --refresh-interval-ms $INTERVAL_MS"
label=$(docker image inspect -f '{{index .Config.Labels "org.srui.revision"}}' "$IMAGE")
# The exact environment. Versions and hardware only: no key, no known_hosts entry, no user name.
{
  echo "date_utc: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "mode: $MODE_NAME"
  echo "revision_under_test: $revision"
  echo "image: $IMAGE id $(docker image inspect -f '{{.Id}}' "$IMAGE") label org.srui.revision=$label"
  echo "image_bases: $(grep -E '^FROM ' "$DEVBOX_DIR/Dockerfile" | tr '\n' ' ')"
  echo "box_build_profile: release (cargo build --release --locked, apps/srtop/devbox/Dockerfile)"
  echo "box_kernel: $(docker exec "$NAME" uname -srm)"
  echo "box_os: $(docker exec "$NAME" sh -c '. /etc/os-release && echo "$PRETTY_NAME"') debian_version $(docker exec "$NAME" cat /etc/debian_version)"
  echo "box_packages: $(docker exec "$NAME" dpkg-query -W -f '${Package}=${Version} ' openssh-server procps htop)"
  echo "box_htop: $(docker exec "$NAME" htop --version | head -1)"
  echo "box_binaries: $(docker exec "$NAME" sha256sum /usr/local/bin/srtop /usr/local/bin/srui-ssh-bridge /usr/local/bin/srtop-demo /usr/local/bin/srtop-demo-worker | awk '{printf "%s=%s ", $2, $1}')"
  echo "box_cpus: $(docker exec "$NAME" nproc) box_memory_kib: $(docker exec "$NAME" awk '/MemTotal/ {print $2}' /proc/meminfo)"
  echo "docker: $(docker version -f 'client {{.Client.Version}} server {{.Server.Version}} {{.Server.KernelVersion}}') ncpu $(docker info -f '{{.NCPU}}') mem_bytes $(docker info -f '{{.MemTotal}}')"
  echo "mac_model: $(sysctl -n hw.model) cpu: $(sysctl -n machdep.cpu.brand_string) ncpu: $(sysctl -n hw.ncpu) mem_bytes: $(sysctl -n hw.memsize)"
  echo "macos: $(sw_vers -productVersion) ($(sw_vers -buildVersion)) $(uname -m)"
  echo "measuring_tool: apps/srtop/demo srtop-demo, release build, sha256 $(shasum -a 256 "$demo" | awk '{print $1}')"
  echo "rustc: $(rustc --version)"
  echo "client_ssh: $(ssh -V 2>&1)"
  echo "ssh_options: -F /dev/null, Ciphers=chacha20-poly1305@openssh.com, Compression=no, -T (srui subsystem) or -tt (htop)"
  echo "settings: interval_ms=$INTERVAL_MS htop_delay_ds=$((INTERVAL_MS / 100)) terminal=${COLUMNS_FIXED}x${LINES_FIXED} runs=$RUNS warmup_ms=$WARMUP_MS window_ms=$DURATION_MS rtt_runs=$RTT_RUNS rtt_window_ms=$RTT_DURATION_MS scale_runs=$SCALE_RUNS idle_window_ms=$IDLE_MS"
  echo "box_processes_at_rest: $(docker exec "$NAME" ps -e --no-headers | wc -l | tr -d ' ')"
} | sed 's/[[:space:]]*$//' >"$evidence/environment.txt"
cat "$evidence/environment.txt"

showcase compare compare.md --runs "$RUNS" --warmup-ms "$WARMUP_MS" --duration-ms "$DURATION_MS" \
  --workloads idle,busy,churn
showcase rtt rtt.md --runs "$RTT_RUNS" --warmup-ms "$RTT_WARMUP_MS" --duration-ms "$RTT_DURATION_MS" \
  --workloads busy --rtts 0,100,300,600
# Bounded workers: nothing a run started is left in the box.
left=$(docker exec "$NAME" ps -eo comm= | grep -c -E '^(srtop-demo|srtop-demo-work|sleep)$' || true)
echo "workers_left_after_all_runs: $left" >>"$evidence/environment.txt"
[ "$left" -eq 0 ] || die "$left workload processes are still running in the box"

box_up "--fake-source --smoke-fixture --refresh-interval-ms 100"
showcase idle idle.md --duration-ms "$IDLE_MS"

for rows in 1000 10000; do
  box_up "--scale-source $rows --scale-interval-ms $INTERVAL_MS"
  showcase scale "scale-$rows.md" --label "$rows" --runs "$SCALE_RUNS" --warmup-ms "$WARMUP_MS" \
    --duration-ms "$DURATION_MS"
done

# Everything this run wrote is audited, the ssh logs included.
audited=()
while IFS= read -r file; do audited+=("$file"); done < <(find "$evidence" -type f | sort)
"$R1_DIR/privacy-audit.sh" "${audited[@]}" || die "the privacy audit found something in $evidence"
echo "showcase-measure: evidence in $evidence"
