"""Re-shoots the recipe images that show what a recipe's embed can't.

    python tools/showcase/recipe_images.py [name ...]
    python tools/showcase/recipe_images.py --check [name ...]

A recipe with a playable embed shows its whole patch there, so it has no
picture of it (#225). The pictures left are of a patch with no embed, listed
in STILLS, and the moments and close-ups below.

For each example in STILLS (or one named, which may be new), this films one
still with

    -2.0 play / -1.0 fit / 0.0 layout / 0.0 still

so the patch has played for two seconds (cables glow, displays show signal)
and the view frames the whole level, as F does. The `layout` cue says where
every module, frame and note was drawn and where the view was, so the still
is cropped to them with a little grid around, resized to 1594 px wide with
Lanczos and saved as a 256-colour palette (median cut, no dither): the house
style for the manual's full-patch pictures.

A patch too big to come out at least 1594 px wide in the first window is
filmed again in a window grown to suit, so pictures are only ever scaled down.
One too big to fit even at the editor's furthest zoom is filmed again with
the interface drawn smaller, so the window holds more.

--check films each one the same way but writes nothing: it reports which
images no longer match the current build. A capture plays out the same way
every time, so a picture that differs by more than a few stray pixels shows
something that has changed.

A few pictures show a moment in a song rather than a whole patch: they're
listed in MOMENTS, each by its image name, and filmed the same way except
that the song plays up to that moment, the view is aimed at one frame at
the scale the picture has always had, and the still is cropped to that
frame (or the part of it that's wanted) without resizing. Playing three
minutes of song frame by frame takes a few minutes.

A few more are close-ups of one module's face, listed in CLOSEUPS: the
example plays up to each moment wanted, with the view aimed at that face,
the same part of the face is cropped from each still, and the crops are set
on a grid.

Build into target/capture first, as for shots.py. --check exits 1 if any
image is stale or missing.
"""

import json
import subprocess
import sys
import tempfile
from collections import namedtuple
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
# The examples pictured whole, each as `recipe-<name>.png`: those whose
# recipe has no embed. Live Looper needs a live input, which an embed
# can't give it
STILLS = ["live-looper"]

# A picture of one moment in a song: `recipe-<image>.png` shows `frame`
# (by title) of patches/<patch>.json once it has played `at` seconds, drawn
# at `zoom` in a window at 1 pixel per point. `part` keeps only the frame's
# top-left (width, height) in patch points, where all of it would be too
# big to read; `clicks` are made a second or so before the still, at points
# from the frame's top-left, for the face to show what's playing
Moment = namedtuple("Moment", "patch at frame zoom part clicks")
# From One Sine's 4-bar unit at 112 BPM: unit n starts at (n - 1) * UNIT
UNIT = 16 * 60 / 112
MOMENTS = {
    # The Chord Sequencer playing Everything's chords
    "from-one-sine-harmony": Moment("from-one-sine", 22 * UNIT, "Harmony: a chord a bar", 1.5, None, []),
    # The motif's Step Sequencer at the Lift, two bars into the line's third
    # pass: a click on its Edit tab C shows the pattern that's playing
    "from-one-sine-line": Moment("from-one-sine", 14.5 * UNIT, "Motif, lead and memory", 1.75, (826, 568),
                                 [(407, 424)]),
}

# Close-ups of one face at several moments: `recipe-<image>.png` shows `crop`
# (x0, y0, x1, y1 in patch points from the module's top-left) of the first
# `module` (by id) in patches/<patch>.json, drawn at `zoom` and `PPP`, as it
# was each of `at` seconds after the patch started playing. The crops are
# set `columns` across, GAP pixels apart and from the edge, on BACKGROUND
Closeup = namedtuple("Closeup", "patch module at crop zoom columns")
CLOSEUPS = {
    # The Vocoder's bands frozen on each of the four sung vowels: ohh, ooh,
    # ee and ahh, each held from one downbeat to the next
    "held-vowels-faces": Closeup("held-vowels", "fx.vocoder", (5.6, 9.3, 13.0, 16.6), (18, 202, 151, 261.5), 2.0, 2),
}
GAP = 28
BACKGROUND = "#1e202c"

# How far apart two pictures may be before one is stale: a pixel counts as
# changed past this much in any channel, and a picture as stale past this
# fraction of changed pixels
CHANNEL_TOLERANCE = 24
STALE_FRACTION = 0.002


def film(patch, out, size, ppp, script=None):
    """Films one still of the patch, fitted unless a script says otherwise,
    and returns its layout."""
    text = script or SCRIPT
    script = out / "recipe.txt"
    script.write_text(text)
    subprocess.run([str(EXE), str(patch), "--capture", str(script), "--out", str(out), "--fps", "10",
                    "--size", f"{size[0]}x{size[1]}", "--ppp", str(ppp)],
                   check=True, cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    if "still" in text and not (out / "shot.ppm").exists():
        raise RuntimeError(f"no still at {size[0]}x{size[1]}: is the window too big for the screen?")
    return json.loads((out / "shot.json").read_text())


def crop_box(lay, foot=0.0):
    """The patch's drawn bounds, frames and notes included, in pixels.

    `foot` reaches past each module's rect, which stops short of the node's
    foot (#199): a picture whose lowest thing is a module needs it."""
    view = lay["view"]
    rects = [m["rect"][:3] + [m["rect"][3] + foot] for m in lay["modules"]]
    rects += [f["rect"] for f in lay["frames"]] + [n["rect"] for n in lay["notes"]]
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


def aim(view, point, zoom):
    """The camera cue that puts patch point `point` at the editor's centre at
    `zoom`, from the view as it opened, and where the patch's origin will
    then be drawn, in screen points."""
    # The camera scales the view about the editor's centre, then shifts it
    centre = [(view["editor"][i] + view["editor"][i + 2]) / 2 for i in range(2)]
    scale = zoom / view["zoom"]
    origin = [centre[i] - point[i] * zoom for i in range(2)]
    offset = [origin[i] - centre[i] - (view["origin"][i] - centre[i]) * scale for i in range(2)]
    return f"camera {offset[0]:.2f} {offset[1]:.2f} {scale:.5f} 0.05", origin


def moment_rect(lay, name):
    """The part of a moment's frame its picture shows, in patch points."""
    moment = MOMENTS[name]
    rect = next((f["rect"] for f in lay["frames"] if f["title"] == moment.frame), None)
    if rect is None:
        raise RuntimeError(f"{name}: no frame titled {moment.frame!r} in {moment.patch}")
    x0, y0, x1, y1 = rect
    if moment.part:
        x1, y1 = min(x1, x0 + moment.part[0]), min(y1, y0 + moment.part[1])
    return x0, y0, x1, y1


def shoot_moment(name, tmp):
    """Films a moment of a song and returns its picture."""
    moment = MOMENTS[name]
    patch = ROOT / "patches" / f"{moment.patch}.json"
    # A window with room for the frame at its zoom, toolbar and all
    saved = next(f for f in json.loads(patch.read_text())["frames"] if f["title"] == moment.frame)
    w, h = moment.part or saved["size"]
    size = (max(1920, round(w * moment.zoom) + 200), max(1080, round(h * moment.zoom) + 200))
    # Where the view opens, to aim the camera from
    out = Path(tmp) / f"{name}-probe"
    out.mkdir()
    lay = film(patch, out, size, 1.0, "0.0 layout shot\n0.1 end\n")
    x0, y0, x1, y1 = moment_rect(lay, name)
    camera, origin = aim(lay["view"], ((x0 + x1) / 2, (y0 + y1) / 2), moment.zoom)
    cues = [f"-{moment.at:.4f} play", f"-{moment.at:.4f} {camera}"]
    for i, (cx, cy) in enumerate(moment.clicks):
        t = -1.5 + 0.3 * i
        x, y = origin[0] + (x0 + cx) * moment.zoom, origin[1] + (y0 + cy) * moment.zoom
        cues += [f"{t - 0.3:.2f} move {x:.1f} {y:.1f} 0.2", f"{t:.2f} press", f"{t + 0.08:.2f} release"]
    # Then the pointer off every face, so nothing shows a hover
    cues += [f"-0.6 move {size[0] - 4} {size[1] - 4} 0.2", "0.0 layout shot", "0.0 still shot", "0.1 end"]
    out = Path(tmp) / name
    out.mkdir()
    lay = film(patch, out, size, 1.0, "\n".join(cues) + "\n")
    x0, y0, x1, y1 = moment_rect(lay, name)
    view = lay["view"]
    zoom, ppp, (ox, oy) = view["zoom"], view["ppp"], view["origin"]
    box = [(ox + x0 * zoom - MARGIN) * ppp, (oy + y0 * zoom - MARGIN) * ppp,
           (ox + x1 * zoom + MARGIN) * ppp, (oy + y1 * zoom + MARGIN) * ppp]
    image = Image.open(out / "shot.ppm").convert("RGB").crop([round(v) for v in box])
    return image.quantize(256, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE)


def closeup_rect(lay, name):
    """The part of a close-up's face its picture shows, in patch points."""
    closeup = CLOSEUPS[name]
    rect = next((m["rect"] for m in lay["modules"] if m["module_id"] == closeup.module), None)
    if rect is None:
        raise RuntimeError(f"{name}: no {closeup.module} in {closeup.patch}")
    x0, y0, x1, y1 = closeup.crop
    return rect[0] + x0, rect[1] + y0, rect[0] + x1, rect[1] + y1


def shoot_closeup(name, tmp):
    """Films a face at each of its moments and returns them on a grid."""
    closeup = CLOSEUPS[name]
    patch = ROOT / "patches" / f"{closeup.patch}.json"
    out = Path(tmp) / f"{name}-probe"
    out.mkdir()
    lay = film(patch, out, SIZE, PPP, "0.0 layout shot\n0.1 end\n")
    x0, y0, x1, y1 = closeup_rect(lay, name)
    camera, _ = aim(lay["view"], ((x0 + x1) / 2, (y0 + y1) / 2), closeup.zoom)
    rows = -(-len(closeup.at) // closeup.columns)
    sheet = None
    # A take for each moment, rather than one take with a still at each: the
    # face's animations (the snowflake, the falling peaks) run on the app's
    # clock, which starts with the warmup, so only this way is each still
    # taken at the same clock every time
    for i, at in enumerate(closeup.at):
        out = Path(tmp) / f"{name}-{i}"
        out.mkdir()
        lay = film(patch, out, SIZE, PPP, f"-{at:.2f} play\n-{at:.2f} {camera}\n0.0 layout shot\n0.0 still shot\n0.1 end\n")
        x0, y0, x1, y1 = closeup_rect(lay, name)
        view = lay["view"]
        zoom, ppp, (ox, oy) = view["zoom"], view["ppp"], view["origin"]
        # Every crop the same size, however the face falls on the pixels
        left, top = round((ox + x0 * zoom) * ppp), round((oy + y0 * zoom) * ppp)
        w, h = round((x1 - x0) * zoom * ppp), round((y1 - y0) * zoom * ppp)
        if sheet is None:
            sheet = Image.new("RGB", (GAP + closeup.columns * (w + GAP), GAP + rows * (h + GAP)), BACKGROUND)
        still = Image.open(out / "shot.ppm").convert("RGB")
        row, column = divmod(i, closeup.columns)
        sheet.paste(still.crop((left, top, left + w, top + h)), (GAP + column * (w + GAP), GAP + row * (h + GAP)))
    return sheet.quantize(256, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE)


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
    names = names or STILLS + list(MOMENTS) + list(CLOSEUPS)
    stale = []
    with tempfile.TemporaryDirectory() as tmp:
        for name in names:
            path = IMAGES / f"recipe-{name}.png"
            if name in MOMENTS:
                image = shoot_moment(name, tmp)
            elif name in CLOSEUPS:
                image = shoot_closeup(name, tmp)
            else:
                image = shoot(name, tmp)
            if check:
                fraction = changed(image, path)
                verdict = "ok" if fraction <= STALE_FRACTION else ("missing" if not path.exists() else f"stale ({fraction:.1%} changed)")
                print(f"{name:22} {verdict}")
                if fraction > STALE_FRACTION:
                    stale.append(name)
            else:
                image.save(path, optimize=True)
                print(f"{name:22} {image.width}x{image.height}, {path.stat().st_size // 1024} KB")
    if check and stale:
        print(f"\nRe-shoot with: python tools/showcase/recipe_images.py {' '.join(stale)}")
    return 1 if stale else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
