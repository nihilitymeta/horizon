"""Resolves conflicts in docs/native-mods.md's "API N adds ..." list: HEAD's side, then each of the
commit's paragraphs that HEAD does not already have. Paragraphs are compared without their number, so a
fork hook renumbered on HEAD is not added again under its old number."""
import re, sys
R = re.compile(r'<<<<<<< [^\n]*\n(.*?)=======\n(.*?)>>>>>>> [^\n]*\n', re.S)
SPLIT = re.compile(r'(?m)^(?=API \d+ adds)')
def body(p):
    return re.sub(r'^API \d+ adds', '', p).replace('"hooks": ', '').strip()
def both(m):
    a, b = m.group(1), m.group(2)
    have = {re.sub(r'\d+', '', body(p))[:200] for p in SPLIT.split(a) if p.strip()}
    add = [p for p in SPLIT.split(b) if p.strip() and re.sub(r'\d+', '', body(p))[:200] not in have]
    return a.rstrip('\n') + '\n\n' + ''.join(add) if add else a
for p in sys.argv[1:]:
    s, n = R.subn(both, open(p).read())
    open(p, 'w').write(s)
    print(n, 'resolved in', p)
