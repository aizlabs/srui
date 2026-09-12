"""Regression tests for the feature ledger's evidence and ownership gates."""

import copy
import json
from pathlib import Path
import unittest

from validate_feature_ledger import validate_ledger


class LedgerValidationTests(unittest.TestCase):
    def setUp(self):
        root = Path(__file__).parent
        self.document = json.loads((root / "feature-ledger.px000.json").read_text())
        self.ticket_ids = {
            task["id"] for task in json.loads((root / "task-index.json").read_text())["tasks"]
        }

    def test_current_ledger_and_default_index_pass(self):
        self.assertEqual(validate_ledger(self.document), [])

    def test_verified_entries_reject_placeholder_evidence(self):
        for item in (None, "", "  ", "TODO", "pending", "N/A", 1, False, {}, [],
                     {"reference": ""}, {"result": "passed"}, "https://"):
            with self.subTest(item=item):
                document = copy.deepcopy(self.document)
                document["entries"][0].update(status="verified", evidence=[item])
                self.assertTrue(any("evidence" in error for error in
                                    validate_ledger(document, self.ticket_ids)))

    def test_every_evidence_item_must_be_valid(self):
        self.document["entries"][0].update(
            status="verified", evidence=["docs/results.md#test", None]
        )
        self.assertTrue(any("evidence" in error for error in
                            validate_ledger(self.document, self.ticket_ids)))

    def test_verified_entries_require_evidence(self):
        self.document["entries"][0]["status"] = "verified"
        self.assertTrue(any("no evidence" in error for error in
                            validate_ledger(self.document, self.ticket_ids)))

    def test_reference_strings_and_structured_records_pass(self):
        for item in ("docs/results.md#test", "https://example.com/runs/123",
                     {"reference": "docs/results.md", "result": "65 tests passed"}):
            with self.subTest(item=item):
                document = copy.deepcopy(self.document)
                document["entries"][0].update(status="verified", evidence=[item])
                self.assertEqual(validate_ledger(document, self.ticket_ids), [])

    def test_unknown_and_malformed_owners_fail(self):
        for owner in ("PX-0088", "PX-003/PX-0088", "PX-003/", "/PX-003",
                      "", " ", None):
            with self.subTest(owner=owner):
                document = copy.deepcopy(self.document)
                document["entries"][0]["owner"] = owner
                self.assertTrue(any("owner" in error for error in
                                    validate_ledger(document, self.ticket_ids)))

    def test_removed_owner_fails_against_supplied_index(self):
        self.ticket_ids.remove("PX-003")
        self.assertTrue(any("unknown owner: 'PX-003'" in error for error in
                            validate_ledger(self.document, self.ticket_ids)))

    def test_multiple_and_gate_owners_pass(self):
        self.document["entries"][0]["owner"] = "PX-003 / PX-010-G01"
        self.assertEqual(validate_ledger(self.document, self.ticket_ids), [])


if __name__ == "__main__":
    unittest.main()
