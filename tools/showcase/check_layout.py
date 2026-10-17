"""Checks that every example's modules still fit their frames.

    python tools/showcase/check_layout.py [name ...]

Modules grow: a knob or a row of tabs added to one can push it out of the
frame drawn around it in an example, over a note, or over its neighbour.
This films frame 0 of each example in patches/ (or the ones named) with a
`layout` cue, which saves where each module, frame and note was drawn, then
steps into every group, at every depth, as a person would (a dive into it,
then F) and takes another. It reports, under the group's path for a level
inside one (`Strings/Pump#2`, as an `enter` cue takes it, to film it by):

- a module that runs out of the frame its middle sits in
- a module that crosses into a frame it isn't in
- a module over a note, or over another module

Frames and notes are drawn on the top level only, so inside a group it's
modules over modules.

An example that can't be filmed is reported as FAILED, with the last
lines the capture wrote, and the rest are still checked.

Build into target/capture first, as for shots.py. Exits 1 if anything is
reported.
"""

import json
import subprocess
import sys
import tempfile
import textwrap
from pathlib import Path

import picture_check

ROOT = Path(__file__).resolve().parents[2]
EXE = ROOT / "target" / "capture" / "release" / "soba.exe"


def overlap(a, b):
    return min(a[2], b[2]) > max(a[0], b[0]) and min(a[3], b[3]) > max(a[1], b[1])


def inside(r, f):
    return r[0] >= f[0] and r[1] >= f[1] and r[2] <= f[2] and r[3] <= f[3]


def area(r):
    return (r[2] - r[0]) * (r[3] - r[1])


# Seconds for a dive in or out to land, and for a fitted view to be drawn
DIVE = 0.6
SETTLE = 0.3


def groups(patch):
    """Every group in a patch, depth first, as (path, name to enter it by
    from the level above). Siblings that share a name are told apart as
    the `enter` cue counts them, as a page reads: `Pump#1`, `Pump#2`."""
    found = []

    def walk(level, path):
        names = [g["name"] for g in level]
        ordered = sorted(level, key=lambda g: (g["position"][1], g["position"][0]))
        for g in level:
            name = g["name"]
            if names.count(name) > 1:
                same = [o for o in ordered if o["name"] == name]
                name = f"{name}#{next(i for i, o in enumerate(same) if o is g) + 1}"
            found.append((path + [name], name))
            walk(g.get("groups", []), path + [name])

    walk(json.loads(Path(patch).read_text(encoding="utf-8")).get("groups", []), [])
    return found


def layouts(patch, out):
    """The top level's layout and each group's, as (path, layout), from one
    capture that dives down into each group and back out to the next."""
    walk = groups(patch)
    cues, t, depth = ["0.0 layout level0"], 0.1, 0
    for i, (path, name) in enumerate(walk, start=1):
        # Out to the level it sits on, then in
        for _ in range(depth - (len(path) - 1)):
            cues.append(f"{t:.2f} leave")
            t += DIVE
        cues.append(f"{t:.2f} enter {name}")
        t += DIVE
        cues.append(f"{t:.2f} fit")
        t += SETTLE
        cues.append(f"{t:.2f} layout level{i}")
        t += 0.1
        depth = len(path)
    cues.append(f"{t:.2f} end")
    script = out / "layout.txt"
    script.write_text("\n".join(cues) + "\n")
    run = subprocess.run([str(EXE), str(patch), "--capture", str(script), "--out", str(out), "--fps", "10", "--size", "960x540"],
                         check=True, cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True, errors="replace")
    # A level it couldn't reach would be checked as the one it stayed on
    lost = [line for line in run.stderr.splitlines() if "can't enter" in line or "no group to leave" in line]
    if lost:
        raise RuntimeError(f"{patch}: " + "; ".join(lost))
    levels = [([], "level0")] + [(path, f"level{i}") for i, (path, _) in enumerate(walk, start=1)]
    return [(path, json.loads((out / f"{name}.json").read_text())) for path, name in levels]


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
            # One patch that can't be filmed is reported, and the rest still checked
            try:
                found = [("/".join(path), p) for path, lay in layouts(ROOT / "patches" / f"{name}.json", out)
                         for p in problems(lay)]
            except Exception as e:
                print(f"{name:20} FAILED")
                print(textwrap.indent(picture_check.failure(e), "    "))
                failed = True
                continue
            print(f"{name:20} {'ok' if not found else ''}")
            for where, p in found:
                print(f"    {where}: {p}" if where else f"    {p}")
            failed |= bool(found)
    sys.exit(1 if failed else 0)
