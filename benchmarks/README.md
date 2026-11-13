# Benchmarks

Gungraun benchmarks for [`zeta-utils`](../crates/zeta-utils) (list joining and
duration/calendar-distance formatting) and for the plugin dispatch pipeline
(`zeta-plugin` + `zeta-plugin-runtime`). Gungraun measures with Valgrind's Callgrind and DHAT
instead of wall-clock sampling, so the instruction and allocation counts are deterministic and
comparable between runs, machines, and CI environments.

## Dispatch pipeline

`benches/dispatch.rs` measures the synchronous path from an IRC message to a plugin handler in
three groups:

- `index` — `EventIndex::dispatch`: classification, command matching, message/CTCP construction,
  URL extraction, and the mailbox hand-off. The dispatch refactor also touches this side
  (per-event span-name/channel hoisting), so the group is compared rather than assumed constant.
- `handler` — invoking one already-built event through the generated plugin dispatch (the
  boxed-future forwarder before the static-dispatch refactor, the native future after it).
- `pipeline` — dispatch plus handler in one call.

Deliberately excluded: Tokio task scheduling and wakeups (the mailbox is drained synchronously
inside the measured body and the handler is polled directly), tracing export, and network,
database, clock or randomness. Handlers are no-ops with fixed inputs. Setup values are returned
from the benchmark functions so their destructors run outside the measured entry point.

### Telemetry paths

The dispatch pipeline has two benchmark suites, one per telemetry path:

- `benches/dispatch.rs` installs no subscriber. `tracing::enabled!` is false, so the handler-span
  metadata and span construction are skipped — the path an operator gets when the handler spans
  are filtered out.
- `benches/dispatch_telemetry.rs` installs a no-op subscriber in setup. `tracing::enabled!` is
  true and the span macros evaluate their fields, while the subscriber's `new_span` returns an
  id without storing or exporting a span. This is the production path: the default filter
  (`zeta=info`) enables the handler spans.

The suites are separate benchmark targets so their fixtures cannot shift each other's compiled
binary and recorded baseline. They share the same case ids (`privmsg`, `one_subscriber`,
`eight_subscribers`, `hit`, `ctcp`, `host_match`, `any`), and `dispatch_telemetry` mirrors the
`dispatch` fixtures.

The no-op subscriber is installed in the setup function (excluded from the measurement), not
with a per-call `with_default` in the measured body, so the dispatcher allocation is not counted
as span-construction cost. `register_callsite` reports `Interest::sometimes` so the enabled state
follows the thread's current dispatcher. That also means the enabled cases pay the `enabled`
checks a permanently installed production subscriber may cache at the callsite; treat their
instruction counts as a stable regression baseline, not as an exact production measurement.

The case ids are stable across the plugin dispatch refactor. The recorded comparison used the
pushed branches `bench/dispatch-pipeline` (old boxed API, based on `71f4a72`) and
`bench/dispatch-pipeline-static` (new `Plugin::handle` API); the only production-API difference
in the suite is the `invoke` shim.

```shell
git switch bench/dispatch-pipeline
cargo bench -p zeta-benchmarks --bench dispatch -- --save-baseline=pre_refactor
git switch bench/dispatch-pipeline-static
cargo bench -p zeta-benchmarks --bench dispatch -- --baseline=pre_refactor --save-baseline=post_refactor
```

Keep both runs in one worktree: Gungraun baselines live under `target/gungraun` and are not
portable between target directories. Once the suites are on `main`, CI records the baseline
automatically and compares every pull request against it.

The handler and pipeline groups are expected to lose the boxed-future allocations once the
handler future is awaited inline; a DHAT `TotalBlocks` increase there is a regression. Each
telemetry suite keeps its own baseline, and the enabled path must keep the filtered-out path's
allocations block for block: span construction itself stores nothing. The recorded index
baselines (Callgrind, 2026-11-13) are:

| Case | `dispatch` Ir | `dispatch_telemetry` Ir | Blocks | Bytes |
| --- | ---: | ---: | ---: | ---: |
| `privmsg` | 740 | 816 | 0 | 0 |
| `one_subscriber` | 896 | 1766 | 1 | 256 |
| `eight_subscribers` | 1638 | 5665 | 1 | 256 |
| `hit` | 1227 | 2075 | 1 | 256 |
| `ctcp` | 1169 | 2039 | 1 | 256 |
| `host_match` | 4919 | 5737 | 3 | 384 |
| `any` | 4847 | 5668 | 3 | 384 |

The non-zero blocks are the shared message (one `Arc<Message>`) and, for URL cases, the
retained URL handles; the span path adds no allocation of its own. A new block on a
`dispatch_telemetry` case is a regression even when the `dispatch` case is unchanged.

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
cargo bench -p zeta-benchmarks --bench dispatch
cargo bench -p zeta-benchmarks --bench dispatch_telemetry
```

Each benchmark runs once. The default tool is Callgrind; the join, dispatch, and
dispatch_telemetry benchmarks additionally run DHAT to track allocations.

## Regression checks

The benchmark files opt into regression limits:

- Callgrind: `Ir` (instructions executed) with a soft limit of 1%.
- DHAT (join, dispatch, and dispatch_telemetry): `TotalBlocks` and `TotalBytes` with a soft
  limit of 0%, so an allocation regression in a measured path fails the run. The borrowing join path is expected
  to allocate exactly once (the result `String`); the streaming iterator path can allocate
  again when the result grows. The dispatch handler path allocates nothing for a no-op
  handler; the pipeline's only remaining allocation is the shared message.

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
  reported as drift instead), plus any growth in DHAT `TotalBlocks`/`TotalBytes` for the
  suites that record DHAT (join and dispatch). The compiled-in ±1% Callgrind limit remains the backstop.
- **Toolchain drift** — the baseline records rustc release/commit, Valgrind, gungraun-runner
  versions and the `Cargo.lock` hash. A mismatch downgrades regressions to warnings with a
  banner instead of producing false alarms. The next main run re-records the baseline under
  the new toolchain; use the `force` dispatch input to re-record immediately.
- **Baseline recording uses the same policy** — main runs apply the instruction floor and the
  waivers before the artifact is updated, so an accepted (waived) regression can become the
  new reference and main does not stay red after merging it.
- **Report surfaces** — the report leads with the verdict and counts and only lists the
  benchmarks whose tier changed; unchanged rows collapse to a per-suite count inside the
  `<details>` inventory. Same-repo PRs get a `Bench report` check run (the same concise report
  plus annotations on regressions), `bench/join` and `bench/duration` commit statuses, and the
  report in the job summary. The baseline job renders the full report (`--full`): every
  inventory row plus the static cross-implementation ratios, which are not PR deltas and never
  appear in a pull-request summary. Fork PRs get the job summary and the gate; their token is
  read-only, so API publishing is skipped. Regressions also upload the raw `summary.json`
  files and Callgrind flamegraph SVGs for old and new code.
- **Waivers** — the `WAIVERS` constant in `xtask/src/bench/config.rs` documents accepted
  regressions (reason and optional `YYYY-MM-DD` expiry). Waived entries stay visible in the
  report but do not fail the check.
- **Adding benchmarks or suites** — no report code needs to change. Case labels come from the
  benchmark ids in the Rust sources (`#[bench::leap_day_anniversary]` renders as "leap day
  anniversary"); new suites are discovered from the summary files, and benchmarks sharing a
  set of case ids are compared automatically. Display overrides, suite source/context paths
  and the reference benchmark per suite live in `xtask/src/bench/config.rs`.
- **Configuration hygiene** — the configuration is Rust code, so it is compile-time checked
  (there are no keys to mistype). After every benchmark run `cargo xtask bench check-config`
  additionally cross-references display overrides, suite entries, comparison references and
  waivers with the measured inventory, failing on renamed or removed benchmarks and warning
  about expired or duplicate waivers. Unit tests cover the tooling and run in both jobs:
  `cargo test -p xtask`.

## Tooling

Benchmark automation is a Rust xtask (`xtask/`), invoked through the cargo alias in
`.cargo/config.toml`:

```shell
cargo xtask bench metadata --output bench-metadata.txt --toolchain stable
cargo xtask bench check-config
cargo xtask bench report --baseline-found --bench-exit-code 3
cargo xtask bench report --baseline-found --full   # complete inventory + orientation ratios
cargo xtask bench gate
cargo xtask bench has-regressions
cargo xtask bench post-report --dry-run
```

Summaries are read with a small in-tree JSON module, so the xtask builds without compiling a
dependency tree. Configuration (policy, display overrides, comparison references, waivers) is
plain Rust data in `xtask/src/bench/config.rs`.
