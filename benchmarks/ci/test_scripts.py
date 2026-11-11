#!/usr/bin/env python3
"""Unit tests for the benchmark report scripts.

Run with `python3 benchmarks/ci/test_scripts.py` (also executed by the benchmark
workflows before they spend time under Valgrind).
"""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from report_check import check
from report_config import Config, ConfigError, Policy, Waiver, load_config, load_waivers
from report_format import humanize, humanize_case, natural_key, number, percent, signed
from report_inventory import build as build_inventory
from report_policy import classify, counts, statuses, verdict
from report_render import build_report
from summary import Benchmark, Metrics, RunSummary, metadata_matches


def benchmark(
    function: str,
    case: str | None,
    old: int | None,
    new: int | None,
    pct: float | None = None,
    suite: str = "join",
) -> Benchmark:
    if pct is None and old is not None and new is not None:
        pct = (new - old) / old * 100 if old else float("inf")
    return Benchmark(
        suite=suite,
        group=suite,
        function=function,
        case=case,
        ir=Metrics(old=old, new=new, pct=pct),
    )


class FormatTests(unittest.TestCase):
    def test_humanizes_benchmark_names(self) -> None:
        self.assertEqual(humanize("bench_oxford_join_slice"), "oxford join slice")
        self.assertEqual(humanize_case("sixty_four_items"), "sixty four items")

    def test_formats_numbers(self) -> None:
        self.assertEqual(number(1234), "1,234")
        self.assertEqual(number(None), "—")
        self.assertEqual(signed(5), "+5")
        self.assertEqual(signed(-5), "-5")
        self.assertEqual(percent(12.345), "+12.35%")
        self.assertEqual(percent(float("inf")), "new")

    def test_sorts_case_ids_naturally(self) -> None:
        cases = ["case_10", "case_2", "case_1"]
        self.assertEqual(sorted(cases, key=natural_key), ["case_1", "case_2", "case_10"])


class ConfigTests(unittest.TestCase):
    def test_rejects_unknown_keys(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory, "report.toml")
            path.write_text("[policy]\nflor_ir = 4\n")
            with self.assertRaisesRegex(ConfigError, "flor_ir"):
                load_config(path)

    def test_rejects_unknown_waiver_keys_and_bad_metrics(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            unknown = Path(directory, "waivers.toml")
            unknown.write_text('[[waiver]]\nbenchmark = "x"\nreasons = "typo"\n')
            with self.assertRaisesRegex(ConfigError, "reasons"):
                load_waivers(unknown)

            metric = Path(directory, "metric.toml")
            metric.write_text(
                '[[waiver]]\nbenchmark = "x"\nreason = "why"\nmetric = "Instructions"\n'
            )
            with self.assertRaisesRegex(ConfigError, "metric"):
                load_waivers(metric)

    def test_loads_valid_configuration(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory, "report.toml")
            path.write_text(
                """
                [policy]
                floor_ir = 8
                limit_ir = 0.5
                [display]
                bench_x = "X"
                [comparisons]
                join = "bench_std_join"
                """
            )
            config = load_config(path)
            self.assertEqual(config.policy.floor_ir, 8)
            self.assertEqual(config.policy.limit_ir, 0.5)
            self.assertEqual(config.display_name("bench_x"), "X")
            self.assertEqual(config.comparisons, {"join": "bench_std_join"})


class PolicyTests(unittest.TestCase):
    def setUp(self) -> None:
        self.config = Config(policy=Policy(floor_ir=16, limit_ir=0.0, backstop_ir=1.0))

    def test_tiers(self) -> None:
        regression = benchmark("bench_x", "case_0", 1000, 1030)
        drift = benchmark("bench_x", "case_1", 1000, 1010)
        improvement = benchmark("bench_x", "case_2", 1000, 900)
        new = benchmark("bench_x", "case_3", None, 100)
        for item in (regression, drift, improvement, new):
            classify(item, [], self.config)
        self.assertEqual(
            (regression.tier, drift.tier, improvement.tier, new.tier),
            ("regressed", "drift", "improved", "new"),
        )

    def test_waiver_downgrades_regression(self) -> None:
        regression = benchmark("bench_x", "case_0", 1000, 1030)
        waiver = Waiver(benchmark="bench_x", case="case_0", metric="Ir", reason="accepted")
        classify(regression, [waiver], self.config)
        self.assertEqual(regression.tier, "waived")
        self.assertIn("accepted", regression.waiver or "")

    def test_verdict_and_statuses_are_per_suite(self) -> None:
        regression = benchmark("bench_x", "case_0", 1000, 1030)
        classify(regression, [], self.config)
        ok = benchmark("bench_y", "case_0", 500, 500, suite="duration")
        classify(ok, [], self.config)
        run = RunSummary(
            benchmarks=[regression, ok],
            baseline={},
            current={},
            baseline_found=True,
            toolchain_matched=True,
            mismatches=[],
            exit_code=3,
        )
        self.assertEqual(verdict(run)[0], "regressed")
        self.assertEqual(counts(run.benchmarks)["regressed"], 1)
        payload = statuses(run, self.config)
        self.assertEqual(payload["bench/join"]["state"], "failure")
        self.assertEqual(payload["bench/duration"]["state"], "success")


class InventoryAndCheckTests(unittest.TestCase):
    def setUp(self) -> None:
        self.benchmarks = [
            benchmark("bench_std_join", "one_item", 100, 100),
            benchmark("bench_join_slice", "one_item", 100, 100),
            benchmark("bench_distance_chrono", "leap_day", 200, 200, suite="duration"),
            benchmark("bench_distance_time", "leap_day", 200, 200, suite="duration"),
        ]

    def test_inventory_finds_comparison_members(self) -> None:
        inventory = build_inventory(self.benchmarks)
        self.assertEqual(inventory.suites, {"join", "duration"})
        self.assertEqual(
            inventory.comparison_members("join", "bench_std_join"), ["bench_join_slice"]
        )

    def test_clean_configuration_passes(self) -> None:
        config = Config(comparisons={"join": "bench_std_join"})
        result = check(config, [], self.benchmarks)
        self.assertTrue(result.ok)
        self.assertFalse(result.warnings)

    def test_detects_stale_configuration(self) -> None:
        config = Config(
            display={"bench_removed": "Removed"},
            comparisons={"join": "bench_missing"},
        )
        result = check(config, [], self.benchmarks)
        messages = "\n".join(result.errors)
        self.assertIn("bench_removed", messages)
        self.assertIn("bench_missing", messages)

    def test_waiver_checks(self) -> None:
        expired = Waiver(
            benchmark="bench_std_join", reason="old", expires="2020-01-01"
        )
        unmatched = Waiver(benchmark="bench_removed", reason="gone")
        result = check(Config(), [expired, unmatched], self.benchmarks)
        self.assertIn("bench_removed", "\n".join(result.errors))
        self.assertIn("expired", "\n".join(result.warnings))


class RenderTests(unittest.TestCase):
    def test_report_contains_sections_and_chart(self) -> None:
        regression = benchmark("bench_join_slice", "one_item", 100, 130)
        reference = benchmark("bench_std_join", "one_item", 100, 100)
        unchanged = benchmark(
            "bench_distance_chrono", "leap_day", 200, 200, suite="duration"
        )
        config = Config(comparisons={"join": "bench_std_join"})
        for item in (regression, reference, unchanged):
            classify(item, [], config)
        metadata = {
            "commit": "abcdef0123456789",
            "timestamp": "2026-01-01T00:00:00Z",
            "toolchain": "stable",
            "rustc-release": "1.99.0",
            "rustc-commit": "abc",
            "valgrind": "valgrind-3.24.0",
            "gungraun-runner": "gungraun-runner 0.20.0",
            "cargo-lock-sha256": "deadbeef",
        }
        run = RunSummary(
            benchmarks=[regression, reference, unchanged],
            baseline=metadata,
            current=metadata,
            baseline_found=True,
            toolchain_matched=True,
            mismatches=[],
            exit_code=3,
        )
        report = build_report(run, config)
        self.assertIn("🔴 Regressions detected", report)
        self.assertIn("### 🔴 Regressions", report)
        self.assertIn("Join vs std join", report)
        self.assertIn("```mermaid", report)
        self.assertIn("All benchmarks (3)", report)


class MetadataTests(unittest.TestCase):
    def test_missing_metadata_is_not_a_mismatch(self) -> None:
        self.assertEqual(metadata_matches({}, {"toolchain": "stable"}), (True, []))

    def test_detects_toolchain_changes(self) -> None:
        baseline = {"toolchain": "stable", "rustc-commit": "a"}
        current = {"toolchain": "stable", "rustc-commit": "b"}
        matched, mismatches = metadata_matches(baseline, current)
        self.assertFalse(matched)
        self.assertEqual(mismatches, [("rustc-commit", "a", "b")])


if __name__ == "__main__":
    unittest.main()
