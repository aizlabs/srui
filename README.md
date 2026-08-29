# SRUI — Semantic Remote UI

SRUI (Semantic Remote UI) is a platform-neutral protocol that replicates application UI meaning and authoritative state to a native local renderer instead of remotely painting pixels. Complete architectural specifications, layer boundaries, and design invariants are defined in the authoritative design document [SRUI_Semantic_Remote_UI_Design_v0.5.md](SRUI_Semantic_Remote_UI_Design_v0.5.md).

---

## Why SRUI?

Traditional remote access mechanisms force a tradeoff between visual fidelity, latency, bandwidth, and user experience:

* **vs. Chrome Remote Desktop (WebRTC Video Streaming):** Chrome Remote Desktop captures the host screen and streams compressed video (VP8/VP9/AV1) over WebRTC. This incurs continuous video encoding/decoding overhead, high bandwidth scaling with display resolution and framerate, compression artifacts on fine typography, and noticeable input lag over WAN. SRUI transmits lightweight semantic state mutations instead of video frames, enabling local rendering with pixel-perfect typography, zero-latency local input handling (IME, text selection, scrolling), and minimal bandwidth.
* **vs. Windows RDP / VNC (Framebuffer Remoting):** RDP and VNC continuously stream framebuffers, video tiles, or dirty pixel rectangles. This approach is bandwidth-heavy, tied to remote display refresh rates (60/120 Hz), and disconnects users from native local OS conventions. SRUI synchronizes abstract UI intent and state, allowing the client to render 100% native platform controls (e.g., AppKit, WinUI, GTK) with native theming and OS-level accessibility integration.
* **vs. X11 Forwarding (Primitive Drawing):** X11 remotes low-level drawing primitives through a chatty, synchronous request-response protocol that degrades severely over WAN networks. Furthermore, X11 applications look alien on modern OSes, cannot persist sessions across network disconnects, and expose broad security attack surfaces. SRUI provides asynchronous state synchronization, full session reconnection over SSH, and renders through platform-native toolkits (e.g., AppKit, WinUI, GTK).
* **vs. Classic Terminals & TUIs (ANSI / VT100 / SSH):** Terminals are constrained to 2D monospaced character grids with brittle screen diffing and rudimentary accessibility. SRUI provides modern rich controls (tables, trees, forms, vector scenes, split panes) with first-class accessibility trees and structured event contracts, while retaining backward compatibility for CLI tools through an optional Terminal extension node.

---

## Feature Matrix

| Feature / Dimension | **SRUI** (Semantic Remote UI) | **Chrome Remote Desktop** | **Windows RDP / VNC** | **X11 Forwarding** | **Terminal / SSH / TUI** |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **Transmission Model** | Replicated semantic UI state & atomic mutations | WebRTC video stream (VP8/VP9/AV1) | Framebuffers / video pixel stream & dirty rects | Low-level 2D draw commands & window events | 2D character grid with ANSI escape sequences |
| **Bandwidth Consumption** | **Minimal** — proportional to state changes only (frame-independent) | **High** — continuous video stream (scales with resolution, FPS, motion) | **High** — scales with resolution, frame rate, and visual complexity | **Moderate to High** — chatty synchronous round-trips | **Extremely Low** — raw text streams |
| **Interaction Latency (Typing, Scroll, IME)** | **Zero-latency local loop** (caret, selection, IME, inertial scroll handled client-side) | Delayed by video encoding/decoding & network RTT | Delayed by network RTT (remote cursor / frame round-trip) | High latency; blocking round-trips over WAN | Local character echo; complex TUI screen redraws lag over RTT |
| **Native Look & Feel** | **100% Platform-native** (macOS AppKit, Windows WinUI, Linux GTK) | Foreign guest OS pixels inside a browser tab / canvas | Foreign guest OS pixels inside a container window | Foreign X11 toolkit widgets; ignores OS gestures/themes | Fixed terminal font grid; no native desktop widgets |
| **Session Resilience & Reconnect** | **Built-in** (authoritative server session & transaction journal) | Reconnects to host daemon session | Server session support, but heavy reconnect payload | **None** (network drop terminates client UI) | Requires `tmux`/`screen`; unformatted terminal buffer |
| **Accessibility (A11y)** | **First-class by design** (native OS accessibility tree populated directly) | Canvas/video stream with limited browser accessibility translation | Visual OCR or pass-through to host accessibility APIs | Rudimentary / non-standard across platforms | Limited to terminal screen readers |
| **Security & Sandbox Model** | **Zero client bytecode execution**; thin state renderer over SSH | WebRTC encrypted, but full desktop OS exposed | Full remote desktop access & large protocol attack surface | Broad X11 protocol access and device snooping vulnerabilities | Secure (SSH), but limited to text execution |
| **UI Expressiveness** | Rich widgets, trees, tables, native forms, vector scenes, and embedded terminal | Full desktop GUI (video stream) | Full desktop GUI (pixels) | Full desktop GUI (draw calls) | Monospaced text characters & ANSI colors only |
