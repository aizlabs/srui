#!/usr/bin/env bash
# Parse-only Swift syntax gate (CI job `swift-parse`).
#
# This is syntax-only. `swiftc -frontend -parse` does not type-check, resolve
# modules, link, or validate Apple-framework APIs. It exists so Linux CI can
# reject reserved identifiers (for example a parameter named `class`) and
# invalid literals (for example `200u32`) without a macOS SDK.
#
# Generated sources are excluded. Changed-file selection is merge-base through
# HEAD for pull requests (base SHA...head SHA).
set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

if ! command -v swiftc >/dev/null 2>&1; then
    echo "error: swiftc is not on PATH; install Swift 6 to run the parse gate." >&2
    exit 1
fi

is_generated_swift() {
    case "$1" in
        *.pb.swift) return 0 ;;
        *.generated.swift) return 0 ;;
        */RegistryTables.swift|RegistryTables.swift) return 0 ;;
    esac
    return 1
}

emit_if_handwritten() {
    local path=$1
    [ -n "$path" ] || return 0
    if is_generated_swift "$path"; then
        return 0
    fi
    if [ -f "$path" ]; then
        printf '%s\n' "$path"
    fi
}

list_all_handwritten() {
    git ls-files '*.swift' | while IFS= read -r path; do
        emit_if_handwritten "$path"
    done
}

list_changed_handwritten() {
    local base=$1
    local head=$2
    # Committed PR range (merge-base through head).
    git diff --name-only --diff-filter=ACMR "${base}...${head}" -- '*.swift' \
        | while IFS= read -r path; do emit_if_handwritten "$path"; done
    # Tracked working-tree edits (empty on a clean CI checkout).
    git diff --name-only --diff-filter=ACMR HEAD -- '*.swift' \
        | while IFS= read -r path; do emit_if_handwritten "$path"; done
    # Untracked files. Pathspec globs do not recurse for `--others`.
    # `grep` exits 1 when nothing matches, which `pipefail` would turn into a gate failure.
    git ls-files --others --exclude-standard \
        | { grep '\.swift$' || true; } \
        | while IFS= read -r path; do emit_if_handwritten "$path"; done
}

base=${SRUI_SWIFT_PARSE_BASE:-${1:-}}
head=${SRUI_SWIFT_PARSE_HEAD:-${2:-HEAD}}

if [ -z "$base" ]; then
    if git rev-parse --verify origin/main >/dev/null 2>&1; then
        base=$(git merge-base origin/main "$head")
    elif git rev-parse --verify main >/dev/null 2>&1; then
        base=$(git merge-base main "$head")
    fi
fi

echo "Swift parse gate: syntax-only (no type checking, module resolution, linking, or Apple SDK validation)."
echo "swiftc: $(command -v swiftc)"
swiftc --version | sed 's/^/  /'

# The selection goes through a temp file rather than `mapfile`: macOS ships bash 3.2, which has
# no such builtin, and `.githooks/pre-push` runs this script with `bash`.
selected=$(mktemp "${TMPDIR:-/tmp}/srui-swift-parse.XXXXXX")
trap 'rm -f "$selected"' EXIT

if [ -n "$base" ]; then
    echo "Range: ${base}...${head} (merge-base through head)"
    list_changed_handwritten "$base" "$head" | sort -u > "$selected"
else
    echo "No merge base; parsing every tracked hand-written Swift file."
    list_all_handwritten | sort -u > "$selected"
fi

if [ ! -s "$selected" ]; then
    echo "No changed hand-written Swift files."
    exit 0
fi

fail=0
count=0
while IFS= read -r path; do
    [ -n "$path" ] || continue
    count=$((count + 1))
    printf 'parse %s\n' "$path"
    # stdin is the file list; keep swiftc from consuming it.
    if ! swiftc -frontend -parse "$path" </dev/null; then
        printf 'FAILED: parser rejected %s\n' "$path" >&2
        fail=1
    fi
done < "$selected"

if [ "$fail" -ne 0 ]; then
    echo "Swift parse gate failed." >&2
    exit 1
fi

echo "Parsed ${count} hand-written Swift file(s)."
