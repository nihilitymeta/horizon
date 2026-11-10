import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "tools/fork-sync/api-docs-both.py"

DIFF3 = """# Native mods

## Hooks

<<<<<<< HEAD
API 2 adds mine_hook.
The fork's own hook.

API 3 adds shared_hook.
||||||| base
API 2 adds base_only_hook.
Upstream's base text, which must not survive.
=======
API 3 adds shared_hook.

API 4 adds theirs_hook.
>>>>>>> upstream

## What the launcher does
"""


class ApiDocsBothTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.path = Path(self.tmp.name) / "native-mods.md"
        self.path.write_text(DIFF3)

    def run_script(self):
        return subprocess.run(
            [sys.executable, "-I", str(SCRIPT), str(self.path)], capture_output=True, text=True
        )

    def test_diff3_base_section_is_not_kept(self):
        proc = self.run_script()
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        out = self.path.read_text()
        self.assertNotIn("base_only_hook", out)
        self.assertNotIn("Upstream's base text", out)
        self.assertNotIn("|||||||", out)

    def test_both_sides_are_kept_once_and_no_marker_is_left(self):
        self.run_script()
        out = self.path.read_text()
        self.assertIn("mine_hook", out)
        self.assertIn("theirs_hook", out)
        self.assertEqual(out.count("shared_hook"), 1)
        self.assertNotIn("<<<<<<<", out)
        self.assertNotIn("=======", out)
        self.assertNotIn(">>>>>>>", out)
        self.assertTrue(out.endswith("## What the launcher does\n"))


if __name__ == "__main__":
    unittest.main()
