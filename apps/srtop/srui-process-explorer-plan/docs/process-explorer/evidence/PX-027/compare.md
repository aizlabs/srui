Steady window 15 s after a 3 s warm-up; sampling 1000 ms (srtop `--refresh-interval-ms 1000`, htop `-d 10`); htop on a 120x40 terminal; 7 runs per side per workload. Bytes per second; min / median / p95 / max.

| Workload | Side | Processes | Payload down B/s | Encrypted down B/s | SSH overhead down B/s | Payload up B/s | Encrypted up B/s | Startup payload B | Startup encrypted B | Attach ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| idle | SRUI (srtop) | 3 / 3 / 3 / 3 | 303 / 318 / 340 / 340 | 343 / 360 / 381 / 381 | 41 / 41 / 52 / 52 | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 | 11438 / 11484 / 11572 / 11572 | 19131 / 19231 / 19311 / 19311 | 767.1 / 848.4 / 1554.4 / 1554.4 |
| idle | htop over ssh -tt | 3 / 3 / 3 / 3 | 562 / 611 / 689 / 689 | 650 / 715 / 797 / 797 | 81 / 103 / 115 / 115 | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 | 6910 / 7322 / 7598 / 7598 | 14979 / 15675 / 15819 / 15819 | 749.9 / 815.1 / 907.9 / 907.9 |
| busy | SRUI (srtop) | 208 / 208 / 208 / 208 | 440 / 471 / 511 / 511 | 517 / 545 / 583 / 583 | 70 / 74 / 78 / 78 | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 | 21977 / 22086 / 22558 / 22558 | 29815 / 29887 / 30419 / 30419 | 730.2 / 857.8 / 948.5 / 948.5 |
| busy | htop over ssh -tt | 208 / 208 / 208 / 208 | 993 / 1185 / 1271 / 1271 | 1140 / 1326 / 1439 / 1439 | 115 / 147 / 180 / 180 | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 | 9367 / 10369 / 10594 / 10594 | 17683 / 18839 / 19027 / 19027 | 856.1 / 891.0 / 1145.2 / 1145.2 |
| churn | SRUI (srtop) | 210 / 210 / 210 / 210 | 576 / 602 / 618 / 618 | 646 / 670 / 691 / 691 | 62 / 71 / 73 / 73 | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 | 22405 / 22629 / 23126 / 23126 | 30239 / 30463 / 30967 / 30967 | 673.8 / 796.6 / 1021.7 / 1021.7 |
| churn | htop over ssh -tt | 210 / 210 / 210 / 210 | 1144 / 1215 / 1387 / 1387 | 1284 / 1386 / 1552 / 1552 | 132 / 160 / 176 / 176 | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 | 9522 / 9913 / 10943 / 10943 | 17819 / 18275 / 19415 / 19415 | 737.7 / 767.4 / 913.9 / 913.9 |

Median encrypted bytes per second, both directions, and which side is larger:

| Workload | SRUI rows | SRUI encrypted B/s | htop encrypted B/s | Larger | Ratio SRUI/htop | Workers gone after (max ms) |
| --- | --- | --- | --- | --- | --- | --- |
| idle | 5 / 5 / 5 / 5 | 360 | 715 | htop | 0.50 | 795 |
| busy | 210 / 210 / 210 / 210 | 545 | 1326 | htop | 0.41 | 7448 |
| churn | 212 / 212 / 212 / 212 | 670 | 1386 | htop | 0.48 | 7597 |
