#!/usr/bin/env python3
"""End-to-end check of the Lua host: two Lua mods in a real game, driven through horizon-ctl.

A scratch game folder gets the two fixture mods (tools/lua_host_e2e/lua-mods): loot-probe adds a
condition-gated emerald pool and a diamond entry with a custom loot function to the zombie table,
and loot-function doubles the stacks of a custom function kind in the simple dungeon chest. The
script launches the game, spawns three dungeon loot rolls, summons and kills a zombie, quits, and
reads the game's log: the mods load, the four loot hooks reach Lua, the custom condition and
function run, and the zombie's drops hold the emerald and the doubled diamonds.

    python3 tools/lua_host_e2e.py                                  # target/fast and .local resources
    python3 tools/lua_host_e2e.py --bin-dir DIR --resources DIR --out DIR [--keep]

The game needs a display: without DISPLAY or WAYLAND_DISPLAY the script re-runs itself under
xvfb-run -a, so the whole run lives in one process (xvfb-run stops the game when it ends).
Exit 0 when every check passes; the result is one JSON object on stdout.
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

try:
    from . import lab
except ImportError:
    import lab

FIXTURES = Path(__file__).resolve().parent / 'lua_host_e2e' / 'lua-mods'
MODS = ['loot-function', 'loot-probe']
TIMEOUT = 400  # seconds, as the other end-to-end scripts give the game on a software renderer
DROPS_WAIT = 120  # seconds to wait for the zombie's drops line after the kill
# Each check passes when one log line matches all of its regular expressions.
CHECKS = {
    'mods': [r'Lua mods: 2 loaded, 0 refused'],
    'table_load': [r'PROBE table_load zombie pools=\d+'],
    'condition': [r'PROBE condition kind=loot_probe:yes'],
    'function': [r'PROBE function kind=loot_function_sample:double'],
    'drops': [r'PROBE drops table=minecraft:entities/zombie', r'minecraft:emeraldx1\b', r'minecraft:diamondx6\b'],
}
SHOW = re.compile(r'PROBE|Lua|ERROR|WARN|panic', re.IGNORECASE)


def check_log(text):
    """Each check's outcome for the game's log text: name -> bool."""
    lines = text.splitlines()
    return {name: any(all(re.search(p, line) for p in patterns) for line in lines)
            for name, patterns in CHECKS.items()}


def scratch_game(work):
    """A game folder with the two fixture mods in lua-mods/."""
    game = work / 'game'
    (game / 'lua-mods').mkdir(parents=True)
    for mod in MODS:
        shutil.copytree(FIXTURES / mod, game / 'lua-mods' / mod)
    return game


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--bin-dir', help='horizon-client, horizon-ctl and horizon-native-mods/ (default: target/fast)')
    parser.add_argument('--resources', help='prepared resources (default: .local/horizon-client/<version>)')
    parser.add_argument('--out', default=str(lab.ROOT / '.local/lua-host-e2e'),
                        help='the game log copy and the result JSON go here')
    parser.add_argument('--keep', action='store_true', help='keep the scratch game folder')
    args = parser.parse_args()

    if not (os.environ.get('DISPLAY') or os.environ.get('WAYLAND_DISPLAY')):
        if not shutil.which('xvfb-run'):
            print('no display (DISPLAY and WAYLAND_DISPLAY are unset) and no xvfb-run to make one', file=sys.stderr)
            sys.exit(2)
        os.execvp('xvfb-run', ['xvfb-run', '-a', sys.executable, str(Path(__file__).resolve()), *sys.argv[1:]])

    bins = Path(args.bin_dir).resolve() if args.bin_dir else lab.cargo_target().resolve() / 'fast'
    client, ctl = bins / 'horizon-client', bins / 'horizon-ctl'
    resources = Path(args.resources).resolve() if args.resources else lab.RESOURCES.resolve()
    for path in (client, ctl):
        if not path.exists():
            sys.exit(f'missing {path}: build with cargo build --profile fast -p horizon-client -p horizon-client-lib')
    if not (resources / 'horizon-resources.json').exists():
        sys.exit(f'no prepared resources in {resources}')
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)

    work = Path(tempfile.mkdtemp(prefix='hzlua-e2e-'))
    game = scratch_game(work)
    home, xdg, socket = work / 'home', work / 'xdg', work / 'ctl.sock'
    env = lab.game_env(HORIZON_HOME=str(home), XDG_DATA_HOME=str(xdg), XDG_CONFIG_HOME=str(xdg))

    def ctl_run(*words, timeout=TIMEOUT):
        return subprocess.run([str(ctl), '--socket', str(socket), *words], env=env, cwd=work,
                              capture_output=True, text=True, timeout=timeout)

    log_path = home / 'logs' / 'latest.log'
    try:
        launch = ctl_run('launch', '--world', 'LootE2E', '--creative', '--test-terrain', '--offscreen', '--wait',
                         '--resources', str(resources), '--game-dir', str(game))
        (out / 'launch.out').write_text(launch.stdout + launch.stderr)
        ctl_run('wait-ready')
        for _ in range(3):
            ctl_run('command', 'loot spawn ~ ~2 ~ loot minecraft:chests/simple_dungeon')
        ctl_run('command', 'execute at @p run summon minecraft:zombie ~3 ~ ~')
        ctl_run('wait', '10')
        ctl_run('command', 'kill @e[type=minecraft:zombie]')
        # Poll the log for the drops line instead of a fixed wait.
        for _ in range(DROPS_WAIT):
            if log_path.exists() and 'PROBE drops table=minecraft:entities/zombie' in log_path.read_text(errors='replace'):
                break
            time.sleep(1)
    finally:
        try:
            ctl_run('quit', timeout=60)
        except subprocess.SubprocessError:
            pass
        # The game removes its control socket when it has saved and exited: wait for that before the
        # scratch folder goes.
        for _ in range(60):
            if not socket.exists():
                break
            time.sleep(1)

    text = log_path.read_text(errors='replace') if log_path.exists() else ''
    if text:
        shutil.copyfile(log_path, out / 'latest.log')
    results = check_log(text)
    print(json.dumps(results))
    if not all(results.values()):
        failed = [name for name, ok in results.items() if not ok]
        print(f'failed: {", ".join(failed)}; log lines that matter ({out / "latest.log"}):', file=sys.stderr)
        shown = [line for line in text.splitlines() if SHOW.search(line)]
        for line in shown[:80] or ['(no log at ' + str(log_path) + ')']:
            print('  ' + line, file=sys.stderr)
    if not args.keep:
        shutil.rmtree(work, ignore_errors=True)
    else:
        print(f'kept the scratch game folder: {work}', file=sys.stderr)
    sys.exit(0 if all(results.values()) else 1)


if __name__ == '__main__':
    main()
