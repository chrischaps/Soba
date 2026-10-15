"""Re-shoots the manual's module close-ups (docs/src/images/module-<name>.png),
each from a small patch in tools/showcase/modules/ that holds the moment its
caption describes.

    python tools/showcase/module_images.py [name ...]
    python tools/showcase/module_images.py --check [name ...]

Each close-up in MODULES names its patch, the module pictured, the zoom it's
drawn at, the width it's saved at and the cues that pose it (a chord held, a
cue playing). The patch is filmed once to find the module, then again with
the view aimed at it, and the still is cropped to the node with room at the
sides for its cable stubs.

Pointer cues aim at the face: `face x y` in a cue stands for the window
point over (x, y) points from the module's top left, as drawn at zoom 1.
Read them off a still: (pixel / PPP - margin) / zoom, with the margins below.
The face's layout shifts a little from one ppp to another, so aim from a
still filmed at PPP.

Only the Sampler is scripted so far (#195); #199 brings the rest across.

Build into target/capture first, as for shots.py. --check films each one
the same way but writes nothing, lists the close-ups that no longer match the
build, and exits 1 if any is stale or missing.
"""

import re
import sys
import tempfile
from collections import namedtuple
from pathlib import Path

from PIL import Image

from manual_images import MODULE_FOOT
from recipe_images import IMAGES, ROOT, SIZE, STALE_FRACTION, aim, changed, film

MODULES_DIR = ROOT / "tools" / "showcase" / "modules"
PPP = 2.0
# Room kept round the node, in points at zoom 1: at the sides for the cable
# stubs, above and below to show it's the whole node
SIDE = 22
TOP = 6

# `patch` is tools/showcase/modules/<patch>.json. The cues run up to the
# still at 0.0; the view is aimed before the first of them. palette=False
# keeps full colour, for faces whose gradients band in 256 colours
Module = namedtuple("Module", "patch module zoom width cues palette", defaults=(True,))
MODULES = {
    # The bell, looped between Lp St and Lp End, held as a five-note chord with a cue
    # (▶ beside its name) playing over it: the voices' dots go round the loop
    # while the cue's line runs on towards End
    "sampler": Module("sampler", "source.sampler", 0.9, 395, """
        -4.0 play
        -3.0 chord 48,55,60,64,67 90 6.0
        -2.6 move face 24 453 0.2
        -2.3 press
        -2.25 release
        -2.2 move face 600 300 0.2
    """, palette=False),
    # A swung, ping-ponged row through a saw chord, the step filter half up: the dot
    # rides a slice partway through the bar, violet slices bound for the right
    "slicer": Module("slicer", "fx.slicer", 1.0, 400, """
        -2.65 play
    """, palette=False),
    # A kick on every beat over a low saw: the dot rides up the knee on each
    # hit, and the reduction hangs from the strip's top in time, the saw's
    # landscape steady beneath it
    "compressor": Module("compressor", "fx.compressor", 1.0, 420, """
        -2.8 play
    """, palette=False),
    # Pink noise through a warm low shelf, a cut in the middle and an airy top,
    # the pointer on the mid band's dot: its wings light, and the header reads it
    "eq": Module("eq", "fx.eq", 1.0, 440, """
        -2.0 play
        -0.6 move face 234 135 0.3
    """, palette=False),
    # A clap into the left side only, ping-ponged on worn tape: its echoes
    # take turns down the train, left above the line and right below,
    # warming as they go
    "delay": Module("delay", "fx.delay", 1.0, 320, """
        -2.6 play
    """, palette=False),
    # Toms on every beat through three voices: the braid lights in bursts
    # where each hit passed, and slowly weaves on between them
    "chorus": Module("chorus", "fx.chorus", 1.0, 320, """
        -2.4 play
    """, palette=False),
}


def module_rect(lay, name):
    """Where the pictured module is drawn, in patch space."""
    module = MODULES[name].module
    rect = next((m["rect"] for m in lay["modules"] if m["module_id"] == module), None)
    if rect is None:
        raise RuntimeError(f"{name}: no {module} in {MODULES[name].patch}.json")
    return rect


def script(cues, camera, origin, rect, zoom):
    """The close-up's cues, aimed at the face, with the view moved to it before
    the first and the still taken at 0.0."""
    def at_face(match):
        x, y = float(match.group(1)), float(match.group(2))
        return f"{origin[0] + (rect[0] + x) * zoom:.1f} {origin[1] + (rect[1] + y) * zoom:.1f}"

    lines = [re.sub(r"face (-?[\d.]+) (-?[\d.]+)", at_face, line.strip()) for line in cues.strip().splitlines()]
    first = min([float(line.split()[0]) for line in lines] + [0.0])
    return "\n".join([f"{first - 0.5:.2f} {camera}"] + lines + ["0.0 layout shot", "0.0 still shot", "0.1 end"]) + "\n"


def shoot(name, tmp):
    """Films a close-up and returns it, cropped and scaled."""
    shot = MODULES[name]
    patch = MODULES_DIR / f"{shot.patch}.json"
    probe = Path(tmp) / f"{name}-probe"
    probe.mkdir()
    lay = film(patch, probe, SIZE, PPP, "0.0 layout shot\n0.1 end\n")
    rect = module_rect(lay, name)
    camera, origin = aim(lay["view"], ((rect[0] + rect[2]) / 2, (rect[1] + rect[3] + MODULE_FOOT) / 2), shot.zoom)
    out = Path(tmp) / name
    out.mkdir()
    lay = film(patch, out, SIZE, PPP, script(shot.cues, camera, origin, rect, shot.zoom))
    x0, y0, x1, y1 = module_rect(lay, name)
    view = lay["view"]
    zoom, (ox, oy) = view["zoom"], view["origin"]
    box = [(ox + (x0 - SIDE) * zoom) * PPP, (oy + (y0 - TOP) * zoom) * PPP,
           (ox + (x1 + SIDE) * zoom) * PPP, (oy + (y1 + MODULE_FOOT + TOP) * zoom) * PPP]
    width, height = Image.open(out / "shot.ppm").size
    if box[0] < 0 or box[1] < 0 or box[2] > width or box[3] > height:
        raise RuntimeError(f"{name}: the node runs out of the window at zoom {shot.zoom}")
    image = Image.open(out / "shot.ppm").convert("RGB").crop([round(v) for v in box])
    image = image.resize((shot.width, round(image.height * shot.width / image.width)), Image.LANCZOS)
    if not shot.palette:
        return image
    return image.quantize(256, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE)


def main(args):
    check = "--check" in args
    names = [a for a in args if not a.startswith("--")] or list(MODULES)
    unknown = [n for n in names if n not in MODULES]
    if unknown:
        print(f"unknown: {' '.join(unknown)} (close-ups: {' '.join(MODULES)})", file=sys.stderr)
        return 2
    stale = []
    with tempfile.TemporaryDirectory() as tmp:
        for name in names:
            path = IMAGES / f"module-{name}.png"
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
        print(f"\nRe-shoot with: python tools/showcase/module_images.py {' '.join(stale)}")
    return 1 if stale else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
