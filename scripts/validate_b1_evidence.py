#!/usr/bin/env python3
"""Fail-closed validator for B1 competitive benchmark root records."""
from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any

TASKS = {f"B{i:02d}" for i in range(1, 11)}
PRODUCTS = {"wancode", "codex", "claude-code"}
TRIAL_STATUSES = {"PASS", "FAIL", "NOT-RUN", "INVALID"}


class EvidenceError(ValueError):
    pass


def _nonempty(value: Any, field: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise EvidenceError(f"{field} must be a non-empty string")
    return value.strip()


def validate(record: Any) -> None:
    if not isinstance(record, dict):
        raise EvidenceError("top level must be an object")
    if record.get("schema") != 1:
        raise EvidenceError("schema must be 1")
    _nonempty(record.get("release"), "release")
    status = record.get("status")
    runs = record.get("runs")
    if not isinstance(runs, list):
        raise EvidenceError("runs must be an array")

    if status == "NOT-RUN":
        _nonempty(record.get("reason"), "reason")
        if runs:
            raise EvidenceError("NOT-RUN must not carry comparison runs")
        if record.get("winner") is not None:
            raise EvidenceError("NOT-RUN must not name a winner")
        return
    if status != "COMPLETE":
        raise EvidenceError("status must be COMPLETE or NOT-RUN")

    prereg = record.get("preregistration")
    if not isinstance(prereg, dict):
        raise EvidenceError("COMPLETE requires preregistration")
    for field in ("task_set", "analysis_plan", "random_seed"):
        _nonempty(prereg.get(field), f"preregistration.{field}")

    seen: dict[tuple[str, str], set[int]] = {}
    for index, run in enumerate(runs):
        if not isinstance(run, dict):
            raise EvidenceError(f"runs[{index}] must be an object")
        task = run.get("task")
        product = run.get("product")
        trial = run.get("trial")
        if task not in TASKS:
            raise EvidenceError(f"runs[{index}].task is not B01-B10")
        if product not in PRODUCTS:
            raise EvidenceError(f"runs[{index}].product is unknown")
        if type(trial) is not int or trial not in {1, 2, 3}:
            raise EvidenceError(f"runs[{index}].trial must be 1, 2, or 3")
        if run.get("status") not in TRIAL_STATUSES:
            raise EvidenceError(f"runs[{index}].status is invalid")
        _nonempty(run.get("evidence"), f"runs[{index}].evidence")
        key = (task, product)
        trials = seen.setdefault(key, set())
        if trial in trials:
            raise EvidenceError(f"duplicate trial {task}/{product}/{trial}")
        trials.add(trial)

    expected = {(task, product) for task in TASKS for product in PRODUCTS}
    if set(seen) != expected or any(trials != {1, 2, 3} for trials in seen.values()):
        raise EvidenceError("COMPLETE requires 3 trials for every B01-B10/product pair")


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print("usage: validate_b1_evidence.py <record.json>", file=sys.stderr)
        return 2
    path = Path(argv[1])
    try:
        validate(json.loads(path.read_text(encoding="utf-8")))
    except (OSError, json.JSONDecodeError, EvidenceError) as exc:
        print(f"B1 evidence invalid: {exc}", file=sys.stderr)
        return 1
    print(f"B1 evidence valid: {path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
