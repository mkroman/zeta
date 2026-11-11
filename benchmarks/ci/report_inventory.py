"""Derived indexes over a run's measured benchmarks.

The report, the policy checks and the configuration check all need the same view of which
functions exist, which case ids they use and which functions are comparable. Building it
once keeps those rules in one place.
"""

from __future__ import annotations

from dataclasses import dataclass, field

from summary import Benchmark


@dataclass
class Inventory:
    #: `function -> set of case ids`
    cases_by_function: dict[str, set[str]] = field(default_factory=dict)
    #: `suite -> function -> set of case ids`
    functions_by_suite: dict[str, dict[str, set[str]]] = field(default_factory=dict)
    suites: set[str] = field(default_factory=set)

    def reference_cases(self, suite: str, reference: str) -> set[str] | None:
        return self.functions_by_suite.get(suite, {}).get(reference)

    def comparison_members(self, suite: str, reference: str) -> list[str]:
        """Functions in `suite` using exactly the reference's case-id set."""
        functions = self.functions_by_suite.get(suite, {})
        cases = functions.get(reference)
        if not cases:
            return []
        return sorted(
            function
            for function, function_cases in functions.items()
            if function != reference and function_cases == cases
        )


def build(benchmarks: list[Benchmark]) -> Inventory:
    inventory = Inventory()
    for benchmark in benchmarks:
        inventory.suites.add(benchmark.suite)
        inventory.cases_by_function.setdefault(benchmark.function, set())
        functions = inventory.functions_by_suite.setdefault(benchmark.suite, {})
        functions.setdefault(benchmark.function, set())
        if benchmark.case is not None:
            inventory.cases_by_function[benchmark.function].add(benchmark.case)
            functions[benchmark.function].add(benchmark.case)
    return inventory
