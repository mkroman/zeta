"""Reading Gungraun summary files and benchmark environment metadata.

The summary schema is versioned by gungraun (`version`); this module understands the
fields the report needs and skips anything it cannot parse, so old artifacts degrade to
"not part of this report" instead of crashing the workflow.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path

IR = "Ir"
CYCLES = "EstimatedCycles"
DHAT_BLOCKS = "TotalBlocks"
DHAT_BYTES = "TotalBytes"

#: Toolchain metadata that must match the baseline for exact-count gating.
TOOLCHAIN_KEYS = ("toolchain", "rustc-release", "rustc-commit", "valgrind", "gungraun-runner")


@dataclass
class Metrics:
    new: int | None = None
    old: int | None = None
    pct: float | None = None

    @property
    def delta(self) -> int | None:
        if self.new is None or self.old is None:
            return None
        return self.new - self.old


@dataclass
class Benchmark:
    """One benchmark case, independent of how suites or display names are configured."""

    #: The benchmark file (for example `join`), which defines the suite by default.
    suite: str
    group: str
    function: str
    case: str | None
    output_dir: str = ""
    ir: Metrics = field(default_factory=Metrics)
    cycles: Metrics = field(default_factory=Metrics)
    dhat_blocks: Metrics = field(default_factory=Metrics)
    dhat_bytes: Metrics = field(default_factory=Metrics)
    tier: str = "unchanged"
    reasons: list[str] = field(default_factory=list)
    waiver: str | None = None


@dataclass
class RunSummary:
    """Everything the policy and renderer need for one benchmark run."""

    benchmarks: list[Benchmark]
    baseline: dict[str, str]
    current: dict[str, str]
    baseline_found: bool
    toolchain_matched: bool
    mismatches: list[tuple[str, str, str]]
    exit_code: int


def parse_pct(raw: object) -> float | None:
    if raw is None:
        return None
    if isinstance(raw, (int, float)):
        return float(raw)
    text = str(raw).strip()
    if text in ("inf", "+inf"):
        return float("inf")
    if text == "-inf":
        return float("-inf")
    try:
        return float(text)
    except ValueError:
        return None


def metric_from(profile: dict, key: str) -> Metrics:
    entry = profile.get("data", {}).get("total", {}).get("metrics", {}).get(key)
    if not entry:
        return Metrics()
    values = entry.get("values") or {}
    return Metrics(
        new=values.get("new"),
        old=values.get("old"),
        pct=parse_pct((entry.get("change") or {}).get("diff_pct")),
    )


def load_benchmarks(root: Path) -> list[Benchmark]:
    benchmarks: list[Benchmark] = []
    for summary_path in sorted(root.rglob("summary.json")):
        try:
            data = json.loads(summary_path.read_text())
        except (OSError, json.JSONDecodeError):
            continue
        profiles = data.get("profiles") or []
        callgrind = next((p for p in profiles if p.get("tool") == "Callgrind"), None)
        if callgrind is None:
            continue

        benchmark_file = data.get("benchmark_file") or (data.get("module_path") or "").split("::", 1)[0]
        suite = Path(benchmark_file).stem or "unknown"
        if not suite:
            continue

        benchmark = Benchmark(
            suite=suite,
            group=data.get("group") or "",
            function=data.get("function_name") or summary_path.parent.name,
            case=data.get("id"),
            output_dir=data.get("output_dir") or "",
            ir=metric_from(callgrind, IR),
            cycles=metric_from(callgrind, CYCLES),
        )
        dhat = next((p for p in profiles if p.get("tool") == "DHAT"), None)
        if dhat is not None:
            benchmark.dhat_blocks = metric_from(dhat, DHAT_BLOCKS)
            benchmark.dhat_bytes = metric_from(dhat, DHAT_BYTES)
        benchmarks.append(benchmark)

    benchmarks.sort(key=lambda b: (b.suite, b.function, b.case or ""))
    return benchmarks


def read_metadata(path: Path) -> dict[str, str]:
    if not path.is_file():
        return {}
    metadata = {}
    for line in path.read_text().splitlines():
        key, separator, value = line.partition("=")
        if separator:
            metadata[key.strip()] = value.strip()
    return metadata


def metadata_age(metadata: dict[str, str], stale_days: int) -> tuple[str, bool]:
    raw = metadata.get("timestamp")
    if not raw:
        return "unknown age", False
    try:
        then = datetime.strptime(raw, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)
    except ValueError:
        return "unknown age", False
    days = (datetime.now(timezone.utc) - then).days
    age = "today" if days == 0 else f"{days}d old"
    return age, days > stale_days


def metadata_matches(
    baseline: dict[str, str], current: dict[str, str]
) -> tuple[bool, list[tuple[str, str, str]]]:
    # Without metadata on both sides there is nothing to compare; the caller decides
    # whether a baseline exists at all.
    if not baseline or not current:
        return True, []
    mismatches = []
    for key in TOOLCHAIN_KEYS:
        old = baseline.get(key, "")
        new = current.get(key, "")
        if old != new:
            mismatches.append((key, old or "?", new or "?"))
    return not mismatches, mismatches
