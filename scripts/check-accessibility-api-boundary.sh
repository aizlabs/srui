#!/usr/bin/env bash
# Enforce the AppKit-free semantic inspection boundary (§22.9, §32.11).
set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
package_root="$repo_root/client-macos"
scratch=$(mktemp -d "${TMPDIR:-/tmp}/srui-accessibility-boundary.XXXXXX")
trap 'rm -rf "$scratch"' EXIT

package_description="$scratch/package-description.json"
target_info="$scratch/target-info.json"
symbol_dir="$scratch/symbols"

swift package --package-path "$package_root" describe --type json > "$package_description"
swift -print-target-info > "$target_info"

target_triple=$(
    python3 - "$package_description" "$target_info" "$package_root" <<'PY'
import json
import re
import sys
from pathlib import Path

description_path = Path(sys.argv[1])
target_info_path = Path(sys.argv[2])
package_root = Path(sys.argv[3])

description = json.loads(description_path.read_text(encoding="utf-8"))
target_info = json.loads(target_info_path.read_text(encoding="utf-8"))
targets = {target["name"]: target for target in description.get("targets", [])}


def fail(message: str) -> None:
    raise SystemExit(f"error: {message}")


def target(name: str) -> dict:
    value = targets.get(name)
    if value is None:
        fail(f"Swift package does not declare target {name}")
    return value


accessibility = target("Accessibility")
accessibility_dependencies = set(accessibility.get("target_dependencies", []))
accessibility_products = set(accessibility.get("product_dependencies", []))
if accessibility_dependencies != {"SemanticModel"} or accessibility_products:
    fail(
        "Accessibility must depend only on SemanticModel; found target dependencies "
        f"{sorted(accessibility_dependencies)} and product dependencies "
        f"{sorted(accessibility_products)}"
    )

session_dependencies = set(target("Session").get("target_dependencies", []))
if "Accessibility" not in session_dependencies:
    fail("Session must depend on Accessibility")

test_dependencies = set(target("AccessibilityTests").get("target_dependencies", []))
if test_dependencies != {"Accessibility", "SemanticModel"}:
    fail(
        "AccessibilityTests must depend exactly on Accessibility and SemanticModel; found "
        f"{sorted(test_dependencies)}"
    )

forbidden_modules = {"AppKit", "Cocoa", "RendererAppKit"}
import_pattern = re.compile(
    r"^\s*(?:@\w+(?:\([^)]*\))?\s+)*import\s+"
    r"(?:(?:typealias|struct|class|enum|protocol|let|var|func)\s+)?"
    r"(?P<module>[A-Za-z_][A-Za-z0-9_]*)",
    re.MULTILINE,
)
violations: list[str] = []
source_root = package_root / accessibility["path"]
for source in accessibility.get("sources", []):
    source_path = source_root / source
    text = source_path.read_text(encoding="utf-8")
    for match in import_pattern.finditer(text):
        module = match.group("module")
        if module in forbidden_modules:
            line = text.count("\n", 0, match.start()) + 1
            violations.append(f"{source_path.relative_to(package_root)}:{line}: import {module}")

if violations:
    fail(
        "Accessibility source imports forbidden toolkit module(s):\n  "
        + "\n  ".join(sorted(violations))
    )

platforms = {
    platform["name"]: platform["version"]
    for platform in description.get("platforms", [])
}
minimum_macos = platforms.get("macos")
unversioned_triple = target_info.get("target", {}).get("unversionedTriple")
if not minimum_macos or not unversioned_triple:
    fail("could not derive the Swift compiler target and package macOS deployment version")

print(f"{unversioned_triple}{minimum_macos}")
PY
)

mkdir -p "$symbol_dir"
bin_path=$(swift build \
    --package-path "$package_root" \
    --target Accessibility \
    --show-bin-path)
sdk_path=$(xcrun --sdk macosx --show-sdk-path)

swift symbolgraph-extract \
    -module-name Accessibility \
    -I "$bin_path/Modules" \
    -target "$target_triple" \
    -sdk "$sdk_path" \
    -minimum-access-level public \
    -output-dir "$symbol_dir"

python3 - "$symbol_dir" <<'PY'
import json
import sys
from pathlib import Path

symbol_dir = Path(sys.argv[1])
graphs = sorted(symbol_dir.glob("Accessibility*.symbols.json"))
if not graphs:
    raise SystemExit("error: symbol extraction produced no Accessibility symbol graph")

forbidden = ("AppKit", "Cocoa", "RendererAppKit", "NSView")
violations: list[str] = []


def without_prose(value):
    if isinstance(value, dict):
        return {
            key: without_prose(child)
            for key, child in value.items()
            if key not in {"docComment", "location"}
        }
    if isinstance(value, list):
        return [without_prose(child) for child in value]
    return value


for graph_path in graphs:
    graph = json.loads(graph_path.read_text(encoding="utf-8"))
    if graph.get("module", {}).get("name") != "Accessibility":
        raise SystemExit(
            f"error: unexpected module in {graph_path.name}: "
            f"{graph.get('module', {}).get('name')!r}"
        )

    for symbol in graph.get("symbols", []):
        public_shape = json.dumps(without_prose(symbol), sort_keys=True)
        hits = sorted({name for name in forbidden if name in public_shape})
        if hits:
            title = symbol.get("names", {}).get(
                "title", symbol.get("identifier", {}).get("precise", "<unknown>")
            )
            violations.append(f"{title}: {', '.join(hits)}")

    for relationship in graph.get("relationships", []):
        public_shape = json.dumps(without_prose(relationship), sort_keys=True)
        hits = sorted({name for name in forbidden if name in public_shape})
        if hits:
            source = relationship.get("source", "<unknown>")
            target = relationship.get("target", "<unknown>")
            violations.append(f"relationship {source} -> {target}: {', '.join(hits)}")

if violations:
    raise SystemExit(
        "error: Accessibility public API exposes forbidden toolkit identity/identities:\n  "
        + "\n  ".join(sorted(set(violations)))
    )

print("Accessibility dependency, source-import, and public-symbol boundaries are valid.")
PY
