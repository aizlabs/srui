# §31.1 parse/render benchmark

The macOS BenchmarkDriver loads the shared coding-agent fixture into a warmed AppKit process and
a warmed WKWebView. It reports first visible work, complete paint, process CPU time, allocator
live-block delta, resident peak, and representation bytes. Smoke mode stops at layout; full mode
also forces display. profile-allocations.sh records an Instruments Allocations trace for
attribution.
