"""Configuration for the benchmark report, with strict validation.

Everything project-specific lives in `report.toml` next to this file: display overrides,
suite metadata (status context, source file, shared case labels), policy thresholds and
cross-implementation comparison references. Adding a benchmark or a whole suite therefore
needs no Python changes — unknown entries fall back to derived names and paths.

Unknown or mistyped keys are rejected instead of silently falling back to defaults; names
that no longer match a measured benchmark are reported by `report.py --check-config`.
"""

from __future__ import annotations

import tomllib
from collections.abc import Mapping
from dataclasses import dataclass, field
from datetime import date, datetime, timezone
from pathlib import Path

from report_format import humanize, humanize_case

DEFAULT_CONFIG = Path(__file__).with_name("report.toml")
DEFAULT_WAIVERS = Path(__file__).with_name("waivers.toml")

_TOP_LEVEL = {"policy", "report", "display", "suites", "case_labels", "comparisons"}
_POLICY_KEYS = {"floor_ir", "limit_ir", "backstop_ir"}
_REPORT_KEYS = {"stale_days", "max_chart_bars", "max_table_rows"}
_SUITE_KEYS = {"display", "source", "context", "cases"}
_WAIVER_KEYS = {"benchmark", "case", "metric", "reason", "pr", "expires"}
_METRICS = {"Ir", "TotalBlocks", "TotalBytes"}


class ConfigError(ValueError):
    """A configuration file is malformed or uses keys the report does not understand."""


def _keys(table: Mapping, allowed: set[str], section: str) -> None:
    unknown = sorted(set(table) - allowed)
    if unknown:
        raise ConfigError(f"unknown key(s) in {section}: {', '.join(unknown)}")


def _table(data: Mapping, key: str, section: str) -> Mapping:
    value = data.get(key)
    if value is None:
        return {}
    if not isinstance(value, Mapping):
        raise ConfigError(f"{section}.{key} must be a table")
    return value


def _string(
    table: Mapping, key: str, section: str, *, default: str | None = None, required: bool = False
) -> str | None:
    value = table.get(key, default)
    if value is None:
        if required:
            raise ConfigError(f"{section}.{key} is required")
        return None
    if not isinstance(value, str):
        raise ConfigError(f"{section}.{key} must be a string")
    return value


def _integer(
    table: Mapping, key: str, section: str, default: int, minimum: int = 0
) -> int:
    value = table.get(key, default)
    if not isinstance(value, int) or isinstance(value, bool):
        raise ConfigError(f"{section}.{key} must be an integer")
    if value < minimum:
        raise ConfigError(f"{section}.{key} must be >= {minimum}")
    return value


def _number(table: Mapping, key: str, section: str, default: float, minimum: float = 0.0) -> float:
    value = table.get(key, default)
    if not isinstance(value, (int, float)) or isinstance(value, bool):
        raise ConfigError(f"{section}.{key} must be a number")
    if value < minimum:
        raise ConfigError(f"{section}.{key} must be >= {minimum}")
    return float(value)


def _string_map(table: Mapping, key: str, section: str) -> dict[str, str]:
    value = table.get(key)
    if value is None:
        return {}
    if not isinstance(value, Mapping):
        raise ConfigError(f"{section}.{key} must be a table of strings")
    for name, label in value.items():
        if not isinstance(label, str):
            raise ConfigError(f"{section}.{key}.{name} must be a string")
    return dict(value)


@dataclass(frozen=True)
class Policy:
    #: A positive Ir delta below this many instructions is churn on trivial benches.
    floor_ir: int = 16
    #: Exact counts with a matched toolchain gate on any reproducible increase.
    limit_ir: float = 0.0
    #: The compiled-in gungraun soft limit, shown as the reference band.
    backstop_ir: float = 1.0


@dataclass(frozen=True)
class ReportLimits:
    """Presentation limits, separate from the gating policy."""

    stale_days: int = 7
    max_chart_bars: int = 12
    max_table_rows: int = 120


@dataclass(frozen=True)
class SuiteConfig:
    display: str | None = None
    #: Source file regressions are annotated against.
    source: str | None = None
    #: Commit status context (for example `bench/join`).
    context: str | None = None
    #: Case labels shared by every benchmark in the suite.
    cases: Mapping[str, str] = field(default_factory=dict)


@dataclass
class Config:
    policy: Policy = field(default_factory=Policy)
    report: ReportLimits = field(default_factory=ReportLimits)
    #: Optional `function -> display name` overrides.
    display: dict[str, str] = field(default_factory=dict)
    suites: dict[str, SuiteConfig] = field(default_factory=dict)
    #: Optional `function -> { case id -> label }` overrides.
    case_labels: dict[str, Mapping[str, str]] = field(default_factory=dict)
    #: `suite -> reference benchmark`; members are discovered by shared case ids.
    comparisons: dict[str, str] = field(default_factory=dict)

    def display_name(self, function: str) -> str:
        return self.display.get(function) or humanize(function)

    def case_name(self, function: str, case: str | None, suite: str | None = None) -> str:
        if not case:
            return ""
        override = self.case_labels.get(function)
        if override and case in override:
            return override[case]
        if suite:
            configured = self.suites.get(suite)
            if configured and case in configured.cases:
                return configured.cases[case]
        return humanize_case(case)

    def label(self, function: str, case: str | None, suite: str | None = None) -> str:
        name = self.display_name(function)
        case_label = self.case_name(function, case, suite)
        return f"{name} · {case_label}" if case_label else name

    def suite_display(self, suite: str) -> str:
        configured = self.suites.get(suite)
        return configured.display if configured and configured.display else suite

    def suite_context(self, suite: str) -> str:
        configured = self.suites.get(suite)
        if configured and configured.context:
            return configured.context
        return f"bench/{suite}"

    def suite_source(self, suite: str) -> str | None:
        configured = self.suites.get(suite)
        if configured and configured.source:
            return configured.source
        return f"benchmarks/benches/{suite}.rs"


def load_config(path: Path | None = None) -> Config:
    path = path or DEFAULT_CONFIG
    if not path.is_file():
        return Config()
    try:
        data = tomllib.loads(path.read_text())
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise ConfigError(f"{path}: {error}") from error

    _keys(data, _TOP_LEVEL, "report.toml")

    policy_data = _table(data, "policy", "report.toml")
    _keys(policy_data, _POLICY_KEYS, "[policy]")
    policy_defaults = Policy()
    policy = Policy(
        floor_ir=_integer(policy_data, "floor_ir", "[policy]", policy_defaults.floor_ir),
        limit_ir=_number(policy_data, "limit_ir", "[policy]", policy_defaults.limit_ir),
        backstop_ir=_number(
            policy_data, "backstop_ir", "[policy]", policy_defaults.backstop_ir
        ),
    )

    report_data = _table(data, "report", "report.toml")
    _keys(report_data, _REPORT_KEYS, "[report]")
    report_defaults = ReportLimits()
    report = ReportLimits(
        stale_days=_integer(
            report_data, "stale_days", "[report]", report_defaults.stale_days
        ),
        max_chart_bars=_integer(
            report_data, "max_chart_bars", "[report]", report_defaults.max_chart_bars, minimum=1
        ),
        max_table_rows=_integer(
            report_data, "max_table_rows", "[report]", report_defaults.max_table_rows, minimum=1
        ),
    )

    suites = {}
    for name, raw in _table(data, "suites", "report.toml").items():
        section = f"[suites.{name}]"
        if not isinstance(raw, Mapping):
            raise ConfigError(f"{section} must be a table")
        _keys(raw, _SUITE_KEYS, section)
        suites[name] = SuiteConfig(
            display=_string(raw, "display", section),
            source=_string(raw, "source", section),
            context=_string(raw, "context", section),
            cases=_string_map(raw, "cases", section),
        )

    case_labels = {}
    for function, raw in _table(data, "case_labels", "report.toml").items():
        section = f'[case_labels."{function}"]'
        if not isinstance(raw, Mapping):
            raise ConfigError(f"{section} must be a table")
        for case, label in raw.items():
            if not isinstance(label, str):
                raise ConfigError(f"{section}.{case} must be a string")
        case_labels[function] = dict(raw)

    comparisons = {}
    for suite, reference in _table(data, "comparisons", "report.toml").items():
        if not isinstance(reference, str):
            raise ConfigError(f"[comparisons].{suite} must be a benchmark name")
        comparisons[suite] = reference

    return Config(
        policy=policy,
        report=report,
        display=_string_map(data, "display", "report.toml"),
        suites=suites,
        case_labels=case_labels,
        comparisons=comparisons,
    )


@dataclass(frozen=True)
class Waiver:
    benchmark: str
    reason: str
    case: str | None = None
    metric: str = "Ir"
    pr: int | None = None
    expires: str | None = None

    def expiry_date(self) -> date | None:
        if not self.expires:
            return None
        try:
            return datetime.strptime(self.expires, "%Y-%m-%d").date()
        except ValueError:
            return None

    def expired(self) -> bool:
        expiry = self.expiry_date()
        return expiry is not None and expiry < datetime.now(timezone.utc).date()

    def matches(self, function: str, case: str | None, metric: str) -> bool:
        if self.benchmark != function or self.metric != metric:
            return False
        if self.case and self.case != case:
            return False
        return not self.expired()

    def describe(self) -> str:
        return f"{self.reason} (expires {self.expires})" if self.expires else self.reason


def load_waivers(path: Path | None = None) -> list[Waiver]:
    path = path or DEFAULT_WAIVERS
    if not path.is_file():
        return []
    try:
        data = tomllib.loads(path.read_text())
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise ConfigError(f"{path}: {error}") from error

    _keys(data, {"waiver"}, "waivers.toml")
    raw_waivers = data.get("waiver") or []
    if not isinstance(raw_waivers, list):
        raise ConfigError("waivers.toml: `waiver` must be an array of tables")

    waivers = []
    for index, raw in enumerate(raw_waivers, start=1):
        section = f"waivers.toml [[waiver]] #{index}"
        if not isinstance(raw, Mapping):
            raise ConfigError(f"{section} must be a table")
        _keys(raw, _WAIVER_KEYS, section)
        metric = _string(raw, "metric", section, default="Ir")
        if metric not in _METRICS:
            raise ConfigError(
                f"{section}.metric must be one of: {', '.join(sorted(_METRICS))}"
            )
        expires = _string(raw, "expires", section)
        if expires:
            try:
                datetime.strptime(expires, "%Y-%m-%d")
            except ValueError as error:
                raise ConfigError(
                    f"{section}.expires must be a YYYY-MM-DD date"
                ) from error
        pr = raw.get("pr")
        if pr is not None and (not isinstance(pr, int) or isinstance(pr, bool)):
            raise ConfigError(f"{section}.pr must be an integer")
        waivers.append(
            Waiver(
                benchmark=_string(raw, "benchmark", section, required=True) or "",
                reason=_string(raw, "reason", section, required=True) or "",
                case=_string(raw, "case", section),
                metric=metric,
                pr=pr,
                expires=expires,
            )
        )
    return waivers
