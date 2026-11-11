# Benchmarks

Gungraun benchmarks for [`zeta-utils`](../crates/zeta-utils): list joining and
duration/calendar-distance formatting. Gungraun measures with Valgrind's Callgrind and DHAT
instead of wall-clock sampling, so the instruction and allocation counts are deterministic and
comparable between runs, machines, and CI environments.

## Prerequisites

- Valgrind 3.20 or newer (`apt-get install valgrind`)
- `gungraun-runner`, matching the `gungraun` library version in `Cargo.toml`:

  ```shell
  cargo install gungraun-runner --version 0.20.0
  ```

- Debug symbols in the `bench` profile. This is configured in the workspace `Cargo.toml`:
  the `bench` profile inherits `release`, which strips debug info, so it is overridden with
  `debug = true` and `strip = false`.

## Running

```shell
cargo bench -p zeta-benchmarks
cargo bench -p zeta-benchmarks --bench join
cargo bench -p zeta-benchmarks --bench duration
```

Each benchmark runs once. The default tool is Callgrind; the join benchmarks additionally run
DHAT to track allocations.

## Regression checks

The benchmark files opt into regression limits:

- Callgrind: `Ir` (instructions executed) with a soft limit of 1%.
- DHAT (join only): `TotalBlocks` and `TotalBytes` with a soft limit of 0%, so an allocation
  regression in any joining path fails the run. The borrowing slice path is expected to
  allocate exactly once (the result `String`); the streaming iterator path can allocate
  again when the result grows.

A run that exceeds a limit exits with code `3` and prints the regressed benchmarks. Limits are
compared with the previous run, so the first run only records data.

## Baselines

Save a reference baseline (for example from `main`) and compare later work against it:

```shell
cargo bench -p zeta-benchmarks -- --save-baseline=main
# ... make changes ...
cargo bench -p zeta-benchmarks -- --baseline=main
```

`--baseline` never overwrites the reference; combining it with `--save-baseline=NAME` compares
against the reference and stores the new results under `NAME`:

```shell
cargo bench -p zeta-benchmarks -- --baseline=main --save-baseline=pr_1234
```

## Collecting statistics

- The default terminal output shows instructions, cache hits/misses, total reads+writes, and
  estimated cycles.
- `--output-format=json` (or `pretty-json`) prints one JSON object per benchmark to stdout.
- `--save-summary=json` writes a `summary.json` per benchmark under
  `target/gungraun/<package>/<bench file>/<group>/<benchmark>`. The
  [`gungraun-summary`](https://docs.rs/gungraun-summary) crate parses those files.

```shell
cargo bench -p zeta-benchmarks -- --save-summary=json
cargo bench -p zeta-benchmarks -- --output-format=json | jq -s
```

## Notes

- Benchmarks of now-relative APIs (`in_words`/`time_ago`/`time_until` on points in time) are
  intentionally absent: they read the clock, so their instruction counts are not deterministic.
  Their shared core is covered by `distance_in_words`.
- The separate workspace member keeps `zeta-utils` compiled as a dependency, so the
  measurements are not skewed by in-crate inlining.

## CI

`.github/workflows/benchmarks.yaml` runs the suites on every push to `main` (baseline
recording), every pull request (comparison), weekly (baseline refresh, which picks up runner
image changes before they show up as warnings) and manually via `workflow_dispatch` (with a
`force` input to re-record after an accepted toolchain change).

- **Last known good baseline** — the `bench-baseline` artifact of the last *successful* main
  run. A run only uploads the artifact after passing its own limits, so a regressing merge
  can never become the reference. PRs download it by run id, run with `--baseline=main`, and
  gate against it.
- **Policy** — because the counts are deterministic, the workflow gates on any reproducible
  increase: `Ir` above a 16-instruction floor (single-instruction churn on trivial benches is
  reported as drift instead), plus any growth in DHAT `TotalBlocks`/`TotalBytes` for the join
  suite. The compiled-in ±1% Callgrind limit remains the backstop, and the chart draws it as
  the reference band.
- **Toolchain drift** — the baseline records rustc release/commit, Valgrind, gungraun-runner
  versions and the `Cargo.lock` hash. A mismatch downgrades regressions to warnings with a
  banner instead of producing false alarms. The next main run re-records the baseline under
  the new toolchain; use the `force` dispatch input to re-record immediately.
- **Baseline recording uses the same policy** — main runs apply the instruction floor and the
  waivers before the artifact is updated, so an accepted (waived) regression can become the
  new reference and main does not stay red after merging it.
- **Report surfaces** — same-repo PRs get a `Bench report` check run (markdown report plus
  annotations on regressions), `bench/join` and `bench/duration` commit statuses, and the full
  report in the job summary. Fork PRs get the job summary and the gate; their token is
  read-only, so API publishing is skipped. Regressions also upload the raw `summary.json`
  files and Callgrind flamegraph SVGs for old and new code.
- **Waivers** — `benchmarks/ci/waivers.toml` documents accepted regressions (reason, PR,
  optional expiry). Waived entries stay visible as ⚠ in the report but do not fail the check.
- **Adding benchmarks or suites** — no report code needs to change. Case labels come from the
  benchmark ids in the Rust sources (`#[bench::leap_day_anniversary]` renders as "leap day
  anniversary"); new suites are discovered from the summary files, and benchmarks sharing a
  set of case ids are compared automatically. `benchmarks/ci/report.toml` only holds the
  policy, report limits, optional display overrides and the reference benchmark per suite.
- **Configuration hygiene** — `report.toml` and `waivers.toml` are validated strictly:
  unknown or mistyped keys fail the run, and after every benchmark run
  `report.py --check-config` cross-references them with the measured benchmark inventory
  (stale display/case/suite entries, invalid comparison references, unmatched or expired
  waivers). Renaming or removing a benchmark therefore requires updating the configuration
  in the same change. The scripts have a unit-test safety net:
  `python3 benchmarks/ci/test_scripts.py` (also run by both benchmark jobs).
