#!/usr/bin/env python3
import unittest

from validate_b1_evidence import EvidenceError, PRODUCTS, TASKS, validate


class B1EvidenceContractTests(unittest.TestCase):
    def test_not_run_requires_reason_and_has_no_winner(self) -> None:
        validate({"schema": 1, "release": "v0.20.0", "status": "NOT-RUN", "reason": "no competitor access", "runs": []})
        with self.assertRaises(EvidenceError):
            validate({"schema": 1, "release": "v0.20.0", "status": "NOT-RUN", "reason": "", "runs": []})
        with self.assertRaises(EvidenceError):
            validate({"schema": 1, "release": "v0.20.0", "status": "NOT-RUN", "reason": "blocked", "winner": "wancode", "runs": []})

    def test_complete_requires_full_three_by_three_matrix(self) -> None:
        runs = [
            {"task": task, "product": product, "trial": trial, "status": "PASS", "evidence": f"runs/{task}/{product}/{trial}"}
            for task in TASKS for product in PRODUCTS for trial in (1, 2, 3)
        ]
        record = {
            "schema": 1,
            "release": "v0.20.0",
            "status": "COMPLETE",
            "preregistration": {"task_set": "tasks/v1", "analysis_plan": "analysis-v1.md", "random_seed": "42"},
            "runs": runs,
        }
        validate(record)
        record["runs"] = runs[:-1]
        with self.assertRaises(EvidenceError):
            validate(record)


if __name__ == "__main__":
    unittest.main()
