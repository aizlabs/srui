# Suite 12 — Toolkit mapping tests (§32 item 12)

**Status:** `active`  
**Spec sections:** §22.4, §32.12

Run this suite alone:

```bash
scripts/run-conformance --suite 12
```

## Fixtures

`mappings.generated.json` — **generated** from `protocol/registry.yaml` by `protocol/generate_conformance_matrix.py`. Do not edit by hand; run `./protocol/generate_proto.sh` and commit the result. CI fails on any diff.

## Runners

**Rust**

```bash
cargo test -p srui-semantic-tree --test conformance_toolkit_mapping_test
```

**Swift**

```bash
swift test --package-path client-macos --filter ToolkitMappingConformanceTests
```
