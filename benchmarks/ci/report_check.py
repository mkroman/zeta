"""Configuration hygiene checks against the measured benchmark inventory.

`report.py --check-config` runs this after a benchmark run (the summaries are the
authoritative inventory) and fails when `report.toml` or `waivers.toml` references something
that no longer exists. That keeps renames and removals from silently disabling display
names, comparison groups or waivers.
"""

from __future__ import annotations

from collections import Counter
from dataclasses import dataclass, field
from datetime import datetime, timezone

from report_config import Config, Waiver
from report_format import humanize, humanize_case
from report_inventory import build as build_inventory
from summary import Benchmark


@dataclass
class CheckResult:
    benchmarks: int = 0
    suites: int = 0
    errors: list[str] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)

    @property
    def ok(self) -> bool:
        return not self.errors


def check(config: Config, waivers: list[Waiver], benchmarks: list[Benchmark]) -> CheckResult:
    inventory = build_inventory(benchmarks)
    result = CheckResult(benchmarks=len(benchmarks), suites=len(inventory.suites))

    for function, display in config.display.items():
        if function not in inventory.cases_by_function:
            result.errors.append(
                f"[display] {function}: no such benchmark (renamed or removed)"
            )
        elif display == humanize(function):
            result.warnings.append(
                f'[display] {function}: override "{display}" is the derived name'
            )

    for function, labels in config.case_labels.items():
        section = f'[case_labels."{function}"]'
        if function not in inventory.cases_by_function:
            result.errors.append(f"{section}: no such benchmark (renamed or removed)")
            continue
        for case, label in labels.items():
            if case not in inventory.cases_by_function[function]:
                result.errors.append(f"{section}.{case}: no such case id")
            elif label == humanize_case(case):
                result.warnings.append(
                    f'{section}.{case}: override "{label}" is the derived label'
                )

    for suite, suite_config in config.suites.items():
        section = f"[suites.{suite}]"
        if suite not in inventory.suites:
            result.errors.append(f"{section}: no such suite")
            continue
        if suite_config.display == suite:
            result.warnings.append(f"{section}.display equals the derived name")
        if suite_config.context == f"bench/{suite}":
            result.warnings.append(f"{section}.context equals the derived context")
        if suite_config.source == f"benchmarks/benches/{suite}.rs":
            result.warnings.append(f"{section}.source equals the derived path")
        suite_cases = {
            case
            for function_cases in inventory.functions_by_suite[suite].values()
            for case in function_cases
        }
        for case, label in suite_config.cases.items():
            if case not in suite_cases:
                result.errors.append(f"{section}.cases.{case}: no such case id in the suite")
            elif label == humanize_case(case):
                result.warnings.append(
                    f'{section}.cases.{case}: override "{label}" is the derived label'
                )

    for suite, reference in config.comparisons.items():
        section = "[comparisons]"
        if suite not in inventory.suites:
            result.errors.append(f"{section}.{suite}: no such suite")
            continue
        reference_cases = inventory.reference_cases(suite, reference)
        if reference_cases is None:
            result.errors.append(
                f"{section}.{suite}: reference {reference} is not a benchmark in the suite"
            )
            continue
        if not reference_cases:
            result.errors.append(f"{section}.{suite}: reference {reference} has no cases")
            continue
        if not inventory.comparison_members(suite, reference):
            result.errors.append(
                f"{section}.{suite}: no benchmark shares the reference's case-id set, "
                "so the comparison would render nothing"
            )

    duplicates = Counter(
        (waiver.benchmark, waiver.case, waiver.metric) for waiver in waivers
    )
    today = datetime.now(timezone.utc).date()
    for waiver in waivers:
        key = (waiver.benchmark, waiver.case, waiver.metric)
        if duplicates[key] > 1:
            description = waiver.benchmark
            if waiver.case:
                description += f"·{waiver.case}"
            result.warnings.append(
                f"waiver for {description} ({waiver.metric}) is duplicated"
            )
        if waiver.benchmark not in inventory.cases_by_function:
            result.errors.append(
                f"waiver for {waiver.benchmark}: no such benchmark (renamed or removed)"
            )
            continue
        if waiver.case and waiver.case not in inventory.cases_by_function[waiver.benchmark]:
            result.errors.append(
                f"waiver for {waiver.benchmark}·{waiver.case}: no such case id"
            )
        expiry = waiver.expiry_date()
        if expiry is not None and expiry < today:
            result.warnings.append(
                f"waiver for {waiver.benchmark}: expired {waiver.expires} — "
                "remove it or extend it with a fresh justification"
            )

    return result


def format_result(result: CheckResult, config_path: str, waivers_path: str) -> str:
    suite_word = "suite" if result.suites == 1 else "suites"
    lines = [
        f"Config check: {result.benchmarks} benchmarks across {result.suites} "
        f"{suite_word} (`{config_path}`, `{waivers_path}`)",
    ]
    if result.ok and not result.warnings:
        lines.append("✔ No configuration drift.")
    for error in result.errors:
        lines.append(f"error: {error}")
    for warning in result.warnings:
        lines.append(f"warning: {warning}")
    if result.errors:
        lines.append(
            "The configuration references benchmarks that no longer exist; update "
            "`benchmarks/ci/report.toml` or `benchmarks/ci/waivers.toml`."
        )
    return "\n".join(lines)
