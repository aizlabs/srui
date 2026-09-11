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
#
# `Process` is not the only route from Swift to fork/exec. A `posix_spawn` child inherits every
# descriptor unless the call is given a `posix_spawn_file_actions_t`, so a nil `file_actions`
# argument wedges the run exactly the same way while using none of the Foundation vocabulary the
# first pass matches on. The second pass reads each `posix_spawn`/`posix_spawnp` argument list --
# across however many lines it spans -- and rejects a nil third argument.
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
scan_posix_spawn_violations() {
    xargs -0 awk '
        function reset_call() {
            scanning = 0
            depth = 0
            argc = 0
            detached = 0
            delete args
        }

        function reset_lexer() {
            block_comment_depth = 0
            in_string = 0
            in_multiline_string = 0
            raw_string_hashes = 0
            raw_multiline_string = 0
            escaped = 0
        }

        # Strip comments and string contents before recognizing call syntax. This keeps
        # look-alike text inert while preserving delimiters in actual Swift expressions.
        function code_only(text, output, i, c, pair, triple, closing, hash_index, raw_start, raw_count) {
            output = ""
            for (i = 1; i <= length(text); i++) {
                c = substr(text, i, 1)
                pair = substr(text, i, 2)
                triple = substr(text, i, 3)
                if (block_comment_depth > 0) {
                    if (pair == "/*") {
                        block_comment_depth++
                        i++
                    } else if (pair == "*/") {
                        block_comment_depth--
                        i++
                    }
                    continue
                }
                if (raw_string_hashes > 0) {
                    closing = raw_multiline_string ? "\"\"\"" : "\""
                    for (hash_index = 0; hash_index < raw_string_hashes; hash_index++) {
                        closing = closing "#"
                    }
                    if (substr(text, i, length(closing)) == closing) {
                        raw_string_hashes = 0
                        raw_multiline_string = 0
                        i += length(closing) - 1
                    }
                    continue
                }
                if (in_multiline_string) {
                    if (triple == "\"\"\"") {
                        in_multiline_string = 0
                        i += 2
                    }
                    continue
                }
                if (in_string) {
                    if (escaped) {
                        escaped = 0
                    } else if (c == "\\") {
                        escaped = 1
                    } else if (c == "\"") {
                        in_string = 0
                    }
                    continue
                }
                if (pair == "//") break
                if (pair == "/*") {
                    block_comment_depth = 1
                    output = output " "
                    i++
                } else if (c == "#") {
                    raw_start = i
                    while (substr(text, raw_start, 1) == "#") raw_start++
                    raw_count = raw_start - i
                    if (substr(text, raw_start, 3) == "\"\"\"") {
                        raw_string_hashes = raw_count
                        raw_multiline_string = 1
                        output = output "\"\""
                        i = raw_start + 2
                    } else if (substr(text, raw_start, 1) == "\"") {
                        raw_string_hashes = raw_count
                        raw_multiline_string = 0
                        output = output "\"\""
                        i = raw_start
                    } else {
                        output = output c
                    }
                } else if (triple == "\"\"\"") {
                    in_multiline_string = 1
                    output = output "\"\""
                    i += 2
                } else if (c == "\"") {
                    in_string = 1
                    escaped = 0
                    output = output "\"\""
                } else {
                    output = output c
                }
            }
            return output
        }

        FNR == 1 {
            reset_call()
            reset_lexer()
        }

        {
            line = code_only($0)
            if (scanning && $0 ~ /stdio:[ 	]*detached/) detached = 1
            while (length(line) > 0) {
                if (!scanning) {
                    # The leading boundary excludes fake_posix_spawn and other identifiers.
                    if (match(line, /(^|[^A-Za-z0-9_])posix_spawnp?[ 	]*\(/) == 0) break
                    start_line = FNR
                    line = substr(line, RSTART + RLENGTH)
                    scanning = 1
                    depth = 1
                    argc = 0
                    args[0] = ""
                    detached = $0 ~ /stdio:[ 	]*detached/
                }

                completed = 0
                for (i = 1; i <= length(line); i++) {
                    c = substr(line, i, 1)
                    if (c == "(" || c == "[" || c == "{") {
                        depth++
                    } else if (c == ")" || c == "]" || c == "}") {
                        depth--
                        if (depth == 0) {
                            actions = args[2]
                            gsub(/^[ 	]+|[ 	]+$/, "", actions)
                            if (!detached && argc >= 2 && actions == "nil") {
                                printf "%s:%d: posix_spawn passes nil file_actions, so the child inherits fds 0/1/2\n", \
                                    FILENAME, start_line
                            }
                            reset_call()
                            line = substr(line, i + 1)
                            completed = 1
                            break
                        }
                    }
                    if (c == "," && depth == 1) {
                        argc++
                        args[argc] = ""
                        continue
                    }
                    args[argc] = args[argc] c
                }
                if (!completed) {
                    if (scanning) args[argc] = args[argc] " "
                    break
                }
            }
        }
    '
}

run_posix_spawn_scanner_self_test() (
    test_directory=$(mktemp -d "${TMPDIR:-/tmp}/srui-stdio-scanner.XXXXXX")
    trap 'rm -rf -- "$test_directory"' EXIT
    cases="$test_directory/cases.swift"
    cat >"$cases" <<'SWIFT'
func scannerCases() {
    // posix_spawn(&pid, path, nil, nil, argv, envp)
    let source = "posix_spawn(&pid, path, nil, nil, argv, envp)"
    fake_posix_spawn(&pid, path, nil, nil, argv, envp)
    let raw = #"quoted " posix_spawn(&pid, path, nil, nil, argv, envp)"#
    let rawMultiline = #"""
    posix_spawn(&pid, path, nil, nil, argv, envp)
    """#
    let multiline = """
    posix_spawn(&pid, path, nil, nil, argv, envp)
    """
    /*
     posix_spawnp(&pid, path, nil, nil, argv, envp)
     */
    posix_spawn(
        &pid,
        path,
        nil,
        nil,
        argv,
        envp
    )
    posix_spawnp(
        &pid,
        path,
        nil,
        nil,
        argv,
        envp
    )
    posix_spawn(
        &pid,
        path,
        &actions,
        nil,
        argv,
        envp
    )
    posix_spawn(&pid, path, &actions, nil, argv, envp); posix_spawnp(&pid, path, nil, nil, argv, envp)
}
SWIFT

    actual=$(printf '%s\0' "$cases" | scan_posix_spawn_violations)
    expected=$(printf \
        '%s:15: posix_spawn passes nil file_actions, so the child inherits fds 0/1/2\n%s:23: posix_spawn passes nil file_actions, so the child inherits fds 0/1/2\n%s:39: posix_spawn passes nil file_actions, so the child inherits fds 0/1/2' \
        "$cases" "$cases" "$cases")
    if [[ "$actual" != "$expected" ]]; then
        echo "error: posix_spawn scanner self-test failed" >&2
        printf 'expected:\n%s\nactual:\n%s\n' "$expected" "$actual" >&2
        exit 1
    fi
)

if [[ $# -gt 1 || ($# -eq 1 && $1 != "--self-test") ]]; then
    echo "usage: $0 [--self-test]" >&2
    exit 2
fi
run_posix_spawn_scanner_self_test
if [[ ${1:-} == "--self-test" ]]; then
    echo "posix_spawn scanner self-test passed."
    exit 0
fi

spawn_violations=$(
    rg --files-with-matches --null --glob '*.swift' \
        'posix_spawnp?[[:space:]]*\(' "${scan_roots[@]}" |
        scan_posix_spawn_violations
)

if [ -n "$violations" ] || [ -n "$spawn_violations" ]; then
    if [ -n "$violations" ]; then
        echo "$violations" >&2
    fi
    if [ -n "$spawn_violations" ]; then
        echo "$spawn_violations" >&2
    fi
    cat >&2 <<'EOF'

error: the processes above inherit the test binary's stdout/stderr.

If such a process outlives the test binary, swift-test blocks forever on read() with every test
already passed and no failure to point at. Assign both standardOutput and standardError (for
example to a FileHandle on /dev/null), or, when the redirection has to happen inside a shell
command, annotate the run() line with `// stdio: detached ...` explaining where.

A posix_spawn child needs a posix_spawn_file_actions_t that opens /dev/null onto STDIN_FILENO,
STDOUT_FILENO and STDERR_FILENO before the spawn; passing nil for file_actions inherits all three.
EOF
    exit 1
fi

echo "No Swift test spawns a process that inherits the test binary's stdout/stderr."
