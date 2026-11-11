#!/usr/bin/env python3
"""Turn Gungraun summary files into the benchmark report for a PR check.

Gungraun's Callgrind/DHAT counts are deterministic: with a matching toolchain the
difference against the last-known-good baseline is exact, so the workflow policy can gate
on any positive `Ir` delta instead of a statistical interval.

Project-specific names, policy and comparison references live in `report.toml`; adding a
benchmark or a new suite needs no Python changes. `--check-config` validates that
configuration against the measured benchmarks. The script is dependency-free (stdlib only)
so the output can be reproduced locally:

    python3 benchmarks/ci/report.py \
        --summaries target/gungraun \
        --baseline-metadata target/gungraun/bench-metadata.txt \
        --current-metadata bench-metadata.txt \
        --baseline-found

    python3 benchmarks/ci/report.py --check-config
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from report_check import check, format_result
from report_config import (
    DEFAULT_CONFIG,
    DEFAULT_WAIVERS,
    ConfigError,
    load_config,
    load_waivers,
)
from report_policy import classify, counts, statuses, verdict
from report_render import build_report, headline
from summary import RunSummary, load_benchmarks, metadata_matches, read_metadata


def _check_config(
    config, waivers, benchmarks, config_path: Path, waivers_path: Path
) -> int:
    result = check(config, waivers, benchmarks)
    print(format_result(result, str(config_path), str(waivers_path)))
    return 0 if result.ok else 1


def _build_payload(run: RunSummary, config) -> dict:
    verdict_name, conclusion, _ = verdict(run)
    return {
        "verdict": verdict_name,
        "conclusion": conclusion,
        "title": headline(run).splitlines()[0],
        "counts": counts(run.benchmarks),
        "statuses": statuses(run, config),
        "regressions": [
            {
                "suite": benchmark.suite,
                "benchmark": benchmark.function,
                "case": benchmark.case,
                "display": config.label(benchmark.function, benchmark.case, benchmark.suite),
                "source": config.suite_source(benchmark.suite),
                "ir_old": benchmark.ir.old,
                "ir_new": benchmark.ir.new,
                "reasons": benchmark.reasons,
            }
            for benchmark in run.benchmarks
            if benchmark.tier == "regressed"
        ],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--summaries", type=Path, default=Path("target/gungraun"))
    parser.add_argument(
        "--baseline-metadata",
        type=Path,
        default=Path("target/gungraun/bench-metadata.txt"),
    )
    parser.add_argument(
        "--current-metadata", type=Path, default=Path("bench-metadata.txt")
    )
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    parser.add_argument("--waivers", type=Path, default=DEFAULT_WAIVERS)
    parser.add_argument(
        "--check-config",
        action="store_true",
        help="Validate report.toml and waivers.toml against the measured benchmarks",
    )
    parser.add_argument("--baseline-found", action="store_true")
    parser.add_argument("--bench-exit-code", type=int, default=0)
    parser.add_argument("--report", type=Path, default=Path("bench-report.md"))
    parser.add_argument("--verdict", type=Path, default=Path("verdict.json"))
    args = parser.parse_args()

    try:
        config = load_config(args.config)
        waivers = load_waivers(args.waivers)
    except ConfigError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1

    benchmarks = load_benchmarks(args.summaries)
    if args.check_config:
        return _check_config(config, waivers, benchmarks, args.config, args.waivers)

    baseline = read_metadata(args.baseline_metadata)
    current = read_metadata(args.current_metadata)
    matched, mismatches = metadata_matches(baseline, current)

    for benchmark in benchmarks:
        classify(benchmark, waivers, config)
        # Counts recorded with a different toolchain are not comparable; keep reporting
        # them, but never gate on them.
        if not matched and benchmark.tier == "regressed":
            benchmark.tier = "warning"
            benchmark.waiver = "toolchain mismatch"

    run = RunSummary(
        benchmarks=benchmarks,
        baseline=baseline,
        current=current,
        baseline_found=args.baseline_found,
        toolchain_matched=matched or not args.baseline_found,
        mismatches=mismatches,
        exit_code=args.bench_exit_code,
    )

    args.report.write_text(build_report(run, config))
    args.verdict.write_text(json.dumps(_build_payload(run, config), indent=2) + "\n")

    verdict_name, _, _ = verdict(run)
    print(f"{verdict_name}: {counts(benchmarks)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
