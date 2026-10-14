"""Frames to review a filmed shot by: where each move lands, partway
through the long ones, and through each dive into or out of a group.

    python tools/showcase/check_film.py <shot> [--from S] [--to S] [--against <other check dir>] [--script file]

Reads the shot's script (tools/showcase/scripts/<shot>.txt) and its film
(target/showcase/<shot>/video.mkv), and writes target/showcase/<shot>-check/:
f000.png ... one frame per moment, and sheet00.png ... tiled with the time
and what the camera is doing.

- An `aim` or `camera` cue gives a frame where it lands, and frames 35% and
  65% of the way through any glide of 2.5 s or more, where a pull-back can
  show empty canvas. These are worth reading before encoding.
- An `enter` and a `leave` (a tour's dives into a group and out) each give
  a frame mid-dive and one after.

--against compares each frame with the same frame of an earlier check
(captures are deterministic): after a change that should touch only one
thing, the frames that moved are the ones to look at.
"""

import argparse
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SHOWCASE = ROOT / "target" / "showcase"
SCRIPTS = Path(__file__).resolve().parent / "scripts"


def moments(script):
    """[(time, what)] worth a frame, from a capture script."""
    out = []
    for line in script.read_text().splitlines():
        words = line.split("#", 1)[0].split()
        if len(words) < 2:
            continue
        t, cue, rest = float(words[0]), words[1], words[2:]
        if cue in ("aim", "camera"):
            # Both are `<x> <y> <zoom> <seconds>`
            dur = float(rest[3]) if len(rest) > 3 else 0.0
            note = line.split("#", 1)[1].strip() if "#" in line else ""
            out.append((t + dur + 0.05, "land " + note))
            if dur >= 2.5:
                out += [(t + 0.35 * dur, "35% " + note), (t + 0.65 * dur, "65% " + note)]
        elif cue == "enter":
            out += [(t + 0.18, f"dive into {' '.join(rest)}, mid"), (t + 0.6, f"in {' '.join(rest)}")]
        elif cue == "leave":
            out += [(t + 0.18, "dive out, mid"), (t + 0.6, "out")]
    # The warmup isn't filmed
    return sorted(m for m in out if m[0] >= 0)


def sheets(frames, out, rows=4):
    from PIL import Image, ImageDraw
    first = Image.open(frames[0][0])
    portrait = first.height > first.width
    cols, width = (7, 270) if portrait else (4, 480)
    height = round(first.height * width / first.width)
    per = cols * rows
    for s in range(0, len(frames), per):
        sheet = Image.new("RGB", (cols * width, rows * (height + 18)), (18, 18, 22))
        draw = ImageDraw.Draw(sheet)
        for i, (path, label) in enumerate(frames[s:s + per]):
            x, y = (i % cols) * width, (i // cols) * (height + 18)
            sheet.paste(Image.open(path).convert("RGB").resize((width, height)), (x, y))
            draw.text((x + 4, y + height + 3), label[:70], fill=(230, 230, 230))
        sheet.save(out / f"sheet{s // per:02}.png")


def compare(out, against):
    """The frames that differ most from an earlier check's."""
    import numpy as np
    from PIL import Image
    moved = []
    for path in sorted(out.glob("f*.png")):
        other = Path(against) / path.name
        if other.exists():
            a = np.asarray(Image.open(path).convert("L"), dtype=float)
            b = np.asarray(Image.open(other).convert("L"), dtype=float)
            if a.shape == b.shape:
                moved.append((float(np.abs(a - b).mean()), path.name))
    moved.sort(reverse=True)
    print("most changed (mean difference, 0-255):")
    for diff, name in moved[:8]:
        print(f"  {name}  {diff:.2f}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("shot")
    ap.add_argument("--from", dest="start", type=float, default=0.0)
    ap.add_argument("--to", type=float, default=1e9)
    ap.add_argument("--against")
    ap.add_argument("--script", help="the script the film was made from, if not scripts/<shot>.txt (a dry run's)")
    a = ap.parse_args()
    video = SHOWCASE / a.shot / "video.mkv"
    if not video.exists():
        sys.exit(f"no film at {video}: film the shot first (shots.py {a.shot})")
    out = SHOWCASE / f"{a.shot}-check"
    out.mkdir(parents=True, exist_ok=True)
    for old in out.glob("*.png"):
        old.unlink()
    frames = []
    for i, (t, what) in enumerate(m for m in moments(Path(a.script) if a.script else SCRIPTS / f"{a.shot}.txt") if a.start <= m[0] <= a.to):
        path = out / f"f{i:03}.png"
        subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-ss", f"{t:.3f}", "-i", str(video), "-frames:v", "1", str(path)],
                       check=True)
        frames.append((path, f"{t:6.2f} {what}"))
    sheets(frames, out)
    print(f"{len(frames)} frames, sheets in {out}")
    if a.against:
        compare(out, a.against)


if __name__ == "__main__":
    main()
