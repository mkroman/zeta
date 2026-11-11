"""Regression policy: verdict tiers, waivers and per-suite status contexts."""

from __future__ import annotations

from report_config import Config, Waiver
from report_format import percent, signed
from summary import DHAT_BLOCKS, DHAT_BYTES, IR, Benchmark, RunSummary

TIERS = ("regressed", "warning", "waived", "drift", "improved", "unchanged", "new")


def counts(benchmarks: list[Benchmark]) -> dict[str, int]:
    return {tier: sum(1 for b in benchmarks if b.tier == tier) for tier in TIERS}


def suite_totals(benchmarks: list[Benchmark]) -> dict[str, tuple[int, int]]:
    totals: dict[str, tuple[int, int]] = {}
    for benchmark in benchmarks:
        if benchmark.ir.new is None or benchmark.ir.old is None:
            continue
        old, new = totals.get(benchmark.suite, (0, 0))
        totals[benchmark.suite] = (old + benchmark.ir.old, new + benchmark.ir.new)
    return totals


def _waiver(waivers: list[Waiver], benchmark: Benchmark, metric: str) -> Waiver | None:
    return next(
        (
            waiver
            for waiver in waivers
            if waiver.matches(benchmark.function, benchmark.case, metric)
        ),
        None,
    )


def classify(benchmark: Benchmark, waivers: list[Waiver], config: Config) -> None:
    """Assign a verdict tier and the human-readable reasons to `benchmark`."""
    policy = config.policy
    deltas = {
        IR: benchmark.ir.delta if benchmark.ir.old is not None else None,
        DHAT_BLOCKS: benchmark.dhat_blocks.delta
        if benchmark.dhat_blocks.old is not None
        else None,
        DHAT_BYTES: benchmark.dhat_bytes.delta
        if benchmark.dhat_bytes.old is not None
        else None,
    }

    if benchmark.ir.old is None:
        benchmark.tier = "new"
        return

    breached: list[str] = []
    if (delta := deltas[IR]) is not None and delta > 0:
        pct = benchmark.ir.pct
        over = delta >= policy.floor_ir and (pct is None or pct > policy.limit_ir)
        if over:
            breached.append(IR)
            if pct is None:
                benchmark.reasons.append(
                    f"Ir {signed(delta)} (limit {policy.limit_ir:.1f}%)"
                )
            else:
                benchmark.reasons.append(
                    f"Ir {signed(delta)} ({percent(pct)}, "
                    f"limit {policy.limit_ir:.1f}%)"
                )
        else:
            benchmark.reasons.append(
                f"Ir {signed(delta)} (below {policy.floor_ir} instruction floor)"
            )
    if (delta := deltas[DHAT_BLOCKS]) is not None and delta > 0:
        breached.append(DHAT_BLOCKS)
        benchmark.reasons.append(f"allocations {signed(delta)} block(s)")
    if (delta := deltas[DHAT_BYTES]) is not None and delta > 0:
        breached.append(DHAT_BYTES)
        benchmark.reasons.append(f"allocated bytes {signed(delta)}")

    if breached:
        matching = {
            metric: waiver
            for metric in breached
            if (waiver := _waiver(waivers, benchmark, metric)) is not None
        }
        if len(matching) == len(breached):
            benchmark.tier = "waived"
            benchmark.waiver = "; ".join(
                f"{metric}: {matching[metric].describe()}" for metric in breached
            )
        else:
            benchmark.tier = "regressed"
        return

    if any(value is not None and value < 0 for value in deltas.values()):
        benchmark.tier = "improved"
    elif any(value is not None and value > 0 for value in deltas.values()):
        benchmark.tier = "drift"
    else:
        benchmark.tier = "unchanged"


def verdict(run: RunSummary) -> tuple[str, str, str]:
    """Return (verdict, check conclusion, status state) for the whole run."""
    tiers = counts(run.benchmarks)
    if run.exit_code not in (0, 3):
        return "error", "failure", "failure"
    if not run.baseline_found:
        return "no-baseline", "neutral", "success"
    if tiers["regressed"]:
        return "regressed", "failure", "failure"
    if not run.toolchain_matched:
        return "warning", "neutral", "success"
    return "ok", "success", "success"


def statuses(run: RunSummary, config: Config) -> dict[str, dict[str, str]]:
    """One commit status per suite, with dynamically discovered suites."""
    verdict_name, _, _ = verdict(run)
    payload = {}
    for suite in sorted({benchmark.suite for benchmark in run.benchmarks}):
        benchmarks = [b for b in run.benchmarks if b.suite == suite]
        regressed = sum(1 for b in benchmarks if b.tier == "regressed")
        waived = sum(1 for b in benchmarks if b.tier == "waived")
        total = sum(1 for b in benchmarks if b.ir.old is not None)
        if not run.baseline_found:
            description = "no baseline recorded yet"
            state = "success"
        elif not run.toolchain_matched:
            description = "warning: toolchain mismatch"
            state = "success"
        elif regressed:
            description = f"{regressed} regression(s) vs main"
            state = "failure"
        elif waived:
            description = f"{waived} waived vs main"
            state = "success"
        else:
            description = f"{total}/{total} within limits vs main"
            state = "failure" if verdict_name == "error" else "success"
        payload[config.suite_context(suite)] = {
            "state": state,
            "description": description,
        }
    return payload
