"""Re-shoots the manual's module close-ups (docs/src/images/module-<name>.png),
each from a small patch in tools/showcase/modules/ that holds the moment its
caption describes.

    python tools/showcase/module_images.py [name ...]
    python tools/showcase/module_images.py --check [name ...]
    python tools/showcase/module_images.py --refresh [name ...]  # --check, then save just the stale ones
    python tools/showcase/module_images.py --check --all          # film them all, changed or not

Each close-up in MODULES names its patch, the module pictured, the zoom it's
drawn at, the width it's saved at and the cues that pose it (a chord held, a
cue playing). The patch is filmed once to find the module, then again with
the view aimed at it, and the still is cropped to the node with room at the
sides for its cable stubs.

Pointer cues aim at the module's parts by name (`move source.sampler cue`,
see src/app/capture_aim.rs), or at a point on it: `move fx.eq 234 135` is
(234, 135) points from its top left, as drawn at zoom 1. Read those off a
still, (pixel / PPP - margin) / zoom with the margins below, filmed at PPP:
the face's layout shifts a little from one ppp to another.

Build into target/capture first, as for shots.py. --check films each one
the same way but writes nothing, lists the close-ups that no longer match the
build, and exits 1 if any is stale or missing.

--check and --refresh film only the close-ups that something they show has
changed since, several at once (--jobs N): see picture_check.py. --all films
them all, and close-ups named on the command line are always filmed.
"""

import sys
from collections import namedtuple
from pathlib import Path

from PIL import Image

import picture_check
from recipe_images import IMAGES, ROOT, SIZE, aim, changed, film

MODULES_DIR = ROOT / "tools" / "showcase" / "modules"
PPP = 2.0
# Room kept round the node, in points at zoom 1: at the sides for the cable
# stubs, above and below to show it's the whole node
SIDE = 22
TOP = 6

# `patch` is tools/showcase/modules/<patch>.json, or an example's path when
# the caption names the example. The cues run up to the
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
        -2.6 move source.sampler cue 0.2
        -2.3 press
        -2.25 release
        -2.2 move source.sampler 600 300 0.2
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

    # Sources. A detuned square whose width an LFO sweeps: the trace is Out, live
    "oscillator": Module("oscillator", "osc.sine", 1.0, 327, "-2.0 play"),
    # White and Pink patched, Brown dimmed
    "noise": Module("noise", "source.noise", 1.0, 326, "-2.0 play"),
    # A kick 60 ms after a MIDI note strikes it: the pitch line has fallen, the
    # level lit to the playhead (the time axis is warped, so that's past halfway)
    "drum": Module("drum", "source.drum", 1.0, 467, """
        -1.0 play
        -0.06 note 36 100 0.05
    """),
    # Cut Up's break in sixteen slices, playing
    "chopper": Module("chopper", "source.chopper", 1.0, 484, "-3.0 play", palette=False),
    # A drum loop on input 1, Channel on 1: Follow scrolls past the Threshold line
    "audio-input": Module("audio-input", "source.audio_input", 1.0, 400, """
        -4.0 input patches/samples/soba-break.wav
        -4.0 play
    """, palette=False),

    # MIDI. C4 and G4 held on the computer keys
    "keyboard": Module("keyboard", "input.keyboard", 1.0, 336, """
        -1.5 play
        -1.2 key Z 3.0
        -0.6 key B 3.0
    """),
    "midi-note": Module("midi-note", "input.midi_note", 1.0, 343, """
        -1.5 play
        -1.0 note 60 100 3.0
        -0.5 note 64 100 3.0
    """),
    "poly-midi": Module("poly-midi", "input.poly_midi", 1.0, 324, """
        -1.5 play
        -1.0 chord 48,55,60,64,67 100 3.0
    """),
    # A phrase and a chord, the newest at the top
    "midi-monitor": Module("midi-monitor", "util.midi_monitor", 1.0, 371, """
        -3.5 play
        -3.0 note 60 90 0.3
        -2.6 note 64 100 0.3
        -2.2 note 67 80 0.3
        -1.8 chord 48,55,64 110 1.0
        -0.5 note 72 70 1.0
    """),

    # Filters, swept: the sound's trail along the floor
    "svf-filter": Module("svf-filter", "filter.svf", 1.0, 395, "-3.0 play", palette=False),
    "ladder-filter": Module("ladder-filter", "filter.ladder", 1.0, 395, "-3.0 play", palette=False),

    # Modulation. An LFO lane into Release: the R lit, Rel reading the stretched time
    "adsr": Module("adsr", "mod.adsr", 1.0, 395, "-3.0 play"),
    # Synced to 1/4: Rate reads the division, the dot in step with the beat
    "lfo": Module("lfo", "mod.lfo", 1.0, 250, "-2.0 play"),
    # Cycling, 1.2 s up and 400 ms down: the dot on its way down the fall
    "slope": Module("slope", "mod.slope", 1.0, 340, "-2.85 play"),
    # Sixteenths swung at 62%, Run and Reset into a sequencer
    "clock": Module("clock", "util.clock", 1.0, 224, "-2.0 play"),

    # Utilities. Level at 80%, an envelope holding a key's note at 44%
    "vca": Module("vca", "util.vca", 1.0, 332, """
        -1.5 play
        -1.2 key Z 3.0
    """),
    "attenuverter": Module("attenuverter", "util.attenuverter", 1.0, 265, "-2.0 play"),
    "mix": Module("mix", "util.mix", 1.0, 393, "-2.0 play"),
    # An LFO rides channel 2's Send and a slower one the Return
    "mixer": Module("mixer", "util.mixer", 1.0, 643, "-3.0 play", palette=False),
    "sample-hold": Module("sample-hold", "util.sample_hold", 1.0, 279, "-2.0 play"),
    # C minor pentatonic, D# playing
    "quantizer": Module("quantizer", "util.quantizer", 1.0, 365, "-3.0 play"),
    # A sixteenth clock divided by 16, the gate open for six: the count on bead 6
    "divider": Module("divider", "util.divider", 1.0, 315, "-0.8 play"),
    # A clock in A, an LFO in CV, nothing in B
    "logic": Module("logic", "util.logic", 1.0, 297, "-2.1 play"),
    # A four-bar loop recorded, closed, then overdubbed: the ring amber
    "looper": Module("looper", "util.looper", 1.0, 330, """
        -16.0 play
        -15.0 param util.looper Pedal Rec 1
        -14.9 param util.looper Pedal Rec 0
        -4.0 param util.looper Pedal Rec 1
        -3.9 param util.looper Pedal Rec 0
    """, palette=False),
    "oscilloscope": Module("oscilloscope", "util.oscilloscope", 1.0, 395, "-1.0 play"),
    # Disco, just after a beat
    "dancer": Module("dancer", "util.dancer", 1.0, 400, "-2.1 play", palette=False),

    # Sequencers. Pattern B, chained A A B C, partway through
    "sequencer": Module("sequencer", "seq.step", 1.0, 505, """
        -4.69 play
        -0.6 move seq.step pattern:B 0.1
        -0.4 press
        -0.35 release
        -0.3 move seq.step 700 300 0.1
    """),
    # A line cliché, Am9 to E7, on Dm9
    "chord-sequencer": Module("chord-sequencer", "seq.chord", 1.0, 479, "-11.0 play"),
    # A Chord Sequencer's five-voice chord lit on the keys, an LFO on CC 2
    # and the clock's beat lamps going round
    "midi-out": Module("midi-out", "output.midi", 1.0, 420, "-5.3 play"),
    # Roll Call's pattern B, its snare roll ratcheting, C next
    "trigger-sequencer": Module("patches/roll-call.json", "seq.trigger", 1.0, 720, """
        -7.57 play
        -0.6 move seq.trigger pattern:B 0.1
        -0.4 press
        -0.35 release
        -0.3 move seq.trigger 700 300 0.1
    """),
    # A five-section song, playing its Build
    "arranger": Module("arranger", "seq.arranger", 1.0, 881, "-10.8 play"),

    # Effects
    # A clap into the left side only, ping-ponged on worn tape: its echoes
    # take turns down the train, left above the line and right below,
    # warming as they go
    "delay": Module("delay", "fx.delay", 1.0, 320, """
        -2.6 play
    """, palette=False),
    # A snare every other second into a 3 s room: a second after the hit
    # the dot has run a third of the way down the slope
    "reverb": Module("reverb", "fx.reverb", 1.0, 320, """
        -3.0 play
    """, palette=False),
    # Pink noise through a warm low shelf, a cut in the middle and an airy top,
    # the pointer on the mid band's dot: its wings light, and the header reads it
    "eq": Module("eq", "fx.eq", 1.0, 440, """
        -2.0 play
        -0.6 move fx.eq 234 135 0.3
    """, palette=False),
    # A low sine into the folder, a little off centre: the lit stretch runs
    # over the first fold, and the wave on the right folds back at its peaks
    "distortion": Module("distortion", "fx.distortion", 1.0, 300, """
        -1.5 play
    """, palette=False),
    # Toms on every beat through three voices: the braid lights in bursts
    # where each hit passed, and slowly weaves on between them
    "chorus": Module("chorus", "fx.chorus", 1.0, 320, """
        -2.4 play
    """, palette=False),
    # A held key brakes the tape halfway through its Stop time
    "tape": Module("tape", "fx.tape", 1.0, 405, """
        -3.0 play
        -0.75 key Z 2.0
    """, palette=False),
    # Frozen mid-word, Flatten halfway
    "vocoder": Module("vocoder", "fx.vocoder", 1.0, 291, """
        -4.0 play
        -4.0 chord 48,55,60,63 90 6.0
        -1.0 param fx.vocoder Freeze 1
    """, palette=False),

    # Pushed hot, so the limiter catches some
    "audio-output": Module("audio-output", "output.audio", 1.0, 359, "-2.0 play"),
}


def module_rect(lay, name):
    """Where the pictured module is drawn, in patch space."""
    module = MODULES[name].module
    rect = next((m["rect"] for m in lay["modules"] if m["module_id"] == module), None)
    if rect is None:
        raise RuntimeError(f"{name}: no {module} in {MODULES[name].patch}.json")
    return rect


def script(cues, camera):
    """The close-up's cues, with the view moved to the module before the first
    and the still taken at 0.0."""
    lines = [line.strip() for line in cues.strip().splitlines()]
    first = min([float(line.split()[0]) for line in lines] + [0.0])
    return "\n".join([f"{first - 0.5:.2f} {camera}"] + lines + ["0.0 layout shot", "0.0 still shot", "0.1 end"]) + "\n"


def shoot(name, tmp):
    """Films a close-up and returns it, cropped and scaled."""
    shot = MODULES[name]
    patch = patch_path(name)
    probe = Path(tmp) / f"{name}-probe"
    probe.mkdir()
    lay = film(patch, probe, SIZE, PPP, "0.0 layout shot\n0.1 end\n")
    rect = module_rect(lay, name)
    camera, _ = aim(lay["view"], ((rect[0] + rect[2]) / 2, (rect[1] + rect[3]) / 2), shot.zoom)
    out = Path(tmp) / name
    out.mkdir()
    lay = film(patch, out, SIZE, PPP, script(shot.cues, camera))
    x0, y0, x1, y1 = module_rect(lay, name)
    view = lay["view"]
    zoom, (ox, oy) = view["zoom"], view["origin"]
    box = [(ox + (x0 - SIDE) * zoom) * PPP, (oy + (y0 - TOP) * zoom) * PPP,
           (ox + (x1 + SIDE) * zoom) * PPP, (oy + (y1 + TOP) * zoom) * PPP]
    width, height = Image.open(out / "shot.ppm").size
    if box[0] < 0 or box[1] < 0 or box[2] > width or box[3] > height:
        raise RuntimeError(f"{name}: the node runs out of the window at zoom {shot.zoom}")
    image = Image.open(out / "shot.ppm").convert("RGB").crop([round(v) for v in box])
    image = image.resize((shot.width, round(image.height * shot.width / image.width)), Image.LANCZOS)
    if not shot.palette:
        return image
    return image.quantize(256, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE)


def patch_path(name):
    patch = MODULES[name].patch
    return ROOT / patch if patch.endswith(".json") else MODULES_DIR / f"{patch}.json"


def main(args):
    unknown = [n for n in picture_check.names_arg(args) if n not in MODULES]
    if unknown:
        print(f"unknown: {' '.join(unknown)} (close-ups: {' '.join(MODULES)})", file=sys.stderr)
        return 2
    return picture_check.run(
        args, "module_images.py", list(MODULES),
        png=lambda name: IMAGES / f"module-{name}.png",
        inputs=lambda name: picture_check.patch_inputs(patch_path(name)),
        shoot=shoot, compare=changed, width=20)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
