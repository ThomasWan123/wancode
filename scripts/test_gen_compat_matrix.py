import tempfile
import unittest
from pathlib import Path

from gen_compat_matrix import MatrixInputError, cell, checked_doc_link, validate_provider


def provider(status="PARTIAL", evidence=None):
    if evidence is None:
        evidence = [{"label": "run", "href": "evidence.md"}]
    return {
        "name": "Provider",
        "preset_id": "provider",
        "endpoint": "example.test/v1",
        "live_status": status,
        "live_verification": "bounded claim",
        "live_evidence": evidence,
    }


class CompatMatrixValidationTests(unittest.TestCase):
    def test_non_not_run_claim_requires_evidence(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(MatrixInputError):
                validate_provider(provider(evidence=[]), Path(tmp))

    def test_not_run_rejects_pass_like_evidence(self):
        with tempfile.TemporaryDirectory() as tmp:
            (Path(tmp) / "evidence.md").write_text("evidence", encoding="utf-8")
            with self.assertRaises(MatrixInputError):
                validate_provider(provider(status="NOT-RUN"), Path(tmp))

    def test_missing_and_parent_links_are_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            with self.assertRaises(MatrixInputError):
                checked_doc_link("missing.md", root)
            with self.assertRaises(MatrixInputError):
                checked_doc_link("../outside.md", root)
            with self.assertRaises(MatrixInputError):
                checked_doc_link(".", root)

    def test_existing_in_tree_link_is_accepted(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "evidence.md").write_text("evidence", encoding="utf-8")
            checked = validate_provider(provider(), root)
            self.assertEqual(checked["live_evidence"][0]["href"], "evidence.md")

    def test_table_cells_escape_structure(self):
        self.assertEqual(cell("a|b\nnext"), "a\\|b next")


if __name__ == "__main__":
    unittest.main()
