# Suite 2 — Widget semantic tests (§32 item 2)

**Status:** `active`  
**Spec sections:** §7.2, §7.3, §7.6, §32.2

Run this suite alone:

```bash
scripts/run-conformance --suite 2
```

## Fixtures

`widgets.generated.json` — **generated** from `protocol/registry.yaml` by `protocol/generate_conformance_matrix.py`. Do not edit by hand; run `./protocol/generate_proto.sh` and commit the result. CI fails on any diff.

## Runners

**Rust**

```bash
cargo test -p srui-semantic-tree --test conformance_widget_semantics_test
```

**Swift**

```bash
swift test --package-path client-macos --filter WidgetSemanticsConformanceTests
```

## Documented gaps

- **SHOULD-tier and deferred-tier widgets (Select, ChoiceGroup, Slider, NumberInput, Tabs, Split, Dialog, Menu, Toolbar) have no renderer implementation.**
  - *Why:* §7.3 marks these below the required tier; ControlFactory rejects them with unsupportedNodeType rather than degrading silently (§4 inv. 13). The suite asserts the rejection, which is the conformant behavior today.
  - *Owner:* none — tier is a deliberate scope boundary, not a defect
