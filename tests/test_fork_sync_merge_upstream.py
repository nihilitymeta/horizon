import re
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "tools/fork-sync/merge-upstream.sh"


def gen_entries():
    m = re.search(r"^GEN='([^']*)'$", SCRIPT.read_text(), re.M)
    if m is None:
        raise AssertionError("merge-upstream.sh has no GEN='...' line")
    return [e for e in m.group(1).split("|") if e]


class GeneratedListTests(unittest.TestCase):
    def test_every_generated_entry_is_a_tracked_file(self):
        missing = []
        for path in gen_entries():
            out = subprocess.run(
                ["git", "ls-files", "--", path], cwd=ROOT, capture_output=True, text=True, check=True
            ).stdout
            if out.strip() != path:
                missing.append(path)
        self.assertEqual(missing, [], "GEN lists paths git does not track")


if __name__ == "__main__":
    unittest.main()
