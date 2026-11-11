#!/usr/bin/env python3
"""Publish the benchmark report to the Checks and Status APIs.

Only `GITHUB_TOKEN`'s built-in API access is used (stdlib `urllib`, no extra tooling).
Publishing is best-effort: fork pull requests get a read-only token, so permission errors
are reported as warnings instead of failing the job. The regression gate lives in a
separate workflow step so a publishing failure can never mask it.

Environment: GITHUB_TOKEN, GITHUB_REPOSITORY, GITHUB_SHA, GITHUB_API_URL (or
GITHUB_SERVER_URL), RUN_URL. Run with `--dry-run` to print the payloads locally.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.request
from pathlib import Path


def api_base() -> str:
    return os.environ.get("GITHUB_API_URL", "https://api.github.com")


def post(path: str, payload: dict, token: str, dry_run: bool) -> bool:
    if dry_run:
        print(f"POST {path}")
        print(json.dumps(payload, indent=2))
        return True

    request = urllib.request.Request(
        f"{api_base()}{path}",
        data=json.dumps(payload).encode(),
        headers={
            "Authorization": f"Bearer {token}",
            "Accept": "application/vnd.github+json",
            "Content-Type": "application/json",
            "X-GitHub-Api-Version": "2022-11-28",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(request) as response:
            response.read()
    except urllib.error.HTTPError as error:
        if error.code in (401, 403, 404):
            print(
                f"warning: could not publish {path} ({error.code}); "
                "the token is probably read-only (fork PR)",
                file=sys.stderr,
            )
            return False
        print(f"warning: publishing {path} failed ({error.code})", file=sys.stderr)
        return False
    return True


def annotations(verdict: dict) -> list[dict]:
    items = []
    for regression in verdict.get("regressions", [])[:50]:
        path = regression.get("source")
        # Annotations must point at a file in the repository; suites without a configured
        # or conventional source file simply do not get one.
        if not path or not Path(path).is_file():
            continue
        message = f"{regression.get('display', 'benchmark')}: "
        message += "; ".join(regression.get("reasons", [])[:3])
        items.append(
            {
                "path": path,
                "start_line": 1,
                "end_line": 1,
                "annotation_level": "failure",
                "title": "Benchmark regression",
                "message": message[:1000],
            }
        )
    return items


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--verdict", type=Path, default=Path("verdict.json"))
    parser.add_argument("--report", type=Path, default=Path("bench-report.md"))
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    verdict = json.loads(args.verdict.read_text())
    report = args.report.read_text()
    token = os.environ.get("GITHUB_TOKEN", "")
    repository = os.environ.get("GITHUB_REPOSITORY", "")
    sha = os.environ.get("GITHUB_SHA", "")
    run_url = os.environ.get("RUN_URL", "")

    if not args.dry_run and not (token and repository and sha):
        print("warning: missing GITHUB_TOKEN/GITHUB_REPOSITORY/GITHUB_SHA", file=sys.stderr)
        return 0

    summary = report[:65000]
    check_run = {
        "name": "Bench report",
        "head_sha": sha,
        "status": "completed",
        "conclusion": verdict["conclusion"],
        "details_url": run_url or None,
        "output": {
            "title": verdict["title"][:255],
            "summary": summary,
            "annotations": annotations(verdict),
        },
    }
    # `details_url: null` is not accepted by the API; trim empty values.
    check_run = {key: value for key, value in check_run.items() if value is not None}
    post(f"/repos/{repository}/check-runs", check_run, token, args.dry_run)

    for context, status in verdict.get("statuses", {}).items():
        payload = {
            "state": status["state"],
            "context": context,
            "description": status["description"][:140],
        }
        if run_url:
            payload["target_url"] = run_url
        post(f"/repos/{repository}/statuses/{sha}", payload, token, args.dry_run)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
