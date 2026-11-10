import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "tools/fork-sync/game-checks.sh"
CHECKS = ["check.sh", "docs-check.sh", "scaffold-check.sh", "example-check.sh", "hook-coverage.sh"]


class GameChecksKillTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        base = Path(self.tmp.name)
        self.modkit = base / "modkit"
        (self.modkit / "tools").mkdir(parents=True)
        for name in CHECKS:
            stub = self.modkit / "tools" / name
            stub.write_text("#!/bin/sh\necho stub-ok\n")
            stub.chmod(0o755)
        self.bin = base / "bin"
        self.bin.mkdir()
        self.log = base / "pkill.log"
        self.write_tool("pkill", f'printf "%s\\n" "$*" >> "{self.log}"\n')
        self.write_tool("sleep", "exit 0\n")
        self.home = base / "home"
        self.home.mkdir()

    def write_tool(self, name, body):
        tool = self.bin / name
        tool.write_text("#!/bin/sh\n" + body)
        tool.chmod(0o755)

    def test_leftover_game_is_killed_by_exact_name_never_by_pattern(self):
        env = {
            "PATH": f"{self.bin}:/usr/bin:/bin",
            "HOME": str(self.home),
            "TMPDIR": self.tmp.name,
        }
        proc = subprocess.run(
            ["bash", str(SCRIPT), str(self.modkit)],
            env=env,
            capture_output=True,
            text=True,
            timeout=60,
        )
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        calls = self.log.read_text().splitlines() if self.log.exists() else []
        self.assertEqual(len(calls), len(CHECKS), calls)
        for call in calls:
            args = call.split()
            self.assertNotIn("-f", args, "pkill -f matches the script's own command line")
            self.assertEqual(args, ["-x", "horizon-client"], call)


if __name__ == "__main__":
    unittest.main()
