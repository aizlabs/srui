#!/usr/bin/env bash
# Fail if a Swift test spawns a child process that inherits this process's stdout/stderr.
#
# swift-test reads the test binary's stdout/stderr through pipes and only returns once every
# descriptor on the write end is closed. A spawned server that inherits those pipes and outlives
# the test binary therefore wedges the entire run *after* every test has already passed: no failing
# test, no output, just a silent stall. Recovering from it means killing the survivor by hand.
#
# The rule is uniform: between `let x = Process()` and `x.run()`, both `x.standardOutput` and
# `x.standardError` must be assigned. A process whose redirection happens in the shell instead
# (`sh -c 'exec cmd >/dev/null 2>&1'`, which is the only form that survives sshd's self-re-exec)
# opts out by annotating its own `run()` line with `stdio: detached`.
set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

scan_roots=(client-macos/Tests client-macos/Benchmarks)
for scan_root in "${scan_roots[@]}"; do
    if [ ! -d "$scan_root" ]; then
        echo "error: $scan_root does not exist" >&2
        exit 1
    fi
done

violations=$(
    find "${scan_roots[@]}" -name '*.swift' -print0 |
        xargs -0 awk '
        FNR == 1 { delete declared; delete out; delete err }

        # `let x = Process()` / `var x = Process()` opens a window for x.
        match($0, /(let|var)[ \t]+[A-Za-z_][A-Za-z0-9_]*[ \t]*=[ \t]*Process\(\)/) {
            s = substr($0, RSTART, RLENGTH)
            sub(/^(let|var)[ \t]+/, "", s)
            sub(/[ \t]*=[ \t]*Process\(\)$/, "", s)
            declared[s] = FNR
            out[s] = 0
            err[s] = 0
        }

        match($0, /[A-Za-z_][A-Za-z0-9_]*\.standardOutput[ \t]*=/) {
            s = substr($0, RSTART, RLENGTH)
            sub(/\.standardOutput[ \t]*=$/, "", s)
            out[s] = 1
        }

        match($0, /[A-Za-z_][A-Za-z0-9_]*\.standardError[ \t]*=/) {
            s = substr($0, RSTART, RLENGTH)
            sub(/\.standardError[ \t]*=$/, "", s)
            err[s] = 1
        }

        # `x.run()` closes the window and is the point the rule is enforced.
        match($0, /[A-Za-z_][A-Za-z0-9_]*\.run\(\)/) {
            s = substr($0, RSTART, RLENGTH)
            sub(/\.run\(\)$/, "", s)
            if (!(s in declared)) next               # not a Process we tracked
            if ($0 ~ /stdio:[ \t]*detached/) next    # redirection lives in the shell command
            missing = ""
            if (!out[s]) missing = "standardOutput"
            if (!err[s]) missing = (missing == "" ? "standardError" : missing " and standardError")
            if (missing != "") {
                printf "%s:%d: `%s` (declared line %d) runs without setting %s\n", \
                    FILENAME, FNR, s, declared[s], missing
            }
        }
    '
)

if [ -n "$violations" ]; then
    echo "$violations" >&2
    cat >&2 <<'EOF'

error: the processes above inherit the test binary's stdout/stderr.

If such a process outlives the test binary, swift-test blocks forever on read() with every test
already passed and no failure to point at. Assign both standardOutput and standardError (for
example to a FileHandle on /dev/null), or, when the redirection has to happen inside a shell
command, annotate the run() line with `// stdio: detached ...` explaining where.
EOF
    exit 1
fi

echo "No Swift test spawns a process that inherits the test binary's stdout/stderr."
