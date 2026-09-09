# §31.4 network and local-interaction benchmark

The macOS driver injects real 0, 100, 300, and 600 ms waits into the server-dependent path while
timing native local interaction paths independently. It covers text entry, caret movement, text
selection, marked-text IME composition, scrolling, hover/pressed feedback, and local menu
preparation. It also exercises a 1 MiB/s bandwidth limit, a deterministic lost-frame retry, and a
controlled interruption.

Every local interaction is compared with the runtime `display.frame_budget` target: one frame at
the active display's measured refresh rate (for example, 16.67 ms at 60 Hz or 8.33 ms at 120 Hz),
with a documented 60 Hz fallback when the host does not report a refresh rate. The largest latency
delta relative to zero RTT is reported explicitly.
