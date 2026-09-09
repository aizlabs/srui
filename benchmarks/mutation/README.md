# §31.3 mutation and frame-independence benchmark

A preloaded AppKit renderer receives transactions containing 1, 100, and 1,000 scalar value
updates. The report separates semantic decode/apply from decode-to-visible layout latency and
compares the former with §23 targets.

One fixed 24-message mutation stream is then replayed at synthetic 60, 120, 144, and 240 Hz
presentation groupings. Exact bytes and message counts must match at every cadence, repaint counts
must vary, and final native control state plus dirty classification must remain identical.
