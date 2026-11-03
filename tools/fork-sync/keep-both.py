"""Resolve every conflict in the given files by keeping HEAD's side, then the commit's side."""
import re, sys
R = re.compile(r'<<<<<<< [^\n]*\n(.*?)(?:\|\|\|\|\|\|\| [^\n]*\n.*?)?=======\n(.*?)>>>>>>> [^\n]*\n', re.S)
for p in sys.argv[1:]:
    s, n = R.subn(lambda m: m.group(1) + m.group(2), open(p).read())
    open(p, 'w').write(s)
    print(n, 'kept both in', p)
