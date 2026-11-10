import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "tools/fork-sync/json-both.py"


class JsonBothTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)

    def run_script(self, name, text):
        path = self.dir / name
        path.write_text(text)
        proc = subprocess.run(
            [sys.executable, "-I", str(SCRIPT), str(path)], capture_output=True, text=True
        )
        return proc, path

    def test_resolves_a_list_into_valid_json(self):
        text = (
            '{\n  "list": [\n    "a",\n<<<<<<< HEAD\n    "b"\n=======\n    "c"\n>>>>>>> other\n  ]\n}\n'
        )
        proc, path = self.run_script("ok.json", text)
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assertEqual(json.loads(path.read_text()), {"list": ["a", "b", "c"]})

    def test_diff3_base_section_is_skipped(self):
        # The base side's "x": 0 must not survive: only HEAD's and the other side's entries are kept.
        text = (
            '{\n<<<<<<< HEAD\n  "x": 1,\n||||||| base\n  "x": 0,\n=======\n  "y": 2,\n>>>>>>> other\n  "z": 3\n}\n'
        )
        proc, path = self.run_script("diff3.json", text)
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assertEqual(json.loads(path.read_text()), {"x": 1, "y": 2, "z": 3})

    def test_invalid_result_is_not_written_and_the_message_names_the_file(self):
        # An empty side after a comma leaves a trailing comma before the closing brace.
        text = '{\n  "a": 1,\n<<<<<<< HEAD\n  "b": 2,\n=======\n>>>>>>> other\n}\n'
        proc, path = self.run_script("trailing.json", text)
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("trailing.json", proc.stdout + proc.stderr)
        self.assertEqual(path.read_text(), text, "the conflicted file must stay as it was")

    def test_duplicate_keys_fail_and_are_not_written(self):
        text = (
            '{\n<<<<<<< HEAD\n  "k": 1\n=======\n  "k": 2\n>>>>>>> other\n}\n'
        )
        proc, path = self.run_script("dup.json", text)
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("dup.json", proc.stdout + proc.stderr)
        self.assertEqual(path.read_text(), text)

    def test_leftover_marker_fails_naming_the_file_and_is_not_written(self):
        text = '{\n  "a": 1,\n<<<<<<< HEAD\n  "b": 2\n}\n'
        proc, path = self.run_script("marker.json", text)
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("marker.json", proc.stdout + proc.stderr)
        self.assertEqual(path.read_text(), text)


if __name__ == "__main__":
    unittest.main()
