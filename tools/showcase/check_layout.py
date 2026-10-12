"""Checks that every example's modules still fit their frames.

    python tools/showcase/check_layout.py [name ...]

Modules grow: a knob or a row of tabs added to one can push it out of the
frame drawn around it in an example, over a note, or over its neighbour.
This films frame 0 of each example in patches/ (or the ones named) with a
`layout` cue, which saves where each module, frame and note was drawn, and
reports:

- a module that runs out of the frame its middle sits in
- a module that crosses into a frame it isn't in
- a module over a note, or over another module

Build into target/capture first, as for shots.py. Exits 1 if anything is
reported.
"""

import json
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
EXE = ROOT / "target" / "capture" / "release" / "soba.exe"


def overlap(a, b):
    return min(a[2], b[2]) > max(a[0], b[0]) and min(a[3], b[3]) > max(a[1], b[1])


def inside(r, f):
    return r[0] >= f[0] and r[1] >= f[1] and r[2] <= f[2] and r[3] <= f[3]


def area(r):
    return (r[2] - r[0]) * (r[3] - r[1])


def layout(patch, out):
    script = out / "layout.txt"
    script.write_text("0.0 layout all\n0.1 end\n")
    subprocess.run([str(EXE), str(patch), "--capture", str(script), "--out", str(out), "--fps", "10", "--size", "960x540"],
                   check=True, cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    return json.loads((out / "all.json").read_text())


def problems(lay):
    modules, frames, notes = lay["modules"], lay["frames"], lay["notes"]
    found = []
    for i, m in enumerate(modules):
        r = m["rect"]
        cx, cy = (r[0] + r[2]) / 2, (r[1] + r[3]) / 2
        # The innermost frame holding its middle is its home
        homes = sorted((f for f in frames if f["rect"][0] <= cx <= f["rect"][2] and f["rect"][1] <= cy <= f["rect"][3]),
                       key=lambda f: area(f["rect"]))
        if homes and not inside(r, homes[0]["rect"]):
            f = homes[0]["rect"]
            past = {"left": f[0] - r[0], "top": f[1] - r[1], "right": r[2] - f[2], "bottom": r[3] - f[3]}
            sides = ", ".join(f"{side} {d:.0f}" for side, d in past.items() if d > 0.5)
            found.append(f"{m['module_id']} runs out of '{homes[0]['title']}' ({sides})")
        for f in frames:
            if f not in homes and overlap(r, f["rect"]) and not inside(f["rect"], r):
                found.append(f"{m['module_id']} crosses into '{f['title']}'")
        for t in notes:
            if overlap(r, t["rect"]):
                found.append(f"{m['module_id']} covers the note '{t['text'][:40]}...'")
        for other in modules[i + 1:]:
            if overlap(r, other["rect"]):
                found.append(f"{m['module_id']} overlaps {other['module_id']}")
    return found


if __name__ == "__main__":
    names = sys.argv[1:] or sorted(p.stem for p in (ROOT / "patches").glob("*.json"))
    failed = False
    with tempfile.TemporaryDirectory() as tmp:
        for name in names:
            out = Path(tmp) / name
            out.mkdir()
            found = problems(layout(ROOT / "patches" / f"{name}.json", out))
            print(f"{name:20} {'ok' if not found else ''}")
            for p in found:
                print(f"    {p}")
            failed |= bool(found)
    sys.exit(1 if failed else 0)
