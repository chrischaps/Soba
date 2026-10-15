"""Shot list for the chaps.dev showcase: writes each shot's capture script
and films it with `soba --capture`.

    python tools/showcase/shots.py <shot> [<shot> ...]   film shots
    python tools/showcase/shots.py --list                 list them
    python tools/showcase/shots.py <shot> --script-only   just write the script
    python tools/showcase/shots.py <shot> --preview       film frame 0 only, to aim from
    python tools/showcase/shots.py <shot> --framings      a still of every camera move, on one sheet
    python tools/showcase/shots.py <shot> --dry           a touring shot's moves packed tight: a still of
                                                          each, and every module's size (see tour.py)
    python tools/showcase/shots.py <shot> --converge      dry runs until the module sizes stop changing
    python tools/showcase/shots.py <shot> --subjects      is each hold's subject playing? (activity.py)

Films (whole songs, toured through their groups) are walked through in FILMS.md.

Each shot is a function that builds a cue script (see src/app/capture.rs for
the cue language). Output goes to target/showcase/<shot>/: video.mkv
(lossless, unless the shot asks for a crf), audio.wav and any stills as
PNG. encode.py turns those into web clips.

Coordinates are in points at the shot's ppp (1.55 by default, where the
toolbar takes two rows in a 1920x1080 frame). Find them from a still:
pixel / ppp. Aim from a still at the shot's own ppp: the layout shifts a few
points from one ppp to another, enough to miss a small button. A press that
just misses a control warns as the shot films.

The shots at ppp 1.55 aim in patch space instead: `s.look` moves the camera
to centre a patch point at a zoom, and `s.on(point)` is where a patch point
is drawn once it lands. So a knob is found once, at zoom 1 (patch point =
pixel / ppp - the editor's top-left), and stays found however the shot is
framed. Each example opens at zoom 1 with patch (0, 0) at the editor's
top-left, below the toolbar.
"""

import subprocess
import sys
from pathlib import Path

from tour import EDITORS, Tour

ROOT = Path(__file__).resolve().parents[2]
EXE = ROOT / "target" / "capture" / "release" / "soba.exe"
SCRIPTS = Path(__file__).resolve().parent / "scripts"
OUT = ROOT / "target" / "showcase"
# What a film keeps between takes: where its dives land (aims*.json) and what
# plays when (activity.json). See FILMS.md
FILMS = Path(__file__).resolve().parent / "films"

SHOTS = {}


def shot(patch, ppp=1.55, fps=60, crf=0, preset="ultrafast", size="1920x1080"):
    """Registers a shot function that fills in a Script. A long shot wants a
    crf: lossless 1080p is about 20 MB a second at 30 fps."""
    def register(fn):
        SHOTS[fn.__name__.replace("_", "-")] = (fn, patch, ppp, fps, crf, preset, size)
        return fn
    return register


# The node editor at ppp 1.55 in a 1920x1080 window, in points: below the
# toolbar's two rows, above the status bar (from a `layout` cue's view)
EDITOR = (0.0, 74.7, 1238.7, 670.8)


class Script:
    def __init__(self):
        self.cues = []
        # A touring shot (tour.py) films its moves packed tight on a dry run
        self.dry = False
        self.tour = None
        # The shot's window, for a tour to find its editor (tour.EDITORS)
        self.size, self.ppp = "1920x1080", 1.55
        # The patch point at the editor's centre, and the zoom: as opened
        x0, y0, x1, y1 = EDITOR
        self.centre = ((x1 - x0) / 2, (y1 - y0) / 2)
        self.zoom = 1.0

    def look(self, t, point, zoom, dur, note=""):
        """Moves the camera over `dur` seconds to centre a patch point at a
        zoom. Later `on` calls aim at the view it lands on."""
        x0, y0, x1, y1 = EDITOR
        cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
        ox, oy = -(x0 + point[0] - cx) * zoom, -(y0 + point[1] - cy) * zoom
        self.at(t, f"camera {ox:.1f} {oy:.1f} {zoom} {dur}" + (f"   # {note}" if note else ""))
        self.centre, self.zoom = point, zoom
        return self

    def on(self, point):
        """Where a patch point is drawn, in screen points, in the view the
        last `look` lands on."""
        x0, y0, x1, y1 = EDITOR
        return (round((x0 + x1) / 2 + (point[0] - self.centre[0]) * self.zoom, 1),
                round((y0 + y1) / 2 + (point[1] - self.centre[1]) * self.zoom, 1))

    def at(self, t, cue):
        self.cues.append((t, cue))
        return self

    def keys(self, t0, bpm, steps, gate=0.6, step=0.25):
        """Plays QWERTY keys in time: one entry per step (a key name, or
        None for a rest, or (key, steps) to hold). Returns the end time."""
        beat = 60.0 / bpm
        t = t0
        for s in steps:
            length = 1
            if isinstance(s, tuple):
                s, length = s
            if s is not None:
                self.at(t, f"key {s} {step * beat * length * gate:.4f}")
            t += step * beat * length
        return t

    def drag_knob(self, t, x, y, dy, dur, settle=0.35):
        """Moves to a knob, then drags it up by `dy` points (down if
        negative) over `dur` seconds. Knobs take 200 points for their whole
        range. Returns the time the drag ends."""
        self.at(t, f"move {x} {y} {settle}")
        t += settle + 0.1
        self.at(t, "press")
        # Past egui's drag threshold first, so the knob starts at once
        self.at(t + 0.02, f"move {x} {y - 4 * (1 if dy > 0 else -1)} 0.04")
        self.at(t + 0.08, f"move {x} {y - dy} {dur}")
        self.at(t + 0.1 + dur, "release")
        return t + 0.1 + dur

    def click(self, t, x, y, travel=0.4):
        self.at(t, f"move {x} {y} {travel}")
        self.at(t + travel + 0.08, "press")
        self.at(t + travel + 0.16, "release")
        return t + travel + 0.16

    def text(self):
        return "\n".join(f"{t:.4f} {cue}" for t, cue in sorted(self.cues, key=lambda c: c[0])) + "\n"


# --- Shots -----------------------------------------------------------------

@shot("patches/basic-subtractive.json")
def filter_sweep(s):
    """A bassline through the SVF while a hand opens the filter and adds
    resonance: the response curve and the sound move together."""
    cutoff, res = (597, 348), (648, 348)
    s.at(-0.8, "play").look(-0.8, (698, 345), 0.9, 0.05, "Source, Tone and Volume")
    s.at(-0.8, "param input.keyboard Octave -1")
    # C minor, sixteenths: Z=C X=D D=Eb B=G J=Bb ,=C+ ;=Eb+
    bar = ["Z", "Z", "Comma", "Z", "D", "Z", "J", "Z", "B", "Z", "Comma", "Z", "Semicolon", "Z", "J", "B"]
    t = 0.0
    for _ in range(6):
        t = s.keys(t, 116, bar, gate=0.55)
    s.at(0.0, "cursor on")
    s.at(1.5, "still filter-closed")
    # Cutoff is a log knob: 200 points span 20 Hz to 20 kHz
    end = s.drag_knob(1.6, *s.on(cutoff), 60, 4.0)
    end = s.drag_knob(end + 0.4, *s.on(res), 80, 2.2)
    s.at(end + 0.5, "still filter-open")
    end = s.drag_knob(end + 1.0, *s.on(cutoff), -90, 3.4)
    s.at(end + 0.3, "move %s %s 1.0" % s.on((150, 560)))
    s.at(t, "end")


@shot("patches/lush-pad.json")
def chords(s):
    """Held chords on Poly MIDI: each voice is its own strand in the cable
    bundles, lit only while its note sounds."""
    s.at(-1.0, "play").look(-1.0, (700, 410), 0.78, 0.05, "Every voice, and the Mixer")
    progression = [
        [48, 55, 59, 62, 64],   # Cmaj9
        [45, 52, 55, 59, 60],   # Am9
        [41, 48, 52, 57, 59],   # Fmaj7#11
        [43, 50, 57, 60, 64],   # G6/9sus
        [48, 55, 59, 64, 67],   # Cmaj7, opened up
    ]
    t = 0.3
    for i, chord in enumerate(progression):
        hold = 3.3 if i < len(progression) - 1 else 4.5
        # A hand on a keyboard: the notes land a few milliseconds apart
        for k, note in enumerate(chord):
            s.at(t + k * 0.012, f"note {note} {84 - k * 4} {hold - 0.15}")
        t += 3.4
    # A slow push in on the bundles, from Poly MIDI to the envelope
    s.look(0.0, (500, 300), 1.3, 19, "in on the bundles")
    s.at(5.0, "still chord-bundles")
    s.at(13.0, "still chord-bundles-close")
    s.at(t + 2.5, "end")


@shot("patches/fm-synthesis.json")
def fm_bell(s):
    """One sine bending another's pitch: turning up the FM depth takes a
    pure tone to a bell to a clang."""
    fm = (857, 494)   # the carrier's FM knob
    s.at(-1.0, "play").look(-1.0, (790, 400), 0.8, 0.05, "Keyboard, Modulator, Carrier")
    # C major pentatonic, eighths at 84 bpm: Z=C X=D C=E B=G N=A ,=C+ .=D+ /=E+
    phrase = ["Comma", None, "N", None, "B", None, "C", "X",
              ("C", 2), "B", None, ("N", 4),
              "Slash", None, "Period", "Comma", "N", None, "B", "C",
              ("X", 3), "C", ("Z", 4)]
    t = s.keys(0.2, 84, phrase, gate=0.5, step=0.5)
    t = s.keys(t, 84, phrase, gate=0.5, step=0.5)
    s.at(0.0, "cursor on")
    # FM Depth: 0 to 5 over 200 points, starting at 2.5
    end = s.drag_knob(3.6, *s.on(fm), 80, 4.5)
    # Into the gap between the frames, so no tooltip shows
    s.at(end + 0.1, "move %s %s 0.4" % s.on((778, 600)))
    s.at(end + 0.6, "still fm-bright")
    end = s.drag_knob(12.6, *s.on(fm), -150, 5.0)
    s.at(end + 0.3, "move %s %s 1.2" % s.on((140, 560)))
    s.at(end + 1.6, "still fm-pure")
    s.at(t + 2.0, "end")


# How far below the Sync selector its first item, Off, opens (screen points)
SYNC_OFF = 33


@shot("patches/rhythmic-sequence.json")
def footswitch(s):
    """The sequence plays itself; a hand stomps the distortion out and back
    in, puts the delay on tape and feeds it back."""
    power, drive = (1508, 88), (1520, 228)
    tape, sync = (1762, 257), (1784, 228)
    time, feedback, mix = (1733, 338), (1784, 338), (1837, 338)
    s.at(-2.0, "play").look(-2.0, (1560, 360), 0.88, 0.05, "Bass, Drive and echo")
    s.at(0.0, "cursor on")
    # Distortion's power switch: out, then back in
    s.click(1.6, *s.on(power), travel=0.7)
    s.at(3.2, "still distortion-bypassed")
    s.click(4.6, *s.on(power), travel=0.2)
    end = s.drag_knob(5.6, *s.on(drive), 60, 2.6)
    # The delay goes to tape
    end = s.click(end + 0.5, *s.on(tape), travel=0.6)
    end = s.drag_knob(end + 0.6, *s.on(feedback), 80, 3.0)
    end = s.drag_knob(end + 0.3, *s.on(mix), 40, 1.4)
    s.at(end + 1.0, "still tape-echoes")
    # Unsync it, so the Time knob turns freely: Off heads Sync's list
    end = s.click(end + 1.6, *s.on(sync), travel=0.5)
    x, y = s.on(sync)
    end = s.click(end + 0.5, x - 8, y + SYNC_OFF, travel=0.35)
    # On tape, changing the time bends the echoes' pitch
    end = s.drag_knob(end + 0.4, *s.on(time), -30, 1.1, settle=0.5)
    end = s.drag_knob(end + 0.5, *s.on(time), 45, 1.6, settle=0.2)
    # Off into empty canvas, so no tooltip lingers
    s.at(end + 0.2, "move %s %s 0.9" % s.on((1500, 640)))
    s.at(end + 1.6, "still tape-time")
    s.at(end + 4.5, "end")


@shot("patches/generative-ambient.json")
def hero(s):
    """Generative Ambient plays itself while the camera follows the signal
    from the clock to the speakers, pulls back to the whole patch, and comes
    home so the clip loops."""
    home, zoom = (690, 360), 0.9
    s.at(-3.0, "play").look(-3.0, home, zoom, 0.05, "the Melody: clock, divider, sequencer")
    s.at(0.5, "still clock-and-sequencer")
    # Along the signal path, clock to output
    s.look(6.5, (2380, 300), zoom, 21.5, "along the signal to the mixer and output")
    s.at(17.0, "still oscillators-and-filter")
    s.at(27.6, "still delay-and-reverb")
    # Back out to the whole patch: it spans 16..3017 across, 16..1587 down
    s.look(28.0, (1516, 800), 0.36, 5, "the whole patch")
    s.at(34.0, "still whole-patch")
    s.look(33.0, (1516, 800), 0.372, 7, "a breath closer")
    # And home, so the clip loops
    s.look(40.0, home, zoom, 4.5, "home")
    s.at(46.5, "end")


def add_module(s, t, x, y, query):
    """Opens the quick-add palette at (x, y), types `query` and adds the
    top match. Returns the time it lands."""
    s.at(t, f"move {x} {y} 0.55")
    s.at(t + 0.7, "key Space 0.05")
    s.at(t + 0.9, f"type {query} 0.09")
    t += 0.9 + 0.09 * len(query) + 0.3
    s.at(t - 0.05, f"still palette-{query}")
    s.at(t, "key Enter 0.05")
    return t + 0.3


# The toolbar's Play button at ppp 1.55, in screen points
PLAY = (158, 21)


@shot("tools/showcase/patches/empty.json")
def from_nothing(s):
    """An empty canvas to a playable voice: five modules from the quick-add
    palette, five cables, then a tune."""
    s.at(0.0, "cursor on")
    # A module's top-left lands at the cursor (patch points; the canvas
    # stays at zoom 1). Its jacks sit at fixed offsets from there
    kb, osc, env, vca, out = (40, 70), (270, 25), (530, 150), (800, 60), (990, 40)
    t = add_module(s, 0.6, *s.on(kb), "keyboard")
    t = add_module(s, t + 0.2, *s.on(osc), "osc")
    t = add_module(s, t + 0.2, *s.on(env), "env")
    t = add_module(s, t + 0.2, *s.on(vca), "vca")
    t = add_module(s, t + 0.2, *s.on(out), "output")
    s.at(t + 0.3, "still placed")
    s.at(t + 0.3, "layout placed")

    def jack(node, dx, dy):
        return s.on((node[0] + dx, node[1] + dy))

    cables = [
        (jack(kb, 185.2, 121.6), jack(osc, 0.3, 41.0)),     # Pitch -> V/Oct
        (jack(kb, 185.2, 101.6), jack(env, 0, 44.2)),       # Gate -> Gate
        (jack(osc, 178.3, 174.5), jack(vca, 0, 44.8)),      # Out -> In
        (jack(env, 229.5, 188.0), jack(vca, 0, 66.1)),      # Out -> CV
        (jack(vca, 126.3, 82.9), jack(out, 0, 86.1)),       # Out -> Mono
    ]
    t += 0.6
    for (x0, y0), (x1, y1) in cables:
        s.at(t, f"move {x0:.1f} {y0:.1f} 0.45")
        t += 0.55
        s.at(t, f"drag {x1:.1f} {y1:.1f} 0.65")
        t += 0.65 + 0.1 + 0.25
    s.at(t, "still patched")
    # Play, on the toolbar
    t = s.click(t + 0.2, *PLAY, travel=0.7) + 0.4
    s.at(t, "move %s %s 0.8" % s.on((1100, 470)))
    # First notes: a C major pentatonic tune, Z=C X=D C=E B=G N=A ,=C+
    tune = ["Z", "C", "B", ("Comma", 2), "N", "B", None,
            "C", "B", "N", ("B", 2), "C", ("X", 2),
            "Z", "C", "B", ("Comma", 2), "Period", "Comma", "N",
            "B", "C", "X", ("Z", 4)]
    t = s.keys(t + 0.3, 112, tune, gate=0.8, step=0.5)
    s.at(t - 2.0, "still playing")
    s.at(t + 1.2, "end")


@shot("patches/lush-pad.json")
def cover(s):
    """A macro of the poly bundles leaving Poly MIDI while a chord rings:
    candidates for the page's cover."""
    s.at(-1.0, "play").look(-0.9, (300, 270), 2.5, 0.05, "Poly MIDI's lit keys and the bundles leaving it")
    for k, note in enumerate([48, 55, 59, 62, 64, 71]):
        s.at(0.2 + k * 0.012, f"note {note} {90 - k * 5} 5.5")
    for i, t in enumerate([1.2, 2.0, 3.0, 4.0, 5.0]):
        s.at(t, f"still cover-{i}")
    s.at(5.2, "end")


# The Dance Floor is 108 BPM, and a Dancer's phrase is 16 beats: four bars.
# At 108, three phrases are a whole number of frames (1600 at 60 fps, 800 at
# 30) and of samples, so the loop's two ends line up exactly
DANCE_BEAT = 60 / 108
DANCE_PHRASE = 16 * DANCE_BEAT
# Each phrase a Dancer changes step, on a walk of its own that never comes
# round again, so a clip only loops where every style on the stage is back
# on the step it started on. For Robot, Disco and Headbang that's phrases 44
# and 47 (dancer_moves::step_in): the shot films from beat 12 of phrase 44,
# so a crash lands 4 beats in, and runs three phrases, the band's four bars
# three times over
DANCE_FROM = (44 * 16 + 12) * DANCE_BEAT
DANCE_LOOP = 3 * DANCE_PHRASE
# encode.py --loop crossfades this much of the end under the start
DANCE_SEAM = 0.5


@shot("tools/showcase/patches/dance-floor.json")
def dancers(s):
    """Three Dancers on one funk vamp, each hearing a different part of the
    band, the big one in the middle hearing all of it, and the crash at
    every phrase change its cue. Loops on its last half second."""
    stage, wide = (2810, 410), 0.72
    big, close = (2810, 448), 1.05
    s.at(-DANCE_FROM, "play")
    s.look(-1.0, stage, wide, 0.05, "the stage, all three")
    # In on the middle one through the second phrase, catching its crash,
    # and back out through the third to where it began
    s.look(4.0, big, close, 6.5, "in on the big one")
    s.look(14.5, stage, wide, 10.5, "back out to all three")
    # Each crash, half a beat into the big move it cues
    for k in range(3):
        s.at((4.5 + 16 * k) * DANCE_BEAT, f"still dancers-accent-{k}")
    s.at(DANCE_LOOP + DANCE_SEAM + 0.1, "end")


# From One Sine is in 4-bar units of 8.571 s (112 BPM); unit n starts at u(n)
SONG_UNIT = 16 * 60 / 112


def u(n):
    return (n - 1) * SONG_UNIT


@shot("patches/from-one-sine.json", ppp=1.0, fps=30, crf=12, preset="fast")
def from_one_sine(s):
    """The whole song, 4:41: the camera follows the score part by part, and
    ends where it began."""
    # Each shot centres a point in patch space at a zoom. At ppp 1.0 in a
    # 1920x1080 frame the editor's centre is (960, 551), and patch y = 0 sits
    # 43 points down (below the toolbar), so the camera offset that centres P
    # is -(P - c) * zoom with c = (960, 508). The frames below are the
    # patch's own: e.g. Drums spans x 0..3246, y 3992..4957. The whole rack
    # is 5923 x 4957, so it fits at the 0.2 minimum zoom.
    c = (960.0, 508.0)
    whole = (2962, 2479)
    # The left column below Harmony moved down 80 when the Chord Sequencer
    # grew its pattern tabs (#240)
    motif = (888, 2471)
    opening = (500, 2278)
    # (start, glide seconds, centre, zoom, what)
    shots = [
        (-1.0, 0.05, opening, 1.5, "the sine and its sequencer, alone, with the note that explains them"),
        (3.0, 13.0, (860, 2328), 1.25, "a slow drift along the motif"),
        (u(3), 12.0, (1895, 2429), 0.43, "back out: the pad, sparkles and harmony arrive"),
        (u(5) - 1.0, 3.0, (3100, 734), 0.64, "Pulse: arp and bass"),
        (u(7) - 0.5, 3.0, (2990, 2033), 0.88, "the bells and sparkles"),
        (u(8) + 1.0, 3.0, (2808, 2995), 1.1, "the riser"),
        (u(9) - 0.6, 1.4, (1623, 4554), 0.56, "Groove: the kit"),
        (u(11), 4.0, (5002, 2206), 0.9, "the desk"),
        (u(13) - 1.0, 3.0, (1150, 2471), 0.95, "Lift: the lead takes the motif"),
        (u(15), 6.0, whole, 0.2, "the whole rack"),
        (u(17) - 1.0, 4.0, (1640, 2305), 1.25, "Memory: the Looper plays the opening back"),
        (u(18) + 4.0, 5.0, (1048, 805), 0.9, "the three Arrangers: Desk's faders, Cues, and Tape bending the Wow"),
        (u(19) + 4.5, 4.0, (5366, 2330), 1.4, "Build: on tape, the mix bending as the reels turn"),
        (u(20) + 4.0, round(SONG_UNIT - 4.0, 3), whole, 0.2, "the build pulls back to the whole rack, landing on the drop"),
        (u(22), 8.0, (2962, 2398), 0.21, "Everything: a breath closer, every module still in frame"),
        (u(23) + 3.0, 5.0, (3100, 1080), 0.95, "the bass, driven harder as the song brightens"),
        (u(25) - 1.0, 3.0, motif, 0.88, "Second wave: the sine over the groove, its ghost beneath"),
        (u(27), 4.0, (1623, 4054), 0.48, "the pad breathing over the kick"),
        (u(29), 6.0, whole, 0.2, "Return"),
        (u(31), 12.0, opening, 1.5, "back to the one sine"),
    ]
    s.at(0.0, "play")
    for start, glide, (x, y), zoom, what in shots:
        s.at(start, f"camera {-(x - c[0]) * zoom:.1f} {-(y - c[1]) * zoom:.1f} {zoom} {glide}   # {what}")
    s.at(u(9) + 0.5, "still groove")
    s.at(u(23), "still everything")
    # The song's last unit ends; stop the clock and let the echoes ring out
    s.at(u(33) - 0.01, "param util.clock Run 0")
    s.at(281.0, "end")


# Canopy is 116 BPM in 4/4, 124 bars of 2.069 s; bar n (from 1) starts at
# bar(n). Its sections start at bars 1 Dawn, 9 Canopy, 17 Groove, 33 Groove
# II, 49 Clearing, 57 Rise, 65 Peak, 81 Peak II, 97 Dusk, 113 Night and 121
# Ring, with a one-bar Fill before each Groove and Peak after the first
CANOPY_BAR = 240 / 116


def bar(n):
    return (n - 1) * CANOPY_BAR


def canopy_tour(s, aims):
    """A Tour of Canopy, every group opened once before recording starts."""
    tour = Tour(s, ROOT / "patches/canopy.json", editor=EDITORS[(s.size, s.ppp)], bases=FILMS / "canopy" / aims, dry=s.dry)
    end = tour.previsit(-46.0)
    assert end < -1.0, f"the previsit runs to {end:.1f} s: start it earlier"
    return tour


@shot("patches/canopy.json", ppp=1.25, fps=30, crf=12, preset="fast")
def canopy(s):
    """Canopy, the whole song (4:24): the camera goes where the music is,
    into each group as it plays and back out, with the Dancer by the desk."""
    tour = canopy_tour(s, "aims.json")
    face = 1.5
    whole = (2095, 994)
    score = (1388, 400)
    s.at(0.0, "play")

    # Dawn: the kalimba fades in under wind and birds
    tour.look(-0.5, (2660, 1330), 0.62, 0.05, "Dawn: the loops and the forest", ["kalimba", "wind"])
    tour.look(0.6, tour.centre_of("Kalimba"), face, 5.0, "the Kalimba", "kalimba")
    tour.enter(6.2, "Kalimba")
    tour.inside(7.0, 14.0, "seq.step", 1.5, "Kalimba", "kalimba")
    tour.leave(14.3)
    tour.look(14.9, tour.centre_of("Bells"), face, 1.6, "the Bells", "kalimba")

    # Canopy: bells, strings and a shaker
    tour.enter(bar(9) + 0.6, "Bells")
    tour.inside(bar(9) + 1.4, bar(9) + 4.6, None, note="Bells", subject="bells")
    tour.enter(bar(9) + 5.0, "FM Bell")
    tour.inside(bar(9) + 5.8, bar(9) + 10.5, "osc.sine", 1.4, "FM Bell", "bells")
    tour.leave(bar(9) + 10.9)
    tour.leave(bar(9) + 11.5)
    tour.look(bar(9) + 12.1, (3240, 700), 0.6, 3.6, "the band and the desk, as the beat comes in", None)

    # Groove: the beat, the bass and the Rhodes
    tour.look(bar(17) + 4.0, tour.centre_of("Drums"), face, 2.0, "the Drums", "drums-kit")
    tour.enter(bar(17) + 6.4, "Drums")
    tour.inside(bar(17) + 7.2, bar(17) + 14.5, "source.drum", 1.6, "Drums", "drums-kit")
    tour.leave(bar(17) + 14.8)
    tour.look(bar(17) + 15.4, tour.centre_of("Bass"), face, 1.5, "the Bass", "bass")
    tour.enter(bar(17) + 17.3, "Bass")
    tour.inside(bar(17) + 18.1, bar(17) + 23.6, "filter.ladder", 1.5, "Bass", "bass")
    tour.leave(bar(17) + 23.9)
    tour.hop(bar(17) + 24.5, tour.centre_of("Rhodes"), face, 2.4, "the Rhodes", "rhodes")
    tour.enter(bar(17) + 27.2, "Rhodes")
    tour.look(bar(17) + 28.0, tour.middle(), tour.fit_zoom(), 0.8, "Rhodes: its sequencer", "rhodes")
    # Through the Fill and into Groove II, inside the Electric Piano
    tour.enter(bar(17) + 29.4, "Electric Piano")
    tour.inside(bar(17) + 30.2, bar(33) + 0.2, "mod.adsr", 1.2, "Electric Piano", "rhodes")

    # Groove II: the harp and the break join
    tour.leave(bar(33) + 0.4)
    tour.leave(bar(33) + 1.0)
    tour.look(bar(33) + 1.6, tour.centre_of("Harp"), face, 1.4, "the Harp", "harp")
    tour.enter(bar(33) + 3.3, "Harp")
    tour.look(bar(33) + 4.1, tour.middle(), tour.fit_zoom(), 0.8, "Harp", "harp")
    tour.enter(bar(33) + 5.6, "Mallet")
    tour.inside(bar(33) + 6.4, bar(33) + 11.6, "filter.svf", 1.4, "Mallet", "harp")
    tour.leave(bar(33) + 11.9)
    tour.leave(bar(33) + 12.5)
    tour.hop(bar(33) + 13.1, tour.centre_of("Break"), face, 2.4, "the Break", "break")
    tour.enter(bar(33) + 15.8, "Break")
    tour.inside(bar(33) + 16.6, bar(33) + 22.6, "source.chopper", 1.5, "Break", "break")
    tour.leave(bar(33) + 22.9)
    tour.look(bar(33) + 23.5, tour.centre_of("Vocal Chops"), face, 1.4, "the Vocal Chops", "vocal-chops")
    tour.enter(bar(33) + 25.2, "Vocal Chops")
    tour.inside(bar(33) + 26.0, bar(48) - 1.6, "source.sampler", 1.4, "Vocal Chops", "vocal-chops")
    tour.leave(bar(48) - 1.3)
    # The Fill: the score turns its page
    tour.hop(bar(48) - 0.7, score, 0.8, 1.9, "the three Arrangers turn the page", None)

    # Clearing: the drums drop out and the flute comes in
    tour.hop(bar(49) + 0.2, tour.centre_of("Flute"), face, 2.6, "the Flute", "flute")
    tour.enter(bar(49) + 3.2, "Flute")
    tour.inside(bar(49) + 4.0, bar(57) - 2.6, "source.noise", 1.3, "Flute: its breath", "flute")
    tour.leave(bar(57) - 2.3)

    # Rise: the drums swell back in under the loops; the riser, once it
    # roars; then out to the whole patch, landing on the Peak's downbeat
    tour.look(bar(57) - 1.7, (2270, 1450), 0.75, 3.0, "the loops and the band, the drums coming back", ["kalimba", "drums-kit"])
    tour.look(bar(57) + 6.0, tour.centre_of("Riser"), 1.7, 1.8, "the Riser", "riser")
    tour.enter(bar(57) + 7.9, "Riser")
    tour.inside(bar(57) + 8.7, bar(57) + 11.6, "filter.svf", 1.6, "Riser", "riser")
    tour.leave(bar(57) + 11.9)
    tour.look(bar(57) + 12.5, whole, 0.35, bar(65) - bar(57) - 12.5, "the build pulls back to the whole patch, landing on the Peak", None)

    # Peak: everything; the Dancer; the strings pumping; the desk
    tour.look(bar(65) + 3.0, tour.centre_of(150), 1.75, 2.6, "the Dancer", None)
    tour.hop(bar(65) + 9.4, tour.centre_of("Strings"), face, 2.8, "the Strings", "strings")
    tour.enter(bar(65) + 12.6, "Strings")
    tour.look(bar(65) + 13.4, tour.middle(), tour.fit_zoom(), 0.8, "Strings: the String Machine and two Pumps", "strings")
    tour.enter(bar(65) + 15.2, "String Machine")
    tour.inside(bar(65) + 16.0, bar(65) + 21.4, "fx.chorus", 1.3, "String Machine", "strings")
    tour.leave(bar(65) + 21.7)
    tour.leave(bar(65) + 22.3)
    tour.hop(bar(65) + 22.9, (2808, 480), 0.85, 2.6, "the desk: four mixers, delay, reverb, tape", None)
    tour.hop(bar(80) - 2.4, score, 0.8, 2.2, "the Arrangers turn the page", None)

    # Peak II: the flute an octave up, the break, the chops
    tour.hop(bar(81) - 0.2, tour.centre_of("Flute"), face, 2.4, "the Flute, an octave up", "flute")
    tour.enter(bar(81) + 2.6, "Flute")
    tour.inside(bar(81) + 3.4, bar(81) + 10.6, "osc.sine", 1.5, "Flute: the tone", "flute")
    tour.leave(bar(81) + 10.9)
    tour.hop(bar(81) + 11.5, tour.centre_of("Break"), face, 1.8, "the Break", "break")
    tour.enter(bar(81) + 13.6, "Break")
    tour.inside(bar(81) + 14.4, bar(81) + 19.6, "filter.ladder", 1.4, "Break", "break")
    tour.leave(bar(81) + 19.9)
    tour.look(bar(81) + 20.5, tour.centre_of("Vocal Chops"), face, 1.4, "the Vocal Chops", "vocal-chops")
    tour.enter(bar(81) + 22.2, "Vocal Chops")
    tour.inside(bar(81) + 23.0, bar(96) - 3.0, "seq.step", 1.3, "Vocal Chops: the hook", "vocal-chops")
    tour.leave(bar(96) - 2.7)
    tour.hop(bar(96) - 2.1, score, 0.8, 2.0, "the Arrangers turn to Dusk", None)

    # Dusk: slowly, the bass fading, then the harmony
    tour.hop(bar(97) + 0.4, tour.centre_of("Bass"), face, 2.8, "the Bass, fading", "bass")
    tour.enter(bar(97) + 3.6, "Bass")
    tour.inside(bar(97) + 4.4, bar(97) + 15.6, "seq.step", 1.4, "Bass", "bass")
    tour.leave(bar(97) + 15.9)
    tour.hop(bar(97) + 16.5, (850, 1500), 0.75, 3.6, "the harmony: one progression, voiced three ways", ["strings", "rhodes", "harp"])
    tour.look(bar(97) + 20.4, (1300, 1500), 0.95, bar(113) - bar(97) - 23.4, "along to the Rhodes, Strings and Harp", ["strings", "rhodes", "harp"])

    # Night: the band fades; the birds come back
    tour.hop(bar(113) - 2.4, tour.centre_of("Birds"), face, 3.0, "the Birds", ["birds", "kalimba"])
    tour.enter(bar(113) + 1.0, "Birds")
    tour.inside(bar(113) + 1.8, bar(113) + 10.4, "mod.slope", 1.5, "Birds", ["birds", "kalimba"])
    tour.leave(bar(113) + 10.7)
    tour.hop(bar(113) + 11.3, tour.centre_of("Kalimba"), face, 4.4, "the Kalimba, as at Dawn", "kalimba")

    # Ring: the loops stop and ring out; the camera pulls back to the whole
    # patch, the Dancer coming to rest as the Clock stops
    t = tour.look(bar(121) + 0.4, whole, 0.35, 11.0, "the whole patch rings out", None)
    if not s.dry:
        s.at(bar(125) - 0.02, "param util.clock Run 0")
    s.at(tour.end(t + 4.0), "end")


@shot("patches/canopy.json", ppp=1.5, fps=30, crf=12, preset="fast", size="1080x2124")
def canopy_vertical(s):
    """Canopy's minute from the Rise into Peak II (1:56-2:56), for a phone:
    a portrait window, cropped to 1080x1920 below the toolbar (y 165).
    A wide group dives in as a thin strip, so each first look inside starts
    as the dive ends: the dive and the push in on a module are one move."""
    tour = canopy_tour(s, "aims-vertical.json")
    s.at(0.0, "play")
    t0 = bar(57)
    # Until the minute starts: the loops and the band
    tour.look(-0.5, (2330, 1330), 0.8, 0.05, "the loops and the band", None)
    tour.look(t0, (2430, 1300), 1.0, 5.4, "the drums coming back", ["kalimba", "drums-kit"])
    tour.hop(t0 + 5.6, tour.centre_of("Riser"), 2.0, 2.2, "the Riser", "riser")
    t = tour.enter(t0 + 8.0, "Riser")
    tour.inside_tall(t, t0 + 11.5, "source.noise", "filter.svf", 1.6, "Riser", "riser")
    tour.leave(t0 + 11.8)
    tour.hop(t0 + 12.4, tour.centre_of("Drums"), 1.5, bar(65) - 0.3 - t0 - 12.4, "the Drums, as the Peak lands", "drums-kit")
    # Into the Drums on the downbeat
    t = tour.enter(bar(65) - 0.14, "Drums")
    tour.look(t, tour.centre_of(tour.find("seq.trigger")), 1.0, 1.0, "Drums: the pattern", "drums-kit")
    tour.look(bar(65) + 2.0, tour.centre_of(tour.find("source.drum", 3)), 1.0, 6.4, "Drums: along the kit", "drums-kit")
    tour.leave(bar(65) + 8.7)
    tour.hop(bar(65) + 9.3, tour.centre_of(150), 1.85, 3.0, "the Dancer", None)
    tour.hop(bar(65) + 17.2, tour.centre_of("Strings"), 2.0, 3.0, "the Strings", "strings")
    t = tour.enter(bar(65) + 20.6, "Strings")
    tour.look(t, tour.centre_of("String Machine", dx=130), 1.25, 0.8, "Strings: the String Machine and a Pump", "strings")
    t = tour.enter(bar(65) + 22.6, "String Machine")
    tour.inside_tall(t, bar(80) - 4.6, "osc.sine", "fx.chorus", 1.4, "String Machine", "strings")
    tour.leave(bar(80) - 4.3)
    tour.leave(bar(80) - 3.7)
    tour.hop(bar(80) - 3.1, (792, 401), 1.15, 2.9, "the Faders turn the page", None)
    tour.hop(bar(81) - 0.2, tour.centre_of("Flute"), 2.0, 2.6, "the Flute, an octave up", "flute")
    t = tour.enter(bar(81) + 2.8, "Flute")
    tour.inside_tall(t, bar(81) + 7.6, "seq.step", "osc.sine", 1.5, "Flute", "flute")
    tour.leave(bar(81) + 7.9)
    t = tour.look(bar(81) + 8.5, (2700, 900), 0.4, 4.0, "out to the band and the desk", None)
    s.at(tour.end(max(t, bar(81) + 11.0)), "end")


def framings(cues):
    """Every camera move of a shot, landed one a second with a still of
    each: a contact sheet to check the aim before a long take. The patch
    plays from the top, so the stills show the framing, not the moment."""
    cameras = [cue.split("#")[0].split()[:4] for t, cue in sorted(cues, key=lambda c: c[0]) if cue.startswith("camera")]
    out = [(0.0, "play")]
    for i, cam in enumerate(cameras):
        out.append((i + 0.0, " ".join(cam + ["0.05"])))
        out.append((i + 0.6, f"still shot{i:02}"))
    out.append((len(cameras) + 0.0, "end"))
    labels = [cue.split("#", 1)[1].strip() if "#" in cue else "" for t, cue in sorted(cues, key=lambda c: c[0])
              if cue.startswith("camera")]
    return out, labels


def contact_sheet(out, labels, columns=4, width=480, pattern="shot*.png"):
    """Tiles the framing stills into sheet.png, each captioned with its move."""
    from PIL import Image, ImageDraw
    stills = sorted(out.glob(pattern))
    if not stills:
        return
    first = Image.open(stills[0])
    height = round(first.height * width / first.width)
    rows = -(-len(stills) // columns)
    sheet = Image.new("RGB", (columns * width, rows * (height + 22)), (18, 18, 22))
    draw = ImageDraw.Draw(sheet)
    for i, path in enumerate(stills):
        x, y = (i % columns) * width, (i // columns) * (height + 22)
        sheet.paste(Image.open(path).convert("RGB").resize((width, height)), (x, y))
        draw.text((x + 6, y + height + 5), f"{i:02} {labels[i] if i < len(labels) else ''}"[:76], fill=(220, 220, 220))
    sheet.save(out / "sheet.png")
    print(out / "sheet.png")


def build(name, dry=False):
    """A shot's Script, as it would film (or, `dry`, as a dry run would)."""
    fn, patch, ppp, fps, crf, preset, size = SHOTS[name]
    s = Script()
    s.dry, s.size, s.ppp = dry, size, ppp
    fn(s)
    return s


def subjects(name):
    """Checks a touring shot's holds against what plays when (the film's
    activity.json, from activity.py): each subject should be sounding."""
    from activity import Activity
    s = build(name)
    path = s.tour.bases.parent / "activity.json"
    holds = [(t0, t1, subject) for t0, t1, subject, _ in s.tour.holds if t1 is not None]
    flagged = Activity.load(path).check(holds)
    for (t0, t1, subject), why in flagged:
        print(f"{t0:7.2f}-{t1:7.2f}  {subject}: {why}")
    print(f"{name}: {len(holds)} holds, {len(flagged)} flagged")


def converge(name, most=6):
    """Dry-runs a touring shot until it aims from the sizes it measures."""
    for k in range(1, most + 1):
        s = run(name, dry=True)
        if s.tour.settled:
            print(f"{name}: settled after {k} dry run{'s' if k > 1 else ''}")
            return
    sys.exit(f"{name}: still unsettled after {most} dry runs")


def run(name, script_only=False, preview=False, framing=False, dry=False):
    fn, patch, ppp, fps, crf, preset, size = SHOTS[name]
    s = build(name, dry)
    labels = []
    if preview:
        # Just the setup, and the first frame as a still to aim from
        s.cues = [c for c in s.cues if c[0] < 0] + [(0.0, "still preview"), (0.05, "end")]
        name += "-preview"
    elif framing:
        s.cues, labels = framings(s.cues)
        crf, preset = 30, "ultrafast"
        name += "-framings"
    elif dry:
        crf, preset = 30, "ultrafast"
        name += "-dry"
    SCRIPTS.mkdir(parents=True, exist_ok=True)
    OUT.mkdir(parents=True, exist_ok=True)
    script = (OUT if preview or framing or dry else SCRIPTS) / f"{name}.txt"
    script.write_text(s.text())
    if script_only:
        print(script)
        return
    out = OUT / name
    out.mkdir(parents=True, exist_ok=True)
    for old in [*out.glob("*.ppm"), *out.glob("lvl[0-9]*.json"), *(out.glob("m[0-9]*.png") if dry else [])]:
        old.unlink()
    print(f"filming {name} ...", flush=True)
    subprocess.run([str(EXE), str(ROOT / patch), "--capture", str(script), "--out", str(out), "--ppp", str(ppp),
                    "--fps", str(fps), "--crf", str(crf), "--preset", preset, "--size", size],
                   check=True, cwd=ROOT)
    from PIL import Image
    for ppm in out.glob("*.ppm"):
        Image.open(ppm).save(ppm.with_suffix(".png"))
        ppm.unlink()
    if framing:
        contact_sheet(out, labels)
    if s.tour and not (preview or framing):
        # Every take's previsit measures the modules, a dry run's or not
        s.tour.settled = s.tour.learn(out)
    if dry and s.tour:
        contact_sheet(out, s.tour.labels, pattern="m*.png")
    return s


if __name__ == "__main__":
    args = sys.argv[1:]
    if not args or "--list" in args:
        for name, (fn, patch, *_) in SHOTS.items():
            print(f"{name:20} {patch:36} {(fn.__doc__ or '').strip().splitlines()[0]}")
        sys.exit(0)
    only = "--script-only" in args
    for name in [a for a in args if not a.startswith("--")]:
        if "--converge" in args:
            converge(name)
        elif "--subjects" in args:
            subjects(name)
        else:
            run(name, only, preview="--preview" in args, framing="--framings" in args, dry="--dry" in args)
