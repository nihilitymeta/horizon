#!/usr/bin/env python3
"""Compare two screenshots: pixels differing by more than a threshold, and a diff image.
Usage: pixdiff.py A.png B.png [out-diff.png] [--crop x0,y0,x1,y1] [--threshold N]"""
import sys
from PIL import Image, ImageChops

args = [a for a in sys.argv[1:] if not a.startswith("--")]
opts = dict(a[2:].split("=", 1) for a in sys.argv[1:] if a.startswith("--") and "=" in a)
a = Image.open(args[0]).convert("RGB")
b = Image.open(args[1]).convert("RGB")
if "crop" in opts:
    box = tuple(int(v) for v in opts["crop"].split(","))
    a, b = a.crop(box), b.crop(box)
threshold = int(opts.get("threshold", 8))
diff = ImageChops.difference(a, b)
# per-pixel max channel difference
mx = diff.convert("L").point(lambda v: v)  # luminance approximation is too soft; use split
r, g, bl = diff.split()
m = ImageChops.lighter(ImageChops.lighter(r, g), bl)
hist = m.histogram()
total = sum(hist)
over = sum(hist[threshold + 1:])
big = sum(hist[64:])
print(f"{args[0]} vs {args[1]}: {a.size[0]}x{a.size[1]} pixels, {over} differ by >{threshold} ({100*over/total:.3f}%), {big} by >=64 ({100*big/total:.3f}%)")
if len(args) > 2:
    m.point(lambda v: min(255, v * 4)).save(args[2])
    print("diff image:", args[2])
