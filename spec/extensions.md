# SRUI Extension Profiles Specification

Authoritative semantics live in
[SRUI_Semantic_Remote_UI_Design_v0.6.md](../SRUI_Semantic_Remote_UI_Design_v0.6.md)
§11 and §21. This note records the v1 Terminal wire contract that is not a
Namespace 0 registry entry.

## Terminal (`org.srui.terminal/1`)

- Negotiated extension profile, not a standard widget.
- Local type ID `1` within the session-assigned namespace means `Terminal`.
- The numeric namespace is allocated per session and advertised in
  `ServerWelcome.extension_namespaces`. Clients must never assume it is `1`.
- The terminal stream ID equals the Terminal semantic node's `NodeId`.
- A Terminal node is a semantic leaf. PTY bytes stay opaque to the semantic
  model.
- A session that emits a Terminal node marks the profile required: Task 30 has
  no semantic fallback.
- Offset, resync, and limit rules are documented in
  [`protocol/README.md`](../protocol/README.md) and `protocol/srui.proto`.
- v1 does not automatically depend on `tmux`. Deployments may configure `tmux`
  as the spawned command; there is no automatic redraw backend after the
  output ring is lost.
