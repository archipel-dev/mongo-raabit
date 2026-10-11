# Performance budgets

The charter's speed budgets are checked in CI (`perf` job) against the release build of `raabit`.
A PR that breaks one turns CI red. The numbers live in [`perf-budgets.toml`](../perf-budgets.toml).

| Check | Budget | Measured as |
|---|---|---|
| `cold_start` | **400 ms** | process start to the first interactive frame (the first frame that renders the plugin view) |
| `frame.welcome` / `dialog` / `tree` / `table` | **16 ms** | 95th percentile of the `update()`/ui CPU time per frame while the screen is driven with the mouse |
| `idle.cpu` | **1 %** of one core | whole-process CPU over 5 s with nothing moving, on the documents table |
| `idle.repaints` | 2 frames | `update()` calls during that window (egui must not repaint when nothing changes) |
| `memory.rss` | 200 MB | resident set after connecting and showing 50 documents; Compass idles around 300–500 MB |

The 400 ms, 16 ms and 1 % budgets are the charter's and are never loosened to pass. The memory
budget is ours: measured ~137 MB, plus ~45 % headroom.

## How it is measured

`raabit --perf-report <file>` (or `RAABIT_PERF=<file>`) turns the hook in
`crates/raabit/src/perf.rs` on; off, it costs one `Option` check per frame. The app records the
time of the first interactive frame (from the top of `main`) and, for every frame, a wall-clock
stamp and the duration of `update()`. It writes JSON when the window closes. The driver asks it
to close by creating `<file>.stop` and moving the mouse.

`cargo xtask perf` (`xtask/src/perf.rs`) does the rest:

1. Seeds `raabit_perf.customers` (240 documents) through the driver, so CI needs no `mongosh`.
2. Starts Xvfb (1280x800) and runs `raabit --dark` with a throwaway `HOME`.
3. Drives it with xdotool, mouse only: hover the welcome screen, open and close the connection
   dialog, "Connect to localhost", expand `raabit_perf`, open `customers` (50 rows per page),
   hover and scroll the table, then sit idle for 5 s.
4. Takes frame times from the report (each screen's window runs from the click that opens it to
   the end of its hovering), idle CPU from `/proc/<pid>/stat`, RSS from `/proc/<pid>/status`.
   Cold start adds the exec-to-`main` gap seen by the driver.
5. Repeats for `--runs` runs (default 5, CI 7), takes the **median** of each metric, prints the
   table and exits non-zero when a check breaks.

After every run the driver asks the server whether it saw a `find`; if not, it did not reach the
table and the run fails instead of reporting numbers for the wrong screen.

### Margins and noise

- CI runners are shared and noisy; a single cold start varies 100–400 ms on a busy machine. The
  median over several runs and the 95th percentile (not the max) per screen absorb that. The
  worst frame is printed next to each frame check for information.
- Frame time is the CPU time of `update()` (building the UI), which is what the app controls.
  Tessellation and GPU upload are not included; under Xvfb they run in software (llvmpipe).
- Current numbers sit one to two orders of magnitude under the frame and idle budgets, and at
  about half of the cold-start budget, so the margins absorb runner noise without loosening.

## Run it locally

Needs Linux, `Xvfb`, `xdotool`, a `mongod` on `localhost:27017` (the address behind "Connect to
localhost") and Mesa's software GL (`libgl1-mesa-dri`). Use a throwaway server: it drops
`raabit_perf`.

```bash
cargo xtask perf                       # builds the release binary, 5 runs
cargo xtask perf --runs 9 --no-build   # reuse target/release/raabit
cargo xtask perf --json out.json       # machine-readable table
```

Other flags: `--bin PATH`, `--budgets FILE`, `--uri URI` (seeding and the `find` check),
`--display :N` (reuse a running X server instead of starting Xvfb).

## When a budget breaks

Fix the code. If the fix is not small or lives outside `crates/raabit`, open an issue labelled
`perf` with the numbers and list the check under `[known_breach]` in `perf-budgets.toml` with the
issue link, e.g. `"frame.table" = "https://github.com/archipel-dev/mongo-raabit/issues/12"`. The
job then prints `KNOWN BREACH (link)` and stays green; remove the entry when the issue is fixed.

## When the UI changes

The driver clicks fixed coordinates in the 1280x800 window (welcome buttons, the toolbar's
Connect, the dialog's close button, the first database and collection rows of the tree). When a
layout change moves them, the `find` check fails the run; update the coordinates in the `at` module of `xtask/src/perf.rs`.

## Current numbers

Median of 7 runs, release build (after the kit port of the chrome, connection manager and tree), shared 2-core VM, MongoDB 7.0.14 (`cargo xtask perf --runs 7`):

| Check | Measured | Budget |
|---|---|---|
| cold start | ~160 ms (105–310 ms per run) | 400 ms |
| frame p95: welcome / dialog / tree / table | 0.3 / 0.3 / 0.2 / 0.7 ms | 16 ms |
| worst single frame | ~8 ms (table, first open) | – |
| idle CPU | 0.00 % | 1 % |
| idle repaints in 5 s | 0 | 2 |
| RSS, 50 documents | 137 MB | 200 MB |

No check breaks today, so `perf-budgets.toml` has no known breaches.
