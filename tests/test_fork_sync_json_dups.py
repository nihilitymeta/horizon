import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "tools/fork-sync/json-dups.py"


class JsonDupsTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)

    def run_script(self, name, text):
        path = self.dir / name
        path.write_text(text)
        return subprocess.run(
            [sys.executable, "-I", str(SCRIPT), str(path)], capture_output=True, text=True
        )

    def test_clean_file_exits_zero_and_prints_nothing(self):
        proc = self.run_script("clean.json", '{"a": {"b": 1}, "c": [{"d": 2}]}')
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assertEqual(proc.stdout, "")

    def test_nested_duplicate_exits_one_and_prints_key_and_path(self):
        proc = self.run_script("nested.json", '{"outer": {"inner": 1, "inner": 2}, "x": 3}')
        self.assertEqual(proc.returncode, 1, proc.stdout + proc.stderr)
        out = proc.stdout + proc.stderr
        self.assertIn('"inner"', out)
        self.assertIn("$.outer", out)
        self.assertIn("nested.json", out)

    def test_each_duplicate_is_printed_with_its_own_path(self):
        proc = self.run_script("two.json", '{"a": 1, "a": 2, "list": [{"k": 1, "k": 1}]}')
        self.assertEqual(proc.returncode, 1)
        out = proc.stdout + proc.stderr
        self.assertIn("$", out)
        self.assertIn("$.list[0]", out)
        self.assertEqual(out.count('"a"'), 1)
        self.assertEqual(out.count('"k"'), 1)

    def test_invalid_json_exits_one_and_names_the_file(self):
        proc = self.run_script("broken.json", '{"a": 1,\n<<<<<<< HEAD\n}')
        self.assertEqual(proc.returncode, 1)
        self.assertIn("broken.json", proc.stdout + proc.stderr)


if __name__ == "__main__":
    unittest.main()
