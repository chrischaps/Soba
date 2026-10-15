"""Builds the Dance Floor, the patch the `dancers` shot films
(tools/showcase/shots.py): a funk vamp and three Dancers who each hear a
different part of the band.

    python tools/showcase/patches/make_dance_floor.py

E Dorian, Em9 -> A13, 108 BPM with the sixteenths swung to 57%. Four bars
that loop: three of the groove and a fill, the lead answering in bars 3
and 4, a crash on every phrase change that cues the big Dancer's big move.
Four bars are a Dancer's phrase (16 beats), so the band and the dance turn
over together.
"""
import json
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
OUT = Path(__file__).resolve().with_name("dance-floor.json")

nodes, conns, frames, notes = [], [], [], []


def node(nid, module, x, y, **params):
    ps = []
    for name, value in params.items():
        name = name.replace("_", " ")
        if isinstance(value, bool):
            ps.append({"name": name, "type": "Toggle", "value": value})
        elif isinstance(value, tuple):  # ("sel", n)
            ps.append({"name": name, "type": "Select", "value": value[1]})
        else:
            ps.append({"name": name, "type": "Number", "value": float(value)})
    nodes.append({"id": nid, "module_id": module, "position": [float(x), float(y)], "parameters": ps})
    return nodes[-1]


def p(node_, name, value):
    """Adds a parameter whose name isn't a valid Python identifier."""
    if isinstance(value, bool):
        node_["parameters"].append({"name": name, "type": "Toggle", "value": value})
    elif isinstance(value, tuple):
        node_["parameters"].append({"name": name, "type": "Select", "value": value[1]})
    else:
        node_["parameters"].append({"name": name, "type": "Number", "value": float(value)})


def wire(a, ap, b, bp):
    conns.append({"from_node": a, "from_port": ap, "to_node": b, "to_port": bp})


S = lambda n: ("sel", n)

# ---------------------------------------------------------------- the clock
CLOCK = 1
node(CLOCK, "util.clock", 40, 80, Tempo=108, Gate_Length=50, Division=S(4), Run=True, Source=S(0), Swing=57)
KEYCLK = 2  # eighths for the keys: every other (on-grid) sixteenth
node(KEYCLK, "util.divider", 40, 520, Divide=2, Offset=0, Length=1)
wire(CLOCK, "Gate", KEYCLK, "Clock")
wire(CLOCK, "Reset", KEYCLK, "Reset")

# ---------------------------------------------------------------- the arranger
ARR = 3
# Sections: (name, bars, {lane: cue})
JUMP, RAMP, HIT, LOG = 1, 2, 3, 6


def cue(kind, level, bars=0):
    return kind * 1_000_000 + bars * 10_000 + int(round(level * 1000))


PAT = {"A": 0.1, "B": 0.35, "C": 0.6, "D": 0.9}
# lanes: 1 Drums, 2 Pattern, 3 Bass, 4 Keys, 5 Lead, 6 Crash, 7 Room
sections = [
    ("Groove", 1, {1: cue(JUMP, 1.0), 2: cue(JUMP, PAT["A"]), 3: cue(JUMP, 0.80), 4: cue(JUMP, 0.55),
                   5: cue(LOG, 0.0, 1), 6: cue(HIT, 1.0), 7: cue(JUMP, 0.22)}),
    ("Groove", 1, {}),
    ("Answer", 1, {5: cue(JUMP, 0.42)}),
    ("Fill", 1, {2: cue(JUMP, PAT["B"])}),
]
# Loop: back to the first section, for ever
arr = node(ARR, "seq.arranger", 300, 80, Steps=16, Sections=len(sections), Loop=S(1))
arr["labels"] = {"Name": "Dance Floor", "Lane 1": "Drums", "Lane 2": "Pattern", "Lane 3": "Bass", "Lane 4": "Keys",
                 "Lane 5": "Lead", "Lane 6": "Crash", "Lane 7": "Room"}
glides = {1: 0.04, 2: 0.0, 3: 0.04, 4: 0.04, 5: 0.04, 6: 0.0, 7: 1.0, 8: 0.0}
for lane, g in glides.items():
    p(arr, f"Glide {lane}", g)
for i, (name, bars, cues) in enumerate(sections, start=1):
    arr["labels"][f"Section {i}"] = name
    p(arr, f"Length {i:02d}", bars)
    for lane, value in cues.items():
        p(arr, f"Section {i:02d} Lane {lane}", value)
wire(CLOCK, "Gate", ARR, "Clock")
wire(CLOCK, "Reset", ARR, "Reset")

# ---------------------------------------------------------------- drums
TRIG = 10
DRUM_LANES = ["Kick", "Snare", "Hat", "Open Hat", "Rim", "Tom Hi", "Tom Lo", "Clap"]
trig = node(TRIG, "seq.trigger", 40, 1100, Steps=16, Gate_Length=40, Accent_Amount=40)


def step(vel, ratchet=1, prob=100):
    return float(ratchet * 1_000_000 + prob * 1000 + vel)


def hats(skip=(), soft=38, loud=72, upto=16):
    return {s: (loud if s % 2 == 1 else soft) for s in range(1, upto + 1) if s not in skip}


patterns = {
    "A": {  # the groove
        1: {1: 100, 3: 62, 8: 78, 11: 88, 14: 70},
        2: {5: 100, 13: 100, 7: 20, 10: 24, 12: 18, 16: 26},
        3: hats(skip=(15,)),
        4: {15: 62},
    },
    "B": {  # a fill into the next bar
        1: {1: 100, 3: 62, 8: 78, 11: 88},
        2: {5: 100, 7: 20, 10: 24, 13: 100, 14: step(70, 2), 16: 90},
        3: hats(upto=12),
        6: {15: 88},
        7: {16: 92},
        8: {13: 70},
    },
    "C": {  # sparse: kick, hats and a cross-stick
        1: {1: 92, 11: 72},
        3: hats(skip=(15,), soft=34, loud=66),
        4: {15: 52},
        5: {5: 70, 13: 70, 16: 35},
    },
    "D": {  # the button: everything on the one
        1: {1: 100},
        2: {1: 90},
        8: {1: 80},
    },
}
for letter, lanes in patterns.items():
    for lane in range(1, 9):
        hits = lanes.get(lane, {})
        for s in range(1, 17):
            v = hits.get(s)
            if v is None:
                value = -step(80)
            elif isinstance(v, float):
                value = v
            else:
                value = step(v)
            p(trig, f"Step {letter}{lane} {s:02d}", value)
wire(CLOCK, "Gate", TRIG, "Clock")
wire(CLOCK, "Reset", TRIG, "Reset")
wire(ARR, "Lane 2", TRIG, "Pattern")

# Drum voices: (id, type, tune, decay, tone, snap, level)
DRUMS = [
    (11, 0, 1.0, 0.50, 0.38, 0.45, 0.88),   # kick
    (12, 1, 1.0, 0.38, 0.55, 0.60, 0.80),   # snare
    (13, 4, 0.0, 0.22, 0.60, 0.40, 0.55),   # closed hat
    (14, 5, 0.0, 0.42, 0.55, 0.40, 0.50),   # open hat
    (15, 7, 0.0, 0.35, 0.50, 0.50, 0.60),   # rim
    (16, 2, 5.0, 0.40, 0.45, 0.50, 0.70),   # tom hi
    (17, 2, -2.0, 0.50, 0.45, 0.50, 0.75),  # tom lo
    (18, 3, 0.0, 0.40, 0.50, 0.50, 0.60),   # clap
]
for i, (nid, typ, tune, decay, tone, snap, level) in enumerate(DRUMS):
    node(nid, "source.drum", 560 + (i % 4) * 290, 1100 + (i // 4) * 380,
         Type=S(typ), Tune=tune, Decay=decay, Tone=tone, Snap=snap, Level=level)
    wire(TRIG, f"Gate {i+1}", nid, "Trig")
    wire(TRIG, f"Vel {i+1}", nid, "Accent")
wire(TRIG, "Gate 3", 14, "Choke")  # the closed hat shuts the open one

CRASH = 19
node(CRASH, "source.drum", 560, 1860, Type=S(6), Tune=0.0, Decay=0.75, Tone=0.45, Snap=0.4, Level=0.55)
wire(ARR, "Gate 6", CRASH, "Trig")
wire(ARR, "Lane 6", CRASH, "Accent")

# Two drum mixers, chained: kick snare hat ohat | rim toms clap+crash
DMIX2 = 21
node(DMIX2, "util.mixer", 1780, 1480, Level_1=0.7, Level_2=0.75, Level_3=0.8, Level_4=0.8,
     Pan_1=-0.25, Pan_2=-0.35, Pan_3=0.3, Pan_4=0.0, Send_1=0.15, Send_2=0.3, Send_3=0.3, Send_4=0.35, Master=0)
wire(15, "Out", DMIX2, "Ch 1")
wire(16, "Out", DMIX2, "Ch 2")
wire(17, "Out", DMIX2, "Ch 3")
DCRASH = 22  # clap and crash share channel 4 through a little mix
node(DCRASH, "util.mix", 1500, 1860, Level_1=0.8, Level_2=0.75, Level_3=0, Level_4=0)
wire(18, "Out", DCRASH, "In 1")
wire(CRASH, "Out", DCRASH, "In 2")
wire(DCRASH, "Out", DMIX2, "Ch 4")

DMIX = 20
node(DMIX, "util.mixer", 2220, 1100, Level_1=1.0, Level_2=0.85, Level_3=0.6, Level_4=0.55,
     Pan_1=0.0, Pan_2=0.05, Pan_3=0.35, Pan_4=0.4, Send_1=0.0, Send_2=0.18, Send_3=0.05, Send_4=0.1, Master=0)
for ch, nid in enumerate([11, 12, 13, 14], start=1):
    wire(nid, "Out", DMIX, f"Ch {ch}")
wire(DMIX2, "Chain Out", DMIX, "Chain In")

# ---------------------------------------------------------------- bass
BSEQ, BOSC, BMIX, BLAD, BVCA, BFENV, BAENV = 30, 31, 32, 33, 34, 35, 36
bseq = node(BSEQ, "seq.step", 40, 2400, Steps=16, Direction=S(0), Gate_Length=55, Glide=0.05)
p(bseq, "Chain 1", S(1))
p(bseq, "Chain 2", S(2))
# Sounding notes (MIDI); the oscillator sits three octaves down, so +36.
E1, G1, A1, B1, D2, E2, Cs2, Fs2, G2, A2, Ds1 = 28, 31, 33, 35, 38, 40, 37, 42, 43, 45, 27
# step: (note, velocity, tie, slide)
bass = {
    "": {1: (E1, 120, True), 2: (E1, 120), 4: (E2, 88), 6: (E1, 58), 7: (G1, 100), 8: (A1, 104),
         10: (E2, 96), 11: (D2, 84), 13: (B1, 100), 15: (D2, 90), 16: (E2, 78, False, True)},
    "B": {1: (A1, 120, True), 2: (A1, 120), 4: (A2, 88), 6: (A1, 58), 7: (Cs2, 100), 8: (E2, 104),
          10: (G2, 96), 11: (Fs2, 84), 12: (E2, 80), 14: (D2, 92), 15: (B1, 86), 16: (Ds1, 90)},
}
for prefix, steps in bass.items():
    for s in range(1, 17):
        note = steps.get(s)
        name = f"Step {prefix}{s}"
        if note is None:
            p(bseq, f"{name} Pitch", E1 + 36)
            p(bseq, f"{name} Gate", False)
            continue
        pitch, vel, *rest = note
        tie = rest[0] if rest else False
        slide = rest[1] if len(rest) > 1 else False
        p(bseq, f"{name} Pitch", pitch + 36)
        p(bseq, f"{name} Gate", True)
        p(bseq, f"{name} Velocity", vel)
        p(bseq, f"{name} Tie", tie)
        p(bseq, f"{name} Slide", slide)
wire(CLOCK, "Gate", BSEQ, "Clock")
wire(CLOCK, "Reset", BSEQ, "Reset")

node(BOSC, "osc.sine", 560, 2400, Octave=-3, Semitone=0, Fine=0, Waveform=S(1), Voices=1)
node(BMIX, "util.mix", 860, 2400, Level_1=0.55, Level_2=0.6, Level_3=0, Level_4=0)
node(BLAD, "filter.ladder", 1120, 2400, Cutoff=170, Resonance=0.32, Drive=2.2, Cutoff_CV=3.6)
node(BVCA, "util.vca", 1420, 2400, Level=0.9, CV_Amount=1.0)
node(BFENV, "mod.adsr", 1120, 2760, Attack=0.001, Decay=0.16, Sustain=0.12, Release=0.07,
     Attack_Curve=0.0, Decay_Curve=0.55, Release_Curve=0.5, Velocity_Amount=0.75)
node(BAENV, "mod.adsr", 1420, 2760, Attack=0.002, Decay=0.35, Sustain=0.65, Release=0.06,
     Attack_Curve=0.2, Decay_Curve=0.5, Release_Curve=0.5, Velocity_Amount=0.55)
wire(BSEQ, "Pitch", BOSC, "V/Oct")
wire(BSEQ, "Gate", BFENV, "Gate")
wire(BSEQ, "Gate", BAENV, "Gate")
wire(BSEQ, "Velocity", BFENV, "Velocity")
wire(BSEQ, "Velocity", BAENV, "Velocity")
wire(BOSC, "Out", BMIX, "In 1")
wire(BOSC, "Sub", BMIX, "In 2")
wire(BMIX, "Out", BLAD, "In")
wire(BFENV, "Out", BLAD, "Cutoff")
wire(BLAD, "LP24", BVCA, "In")
wire(BAENV, "Out", BVCA, "CV")

# ---------------------------------------------------------------- keys
CSEQ, EP, WAH, WENV = 40, 1400, 42, 43
cseq = node(CSEQ, "seq.chord", 40, 3200, Steps=16, Direction=S(0), Gate_Length=42, Voicing=S(0),
            Voice_Leading=True, Voices=4, Range=3, Bass_Voice=False, Rootless=True, Strum=6,
            Strum_Dir=S(2), Strum_Taper=20)
EM9, A13 = (52, 14), (57, 17)  # (root, type): m9 and 13
# eighth-note steps, two bars. (chord, velocity, tie) or None for a rest
keys = {1: (EM9, 100), 4: (EM9, 92), 6: (EM9, 84), 8: (A13, 104, True),
        9: (A13, 100), 12: (A13, 94), 14: (A13, 84), 15: (A13, 70), 16: (EM9, 104, True)}
for s in range(1, 17):
    k = keys.get(s)
    chord = EM9 if s < 8 or s == 16 else A13
    vel, tie, gate = 90, False, False
    if k:
        chord, vel, *rest = k
        tie = rest[0] if rest else False
        gate = True
    p(cseq, f"Step {s} Root", chord[0])
    p(cseq, f"Step {s} Type", S(chord[1]))
    p(cseq, f"Step {s} Gate", gate)
    p(cseq, f"Step {s} Velocity", vel)
    p(cseq, f"Step {s} Tie", tie)
wire(KEYCLK, "Gate", CSEQ, "Clock")
wire(CLOCK, "Reset", CSEQ, "Reset")

# The Electric Piano from the library, embedded as a group
lib = json.load(open(REPO / "library" / "electric-piano.json"))["groups"][0]
remap = {n["id"]: EP + n["id"] for n in lib["nodes"]}
remap[lib["id"]] = EP
group = dict(lib)
group["id"] = EP
group["position"] = [600.0, 3200.0]
group["nodes"] = [dict(n, id=remap[n["id"]]) for n in lib["nodes"]]
group["connections"] = [dict(c, from_node=remap[c["from_node"]], to_node=remap[c["to_node"]]) for c in lib["connections"]]
groups = [group]
wire(CSEQ, "Pitch", EP, "Pitch")
wire(CSEQ, "Gate", EP, "Gate")
wire(CSEQ, "Velocity", EP, "Velocity")

# An envelope filter after it: each stab opens the filter and lets it fall (the quack)
node(WAH, "filter.svf", 900, 3200, Cutoff=520, Resonance=0.62, Drive=1.4, Cutoff_CV=2.6)
node(WENV, "mod.adsr", 900, 3560, Attack=0.012, Decay=0.22, Sustain=0.1, Release=0.12,
     Attack_Curve=0.3, Decay_Curve=0.55, Release_Curve=0.5, Velocity_Amount=0.6)
wire(EP, "Out", WAH, "In")
wire(CSEQ, "Gate", WENV, "Gate")
wire(CSEQ, "Velocity", WENV, "Velocity")
wire(WENV, "Out", WAH, "Cutoff")

KCHO = 44
node(KCHO, "fx.chorus", 1180, 3200, Rate=0.6, Depth=0.35, Delay=9, Feedback=0, Voices=S(1), Mix=0.4, Shape=S(0))
wire(WAH, "LowPass", KCHO, "In L")

# ---------------------------------------------------------------- lead
LSEQ, LOSC, LLAD, LVCA, LFENV, LAENV, LDLY, VIB = 50, 51, 52, 53, 54, 55, 56, 57
lseq = node(LSEQ, "seq.step", 40, 3900, Steps=16, Direction=S(0), Gate_Length=70, Glide=0.07)
p(lseq, "Chain 1", S(2))  # B A: the song's even bars are A
p(lseq, "Chain 2", S(1))
E4, G4, A4, B4, D5, E5 = 64, 67, 69, 71, 74, 76
lead = {
    "": {3: (B4, 96), 4: (D5, 104, True, True), 5: (D5, 104), 7: (E5, 112, True), 8: (E5, 112),
         10: (D5, 82), 11: (B4, 90), 13: (A4, 96, True), 14: (A4, 96), 15: (G4, 80)},
    "B": {1: (A4, 100, True), 2: (A4, 100, True), 3: (A4, 100), 6: (G4, 80), 7: (E4, 100, True),
          8: (E4, 100, True), 9: (E4, 100), 11: (G4, 86), 12: (A4, 92, False, True), 13: (B4, 100, True),
          14: (B4, 100)},
}
for prefix, steps in lead.items():
    for s in range(1, 17):
        note = steps.get(s)
        name = f"Step {prefix}{s}"
        if note is None:
            p(lseq, f"{name} Pitch", E4)
            p(lseq, f"{name} Gate", False)
            continue
        pitch, vel, *rest = note
        p(lseq, f"{name} Pitch", pitch)
        p(lseq, f"{name} Gate", True)
        p(lseq, f"{name} Velocity", vel)
        p(lseq, f"{name} Tie", rest[0] if rest else False)
        p(lseq, f"{name} Slide", rest[1] if len(rest) > 1 else False)
wire(CLOCK, "Gate", LSEQ, "Clock")
wire(CLOCK, "Reset", LSEQ, "Reset")

node(LOSC, "osc.sine", 560, 3900, Octave=0, Waveform=S(1), Voices=3, Detune=0.12, Spread=0.5, Exp_FM_Depth=0.015)
node(VIB, "mod.lfo", 560, 4300, Rate=5.2, Waveform=S(0), Bipolar=True)
node(LLAD, "filter.ladder", 860, 3900, Cutoff=900, Resonance=0.28, Drive=1.6, Cutoff_CV=2.2)
node(LVCA, "util.vca", 1160, 3900, Level=0.55, CV_Amount=1.0)
node(LFENV, "mod.adsr", 860, 4260, Attack=0.004, Decay=0.3, Sustain=0.35, Release=0.12,
     Attack_Curve=0.2, Decay_Curve=0.5, Release_Curve=0.5, Velocity_Amount=0.6)
node(LAENV, "mod.adsr", 1160, 4260, Attack=0.006, Decay=0.25, Sustain=0.8, Release=0.14,
     Attack_Curve=0.2, Decay_Curve=0.5, Release_Curve=0.5, Velocity_Amount=0.5)
node(LDLY, "fx.delay", 1460, 3900, Time=430, Feedback=0.32, Mix=0.25, High_Cut=4500, Low_Cut=250,
     **{"Ping-Pong": True}, Sync=S(8), Tape=True)
wire(LSEQ, "Pitch", LOSC, "V/Oct")
wire(VIB, "Out", LOSC, "Exp FM")
wire(LSEQ, "Gate", LFENV, "Gate")
wire(LSEQ, "Gate", LAENV, "Gate")
wire(LSEQ, "Velocity", LFENV, "Velocity")
wire(LSEQ, "Velocity", LAENV, "Velocity")
wire(LOSC, "Out", LLAD, "In")
wire(LFENV, "Out", LLAD, "Cutoff")
wire(LLAD, "LP24", LVCA, "In")
wire(LAENV, "Out", LVCA, "CV")
wire(LVCA, "Out", LDLY, "In L")
wire(LVCA, "Out", LDLY, "In R")

# ---------------------------------------------------------------- the mix
MIX2, MIX, REV, TAPE, OUTN = 61, 60, 62, 63, 64
# Sidecar: lead (stereo, from the delay), keys chorus right
node(MIX2, "util.mixer", 2220, 3600, Level_1=0, Level_2=0, Level_3=0.0, Level_4=0.0,
     Pan_1=-0.55, Pan_2=0.55, Send_1=0.25, Send_2=0.25, Master=0)
wire(LDLY, "Out L", MIX2, "Ch 1")
wire(LDLY, "Out R", MIX2, "Ch 2")
wire(ARR, "Lane 5", MIX2, "Level 1")
wire(ARR, "Lane 5", MIX2, "Level 2")

# Main: drums L/R, bass, keys (chorus L/R folded into 4 with the sidecar's 3/4)
node(MIX, "util.mixer", 2700, 2400, Level_1=0, Level_2=0, Level_3=0, Level_4=0,
     Pan_1=-1.0, Pan_2=1.0, Pan_3=0.0, Pan_4=-0.45, Send_1=0.0, Send_2=0.0, Send_3=0.0, Send_4=0.0,
     Return=0.0, Width_4=0.0, Master=0.0)
wire(DMIX, "Out L", MIX, "Ch 1")
wire(DMIX, "Out R", MIX, "Ch 2")
wire(BVCA, "Out", MIX, "Ch 3")
wire(KCHO, "Out L", MIX, "Ch 4")
wire(KCHO, "Out R", MIX2, "Ch 3")
p(next(n for n in nodes if n["id"] == MIX2), "Pan 3", 0.45)
wire(ARR, "Lane 1", MIX, "Level 1")
wire(ARR, "Lane 1", MIX, "Level 2")
wire(ARR, "Lane 3", MIX, "Level 3")
wire(ARR, "Lane 4", MIX, "Level 4")
wire(ARR, "Lane 4", MIX2, "Level 3")
wire(MIX2, "Chain Out", MIX, "Chain In")
# The Room lane rides how much of the keys go to the reverb
wire(ARR, "Lane 7", MIX, "Send 4")
wire(ARR, "Lane 7", MIX2, "Send 3")

# One room for everything: the drum mixer's sends and the main's sends both feed it
DSEND = 66
node(DSEND, "util.mixer", 2700, 1480, Level_1=1, Level_2=1, Level_3=0.6, Level_4=0.6,
     Pan_1=-1, Pan_2=1, Pan_3=-1, Pan_4=1, Master=0)
wire(DMIX, "Send L", DSEND, "Ch 1")
wire(DMIX, "Send R", DSEND, "Ch 2")
wire(MIX, "Send L", DSEND, "Ch 3")
wire(MIX, "Send R", DSEND, "Ch 4")
node(REV, "fx.reverb", 3100, 1480, Size=0.42, Decay=1.6, Damping=0.6, **{"Pre-Delay": 12}, Mix=1.0, Width=1.0, Mod=0.2)
wire(DSEND, "Out L", REV, "In L")
wire(DSEND, "Out R", REV, "In R")
FINAL = 67
node(FINAL, "util.mixer", 3400, 1900, Level_1=1, Level_2=1, Level_3=0, Level_4=0, Pan_1=-1, Pan_2=1,
     Return=0.85, Master=2.0)
wire(MIX, "Out L", FINAL, "Ch 1")
wire(MIX, "Out R", FINAL, "Ch 2")
wire(REV, "Out L", FINAL, "Return L")
wire(REV, "Out R", FINAL, "Return R")

node(TAPE, "fx.tape", 3750, 1900, Wow=0.08, Flutter=0.1, Width=0.2, Saturation=0.28, Age=0.12, Hiss=0.04,
     Mix=1.0, Speed=S(1), Stop_Time=1.0)
node(OUTN, "output.audio", 4050, 1900, Volume=0.8, Limiter=True, Character=False)
wire(FINAL, "Out L", TAPE, "In L")
wire(FINAL, "Out R", TAPE, "In R")
wire(TAPE, "Out L", OUTN, "Left")
wire(TAPE, "Out R", OUTN, "Right")

# ---------------------------------------------------------------- the stage
# Each Dancer hears one part: the Robot the bass, the Headbanger the drums,
# and the big one in the middle the whole band, from the Tape. The crash is
# its cue. Every cable comes in clear of the faces: the Tape and the crash
# over the Robot's head, the drums up between the big one and the Headbanger
ROBOT, DISCO, HEADBANG = 70, 71, 72
DANCERS = [(ROBOT, 2, BVCA, "Out"), (DISCO, 1, TAPE, "Out L"), (HEADBANG, 3, DMIX, "Out L")]
for nid, style, src, port in DANCERS:
    node(nid, "util.dancer", 0, 0, Style=S(style), Size=3.0 if nid == DISCO else 1.8)
    wire(src, port, nid, "Audio")
wire(ARR, "Gate 6", DISCO, "Accent")


# ---------------------------------------------------------------- layout
# The Dancers stand on one floor, y 700 (a Size-1.8 node is 364 tall, the
# Size-3 one 503), either side of the stage's middle, x 2810
STAGE_AT = [(2153, 336), (2585, 197), (3185, 336)]
POS = {
    CLOCK: (40, 80), KEYCLK: (40, 460), ARR: (260, 80),
    TRIG: (40, 900),
    11: (440, 900), 12: (730, 900), 13: (1020, 900), 14: (1310, 900),
    15: (440, 1270), 16: (730, 1270), 17: (1020, 1270), 18: (1310, 1270),
    DCRASH: (1600, 900), CRASH: (1600, 1270), DMIX2: (1920, 1270), DMIX: (2660, 900),
    BSEQ: (40, 1820), BOSC: (440, 1820), BMIX: (660, 1820), BLAD: (930, 1820), BFENV: (930, 2140),
    BVCA: (1200, 1820), BAENV: (1200, 2140),
    CSEQ: (40, 2700), WAH: (720, 2700), WENV: (720, 3060), KCHO: (990, 2700),
    LSEQ: (40, 3640), LOSC: (440, 3640), VIB: (440, 4220), LLAD: (660, 3640), LFENV: (660, 3960),
    LVCA: (930, 3640), LAENV: (930, 3960), LDLY: (1200, 3640),
    MIX2: (2400, 2700), MIX: (2880, 1820), DSEND: (3180, 900), REV: (3620, 900),
    FINAL: (3320, 1820), TAPE: (1220, 180), OUTN: (1480, 520),
    ROBOT: STAGE_AT[0], DISCO: STAGE_AT[1], HEADBANG: STAGE_AT[2],
}
for n in nodes:
    n["position"] = [float(v) for v in POS[n["id"]]]
group["position"] = [440.0, 2700.0]

frames += [
    {"title": "Time", "position": [16.0, 40.0], "size": [1160.0, 780.0], "color": "slate"},
    {"title": "Drums", "position": [16.0, 860.0], "size": [3080.0, 880.0], "color": "amber"},
    {"title": "Bass", "position": [16.0, 1780.0], "size": [1680.0, 820.0], "color": "blue"},
    {"title": "Keys", "position": [16.0, 2660.0], "size": [1680.0, 860.0], "color": "green"},
    {"title": "Lead", "position": [16.0, 3600.0], "size": [1680.0, 1000.0], "color": "violet"},
    {"title": "Mix", "position": [2360.0, 1780.0], "size": [1400.0, 1400.0], "color": "teal"},
    {"title": "Out", "position": [1200.0, 40.0], "size": [510.0, 780.0], "color": "teal"},
    {"title": "Stage", "position": [2040.0, 40.0], "size": [1540.0, 740.0], "color": "rose"},
]
notes += [
    {"text": "**Dance Floor**: a funk vamp in E Dorian, Em9 to A13, at 108 BPM with the sixteenths swung to 57%. "
             "Four bars that loop, as long as a Dancer's phrase, so the band and the dance turn over together.",
     "position": [860.0, 90.0], "width": 290.0},
    {"text": "The **Pattern** lane plays the groove for three bars and a fill on the fourth. **Crash** hits on "
             "every phrase change, and it's the big Dancer's Accent too: its big move lands with it.",
     "position": [860.0, 330.0], "width": 290.0},
    {"text": "Each Dancer hears one part of the band: the Robot the bass, the Headbanger the drums, and the "
             "one in the middle all of it, as it leaves the Tape.", "position": [2070.0, 90.0], "width": 300.0},
]

patch = {"name": "Dance Floor", "version": 6, "nodes": nodes, "connections": conns, "midi_mappings": [],
         "frames": frames, "notes": notes, "groups": groups}
OUT.write_text(json.dumps(patch, indent=1) + "\n")
print(f"{len(nodes)} modules, {len(conns)} cables -> {OUT}")
