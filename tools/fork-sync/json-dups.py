"""Print duplicate keys in JSON files."""
import json, sys
for p in sys.argv[1:]:
    def hook(pairs, p=p):
        seen = set()
        for k, _ in pairs:
            if k in seen:
                print(p, 'DUP', k)
            seen.add(k)
        return dict(pairs)
    json.load(open(p), object_pairs_hook=hook)
