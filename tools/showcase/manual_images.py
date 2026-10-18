"""Re-shoots the manual's interface pictures: the window, its menus, the
tutorial's finished patch, a chord played into the Chord Sequencer's
popover and a lesson under way, each played and posed by a capture script.

    python tools/showcase/manual_images.py [name ...]
    python tools/showcase/manual_images.py --check [name ...]
    python tools/showcase/manual_images.py --refresh [name ...]  # --check, then save just the stale ones
    python tools/showcase/manual_images.py --check --all          # film them all, changed or not

Each picture in PICTURES names a patch, the cues that pose it (a chord
held, a menu opened, a key down) and what to keep of the window:

- "window": all of it, toolbar and status bar included
- "patch": the patch's modules, frames and notes with a little grid
  around, as recipe_images.py crops
- (x0, y0, x1, y1): a box in window points, for a menu or the palette,
  which the layout doesn't know about

A picture of a lesson names it (lesson="shaping-with-a-filter"): the app
starts it with --lesson, and its cues play the learner's moves, so the
caption and the ghost pointer are the ones the lesson draws. Its patch is
the one the lesson opens.

Every picture is filmed at 2 pixels per point, scaled down with Lanczos to
its width and saved as a 256-colour palette, as the recipe images are
(unless it says palette=False).
The cues aim at what they press by name where they can: a Chord Sequencer's
`step:2`, a frame's title bar (see src/app/capture_aim.rs). A menu's items
are aimed at from where it opened (`move by dx dy`). What's left in window
points, worked out from a still (pixel / 2), is empty canvas where a menu is
opened, since the crop around the menu is in window points too: fit always
frames a patch the same way in the same window, so they land on the same
spot every time.

--check films each one the same way but writes nothing, and lists the
pictures that no longer match the build. The overview shows the toolbar's
Output menu, which names this machine's audio device: on another machine
it reads stale for that alone.

Build into target/capture first, as for shots.py. --check exits 1 if any
picture is stale or missing.

--check and --refresh film only the pictures that something they show has
changed since, several at once (--jobs N): see picture_check.py. A picture
of a menu that lists every module says every_module=True, so a change to
any module films it. --all films them all, and pictures named on the
command line are always filmed.
"""

import json
import subprocess
import sys
from pathlib import Path

from PIL import Image

import picture_check
from common import ROOT, exe
from recipe_images import IMAGES, changed, crop_box

PPP = 2.0
SIZE = (3600, 2100)

# First Sound fitted in the 3600x2100 window: the canvas below the Audio
# Output is empty from y 530, and the Shape frame's floor is empty just below
# its title bar, 44 patch points down from its corner. A menu opens with its
# top left at the pointer
PICTURES = {
    "interface-overview": dict(
        patch="patches/lush-pad.json",
        every_module=True,
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
        every_module=True,
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
            0.8 move by 35 86 0.4
            1.2 move by 10 0 0.2
            1.8 wait
        """,
    ),
    "interface-quick-add": dict(
        patch="patches/first-sound.json",
        every_module=True,
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
            0.0 move frame:Shape 85.82 44.2 0.3
            0.5 rclick
            1.0 move by 101 41 0.4
            1.6 wait
        """,
    ),
    # Afterglow's Chord Sequencer, its steps aimed to y 262 so the popover
    # opens below them in full: step 2 (Cadd9) right-clicked open and
    # G# A C E held into it on the letter keys (H N , /), heard as Am/G#
    # with E on top. Stopped, so the step highlight stays still
    "chord-sequencer-play": dict(
        patch="patches/afterglow.json",
        crop=(745, 274, 1142, 774),
        width=588,
        cues="""
            -3.0 aim 451.4 1920.9 1.0 0.0
            -2.0 move seq.chord step:2 0.3
            -1.5 rclick
            -1.2 move 1500 900 0.2
            -1.0 key H 3.0
            -1.0 key N 3.0
            -1.0 key Comma 3.0
            -1.0 key Slash 3.0
            0.0 wait
        """,
    ),
    # Shaping with a Filter, its third step: the filter added from the
    # palette, and the ghost pointer partway through showing the cable from
    # the Oscillator's Out to the filter's In
    "lesson-shaping-with-a-filter": dict(
        patch="lessons/shaping-with-a-filter.json",
        lesson="shaping-with-a-filter",
        crop=(220, 250, 1390, 930),
        width=1400,
        cues="""
            -6.0 play
            -5.6 move 860 560 0.3
            -5.2 key Space 0.05
            -5.0 type svf 0.05
            -4.6 key Enter 0.05
            -4.3 move off
            0.0 wait
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
    lesson = ["--lesson", pic["lesson"]] if "lesson" in pic else []
    subprocess.run([str(exe()), str(ROOT / pic["patch"]), *lesson, "--capture", str(out / "manual.txt"), "--out", str(out),
                    "--fps", "10", "--size", f"{size[0]}x{size[1]}", "--ppp", str(PPP)],
                   check=True, cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True, errors="replace")
    image = Image.open(out / "shot.ppm").convert("RGB")
    crop = pic["crop"]
    if crop == "patch":
        box, overrun = crop_box(json.loads((out / "shot.json").read_text()))
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


def lesson_inputs(name):
    """A lesson picture's lesson, and the code that draws it."""
    lesson = PICTURES[name].get("lesson")
    if not lesson:
        return []
    return [ROOT / "lessons" / f"{lesson}.txt", ROOT / "src" / "app" / "lesson.rs", ROOT / "src" / "app" / "synth_app" / "lessons.rs"]


def main(args):
    unknown = [n for n in picture_check.names_arg(args) if n not in PICTURES]
    if unknown:
        print(f"unknown: {' '.join(unknown)} (pictures: {' '.join(PICTURES)})", file=sys.stderr)
        return 2
    return picture_check.run(
        args, "manual_images.py", list(PICTURES),
        png=lambda name: IMAGES / f"{name}.png",
        inputs=lambda name: picture_check.patch_inputs(ROOT / PICTURES[name]["patch"], extra=lesson_inputs(name),
                                                       every_module=PICTURES[name].get("every_module", False)),
        shoot=shoot, compare=changed, width=28)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
