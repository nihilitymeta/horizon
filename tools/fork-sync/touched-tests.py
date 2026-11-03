"""Prints the nextest filter for the tests in the files a branch changes, from `git diff --name-only main...<branch>`
on stdin, or nothing when it changes no Rust module with tests of its own.
  git diff --name-only main...<branch> | python3 tools/fork-sync/touched-tests.py
A file crates/<dir>/src/a/b.rs gives the tests under a::b:: in that crate's package (crates/horizon-client/src is
tested as horizon-client-lib too, whose root is client_lib.rs there); crates/<dir>/tests/x.rs that test binary.
A file's own tests only: a::b's submodules that live in files of their own (src/a/b/*.rs) are left out, so a
one-line hook in game.rs doesn't select every game:: test. A crate root (lib.rs, main.rs, client_lib.rs) gives its own inline tests, by the same rule.
Non-Rust files add nothing either: engine-work's full suite covers them."""
import os, re, subprocess, sys
REV = os.environ.get('REV', 'HEAD')
ROOTS = {'lib.rs', 'main.rs', 'client_lib.rs'}
def children(d, path):
    """Submodules of crates/<d>/src/<path> that are files of their own."""
    out = subprocess.run(['git', 'ls-tree', '--name-only', f'{REV}:crates/{d}/src/' + '/'.join(path)],
                         capture_output=True, text=True).stdout.split()
    return sorted({n[:-3] if n.endswith('.rs') else n for n in out if n.endswith('.rs') or '.' not in n} - {'mod'})
def package(d):
    try:
        toml = subprocess.run(['git', 'show', f'{REV}:crates/{d}/Cargo.toml'], capture_output=True, text=True).stdout
    except OSError:
        return d
    m = re.search(r'^\[package\][^\[]*?^name\s*=\s*"([^"]+)"', toml, re.M | re.S)
    return m.group(1) if m else d
mods, bins = {}, set()
for f in sys.stdin.read().split():
    p = f.split('/')
    if len(p) < 4 or p[0] != 'crates' or not f.endswith('.rs'):
        continue
    d, kind, rest = p[1], p[2], p[3:]
    if kind == 'tests' and len(rest) == 1:
        bins.add((package(d), rest[0][:-3])); continue
    if kind != 'src':
        continue
    root = len(rest) == 1 and rest[0] in ROOTS
    path = [] if root else rest[:-1] if rest[-1] == 'mod.rs' else rest[:-1] + [rest[-1][:-3]]
    pkgs = [package(d)] + (['horizon-client-lib'] if d == 'horizon-client' else [])
    mod = '::'.join(path)
    pre = f"{mod}::" if mod else ""
    term = f"test(/^{pre}/)"
    kids = children(d, path)
    if kids:
        term = f"({term} - test(/^{pre}({'|'.join(kids)})::/))"
    for pk in pkgs:
        mods.setdefault(pk, set()).add(term)
terms = [f"(package({pk}) & ({' | '.join(sorted(m))}))" for pk, m in sorted(mods.items())]
terms += [f"(package({pk}) & binary({b}))" for pk, b in sorted(bins)]
if '--packages' in sys.argv:
    print(' '.join(sorted(set(mods) | {pk for pk, _ in bins})))
else:
    print(' | '.join(terms))
