import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "tools/fork-sync/metal-launch.sh"

# A stand-in horizon-ctl: it records its arguments and the game-home variables it was started with.
FAKE_CTL = """#!/bin/sh
{
  echo "ARGS: $*"
  echo "HORIZON_HOME=$HORIZON_HOME"
  echo "HORIZON_LAUNCHER_HOME=$HORIZON_LAUNCHER_HOME"
  echo "XDG_DATA_HOME=$XDG_DATA_HOME"
  echo "XDG_CONFIG_HOME=$XDG_CONFIG_HOME"
  echo "HORIZON_LAUNCHER_NO_SELF_UPDATE=$HORIZON_LAUNCHER_NO_SELF_UPDATE"
  echo "HORIZON_NO_AUDIO=$HORIZON_NO_AUDIO"
} >> "$FAKE_LOG"
exit 0
"""


class MetalLaunchDefaultsTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        base = Path(self.tmp.name)
        self.home = base / "home"
        self.real_save = self.home / "Library/Application Support/horizon"
        self.real_save.mkdir(parents=True)
        self.tmpdir = base / "tmp"
        self.tmpdir.mkdir()
        self.checkout = base / "checkout"
        (self.checkout / "tools/fork-sync").mkdir(parents=True)
        shutil.copy(SCRIPT, self.checkout / "tools/fork-sync/metal-launch.sh")
        (self.checkout / "target/fast").mkdir(parents=True)
        self.log = base / "ctl.log"
        ctl = self.checkout / "target/fast/horizon-ctl"
        ctl.write_text(FAKE_CTL)
        ctl.chmod(0o755)
        self.bin = base / "bin"
        self.bin.mkdir()
        for name in ("caffeinate", "sleep"):
            tool = self.bin / name
            tool.write_text("#!/bin/sh\nexit 0\n")
            tool.chmod(0o755)

    def run_script(self, extra_env=None):
        env = {
            "PATH": f"{self.bin}:/usr/bin:/bin",
            "HOME": str(self.home),
            "TMPDIR": str(self.tmpdir),
            "FAKE_LOG": str(self.log),
        }
        env.update(extra_env or {})
        script = self.checkout / "tools/fork-sync/metal-launch.sh"
        return subprocess.run(
            ["bash", str(script), str(self.tmpdir / "shot.png")],
            env=env,
            capture_output=True,
            text=True,
            timeout=60,
        )

    def launch_record(self):
        lines = self.log.read_text().splitlines()
        launch = next(line for line in lines if line.startswith("ARGS: ") and " launch " in line)
        return launch, {k: v for k, _, v in (line.partition("=") for line in lines if "=" in line and not line.startswith("ARGS"))}

    def test_default_game_dir_is_a_fresh_scratch_folder_not_the_player_save(self):
        proc = self.run_script()
        self.assertTrue(self.log.exists(), proc.stdout + proc.stderr)
        launch, env = self.launch_record()
        game_dir = launch.split("--game-dir ")[1].split(" --resources")[0].strip()
        self.assertNotEqual(Path(game_dir), self.real_save)
        self.assertFalse(Path(game_dir).is_relative_to(self.home), game_dir)
        self.assertTrue(Path(game_dir).is_relative_to(self.tmpdir), game_dir)
        self.assertEqual(env["HORIZON_HOME"], game_dir)

    def test_launcher_and_xdg_homes_are_scratch_and_updates_and_audio_are_off(self):
        self.run_script()
        _, env = self.launch_record()
        for key in ("HORIZON_LAUNCHER_HOME", "XDG_DATA_HOME", "XDG_CONFIG_HOME"):
            self.assertTrue(env[key], key)
            self.assertFalse(Path(env[key]).is_relative_to(self.home), key)
            self.assertTrue(Path(env[key]).is_relative_to(self.tmpdir), key)
        self.assertEqual(env["HORIZON_LAUNCHER_NO_SELF_UPDATE"], "1")
        self.assertEqual(env["HORIZON_NO_AUDIO"], "1")

    def test_explicit_game_dir_still_overrides_the_default(self):
        chosen = self.tmpdir / "my-game"
        self.run_script({"GAME_DIR": str(chosen)})
        launch, env = self.launch_record()
        self.assertIn(f"--game-dir {chosen}", launch)
        self.assertEqual(env["HORIZON_HOME"], str(chosen))


if __name__ == "__main__":
    unittest.main()
