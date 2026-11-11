# memprobe results

Cells: Memory (private working set, what Task Manager shows) / working set / private bytes, in MB.
p95: frame CPU time in ms, 95th percentile of the frames between marks ("-" = no frames drawn).

| Date | Label | Mode | Modules | 10 s | 30 s | 120 s | Browsed | p95 (ms) |
|---|---|---|---|---|---|---|---|---|
| 2026-11-11 04:07 | baseline: thin LTO, unwind | demo | all | 100.9 / 163.9 / 175.3 | 101.4 / 164.3 / 175.6 | 101.2 / 164.2 / 175.0 | - | 174.2 / 2.5 / 3.2 |
| 2026-11-11 04:09 | baseline: thin LTO, unwind | demo | default | 46.8 / 94.3 / 117.7 | 50.0 / 97.7 / 121.5 | 49.9 / 97.7 / 120.9 | - | 67.5 / 2.6 / - |
| 2026-11-11 04:12 | baseline: thin LTO, unwind | demo-big | all | 66.7 / 115.1 / 140.0 | 67.3 / 115.4 / 140.6 | 67.0 / 115.3 / 141.1 | - | 5.4 / 7.7 / 8.0 |
| 2026-11-11 04:14 | baseline: thin LTO, unwind | demo-big | default | 64.8 / 107.1 / 135.5 | 65.3 / 113.4 / 138.1 | 64.6 / 113.1 / 138.9 | - | 7.0 / 8.9 / 6.8 |
| 2026-11-11 04:16 | baseline: thin LTO, unwind | live 1569296665167536269 | (own settings) | 66.6 / 109.1 / 132.9 | 66.5 / 109.1 / 132.8 | 66.0 / 109.0 / 133.4 | - | - / - / 5.6 |
| 2026-11-11 04:18 | baseline, visible window | demo (visible) | all | 52.5 / 99.6 / 121.3 | 51.4 / 99.3 / 121.3 | 49.9 / 98.1 / 119.0 | - | 3.3 / 4.3 / 3.3 |
| 2026-11-11 04:20 | baseline, visible window | demo-big (visible) | all | 53.2 / 96.3 / 101.4 | 53.8 / 96.7 / 102.1 | 53.9 / 97.0 / 102.1 | - | 2.1 / 2.3 / 2.7 |
| 2026-11-11 04:22 | baseline: thin LTO, unwind (repeat) | demo | all | 41.4 / 83.8 / 87.1 | 41.4 / 84.0 / 87.1 | 41.5 / 84.3 / 87.6 | - | 1.0 / 4.0 / 1.2 |
| 2026-11-11 04:24 | panic=abort, thin LTO | demo | all | 86.2 / 145.2 / 134.1 | 85.4 / 144.6 / 133.1 | 86.5 / 145.7 / 134.4 | - | 163.2 / 4.4 / 1.2 |
| 2026-11-11 04:27 | panic=abort, thin LTO | demo | default | 36.8 / 78.8 / 83.0 | 36.8 / 79.0 / 83.0 | 36.7 / 78.9 / 82.8 | - | 0.8 / 0.8 / - |
| 2026-11-11 04:29 | panic=abort, thin LTO | demo-big | all | 49.5 / 91.6 / 97.1 | 53.5 / 95.9 / 101.1 | 54.1 / 96.6 / 101.8 | - | 2.1 / 2.7 / 2.3 |
| 2026-11-11 04:31 | panic=abort, thin LTO | demo-big | default | 53.3 / 95.5 / 101.3 | 53.3 / 95.8 / 101.3 | 53.3 / 96.0 / 101.6 | - | 1.1 / 1.5 / 2.3 |
| 2026-11-11 04:33 | panic=abort, fat LTO | demo | all | 90.8 / 138.3 / 137.9 | 90.9 / 138.4 / 137.9 | 91.4 / 139.2 / 138.4 | - | 0.8 / 4.1 / 1.2 |
| 2026-11-11 04:35 | panic=abort, fat LTO | demo | default | 37.1 / 78.6 / 82.2 | 37.1 / 78.9 / 82.2 | 37.2 / 79.2 / 82.0 | - | 0.9 / 0.9 / 5.1 |
| 2026-11-11 04:37 | panic=abort, fat LTO | demo-big | all | 55.0 / 96.8 / 102.0 | 37.8 / 81.3 / 102.0 | 38.0 / 81.6 / 102.0 | - | 0.6 / 2.2 / 3.1 |
| 2026-11-11 04:39 | panic=abort, fat LTO | demo-big | default | 49.2 / 91.0 / 95.3 | 49.2 / 91.3 / 95.3 | 49.2 / 91.5 / 95.3 | - | 1.4 / 3.2 / 0.7 |
| 2026-11-11 04:42 | panic=abort, thin LTO + mimalloc | demo | all | 94.8 / 153.9 / 160.2 | 91.8 / 151.1 / 148.9 | 91.3 / 150.6 / 149.6 | - | 1.2 / 4.7 / 1.4 |
| 2026-11-11 04:44 | panic=abort, thin LTO + mimalloc | demo | default | 46.0 / 88.2 / 109.7 | 42.7 / 84.9 / 98.3 | 42.1 / 84.4 / 98.1 | - | 0.5 / 0.9 / - |
| 2026-11-11 04:46 | panic=abort, thin LTO + mimalloc | demo-big | all | 175.6 / 217.8 / 236.3 | 78.8 / 121.1 / 137.2 | 74.2 / 116.9 / 133.8 | - | 1.1 / 1.8 / 3.3 |
| 2026-11-11 04:48 | panic=abort, thin LTO + mimalloc | demo-big | default | 175.7 / 218.0 / 235.0 | 78.8 / 121.3 / 135.9 | 78.6 / 121.2 / 135.7 | - | 0.8 / 1.8 / - |
| 2026-11-11 04:50 | panic=abort, thin LTO + mimalloc | live 1569296665167536269 | (own settings) | 101.8 / 143.3 / 302.5 | 76.8 / 118.2 / 200.4 | 76.8 / 118.3 / 200.3 | - | - / 1.7 / - |
| 2026-11-11 04:52 | baseline: thin LTO, unwind (repeat) | demo | all | 41.5 / 84.0 / 86.0 | 41.9 / 84.5 / 86.5 | 41.9 / 84.8 / 86.3 | - | 0.7 / 3.6 / 1.6 |
| 2026-11-11 04:54 | baseline: thin LTO, unwind (repeat) | demo-big | all | 53.1 / 95.8 / 102.0 | 65.2 / 113.5 / 132.8 | 65.7 / 114.4 / 115.4 | - | 1.1 / 4.2 / 1.3 |
| 2026-11-11 04:56 | panic=abort, thin LTO (repeat) | demo | all | 37.1 / 79.0 / 82.4 | 37.5 / 79.6 / 82.5 | 41.6 / 83.9 / 86.7 | - | 1.6 / 1.5 / 1.2 |
| 2026-11-11 05:00 | panic=abort, fat LTO (repeat) | demo | all | 36.6 / 78.0 / 82.5 | 35.9 / 77.5 / 81.7 | 37.0 / 78.8 / 82.9 | - | 215.4 / 6.1 / 1.3 |
| 2026-11-11 05:02 | panic=abort, fat LTO (repeat) | demo-big | all | 39.7 / 74.9 / 86.3 | 48.7 / 90.5 / 96.1 | 53.4 / 95.4 / 100.6 | - | 1.4 / 3.2 / 2.1 |
| 2026-11-11 05:05 | panic=abort, thin LTO + mimalloc (repeat) | demo | all | 45.5 / 87.4 / 108.8 | 42.3 / 84.4 / 97.4 | 41.9 / 84.2 / 98.0 | - | 0.5 / 4.5 / 0.8 |
| 2026-11-11 05:07 | panic=abort, thin LTO + mimalloc (repeat) | demo-big | all | 174.7 / 216.9 / 236.0 | 78.7 / 121.1 / 136.8 | 74.0 / 116.6 / 133.5 | - | 0.8 / 1.4 / 1.4 |

## Release profile and allocator (rb/mem-measure, 2026-11-11)

Off-screen probe windows (RUSTCORD_START_OFFSCREEN), Windows 11, Task
Manager "Memory" at 120 s in MB. Single runs vary a lot (the first
"demo / all" run of a batch reads 86-101 MB, repeats 37-42 MB), so compare
several rows, not one.

| Build | demo, default modules | demo-big, all on | demo-big, default | live bot | Release build | exe |
|---|---|---|---|---|---|---|
| baseline: thin LTO, unwind | 49.9 | 67.0 / 65.7 | 64.6 | 66.0 | 387 s | 29.6 MB |
| panic=abort, thin LTO (kept) | 36.7 | 54.1 | 53.3 | - | 286 s | 23.9 MB |
| panic=abort, fat LTO | 37.2 | 38.0 / 53.4 | 49.2 | - | 760 s | 22.6 MB |
| + mimalloc 0.1 (dropped) | 42.1 | 74.2 / 74.0 | 78.6 | 76.8 | 304 s | 23.9 MB |

- panic=abort: about 10 MB lower Memory and 5.7 MB smaller exe. Kept.
- Fat LTO: maybe a few MB lower, within the run-to-run noise, for 2.6x the
  build time. Thin kept.
- mimalloc: 10-25 MB higher at 120 s and about 175 MB at 10 s on
  demo-big (it holds READY's freed pages). Dropped.
- Frame-time p95 stayed at 1-8 ms in all builds.
- Visible window, baseline: demo 49.9 and demo-big 53.9. A minimised
  window doesn't run its UI in eframe (about 35 MB, not comparable).
