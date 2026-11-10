"""Resolve conflicts in JSON files by keeping both sides (HEAD first), fixing the comma between them.
A file is written only if the result parses as JSON with no duplicate key in one object. Otherwise it is left
as it was (still conflicted), the message names it, and the exit status is 1. A duplicate key means the conflict
began inside an object, so the two sides were joined into one and need splitting by hand."""
import json, re, sys
R = re.compile(r'<<<<<<< [^\n]*\n(.*?)(?:\|\|\|\|\|\|\| [^\n]*\n.*?)?=======\n(.*?)>>>>>>> [^\n]*\n', re.S)
def both(m):
    a, b = m.group(1).rstrip('\n'), m.group(2)
    if a and not a.rstrip().endswith((',', '{', '[')):
        a += ','
    return (a + '\n' if a else '') + b
def check(text):
    """Return the duplicate keys of text; raise ValueError if it is not JSON."""
    dups = []
    def hook(pairs):
        keys = [k for k, _ in pairs]
        dups.extend(k for k in set(keys) if keys.count(k) > 1)
        return dict(pairs)
    json.loads(text, object_pairs_hook=hook)
    return sorted(set(dups))
bad = False
for p in sys.argv[1:]:
    s = R.sub(both, open(p).read())
    try:
        dups = check(s)
    except ValueError as e:
        bad = True
        print('INVALID JSON', p, '-', e, '- not written: the file is unchanged, resolve it by hand')
        continue
    if dups:
        bad = True
        print('DUPLICATE KEYS', p, dups, '- two objects were joined: split them by hand; not written')
        continue
    open(p, 'w').write(s)
    print('ok', p)
sys.exit(1 if bad else 0)
