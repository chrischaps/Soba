"""Does everything on a patch scale smoothly as the view zooms?

    python tools/showcase/zoom_jitter.py patch.json [--ppp 1.25] [--steps 80] [--zoom 0.3 2.0] [--max N]

Zooms slowly over each of the patch's frames (or the whole top level, if it
has none) in a capture, dumping `layout` at every step, then reports how
much each module and note changes size in patch units, counting only the
steps where it was wholly on screen. A module that scales cleanly keeps one
size at every zoom. One whose size depends on text laid out at the zoomed
font saws: text steps a whole pixel at a time while the rest scales
smoothly (#255). Notes that rewrap jump by whole lines.

--max N exits non-zero if anything moves by more than N patch units. Since
#255 every example holds still to within a few hundredths of a unit, so --max
0.5 is the check: a face whose size moves sizes something by its text.
Size it with src/graph/steady_text.rs instead.

Needs the capture build (CARGO_TARGET_DIR=target/capture cargo build
--release). Output goes to target/showcase/zoom-jitter/<patch>/.
"""

import argparse
import collections
import json
import subprocess
import sys
from pathlib import Path

from tour import EDITORS

ROOT = Path(__file__).resolve().parents[2]
EXE = ROOT / "target" / "capture" / "release" / "soba.exe"
SIZE = "1920x1080"


def centres(patch):
    """The middle of each frame, or of the whole top level."""
    frames = patch.get("frames") or []
    if frames:
        return [(f["title"], (f["position"][0] + f["size"][0] / 2, f["position"][1] + f["size"][1] / 2)) for f in frames]
    xs = [n["position"][0] for n in patch["nodes"]]
    ys = [n["position"][1] for n in patch["nodes"]]
    return [("whole patch", ((min(xs) + max(xs)) / 2 + 100, (min(ys) + max(ys)) / 2 + 100))]


def names_of(patch):
    names = {}
    for n in patch["nodes"]:
        names[(round(n["position"][0]), round(n["position"][1]))] = f"{n['module_id']} #{n['id']}"
    for g in patch.get("groups", []):
        names[(round(g["position"][0]), round(g["position"][1]))] = f"group {g['name']}"
    return names


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("patch")
    ap.add_argument("--ppp", type=float, default=1.25)
    ap.add_argument("--steps", type=int, default=80)
    ap.add_argument("--zoom", type=float, nargs=2, default=(0.3, 2.0))
    ap.add_argument("--max", type=float)
    a = ap.parse_args()
    path = Path(a.patch).resolve()
    patch = json.loads(path.read_text(encoding="utf-8"))
    x0, y0, x1, y1 = EDITORS[(SIZE, a.ppp)]
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    out = ROOT / "target" / "showcase" / "zoom-jitter" / path.stem
    out.mkdir(parents=True, exist_ok=True)
    for old in out.glob("z*.json"):
        old.unlink()

    # The camera is relative to the view as the patch opens (zoom 1, patch
    # (0, 0) at the editor's top left): centring P at zoom Z is this cue
    lines, t, k = ["-1 play"], 0.0, 0
    lo, hi = a.zoom
    for _, (px, py) in centres(patch):
        for i in range(a.steps):
            z = lo * (hi / lo) ** (i / (a.steps - 1))
            lines.append(f"{t:.3f} camera {-(x0 + px - cx) * z:.2f} {-(y0 + py - cy) * z:.2f} {z:.5f} 0.03")
            lines.append(f"{t + 0.15:.3f} layout z{k:04}")
            t, k = t + 0.2, k + 1
    lines.append(f"{t + 0.1:.3f} end")
    script = out / "sweep.txt"
    script.write_text("\n".join(lines) + "\n")
    print(f"sweeping {len(centres(patch))} region(s), {a.steps} zooms each ...", flush=True)
    subprocess.run([str(EXE), str(path), "--capture", str(script), "--out", str(out), "--fps", "30", "--ppp", str(a.ppp),
                    "--size", SIZE, "--crf", "30"], check=True, capture_output=True)

    names = names_of(patch)
    sizes = collections.defaultdict(list)
    for dump in sorted(out.glob("z*.json")):
        d = json.loads(dump.read_text())
        v = d["view"]
        z, (ox, oy), e = v["zoom"], v["origin"], v["editor"]

        def seen(r):
            return ox + r[0] * z >= e[0] and oy + r[1] * z >= e[1] and ox + r[2] * z <= e[2] and oy + r[3] * z <= e[3]

        for m in d["modules"]:
            r = m["rect"]
            if seen(r):
                sizes[names.get((round(r[0]), round(r[1])), m["module_id"])].append((z, r[2] - r[0], r[3] - r[1]))
        for note in d.get("notes", []):
            r = note["rect"] if isinstance(note, dict) else note
            if seen(r):
                sizes[f"note at {round(r[0])}, {round(r[1])}"].append((z, r[2] - r[0], r[3] - r[1]))

    rows = []
    for name, seen_at in sizes.items():
        if len(seen_at) >= 5:
            ws, hs = [w for _, w, _ in seen_at], [h for _, _, h in seen_at]
            rows.append((max(ws) - min(ws), max(hs) - min(hs), len(seen_at), name, min(ws), min(hs)))
    rows.sort(key=lambda r: -max(r[0], r[1]))
    print(f"\n{'':40} {'width':>7} {'height':>7}  how much each changes, in patch units")
    for dw, dh, n, name, w, h in rows:
        print(f"{name[:40]:40} {dw:7.1f} {dh:7.1f}  ({w:.0f} x {h:.0f}, {n} zooms)")
    (out / "report.json").write_text(json.dumps([{"name": r[3], "width": r[0], "height": r[1], "seen": r[2]} for r in rows], indent=1))
    worst = max((max(r[0], r[1]) for r in rows), default=0.0)
    if a.max is not None and worst > a.max:
        sys.exit(f"\n{worst:.1f} patch units moved: more than --max {a.max}")


if __name__ == "__main__":
    main()
