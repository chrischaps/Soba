"""Builds The Rig (patches/the-rig.json): a song in which Soba conducts a
room of hardware, and plays it alone when the room is empty.

    python tools/examples/make_the_rig.py

A minor, Am9 - Fmaj9 - Cmaj7 - G6, a bar each, at 100 BPM. Four devices,
each on a port of its own:

- A  Digitakt: plays its own patterns. The Setlist moves it through A01 to
     A05, each sent 500 ms before the bar it starts on.
- B  MicroFreak: an Instrument, played by a step sequencer, its Cutoff
     ridden by a two-bar LFO.
- C  volca fm: an Instrument plays its notes, and an FM Voice holds the
     DX7 voice it plays, so opening the patch gives the volca its sound.
- D  RC-505: a MIDI Out presses Rec/Play (CC 14, as its profile assigns
     it) on the Take's downbeat and again on the Verse's, so it records
     four bars of the room and plays them back.

Every device has a stand-in, a Soba voice playing its part, on one channel
of the Stand-ins mixer each: Soba drums for the Digitakt, a saw for the
MicroFreak, the Electric Piano for the volca fm, and a Looper taking the
same four bars as the RC-505. With no hardware the stand-ins are the song;
with the room plugged in, a device's channel is muted and the real one
plays. Soba's own pad and bass sit beside them on the main mixer.

Intro 4, Take 4, Verse 8, Chorus 8, Break 4, Verse 8, then round from the
Chorus for ever. The Take happens once: after it the song cycles Chorus,
Break, Verse, and the loop plays on under it.
"""
import json
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
OUT = REPO / "patches" / "the-rig.json"
PROFILES = REPO / "src" / "hardware" / "profiles"

nodes, conns, frames, notes, groups = [], [], [], [], []


def value(name, v):
    if isinstance(v, bool):
        return {"name": name, "type": "Toggle", "value": v}
    if isinstance(v, tuple):  # ("sel", n)
        return {"name": name, "type": "Select", "value": v[1]}
    return {"name": name, "type": "Number", "value": float(v)}


def node(nid, module, **params):
    nodes.append({"id": nid, "module_id": module, "position": [0.0, 0.0],
                  "parameters": [value(k.replace("_", " "), v) for k, v in params.items()]})
    return nodes[-1]


def p(node_, name, v):
    """Adds a parameter whose name isn't a valid Python identifier."""
    node_["parameters"].append(value(name, v))


def wire(a, ap, b, bp):
    conns.append({"from_node": a, "from_port": ap, "to_node": b, "to_port": bp})


def library(file_name, gid):
    """A library group, its nodes renumbered from `gid` so they're unique."""
    lib = json.loads((REPO / "library" / file_name).read_text(encoding="utf-8"))["groups"][0]
    remap = {n["id"]: gid + n["id"] for n in lib["nodes"]}
    remap[lib["id"]] = gid
    group = dict(lib, id=gid)
    group["nodes"] = [dict(n, id=remap[n["id"]]) for n in lib["nodes"]]
    group["connections"] = [dict(c, from_node=remap[c["from_node"]], to_node=remap[c["to_node"]]) for c in lib["connections"]]
    groups.append(group)
    return group


def group(gid, name, inputs, outputs, build, at):
    """A group of the modules `build` adds, wired to its edge through `gid`;
    `at` places them inside it, {id: (x, y)}. Inputs and outputs are
    [(name, signal type)]."""
    global nodes, conns
    outer = nodes, conns
    nodes, conns = [], []
    build()
    for n in nodes:
        n["position"] = [float(v) for v in at[n["id"]]]
    xs = [n["position"][0] for n in nodes]
    g = {"id": gid, "name": name, "position": [0.0, 0.0],
         "inputs": [{"name": n, "type": t} for n, t in inputs],
         "outputs": [{"name": n, "type": t} for n, t in outputs],
         "inputs_position": [min(xs) - 320.0, 0.0], "outputs_position": [max(xs) + 320.0, 0.0],
         "nodes": nodes, "connections": conns}
    nodes, conns = outer
    groups.append(g)
    return g


def profile(name):
    for f in PROFILES.glob("*.json"):
        prof = json.loads(f.read_text(encoding="utf-8"))
        if prof["name"] == name:
            return prof
    raise KeyError(name)


def compact(obj):
    return json.dumps(obj, separators=(",", ":"), ensure_ascii=False)


def hold(inst, prof, slots):
    """Has an Instrument play `prof`, knob N holding the named parameter at
    `travel` (0 to 1): slots is [(parameter name, travel)], in knob order."""
    by_name = {q["name"]: q for q in prof["parameters"]}
    inst.setdefault("labels", {})["profile"] = prof["name"]
    for i, (name, travel) in enumerate(slots, start=1):
        inst["labels"][f"slot {i}"] = compact(by_name[name])
        p(inst, f"Slot {i}", travel)


S = lambda n: ("sel", n)
PORT_A, PORT_B, PORT_C, PORT_D = 0, 1, 2, 3

# ---------------------------------------------------------------- time
CLOCK, EIGHTHS, QUARTERS, BARS = 1, 2, 3, 4
# Auto: Soba leads until a device sends clock, follows it while it does,
# and carries on from there when it stops
node(CLOCK, "util.clock", Tempo=100, Gate_Length=50, Division=S(4), Run=True, Source=S(3), Swing=54)
for nid, divide in [(EIGHTHS, 2), (QUARTERS, 4), (BARS, 16)]:
    node(nid, "util.divider", Divide=divide, Offset=0, Length=1)
    wire(CLOCK, "Gate", nid, "Clock")
    wire(CLOCK, "Reset", nid, "Reset")

# ---------------------------------------------------------------- the song
ARR = 5
JUMP, RAMP, HIT = 1, 2, 3


def cue(kind, level, bars=0):
    return kind * 1_000_000 + bars * 10_000 + int(round(level * 1000))


PAT = {"A": 0.1, "B": 0.35, "C": 0.6, "D": 0.9}
DRUMS, FREAK, KEYS, BASS, LOOP, PAD, LOOPED, ROOM = range(1, 9)
LANES = {DRUMS: "Drums", FREAK: "MicroFreak", KEYS: "volca fm", BASS: "Bass", LOOP: "RC-505",
         PAD: "Pad", LOOPED: "Loop level", ROOM: "Room"}
# (name, bars, {lane: cue}, {lane: ramp's start level})
SECTIONS = [
    ("Intro", 4, {DRUMS: cue(JUMP, PAT["A"]), FREAK: cue(JUMP, PAT["D"]), KEYS: cue(JUMP, PAT["B"]),
                  BASS: cue(JUMP, 0.0), PAD: cue(RAMP, 0.5), LOOPED: cue(JUMP, 0.0), ROOM: cue(JUMP, 0.9)},
     {PAD: 0.0}),
    ("Take", 4, {FREAK: cue(JUMP, PAT["A"]), LOOP: cue(HIT, 1.0)}, {}),
    ("Verse", 8, {DRUMS: cue(JUMP, PAT["B"]), FREAK: cue(JUMP, PAT["B"]), LOOP: cue(HIT, 1.0),
                  BASS: cue(RAMP, 0.95, 2), PAD: cue(JUMP, 0.38), LOOPED: cue(JUMP, 0.6), ROOM: cue(RAMP, 0.7, 2)},
     {BASS: 0.0}),
    ("Chorus", 8, {DRUMS: cue(JUMP, PAT["C"]), FREAK: cue(JUMP, PAT["C"]), KEYS: cue(JUMP, PAT["A"]),
                   BASS: cue(JUMP, 1.0), PAD: cue(JUMP, 0.42), LOOPED: cue(JUMP, 0.45), ROOM: cue(JUMP, 0.75)}, {}),
    ("Break", 4, {DRUMS: cue(JUMP, PAT["D"]), FREAK: cue(JUMP, PAT["D"]), KEYS: cue(JUMP, PAT["B"]),
                  BASS: cue(JUMP, 0.0), PAD: cue(JUMP, 0.55), LOOPED: cue(JUMP, 0.9), ROOM: cue(JUMP, 1.0)}, {}),
    ("Verse", 8, {DRUMS: cue(JUMP, PAT["B"]), FREAK: cue(JUMP, PAT["B"]), BASS: cue(RAMP, 0.95, 1),
                  PAD: cue(JUMP, 0.38), LOOPED: cue(JUMP, 0.6), ROOM: cue(RAMP, 0.7, 2)},
     {BASS: 0.0}),
]
LOOP_TO = 4
arr = node(ARR, "seq.arranger", Steps=16, Sections=len(SECTIONS), Loop=S(LOOP_TO))
arr["labels"] = {"Name": "The Rig"}
for lane, name in LANES.items():
    arr["labels"][f"Lane {lane}"] = name
for lane, glide in {DRUMS: 0, FREAK: 0, KEYS: 0, BASS: 0.05, LOOP: 0, PAD: 0.6, LOOPED: 0.3, ROOM: 1.0}.items():
    p(arr, f"Glide {lane}", glide)
for i, (name, bars, cues, starts) in enumerate(SECTIONS, start=1):
    arr["labels"][f"Section {i}"] = name
    p(arr, f"Length {i:02d}", bars)
    for lane, c in cues.items():
        p(arr, f"Section {i:02d} Lane {lane}", c)
    for lane, start in starts.items():
        p(arr, f"Section {i:02d} Lane {lane} From", round(start * 1000))
wire(CLOCK, "Gate", ARR, "Clock")
wire(CLOCK, "Reset", ARR, "Reset")

# The Setlist: the Digitakt's pattern for each section, its lead ahead of
# the bar (it queues a pattern and starts it when the one playing ends),
# and the RC-505's phrase memory at the top
SETLIST = 6
setlist = node(SETLIST, "output.setlist")
DIGITAKT_PATTERNS = [0, 1, 2, 3, 4, 2]  # A01 A02 A03 A04 A05 A03
setlist["labels"] = {}
for section, pattern in enumerate(DIGITAKT_PATTERNS, start=1):
    setlist["labels"][f"row {section}.1"] = compact({"port": PORT_A, "device": "Digitakt", "program": pattern, "channel": 1, "lead": 500})
setlist["labels"]["row 1.2"] = compact({"port": PORT_D, "device": "RC-505", "program": 0, "channel": 1})
wire(ARR, "Section", SETLIST, "Section")
wire(ARR, "Next", SETLIST, "Next")
wire(ARR, "Bar", SETLIST, "Bar")

# ---------------------------------------------------------------- Digitakt's stand-in
# The Digitakt plays its own patterns; Soba's drums stand in for them, a
# pattern a section as the Setlist moves the real one
TRIG, KIT = 10, 1100
trig = node(TRIG, "seq.trigger", Steps=16, Gate_Length=40, Accent_Amount=40)


def step(vel, ratchet=1, prob=100):
    return float(ratchet * 1_000_000 + prob * 1000 + vel)


KICK, SNARE, HAT, OPEN = 1, 2, 3, 4
DRUM_PATTERNS = {
    "A": {KICK: {1: 66}, HAT: {1: 62, 3: 44, 5: 70, 7: 44, 9: 62, 11: 44, 13: 70, 15: 48}},
    "B": {KICK: {1: 94, 9: 86, 11: 56}, SNARE: {5: 82, 13: 88},
          HAT: {s: (76 if s % 4 == 1 else 58) for s in range(1, 16, 2)}},
    "C": {KICK: {1: 98, 4: 52, 7: 70, 9: 94, 11: 60}, SNARE: {5: 90, 13: 94, 16: 34},
          HAT: {s: (78 if s % 2 == 1 else 44) for s in range(1, 15)}, OPEN: {15: 70}},
    "D": {KICK: {1: 80}, HAT: {9: 52, 13: 46}},
}
for letter, lanes in DRUM_PATTERNS.items():
    for lane in range(1, 9):
        hits = lanes.get(lane, {})
        for s in range(1, 17):
            v = hits.get(s)
            p(trig, f"Step {letter}{lane} {s:02d}", step(v) if v else -step(80))
wire(CLOCK, "Gate", TRIG, "Clock")
wire(CLOCK, "Reset", TRIG, "Reset")
wire(ARR, f"Lane {DRUMS}", TRIG, "Pattern")
DRUM_VOICES = [  # (lane, name, type, tune, decay, tone, snap, level)
    (KICK, "Kick", 0, -2.0, 0.55, 0.42, 0.35, 0.62),
    (SNARE, "Snare", 1, 0.0, 0.42, 0.5, 0.5, 0.75),
    (HAT, "Hat", 4, 0.0, 0.26, 0.55, 0.45, 0.8),
    (OPEN, "Open Hat", 5, 0.0, 0.45, 0.5, 0.45, 0.6),
]
KMIX = KIT + 5


def drums():
    for lane, name, typ, tune, decay, tone, snap, level in DRUM_VOICES:
        nid = KIT + lane
        node(nid, "source.drum", Type=S(typ), Tune=tune, Decay=decay, Tone=tone, Snap=snap, Level=level)
        wire(KIT, name, nid, "Trig")
        wire(KIT, f"{name} Vel", nid, "Accent")
        wire(nid, "Out", KMIX, f"In {lane}")
    wire(KIT, "Hat", KIT + OPEN, "Choke")
    node(KMIX, "util.mix", Level_1=1.0, Level_2=1.0, Level_3=1.0, Level_4=0.8, Sum_Voices=False)
    wire(KMIX, "Out", KIT, "Out")


group(KIT, "Stand-in drums", [(n, t) for _, name, *_ in DRUM_VOICES for n, t in ((name, "Gate"), (f"{name} Vel", "Control"))],
      [("Out", "Audio")], drums,
      {KIT + 1: (0, 0), KIT + 2: (300, 0), KIT + 3: (0, 400), KIT + 4: (300, 400), KMIX: (620, 200)})
for lane, name, *_ in DRUM_VOICES:
    wire(TRIG, f"Gate {lane}", KIT, name)
    wire(TRIG, f"Vel {lane}", KIT, f"{name} Vel")

# ---------------------------------------------------------------- MicroFreak
FSEQ, FLFO, FATT, FREAK_I = 20, 21, 22, 23
fseq = node(FSEQ, "seq.step", Steps=16, Direction=S(0), Gate_Length=55, Glide=0.04, Scale=S(2), Root=S(9))
E4, G4, A4, B4, C5, D5, E5, G5 = 64, 67, 69, 71, 72, 74, 76, 79
# step: (note, velocity, tie)
FREAK_PATTERNS = {
    # The Take: a figure for the loop to keep
    "A": {1: (E4, 92), 3: (B4, 70), 4: (A4, 58), 7: (G4, 80), 9: (E5, 96), 11: (D5, 68), 12: (B4, 62), 15: (A4, 72)},
    # The Verse: an answer over the loop, longer and higher
    "B": {3: (E5, 86, True), 4: (E5, 86), 6: (D5, 66), 7: (B4, 74, True), 8: (B4, 74), 11: (A4, 66),
          13: (G4, 78, True), 14: (G4, 78), 16: (A4, 58)},
    # The Chorus: climbing, an E or a G ringing above each note
    "C": {1: (A4, 94), 2: (E5, 52), 3: (A4, 68), 5: (B4, 86), 6: (E5, 52), 7: (B4, 68), 9: (C5, 90),
          10: (E5, 54), 11: (C5, 68), 13: (D5, 86), 14: (G5, 58), 15: (D5, 68)},
    "D": {},
}
for letter, steps in FREAK_PATTERNS.items():
    prefix = "" if letter == "A" else letter
    for s in range(1, 17):
        n = steps.get(s)
        name = f"Step {prefix}{s}"
        p(fseq, f"{name} Pitch", n[0] if n else E4)
        p(fseq, f"{name} Gate", bool(n))
        if n:
            p(fseq, f"{name} Velocity", n[1])
            p(fseq, f"{name} Tie", len(n) > 2 and n[2])
wire(CLOCK, "Gate", FSEQ, "Clock")
wire(CLOCK, "Reset", FSEQ, "Reset")
wire(ARR, f"Lane {FREAK}", FSEQ, "Pattern")

# A two-bar LFO rides the filter, the real one and the stand-in's, between
# a third and three quarters of the way open
node(FLFO, "mod.lfo", Rate=0.25, Waveform=S(0), Bipolar=False, Tempo_Sync=S(2))
node(FATT, "util.attenuverter", Amount=0.4, Offset=0.35, Range=S(0))
wire(FLFO, "Out", FATT, "In")

freak = node(FREAK_I, "source.instrument", Port=S(PORT_B), Channel=S(0))
# Its Osc Type is left alone: whichever engine the MicroFreak is on, the
# patch shapes it rather than choosing it
hold(freak, profile("MicroFreak"), [
    ("Cutoff", 0.5), ("Resonance", 0.32), ("Filter Amount", 0.62), ("Osc Wave", 0.4),
    ("Osc Timbre", 0.45), ("Osc Shape", 0.5), ("Attack", 0.02), ("Decay", 0.42),
    ("Sustain", 0.3), ("Glide", 0.06), ("Spice", 0.2),
])
wire(FSEQ, "Pitch", FREAK_I, "Pitch")
wire(FSEQ, "Gate", FREAK_I, "Gate")
wire(FSEQ, "Velocity", FREAK_I, "Velocity")
wire(FATT, "Out", FREAK_I, "Slot 1")

# Its stand-in: a saw through a ladder the same LFO opens
FSTAND = 1200
FOSC, FLAD, FVCA, FENV = FSTAND + 1, FSTAND + 2, FSTAND + 3, FSTAND + 4


def synth():
    node(FOSC, "osc.sine", Octave=0, Waveform=S(1), Voices=2, Detune=0.1, Spread=0.4)
    node(FLAD, "filter.ladder", Cutoff=440, Resonance=0.28, Drive=2.0, Cutoff_CV=3.0)
    node(FVCA, "util.vca", Level=1.0, CV_Amount=1.0)
    node(FENV, "mod.adsr", Attack=0.008, Decay=0.32, Sustain=0.4, Release=0.28,
         Attack_Curve=0.3, Decay_Curve=0.5, Release_Curve=0.5, Velocity_Amount=0.6)
    wire(FSTAND, "Pitch", FOSC, "V/Oct")
    wire(FSTAND, "Gate", FENV, "Gate")
    wire(FSTAND, "Velocity", FENV, "Velocity")
    wire(FSTAND, "Cutoff", FLAD, "Cutoff")
    wire(FOSC, "Out", FLAD, "In")
    wire(FLAD, "LP24", FVCA, "In")
    wire(FENV, "Out", FVCA, "CV")
    wire(FVCA, "Out", FSTAND, "Out")


group(FSTAND, "Stand-in synth", [("Pitch", "Control"), ("Gate", "Gate"), ("Velocity", "Control"), ("Cutoff", "Control")],
      [("Out", "Audio")], synth, {FOSC: (0, 0), FLAD: (320, 0), FVCA: (640, 0), FENV: (640, 380)})
wire(FSEQ, "Pitch", FSTAND, "Pitch")
wire(FSEQ, "Gate", FSTAND, "Gate")
wire(FSEQ, "Velocity", FSTAND, "Velocity")
wire(FATT, "Out", FSTAND, "Cutoff")

# ---------------------------------------------------------------- volca fm
KSEQ, VOLCA_I, FMV, EP = 30, 31, 32, 1400
Am9, Fmaj9, Cmaj7, G6 = (57, 14), (53, 13), (48, 3), (55, 10)  # (root, type)
PROGRESSION = [Am9, Fmaj9, Cmaj7, G6]
kseq = node(KSEQ, "seq.chord", Steps=16, Direction=S(0), Gate_Length=88, Voicing=S(0), Voice_Leading=True,
            Voices=3, Range=2, Bass_Voice=False, Rootless=False, Strum=8, Strum_Dir=S(0), Strum_Taper=15)
# Quarter notes, a chord a bar: struck on one and held, touched again on three.
# Pattern A plays in the Chorus; B is silence
for letter in "AB":
    prefix = "" if letter == "A" else letter
    for s in range(1, 17):
        chord = PROGRESSION[(s - 1) // 4]
        beat = (s - 1) % 4
        on = letter == "A" and beat in (0, 2)
        p(kseq, f"Step {prefix}{s} Root", chord[0])
        p(kseq, f"Step {prefix}{s} Type", S(chord[1]))
        p(kseq, f"Step {prefix}{s} Gate", on)
        p(kseq, f"Step {prefix}{s} Velocity", 100 if beat == 0 else 72)
        p(kseq, f"Step {prefix}{s} Tie", on and beat == 0)
wire(QUARTERS, "Gate", KSEQ, "Clock")
wire(CLOCK, "Reset", KSEQ, "Reset")
wire(ARR, f"Lane {KEYS}", KSEQ, "Pattern")

volca = node(VOLCA_I, "source.instrument", Port=S(PORT_C), Channel=S(0))
# The FM Voice holds the voice, so the knobs that would set it absolutely
# (Algorithm, the LFO) are left off; the envelope knobs nudge it from the
# middle, and the arpeggiator stays off
hold(volca, profile("volca fm"), [
    ("Mod Attack", 0.5), ("Mod Decay", 0.5), ("Carrier Attack", 0.5), ("Carrier Decay", 0.5),
    ("Arp Type", 0.0), ("Transpose", 0.5),
])
wire(KSEQ, "Pitch", VOLCA_I, "Pitch")
wire(KSEQ, "Gate", VOLCA_I, "Gate")
wire(KSEQ, "Velocity", VOLCA_I, "Velocity")
# Its garden starts from the four seeds, SOFT KEYS playing
node(FMV, "output.fm_voice", Port=S(PORT_C), Channel=S(0), Format=S(0), Amount=0.35)

library("electric-piano.json", EP)["name"] = "Stand-in keys"
wire(KSEQ, "Pitch", EP, "Pitch")
wire(KSEQ, "Gate", EP, "Gate")
wire(KSEQ, "Velocity", EP, "Velocity")

# ---------------------------------------------------------------- RC-505
RC, LOOPER = 40, 41
node(RC, "output.midi", Port=S(PORT_D), Channel=S(0), **{"CC 1 #": 14})
wire(ARR, f"Gate {LOOP}", RC, "CC 1")
# Its stand-in takes the same four bars: the first press records, the
# second closes the loop on the bar and plays it
node(LOOPER, "util.looper", Speed=S(1), Reverse=False, Bars=S(0), Latency=False, Feedback=1.0,
     Loop_Level=1.0, Dry_Level=0.0)
wire(FSTAND, "Out", LOOPER, "In L")
wire(FSTAND, "Out", LOOPER, "In R")
wire(ARR, f"Gate {LOOP}", LOOPER, "Rec")
wire(CLOCK, "Gate", LOOPER, "Clock")

# ---------------------------------------------------------------- Soba's own
PSEQ, PAD_V, BSEQ, BASS_V = 50, 1500, 51, 300
pseq = node(PSEQ, "seq.chord", Steps=4, Direction=S(0), Gate_Length=94, Voicing=S(1), Voice_Leading=True,
            Voices=4, Range=3, Bass_Voice=False, Rootless=False, Strum=0)
for s, chord in enumerate(PROGRESSION, start=1):
    p(pseq, f"Step {s} Root", chord[0])
    p(pseq, f"Step {s} Type", S(chord[1]))
    p(pseq, f"Step {s} Gate", True)
    p(pseq, f"Step {s} Velocity", 90)
wire(BARS, "Gate", PSEQ, "Clock")
wire(CLOCK, "Reset", PSEQ, "Reset")
library("string-machine.json", PAD_V)
wire(PSEQ, "Pitch", PAD_V, "Pitch")
wire(PSEQ, "Gate", PAD_V, "Gate")
wire(PSEQ, "Velocity", PAD_V, "Velocity")

bseq = node(BSEQ, "seq.step", Steps=16, Direction=S(0), Gate_Length=72, Glide=0.03, Scale=S(2), Root=S(9))
p(bseq, "Chain 1", S(1))
p(bseq, "Chain 2", S(2))
A1, E2, G2, F1, C2, G1, D2, B1 = 33, 40, 43, 29, 36, 31, 38, 35
UP = 24  # the bass's oscillator sits two octaves down
# Eighths, two bars a pattern: A over Am9 and Fmaj9, B over Cmaj7 and G6
BASS_PATTERNS = {
    "A": {1: (A1, 100, True), 2: (A1, 100), 4: (A1, 66), 5: (E2, 80), 7: (G2, 72),
          9: (F1, 100, True), 10: (F1, 100), 12: (F1, 66), 13: (C2, 80), 15: (E2, 70)},
    "B": {1: (C2, 100, True), 2: (C2, 100), 4: (C2, 66), 5: (G2, 80), 7: (E2, 72),
          9: (G1, 100, True), 10: (G1, 100), 12: (G1, 66), 13: (D2, 80), 15: (B1, 74)},
}
for letter, steps in BASS_PATTERNS.items():
    prefix = "" if letter == "A" else letter
    for s in range(1, 17):
        n = steps.get(s)
        name = f"Step {prefix}{s}"
        p(bseq, f"{name} Pitch", (n[0] if n else A1) + UP)
        p(bseq, f"{name} Gate", bool(n))
        if n:
            p(bseq, f"{name} Velocity", n[1])
            p(bseq, f"{name} Tie", len(n) > 2 and n[2])
wire(EIGHTHS, "Gate", BSEQ, "Clock")
wire(CLOCK, "Reset", BSEQ, "Reset")
library("reese-bass.json", BASS_V)
wire(BSEQ, "Pitch", BASS_V, "Pitch")
wire(BSEQ, "Gate", BASS_V, "Gate")
wire(BSEQ, "Velocity", BASS_V, "Velocity")

# ---------------------------------------------------------------- the mix
STAND, MAIN, DELAY, REVERB, OUTPUT = 60, 61, 62, 63, 64
# One channel a device: mute a stand-in when its device is in the room
node(STAND, "util.mixer", Level_1=1.0, Level_2=0.75, Level_3=0.65, Level_4=0.0,
     Pan_1=0.0, Pan_2=-0.3, Pan_3=0.3, Pan_4=0.15, Send_1=0.06, Send_2=0.35, Send_3=0.3, Send_4=0.2,
     Width_1=0.3, Width_3=0.5, Master=0)
wire(KIT, "Out", STAND, "Ch 1")
wire(FSTAND, "Out", STAND, "Ch 2")
wire(EP, "Out", STAND, "Ch 3")
wire(LOOPER, "Loop L", STAND, "Ch 4")
wire(ARR, f"Lane {LOOPED}", STAND, "Level 4")

node(MAIN, "util.mixer", Level_1=0.0, Level_2=0.0, Pan_1=0.0, Pan_2=0.0, Send_1=0.4, Send_2=0.04,
     Width_1=0.8, Return=0.8, Master=1.5)
wire(PAD_V, "Out", MAIN, "Ch 1")
wire(BASS_V, "Out", MAIN, "Ch 2")
wire(ARR, f"Lane {PAD}", MAIN, "Level 1")
wire(ARR, f"Lane {BASS}", MAIN, "Level 2")
wire(ARR, f"Lane {ROOM}", MAIN, "Return")
wire(STAND, "Chain Out", MAIN, "Chain In")

node(DELAY, "fx.delay", Time=450, Feedback=0.34, Mix=0.35, High_Cut=4200, Low_Cut=260,
     **{"Ping-Pong": True}, Sync=S(8), Tape=True)
node(REVERB, "fx.reverb", Size=0.72, Decay=3.4, Damping=0.5, **{"Pre-Delay": 22}, Mix=1.0, Width=1.0, Mod=0.3)
wire(MAIN, "Send L", DELAY, "In L")
wire(MAIN, "Send R", DELAY, "In R")
wire(DELAY, "Out L", REVERB, "In L")
wire(DELAY, "Out R", REVERB, "In R")
wire(REVERB, "Out L", MAIN, "Return L")
wire(REVERB, "Out R", MAIN, "Return R")

# A clean tape, saturation only: it rounds the first moment of each kick,
# so the song sits at the house level without its peaks reaching the limiter
TAPE = 65
node(TAPE, "fx.tape", Wow=0.0, Flutter=0.0, Width=0.0, Saturation=0.16, Age=0.0, Hiss=0.0, Mix=1.0, Speed=S(1), Stop_Time=1.0)
wire(MAIN, "Out L", TAPE, "In L")
wire(MAIN, "Out R", TAPE, "In R")
node(OUTPUT, "output.audio", Volume=0.8, Limiter=True, Character=False)
wire(TAPE, "Out L", OUTPUT, "Left")
wire(TAPE, "Out R", OUTPUT, "Right")

# ---------------------------------------------------------------- layout
# Sizes measured from a capture's `layout` cue, in patch units
SIZE = {
    CLOCK: (178, 380), EIGHTHS: (178, 323), QUARTERS: (178, 323), BARS: (178, 323), ARR: (566, 745), SETLIST: (282, 345),
    TRIG: (336, 596), KIT: (206, 345),
    FSEQ: (310, 651), FLFO: (170, 372), FATT: (158, 203), FREAK_I: (302, 744), FSTAND: (206, 261),
    KSEQ: (339, 687), VOLCA_I: (302, 744), FMV: (282, 391), EP: (230, 394),
    RC: (230, 568), LOOPER: (230, 668),
    PSEQ: (339, 648), PAD_V: (230, 394), BSEQ: (310, 651), BASS_V: (230, 394),
    STAND: (394, 454), MAIN: (394, 454), DELAY: (282, 405), REVERB: (230, 356), TAPE: (230, 447), OUTPUT: (207, 275),
}
# Each note's height, measured likewise (a note is at most 640 wide)
NOTE_H = {"song": 85, "mix": 84, "room": 84}
NOTE_W = 640
GAP, PAD, TOP, NOTE_GAP = 40, 24, 40, 24
POS = {}


def place(x, y, *rows):
    """Lays out columns from (x, y): each column a list of ids stacked
    downwards, each column `GAP` right of the widest in the one before.
    Returns the right edge."""
    for col in rows:
        cy = y
        for nid in col:
            POS[nid] = (x, cy)
            cy += SIZE[nid][1] + GAP
        x += max(SIZE[nid][0] for nid in col) + GAP
    return x - GAP


def frame(title, color, ids):
    """A frame round `ids`, as [x0, y0, x1, y1]."""
    x0 = min(POS[i][0] for i in ids) - PAD
    y0 = min(POS[i][1] for i in ids) - TOP
    x1 = max(POS[i][0] + SIZE[i][0] for i in ids) + PAD
    y1 = max(POS[i][1] + SIZE[i][1] for i in ids) + PAD
    frames.append({"title": title, "position": [x0, y0], "size": [x1 - x0, y1 - y0], "color": color})
    return [x0, y0, x1, y1]


def note(key, text, under, nth=0):
    """A note beneath a frame, as wide as it can be, the `nth` along.
    Returns its bottom."""
    x0, _, x1, y1 = under
    width = min(x1 - x0, NOTE_W)
    notes.append({"text": text, "position": [x0 + nth * (NOTE_W + GAP), y1 + NOTE_GAP], "width": width})
    return y1 + NOTE_GAP + NOTE_H.get(key, 160)


Y1 = 80
# The song: the Clock and its dividers, the Arranger, the Setlist
place(40, Y1, [CLOCK])
POS[ARR] = (258, Y1)
POS[SETLIST] = (864, Y1)
place(864, Y1 + SIZE[SETLIST][1] + GAP, [EIGHTHS], [QUARTERS], [BARS])
song = frame("The song", "slate", [CLOCK, ARR, SETLIST, EIGHTHS, QUARTERS, BARS])
x = song[2] + 48
# Soba's own: the pad and the bass
own_right = place(x + PAD, Y1, [PSEQ], [PAD_V], [BSEQ], [BASS_V])
own = frame("Soba's own", "blue", [PSEQ, PAD_V, BSEQ, BASS_V])
x = own[2] + 48
# The mix: the stand-ins, Soba's own, one room, a tape and the output
place(x + PAD, Y1, [STAND, MAIN], [TAPE, DELAY], [OUTPUT, REVERB])
POS[DELAY] = (POS[DELAY][0], POS[MAIN][1])
POS[REVERB] = (POS[REVERB][0], POS[MAIN][1])
mix = frame("Stand-ins and the mix", "teal", [STAND, MAIN, TAPE, DELAY, OUTPUT, REVERB])

below = max(
    note("song", "**The Rig** is a song in which Soba conducts a room of hardware: a **Digitakt** on port A, a "
         "**MicroFreak** on B, a **volca fm** on C and an **RC-505** on D, chosen in the Rig (`Ctrl+Shift+H`). Press "
         "**▶ Play** with nothing plugged in and Soba plays every part. The Arranger runs the song, and the "
         "**Setlist** moves the Digitakt through patterns A01 to A05, each half a second ahead of its bar.", song),
    own[3],
    note("mix", "**Stand-ins**, one channel a device: **1** the Digitakt, **2** the MicroFreak, **3** the volca fm, "
         "**4** the RC-505. Each plays what its device would, so the song is whole with an empty room. Plug a device "
         "in and press its channel's **M**: the real one takes over. The Clock's **Source** is **Auto**: a device that "
         "starts playing leads, and Soba follows it.", mix),
)

Y2 = below + 48 + TOP
# A: the Digitakt plays its own patterns; Soba's drums stand in
place(40, Y2, [TRIG], [KIT])
digitakt = frame("Digitakt · port A", "amber", [TRIG, KIT])
# B: the MicroFreak, played and swept; its stand-in sits low, clear of the cables
x = place(digitakt[2] + 48 + PAD, Y2, [FSEQ], [FLFO, FATT])
POS[FSTAND] = (x + GAP, Y2 + 440)
POS[FREAK_I] = (x + GAP + SIZE[FSTAND][0] + GAP, Y2)
freak_f = frame("MicroFreak · port B", "violet", [FSEQ, FLFO, FATT, FSTAND, FREAK_I])
# C: the volca fm, its notes and its voice
x = place(freak_f[2] + 48 + PAD, Y2, [KSEQ])
POS[EP] = (x + GAP, Y2 + 350)
x = place(x + GAP + SIZE[EP][0] + GAP, Y2, [VOLCA_I], [FMV])
volca_f = frame("volca fm · port C", "green", [KSEQ, EP, VOLCA_I, FMV])
# D: the RC-505's button, and the Looper taking the same four bars
place(volca_f[2] + 48 + PAD, Y2, [RC], [LOOPER])
rc_f = frame("RC-505 · port D", "rose", [RC, LOOPER])

note("room", "**Before you play the room:** in the Rig, set ports A and D to **Clock + Transport**. On the RC-505, "
     "assign **CC 14** to Target Track Rec/Play: the Arranger presses it on the Take's downbeat and the Verse's, "
     "four bars recorded, then played. Opening the patch sends the MicroFreak its knobs and the volca fm its voice.",
     digitakt)

for n in nodes + groups:
    n["position"] = [float(v) for v in POS[n["id"]]]

patch = {"name": "The Rig", "version": 6, "nodes": nodes, "connections": conns, "midi_mappings": [],
         "frames": frames, "notes": notes, "groups": groups}

if __name__ == "__main__":
    OUT.write_text(json.dumps(patch, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"{len(nodes)} modules, {len(groups)} groups, {len(conns)} cables -> {OUT}")
