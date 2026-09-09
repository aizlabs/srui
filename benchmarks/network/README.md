# §31.4 network and local-interaction benchmark

The macOS driver injects real 0, 100, 300, and 600 ms waits into the server-dependent path while
timing native local interaction paths independently. It covers text entry, caret movement, text
selection, marked-text IME composition, scrolling, hover/pressed feedback, and local menu
preparation. It also exercises a 1 MiB/s bandwidth limit, a deterministic lost-frame retry, and a
controlled interruption.

Every local interaction has a one-frame 16.67 ms reporting target, and the largest latency delta
relative to zero RTT is reported explicitly.
