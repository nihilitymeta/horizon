"""Print every duplicate key in JSON files, with the path of the object that holds it (for example $.outer).
A JSON parser accepts a duplicate key silently and keeps the last value. Exits 1 if a file has a duplicate key
or is not JSON, 0 otherwise."""
import json, sys

class Pairs(list):
    """An object, kept as its list of (key, value) pairs so that a duplicate key survives parsing."""

def walk(node, path, found):
    if isinstance(node, Pairs):
        seen = set()
        for k, v in node:
            if k in seen:
                found.append((path, k))
            seen.add(k)
            walk(v, f"{path}.{k}", found)
    elif isinstance(node, list):
        for i, v in enumerate(node):
            walk(v, f"{path}[{i}]", found)

bad = False
for p in sys.argv[1:]:
    try:
        with open(p) as f:
            root = json.load(f, object_pairs_hook=Pairs)
    except ValueError as e:
        bad = True
        print(f"{p}: not valid JSON: {e}")
        continue
    found = []
    walk(root, "$", found)
    for path, key in found:
        bad = True
        print(f'{p}: duplicate key "{key}" at {path}')
sys.exit(1 if bad else 0)
