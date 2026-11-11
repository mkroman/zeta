"""Markdown/Mermaid rendering of the benchmark report."""

from __future__ import annotations

from report_config import Config
from report_format import natural_key, number, percent, signed
from report_inventory import Inventory, build as build_inventory
from report_policy import counts, suite_totals, verdict
from summary import Benchmark, RunSummary, metadata_age

TIER_LABELS = {
    "regressed": "🔴 regression",
    "warning": "⚠ warning",
    "waived": "⚠ waived",
    "drift": "🟡 drift",
    "improved": "🟢 improved",
    "unchanged": "⚪ unchanged",
    "new": "🆕 new",
}


def headline(run: RunSummary) -> str:
    verdict_name, _, _ = verdict(run)
    tiers = counts(run.benchmarks)
    parts = []
    for tier, icon, label in (
        ("regressed", "🔴", "regression(s)"),
        ("warning", "⚠", "warning(s)"),
        ("waived", "⚠", "waived"),
        ("drift", "🟡", "drift"),
        ("improved", "🟢", "improved"),
        ("unchanged", "⚪", "unchanged"),
        ("new", "🆕", "new"),
    ):
        if tiers[tier]:
            parts.append(f"{icon} {tiers[tier]} {label}")
    detail = " · ".join(parts) if parts else "no benchmarks"

    if verdict_name == "regressed":
        return f"🔴 Regressions detected — check fails\n\n{detail}"
    if verdict_name == "warning":
        return f"⚠ Toolchain mismatch — regressions reported as warnings\n\n{detail}"
    if verdict_name == "no-baseline":
        return "ℹ️ No baseline recorded yet — this run records measurements but does not gate"
    if verdict_name == "error":
        return "❌ Benchmark run failed — see the job log"
    return f"✅ No regressions\n\n{detail}"


def metadata_block(run: RunSummary, config: Config) -> list[str]:
    if not run.baseline_found:
        return ["ℹ️ No last-known-good baseline artifact was available."]

    rows = []
    for key, label in (
        ("rustc-release", "rustc"),
        ("valgrind", "valgrind"),
        ("gungraun-runner", "gungraun"),
    ):
        old, new = run.baseline.get(key, "?"), run.current.get(key, "?")
        rows.append(f"{label} {new} ({'✓' if old == new else '⚠'})")
    lock = (
        "identical"
        if run.baseline.get("cargo-lock-sha256")
        and run.baseline.get("cargo-lock-sha256") == run.current.get("cargo-lock-sha256")
        else "changed"
    )
    age, stale = metadata_age(run.baseline, config.report.stale_days)
    sha = run.baseline.get("commit", "?")[:7]
    lines = [
        " · ".join(rows) + f" · Cargo.lock: {lock}",
        f"Baseline: main@{sha} ({age})"
        + (f" — ⚠ older than {config.report.stale_days} days" if stale else ""),
    ]
    if not run.toolchain_matched:
        lines.extend(
            [
                "",
                "> ⚠ **Toolchain mismatch vs baseline** — instruction counts are not comparable; "
                "regressions are reported as warnings. Re-record the baseline from main "
                "(`Benchmarks` → Run workflow) after the toolchain settles.",
                "",
            ]
        )
        for key, old, new in run.mismatches:
            lines.append(f"> - `{key}`: baseline `{old}` vs this run `{new}`")
    return lines


def _has_allocations(benchmarks: list[Benchmark]) -> bool:
    return any(
        benchmark.dhat_blocks.old is not None
        or benchmark.dhat_blocks.new is not None
        or benchmark.dhat_bytes.old is not None
        or benchmark.dhat_bytes.new is not None
        for benchmark in benchmarks
    )


def table(benchmarks: list[Benchmark], config: Config) -> list[str]:
    header = "| Benchmark | Case | Ir baseline → PR | Δ Ir | Δ Ir% | Verdict |"
    divider = "| --- | --- | ---: | ---: | ---: | --- |"
    include_allocations = _has_allocations(benchmarks)
    if include_allocations:
        header = header[:-1] + "| Alloc blocks | Alloc bytes |"
        divider = divider[:-1] + "| ---: | ---: |"

    lines = [header, divider]
    for benchmark in benchmarks:
        text = TIER_LABELS.get(benchmark.tier, benchmark.tier)
        if benchmark.waiver:
            text += f" — {benchmark.waiver}"
        cells = [
            config.display_name(benchmark.function),
            config.case_name(benchmark.function, benchmark.case, benchmark.suite) or "—",
            f"{number(benchmark.ir.old)} → {number(benchmark.ir.new)}",
            signed(benchmark.ir.delta),
            percent(benchmark.ir.pct),
            text,
        ]
        if include_allocations:
            cells.append(
                f"{number(benchmark.dhat_blocks.old)} → {number(benchmark.dhat_blocks.new)}"
            )
            cells.append(
                f"{number(benchmark.dhat_bytes.old)} → {number(benchmark.dhat_bytes.new)}"
            )
        lines.append("| " + " | ".join(cells) + " |")
    return lines


def _chart_points(benchmarks: list[Benchmark], config: Config) -> list[str]:
    policy = config.policy
    changed = [b for b in benchmarks if b.ir.delta not in (None, 0)]
    changed.sort(key=lambda b: abs(b.ir.delta or 0), reverse=True)
    changed = changed[: config.report.max_chart_bars]
    if not changed:
        return []

    labels = [config.label(b.function, b.case, b.suite).replace('"', "'") for b in changed]
    values = {tier: [] for tier in ("regressed", "drift", "improved")}
    for benchmark in changed:
        pct = benchmark.ir.pct
        if pct in (None, float("inf"), float("-inf")):
            pct = 100.0 if pct == float("inf") else (-100.0 if pct == float("-inf") else 0.0)
        for tier in values:
            values[tier].append(pct if benchmark.tier == tier else 0.0)

    worst = min(-policy.backstop_ir, *values["regressed"], *values["drift"], *values["improved"], -5.0)
    best = max(policy.backstop_ir, *values["regressed"], *values["drift"], *values["improved"], 5.0)
    note = ""
    if best > 50 or worst < -50:
        note = " (axis clipped at ±50%)"
        best = min(best, 50.0)
        worst = max(worst, -50.0)

    def bars(points: list[float]) -> str:
        return f"    bar [{', '.join(f'{point:.2f}' for point in points)}]"

    def line(value: float) -> str:
        return f"    line [{', '.join(f'{value:.2f}' for _ in labels)}]"

    return [
        "```mermaid",
        "---",
        "config:",
        "  themeVariables:",
        "    xyChart:",
        '      plotColorPalette: "#cf222e, #9a6700, #1a7f37, #8c959f, #d0d7de, #d0d7de"',
        "---",
        "xychart-beta",
        f'    title "ΔIr% vs baseline — red = regression, amber = drift, green = improved{note}"',
        "    x-axis [" + ", ".join(f'"{label}"' for label in labels) + "]",
        f'    y-axis "Δ %" {worst:.2f} --> {best:.2f}',
        bars(values["regressed"]),
        bars(values["drift"]),
        bars(values["improved"]),
        line(policy.limit_ir),
        line(policy.backstop_ir),
        line(-policy.backstop_ir),
        "```",
    ]


def _capped(benchmarks: list[Benchmark], limit: int) -> tuple[list[Benchmark], int]:
    if len(benchmarks) <= limit:
        return benchmarks, 0
    return benchmarks[:limit], len(benchmarks) - limit


def _details(title: str, body: list[str]) -> list[str]:
    return ["<details>", f"<summary>{title}</summary>", "", *body, "", "</details>", ""]


def _comparison_table(
    benchmarks: list[Benchmark],
    inventory: Inventory,
    suite: str,
    reference: str,
    config: Config,
) -> list[str]:
    reference_cases = inventory.reference_cases(suite, reference)
    if not reference_cases:
        return []
    members = sorted(
        inventory.comparison_members(suite, reference), key=config.display_name
    )
    if not members:
        return []

    suite_benchmarks = [b for b in benchmarks if b.suite == suite]
    reference_ir = {
        b.case: b.ir.new for b in suite_benchmarks if b.function == reference
    }
    cases = sorted(
        (case for case in reference_cases if reference_ir.get(case) is not None),
        key=natural_key,
    )
    if not cases:
        return []

    header = "| Case | " + f"{config.display_name(reference)} (Ir)" + " | "
    header += " | ".join(config.display_name(member) for member in members) + " |"
    divider = "| --- | ---: | " + " | ".join("---:" for _ in members) + " |"
    title = f"{config.suite_display(suite).capitalize()} vs {config.display_name(reference)}"
    lines = [f"**{title}**", "", header, divider]

    by_key = {(b.function, b.case): b.ir.new for b in suite_benchmarks}
    for case in cases:
        base = reference_ir[case]
        cells = [f"{number(base) if base is not None else '—'}"]
        for member in members:
            value = by_key.get((member, case))
            if value is None:
                cells.append("—")
            elif base:
                cells.append(f"{number(value)} ({value / base:.2f}×)")
            else:
                cells.append(number(value))
        lines.append(
            f"| {config.case_name(reference, case, suite) or case} | "
            + " | ".join(cells)
            + " |"
        )
    lines.append("")
    return lines


def _cross_implementation(
    benchmarks: list[Benchmark], config: Config, limit: int
) -> list[str]:
    inventory = build_inventory(benchmarks)
    groups = [
        _comparison_table(benchmarks, inventory, suite, reference, config)
        for suite, reference in config.comparisons.items()
    ]
    groups = [group for group in groups if group]
    if not groups:
        return []
    lines = [
        "Ratios are instruction counts within this run, normalized to the implementation "
        "named in the first column. They are informative only — improving the reference "
        "implementation would otherwise look like a regression.",
        "",
    ]
    for group in groups[:limit]:
        lines.extend(group)
    return lines


def _totals_line(run: RunSummary, config: Config) -> list[str]:
    totals = suite_totals(run.benchmarks)
    if not totals:
        return []
    parts = []
    for suite, (old, new) in sorted(totals.items()):
        pct = (new - old) / old * 100 if old else 0.0
        parts.append(f"`{config.suite_display(suite)}` {percent(pct)} total Ir")
    return ["**Suite totals (orientation only):** " + " · ".join(parts), ""]


def _changed_sections(run: RunSummary, config: Config) -> list[str]:
    lines = []
    for tier, title in (
        ("regressed", "🔴 Regressions"),
        ("waived", "⚠ Waived regressions"),
        ("warning", "⚠ Warnings"),
    ):
        selected = [b for b in run.benchmarks if b.tier == tier]
        selected.sort(key=lambda b: abs(b.ir.delta or 0), reverse=True)
        selected, extra = _capped(selected, config.report.max_table_rows)
        if selected:
            lines.extend([f"### {title}", "", *table(selected, config), ""])
            if extra:
                lines.extend([f"*… and {extra} more; see the `bench-results` artifact.*", ""])
    return lines


def _chart_section(run: RunSummary, config: Config) -> list[str]:
    chart = _chart_points(run.benchmarks, config)
    if not chart:
        return ["✅ No measurable instruction changes.", ""]
    return [*chart, ""]


def _collapsed_sections(run: RunSummary, config: Config) -> list[str]:
    lines = []
    for tier, title in (("drift", "🟡 Drift within limits"), ("improved", "🟢 Improvements")):
        selected = [b for b in run.benchmarks if b.tier == tier]
        selected, extra = _capped(selected, config.report.max_table_rows)
        if not selected:
            continue
        body = table(selected, config)
        if extra:
            body.extend([f"*… and {extra} more.*", ""])
        lines.extend(_details(f"{title} ({len(selected) + extra})", body))

    comparisons = _cross_implementation(
        run.benchmarks, config, config.report.max_table_rows
    )
    if comparisons:
        lines.extend(_details("Cross-implementation ratios", comparisons))
    return lines


def _all_benchmarks_section(run: RunSummary, config: Config) -> list[str]:
    selected, extra = _capped(run.benchmarks, config.report.max_table_rows)
    body = []
    for suite in sorted({b.suite for b in selected}):
        body.extend(
            [
                f"**{config.suite_display(suite)}**",
                "",
                *table([b for b in selected if b.suite == suite], config),
                "",
            ]
        )
    if extra:
        body.extend(
            [
                f"*… and {extra} more; the complete list is in the `bench-results` "
                "artifact.*",
                "",
            ]
        )
    return _details(f"All benchmarks ({len(run.benchmarks)})", body)


def build_report(run: RunSummary, config: Config) -> str:
    lines = ["## 🔬 Bench report", "", headline(run), "", *metadata_block(run, config), ""]
    if run.baseline_found:
        lines.extend(_totals_line(run, config))
        lines.extend(_changed_sections(run, config))
        lines.extend(_chart_section(run, config))
        lines.extend(_collapsed_sections(run, config))
        lines.extend(_all_benchmarks_section(run, config))

    lines.extend(
        [
            "---",
            "",
            "*Methodology: one-shot Valgrind Callgrind and DHAT — counts are exact integers, "
            "not statistical samples, so there are no confidence intervals. The chart lines "
            "show the configured acceptance limit and gungraun's compiled-in soft limit as a "
            "reference band; the workflow gates on any reproducible increase above the "
            "instruction floor. Cross-implementation ratios are informational and never "
            "gated. Flamegraph SVGs and the raw `summary.json` files are attached to the "
            "run artifacts.*",
            "",
        ]
    )
    return "\n".join(lines)
