"""Re-shoots the manual's interface pictures: the window, its menus and the
tutorial's finished patch, each played and posed by a capture script.

    python tools/showcase/manual_images.py [name ...]
    python tools/showcase/manual_images.py --check [name ...]

Each picture in PICTURES names a patch, the cues that pose it (a chord
held, a menu opened, a key down) and what to keep of the window:

- "window": all of it, toolbar and status bar included
- "patch": the patch's modules, frames and notes with a little grid
  around, as recipe_images.py crops
- (x0, y0, x1, y1): a box in window points, for a menu or the palette,
  which the layout doesn't know about

Every picture is filmed at 2 pixels per point, scaled down with Lanczos to
its width and saved as a 256-colour palette, as the recipe images are
(unless it says palette=False).
Pointer positions in the cues are in window points, worked out from a still
(pixel / 2): fit always frames a patch the same way in the same window, so
they land on the same spot every time.

--check films each one the same way but writes nothing, and lists the
pictures that no longer match the build. The overview shows the toolbar's
Output menu, which names this machine's audio device: on another machine
it reads stale for that alone.

Build into target/capture first, as for shots.py. --check exits 1 if any
picture is stale or missing.
"""

import json
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image

from recipe_images import EXE, IMAGES, ROOT, STALE_FRACTION, changed, crop_box

# How far a node's foot reaches below its layout rect (#199), so a module at
# the bottom of a picture isn't cut off
MODULE_FOOT = 12.0

PPP = 2.0
SIZE = (3600, 2100)

# First Sound fitted in the 3600x2100 window: its Shape frame's title bar
# runs along y 221 from x 794 to 1243, and the canvas below the Audio Output
# is empty from y 530. A menu opens with its top left at the pointer
PICTURES = {
    "interface-overview": dict(
        patch="patches/lush-pad.json",
        # 1800 points keeps the toolbar to one row
        size=(3600, 1800),
        crop="window",
        width=1600,
        cues="""
            -4.0 play
            -4.0 fit
            -3.0 chord 48,55,59,62,64 84 6.0
            0.0 wait
        """,
    ),
    "interface-context-menu": dict(
        patch="patches/first-sound.json",
        crop=(1339, 549, 1609, 907),
        width=405,
        # Seven category colours on a small picture: a palette merges
        # Source's blue into Effect's cyan, and full colour is only 58 KB
        palette=False,
        cues="""
            -2.0 fit
            0.0 cursor on
            0.0 move 1350 560 0.3
            0.5 rclick
            0.8 move 1385 646 0.4
            1.2 move 1395 646 0.2
            1.8 wait
        """,
    ),
    "interface-quick-add": dict(
        patch="patches/first-sound.json",
        crop=(600, 325, 1350, 800),
        width=1120,
        cues="""
            -2.0 play
            -2.0 fit
            -1.5 key Z 4.0
            0.0 move 840 340 0.3
            0.5 key Space 0.05
            0.8 key ArrowDown 0.05
            1.0 key ArrowDown 0.05
            1.6 wait
        """,
    ),
    "interface-frames-and-notes": dict(
        patch="patches/first-sound.json",
        crop="patch",
        width=1594,
        cues="""
            -2.0 play
            -2.0 fit
            -1.5 key Z 4.0
            0.0 cursor on
            0.0 move 880 221 0.3
            0.5 rclick
            1.0 move 981 262 0.4
            1.6 wait
        """,
    ),
    "tutorial-complete-patch": dict(
        patch="tools/showcase/patches/first-patch.json",
        crop="patch",
        width=1594,
        cues="""
            -2.0 play
            -2.0 fit
            -0.6 key Z 2.0
            0.0 wait
        """,
    ),
}


def script(cues):
    """The picture's cues, with its still taken at the `wait` cue's time."""
    lines = []
    for line in cues.strip().splitlines():
        t, cue = line.split(None, 1)
        if cue == "wait":
            lines += [f"{t} layout shot", f"{t} still shot", f"{float(t) + 0.1:.1f} end"]
        else:
            lines.append(f"{t} {cue}")
    return "\n".join(lines) + "\n"


def shoot(name, tmp):
    """Films a picture and returns it, cropped, scaled and paletted."""
    pic = PICTURES[name]
    size = pic.get("size", SIZE)
    out = Path(tmp) / name
    out.mkdir()
    (out / "manual.txt").write_text(script(pic["cues"]))
    subprocess.run([str(EXE), str(ROOT / pic["patch"]), "--capture", str(out / "manual.txt"), "--out", str(out),
                    "--fps", "10", "--size", f"{size[0]}x{size[1]}", "--ppp", str(PPP)],
                   check=True, cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    image = Image.open(out / "shot.ppm").convert("RGB")
    crop = pic["crop"]
    if crop == "patch":
        box, overrun = crop_box(json.loads((out / "shot.json").read_text()), foot=MODULE_FOOT)
        if overrun:
            print(f"    {name}: the fit left part of the patch out of view", file=sys.stderr)
        image = image.crop(box)
    elif crop != "window":
        image = image.crop([round(v * PPP) for v in crop])
    width = pic["width"]
    image = image.resize((width, round(image.height * width / image.width)), Image.LANCZOS)
    if not pic.get("palette", True):
        return image
    return image.quantize(256, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE)


def main(args):
    check = "--check" in args
    names = [a for a in args if not a.startswith("--")] or list(PICTURES)
    unknown = [n for n in names if n not in PICTURES]
    if unknown:
        print(f"unknown: {' '.join(unknown)} (pictures: {' '.join(PICTURES)})", file=sys.stderr)
        return 2
    stale = []
    with tempfile.TemporaryDirectory() as tmp:
        for name in names:
            path = IMAGES / f"{name}.png"
            image = shoot(name, tmp)
            if check:
                fraction = changed(image, path)
                verdict = "ok" if fraction <= STALE_FRACTION else ("missing" if not path.exists() else f"stale ({fraction:.1%} changed)")
                print(f"{name:28} {verdict}")
                if fraction > STALE_FRACTION:
                    stale.append(name)
            else:
                image.save(path, optimize=True)
                print(f"{name:28} {image.width}x{image.height}, {path.stat().st_size // 1024} KB")
    if check and stale:
        print(f"\nRe-shoot with: python tools/showcase/manual_images.py {' '.join(stale)}")
    return 1 if stale else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
