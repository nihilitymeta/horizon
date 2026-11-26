"""tools/lua_host_e2e.py: the pass/fail rule over a game log, its command line, and its fixture mods."""
import subprocess
import sys
import tomllib
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tools'))
import lua_host_e2e as e2e  # noqa: E402

PASSING_LOG = """\
[Render thread/INFO]: [event_horizon_lua::host] Lua mods: 2 loaded, 0 refused
[Server thread/INFO]: [event_horizon_lua] loot_probe: PROBE table_load zombie pools=6
[Server thread/INFO]: [event_horizon_lua] loot_probe: PROBE condition kind=loot_probe:yes luck=0
[Server thread/INFO]: [event_horizon_lua] loot_probe: PROBE function kind=loot_function_sample:double count=2
[Server thread/INFO]: [event_horizon_lua] loot_probe: PROBE drops table=minecraft:entities/zombie player=nil n=2 [minecraft:emeraldx1,minecraft:diamondx6]
"""


class CheckLog(unittest.TestCase):
    def test_a_log_with_every_line_passes_every_check(self):
        results = e2e.check_log(PASSING_LOG)
        self.assertEqual(set(results), {'mods', 'table_load', 'condition', 'function', 'drops'})
        self.assertTrue(all(results.values()), results)

    def test_a_log_without_the_condition_line_fails_and_names_the_condition(self):
        log = '\n'.join(line for line in PASSING_LOG.splitlines() if 'PROBE condition' not in line)
        results = e2e.check_log(log)
        self.assertEqual([name for name, ok in results.items() if not ok], ['condition'])


class CommandLine(unittest.TestCase):
    def test_help_exits_zero_without_a_game(self):
        done = subprocess.run([sys.executable, str(ROOT / 'tools/lua_host_e2e.py'), '--help'],
                              capture_output=True, text=True, timeout=60)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn('--bin-dir', done.stdout)


class FixtureMods(unittest.TestCase):
    def test_each_fixture_manifest_parses_and_names_an_existing_entry(self):
        for mod in e2e.MODS:
            folder = e2e.FIXTURES / mod
            with self.subTest(mod=mod):
                manifest = tomllib.loads((folder / 'lua-mod.toml').read_text())
                self.assertTrue(manifest.get('id'), 'the manifest names no id')
                self.assertTrue(manifest.get('entries'), 'the manifest lists no entries')
                for entry in manifest['entries']:
                    self.assertTrue((folder / entry).is_file(), f'missing entry {entry}')


if __name__ == '__main__':
    unittest.main()
