"""Resolve conflicts in JSON files by keeping both sides (HEAD first), fixing the comma between them.
Fails, leaving the file as resolved, when the result has a duplicate key in one object: the conflict began
inside an object, so the two sides were joined into one and need splitting by hand."""
import json, re, sys
R = re.compile(r'<<<<<<< [^\n]*\n(.*?)(?:\|\|\|\|\|\|\| [^\n]*\n.*?)?=======\n(.*?)>>>>>>> [^\n]*\n', re.S)
def both(m):
    a, b = m.group(1).rstrip('\n'), m.group(2)
    if a and not a.rstrip().endswith((',', '{', '[')):
        a += ','
    return (a + '\n' if a else '') + b
bad = False
for p in sys.argv[1:]:
    s = R.sub(both, open(p).read())
    open(p, 'w').write(s)
    dups = []
    def hook(pairs):
        keys = [k for k, _ in pairs]
        dups.extend(k for k in set(keys) if keys.count(k) > 1)
        return dict(pairs)
    json.loads(s, object_pairs_hook=hook)
    if dups:
        bad = True
        print('DUPLICATE KEYS', p, sorted(set(dups)), '- two objects were joined: split them by hand')
    else:
        print('ok', p)
sys.exit(1 if bad else 0)
