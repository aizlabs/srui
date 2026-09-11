# Coding Agent Demo Example

Task 31 composes the required Standard Widget tier, a model-backed project-file view, native
`TEXT_EDIT` handling, a real PTY Terminal, and an unsupported extension carrying a Standard Widget
fallback (§11.1, §30).

The placeholder profile is `org.example.diff/1`; it deliberately does not claim to implement the
future `org.srui.coding/1` profile. Its one ordered child is the root of a namespace-0-only fallback
subtree. A base client renders that subtree, while a client with a registered extension renderer
suppresses it.

Run the in-process interaction demonstration:

```bash
cargo run --manifest-path examples/coding-agent-demo/Cargo.toml
```

Host the application over a Unix socket:

```bash
cargo run --manifest-path examples/coding-agent-demo/Cargo.toml -- \
  --socket /tmp/srui-coding-agent.sock
```

The complete graph is bootstrapped before the listener accepts clients. Revision 1 creates the
model and non-terminal tree, revision 2 atomically spawns/inserts Terminal through sessiond, and
revision 3 appends the action row after Terminal. Fresh clients therefore receive one coherent
revision-3 snapshot.
