"""Keep plan verification focused on ticket and ledger consistency."""
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


class PlanValidationTests(unittest.TestCase):
    def test_plan_checks_consistency_without_parsing_documentation_links(self):
        with tempfile.TemporaryDirectory(prefix="srui-plan-test-") as temporary:
            root = Path(temporary) / "plan"
            shutil.copytree(Path(__file__).parent, root,
                            ignore=shutil.ignore_patterns("__pycache__"))
            (root / "link-examples.md").write_text(
                '[guide](README.md "Guide title")\n\n'
                + "Use " + chr(96) + "[x](missing.md)" + chr(96) + " as syntax.\n"
                + "\n    [x](missing.md)\n\n[example](absent.md)\n"
            )
            command = [sys.executable, str(root / "validate_plan.py")]
            result = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            ledger = root / "feature-ledger.seed.json"
            document = json.loads(ledger.read_text())
            document["families"][0]["tickets"] = ["PX-999"]
            ledger.write_text(json.dumps(document))
            result = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            self.assertIn("Unknown owner PX-999", result.stdout)


if __name__ == "__main__":
    unittest.main()
