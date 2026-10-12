"""Re-shoots the recipe images: each example, playing, fitted and cropped.

    python tools/showcase/recipe_images.py [name ...]
    python tools/showcase/recipe_images.py --check [name ...]

For each example in patches/ that has a docs/src/images/recipe-<name>.png
(or the ones named, which may be new), this films one still with

    -2.0 play / -1.0 fit / 0.0 layout / 0.0 still

so the patch has played for two seconds (cables glow, displays show signal)
and the view frames the whole level, as F does. The `layout` cue says where
every module, frame and note was drawn and where the view was, so the still
is cropped to them with a little grid around, resized to 1594 px wide with
Lanczos and saved as a 256-colour palette (median cut, no dither): the house
style for the manual's full-patch pictures.

A patch too big to come out at least 1594 px wide in the first window is
filmed again in a window grown to suit, so pictures are only ever scaled down.
One too big to fit even at the editor's furthest zoom (From One Sine) is
filmed again with the interface drawn smaller, so the window holds more.

--check films each one the same way but writes nothing: it reports which
images no longer match the current build. A capture plays out the same way
every time, so a picture that differs by more than a few stray pixels shows
something that has changed.

Build into target/capture first, as for shots.py. --check exits 1 if any
image is stale or missing.
"""

import json
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image, ImageChops

ROOT = Path(__file__).resolve().parents[2]
EXE = ROOT / "target" / "capture" / "release" / "soba.exe"
IMAGES = ROOT / "docs" / "src" / "images"

WIDTH = 1594
# The first window, in pixels, drawn at 2 pixels per point. Wide enough that
# the toolbar keeps to one row
SIZE = (3600, 2100)
PPP = 2.0
# The most a window grows to for a big patch (Windows lets a window run
# past the screen, but not without limit)
LARGEST = (6000, 4000)
# Grid kept around the patch, in screen points
MARGIN = 6
SCRIPT = "-2.0 play\n-1.0 fit\n0.0 layout shot\n0.0 still shot\n0.1 end\n"

# How far apart two pictures may be before one is stale: a pixel counts as
# changed past this much in any channel, and a picture as stale past this
# fraction of changed pixels
CHANNEL_TOLERANCE = 24
STALE_FRACTION = 0.002


def film(patch, out, size, ppp):
    """Films one still of the patch, fitted, and returns its layout."""
    script = out / "recipe.txt"
    script.write_text(SCRIPT)
    subprocess.run([str(EXE), str(patch), "--capture", str(script), "--out", str(out), "--fps", "10",
                    "--size", f"{size[0]}x{size[1]}", "--ppp", str(ppp)],
                   check=True, cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    if not (out / "shot.ppm").exists():
        raise RuntimeError(f"no still at {size[0]}x{size[1]}: is the window too big for the screen?")
    return json.loads((out / "shot.json").read_text())


def crop_box(lay):
    """The patch's drawn bounds, frames and notes included, in pixels."""
    view = lay["view"]
    rects = [m["rect"] for m in lay["modules"]] + [f["rect"] for f in lay["frames"]] + [n["rect"] for n in lay["notes"]]
    x0, y0 = min(r[0] for r in rects), min(r[1] for r in rects)
    x1, y1 = max(r[2] for r in rects), max(r[3] for r in rects)
    zoom, ppp, (ox, oy) = view["zoom"], view["ppp"], view["origin"]
    box = [(ox + x0 * zoom - MARGIN) * ppp, (oy + y0 * zoom - MARGIN) * ppp,
           (ox + x1 * zoom + MARGIN) * ppp, (oy + y1 * zoom + MARGIN) * ppp]
    editor = [v * ppp for v in view["editor"]]
    # How many times too big for the editor it is, if it's out of view
    overrun = max((box[2] - box[0]) / (editor[2] - editor[0]), (box[3] - box[1]) / (editor[3] - editor[1]))
    clipped = box[0] < editor[0] or box[1] < editor[1] or box[2] > editor[2] or box[3] > editor[3]
    return [round(v) for v in box], overrun if clipped else 0.0


def shoot(name, tmp):
    """Films an example and returns its recipe picture."""
    patch = ROOT / "patches" / f"{name}.json"
    size, ppp = SIZE, PPP
    for attempt in range(3):
        out = Path(tmp) / f"{name}-{attempt}"
        out.mkdir()
        lay = film(patch, out, size, ppp)
        box, overrun = crop_box(lay)
        wide = box[2] - box[0]
        if overrun:
            # Past the furthest zoom: draw it all smaller, in as many pixels
            ppp = ppp / overrun / 1.25
            continue
        if wide >= WIDTH or size == LARGEST or lay["view"]["zoom"] >= 0.999:
            break
        # Too small to scale down to WIDTH: a window this much bigger fits
        # the patch this much closer (with a little to spare)
        grow = WIDTH / wide * 1.05
        size = (min(LARGEST[0], round(size[0] * grow)), min(LARGEST[1], round(size[1] * grow)))
    if overrun:
        print(f"    {name}: the fit left part of the patch out of view", file=sys.stderr)
    if wide < WIDTH:
        print(f"    {name}: only {wide} px across, scaled up to {WIDTH}", file=sys.stderr)
    image = Image.open(out / "shot.ppm").convert("RGB").crop(box)
    image = image.resize((WIDTH, round(image.height * WIDTH / image.width)), Image.LANCZOS)
    return image.quantize(256, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE)


def changed(new, path):
    """The fraction of pixels that differ from the picture at `path`, or 1
    if it's missing or a different size."""
    if not path.exists():
        return 1.0
    old = Image.open(path).convert("RGB")
    if old.size != new.size:
        return 1.0
    r, g, b = ImageChops.difference(old, new.convert("RGB")).split()
    hist = ImageChops.lighter(ImageChops.lighter(r, g), b).histogram()
    return sum(hist[CHANNEL_TOLERANCE + 1:]) / (new.width * new.height)


def main(args):
    check = "--check" in args
    names = [a for a in args if not a.startswith("--")]
    names = names or sorted(p.stem for p in (ROOT / "patches").glob("*.json") if (IMAGES / f"recipe-{p.stem}.png").exists())
    stale = []
    with tempfile.TemporaryDirectory() as tmp:
        for name in names:
            path = IMAGES / f"recipe-{name}.png"
            image = shoot(name, tmp)
            if check:
                fraction = changed(image, path)
                verdict = "ok" if fraction <= STALE_FRACTION else ("missing" if not path.exists() else f"stale ({fraction:.1%} changed)")
                print(f"{name:20} {verdict}")
                if fraction > STALE_FRACTION:
                    stale.append(name)
            else:
                image.save(path, optimize=True)
                print(f"{name:20} {image.width}x{image.height}, {path.stat().st_size // 1024} KB")
    if check and stale:
        print(f"\nRe-shoot with: python tools/showcase/recipe_images.py {' '.join(stale)}")
    return 1 if stale else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
