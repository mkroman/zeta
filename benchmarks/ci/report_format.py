"""Shared formatting helpers for the report and its policy messages."""

from __future__ import annotations

import re

INF = float("inf")


def humanize(function: str) -> str:
    """Turn a Rust benchmark function name into a readable label."""
    return function.removeprefix("bench_").replace("_", " ")


def humanize_case(case: str) -> str:
    """Turn a benchmark case id into a readable label."""
    return case.replace("_", " ")


def number(value: int | None) -> str:
    return "—" if value is None else f"{value:,}"


def signed(value: int | None) -> str:
    if value is None:
        return "—"
    return f"+{value:,}" if value > 0 else f"{value:,}"


def percent(value: float | None) -> str:
    if value is None:
        return "—"
    if value == INF:
        return "new"
    if value == -INF:
        return "gone"
    return f"{value:+.2f}%"


def natural_key(case: str | None) -> list[tuple[int, object]]:
    """Sort case ids naturally (`case_2` before `case_10`)."""
    return [
        (0, int(part)) if part.isdigit() else (1, part)
        for part in re.split(r"(\d+)", case or "")
    ]
