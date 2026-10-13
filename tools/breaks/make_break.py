"""Makes the Chopper example's break, Soba's own: two bars of a swung funk
break played by its Drum voices, rendered and cut to an exact loop.

    python tools/breaks/make_break.py

Writes the patch that plays it (tools/breaks/soba-break-kit.json), renders
two passes with the release build's `render`, and keeps the second, so the
loop starts with the tails of its own last bar and wraps without a seam.
The loop is mono, 16-bit, peaking at -1 dBFS: patches/samples/soba-break.wav.

Needs a release build: cargo build --release (or CARGO_TARGET_DIR set to
where one is).
"""

import json
import os
import subprocess
import sys
import tempfile
import wave
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
KIT = ROOT / "tools" / "breaks" / "soba-break-kit.json"
OUT = ROOT / "patches" / "samples" / "soba-break.wav"

TEMPO = 96.0
BARS = 2
SWING = 57.0
RATE = 48000
# How hard the loop is driven into tape: 1 barely touches it
DRIVE = 2.5

# Sixteen steps a bar, A then B. Each hit is (step, velocity %), from 1.
# Lanes: kick, snare, closed hat, open hat.
PATTERNS = {
    "A": {
        1: [(1, 100), (8, 70), (11, 92)],
        2: [(5, 100), (8, 22), (10, 30), (13, 100), (16, 26)],
        3: [(1, 78), (3, 55), (5, 80), (7, 52), (9, 76), (11, 56), (12, 30), (13, 80)],
        4: [(15, 62)],
    },
    "B": {
        1: [(1, 100), (3, 84), (10, 76), (11, 94)],
        2: [(5, 100), (7, 24), (12, 28), (13, 100), (15, 30), (16, 62)],
        3: [(1, 78), (3, 55), (5, 80), (9, 76), (11, 56), (13, 80), (14, 34)],
        4: [(7, 58), (15, 66)],
    },
}

# (type, tune, decay, tone, snap, level): a dry, tight kit
DRUMS = [
    (0, -2.0, 0.38, 0.4, 0.62, 0.9),  # Kick
    (1, 1.5, 0.34, 0.62, 0.7, 0.8),  # Snare
    (4, 0.0, 0.3, 0.66, 0.42, 0.62),  # Closed hat
    (5, 0.0, 0.5, 0.62, 0.45, 0.5),  # Open hat
]


def number(name, value):
    return {"name": name, "type": "Number", "value": float(value)}


def select(name, value):
    return {"name": name, "type": "Select", "value": value}


def toggle(name, value):
    return {"name": name, "type": "Toggle", "value": value}


def kit():
    """The patch that plays the break."""
    trigger = [number("Steps", 16), select("Chain 1", 1), select("Chain 2", 2)]
    for letter, lanes in PATTERNS.items():
        for lane, hits in lanes.items():
            for step, velocity in hits:
                trigger.append(number(f"Step {letter}{lane} {step:02}", 1_100_000 + velocity))
    nodes = [
        {"id": 1, "module_id": "util.clock", "position": [40.0, 80.0], "parameters": [
            number("Tempo", TEMPO), select("Division", 4), toggle("Run", True), number("Swing", SWING)]},
        {"id": 2, "module_id": "seq.trigger", "position": [300.0, 40.0], "parameters": trigger},
    ]
    connections = [{"from_node": 1, "from_port": "Gate", "to_node": 2, "to_port": "Clock"},
                   {"from_node": 1, "from_port": "Reset", "to_node": 2, "to_port": "Reset"}]
    for lane, (kind, tune, decay, tone, snap, level) in enumerate(DRUMS, start=1):
        node = 10 + lane
        nodes.append({"id": node, "module_id": "source.drum", "position": [800.0, 40.0 + 220.0 * lane], "parameters": [
            select("Type", kind), number("Tune", tune), number("Decay", decay), number("Tone", tone),
            number("Snap", snap), number("Level", level)]})
        connections += [{"from_node": 2, "from_port": f"Gate {lane}", "to_node": node, "to_port": "Trig"},
                        {"from_node": 2, "from_port": f"Vel {lane}", "to_node": node, "to_port": "Accent"},
                        {"from_node": node, "from_port": "Out", "to_node": 20, "to_port": f"Ch {lane}"}]
    # The closed hat chokes the open one, as a foot on the pedal does
    connections.append({"from_node": 2, "from_port": "Gate 3", "to_node": 14, "to_port": "Choke"})
    nodes += [
        {"id": 20, "module_id": "util.mixer", "position": [1150.0, 200.0], "parameters": [
            number("Level 1", 0.9), number("Level 2", 0.75), number("Level 3", 0.42), number("Level 4", 0.4)]},
        # A small room, so the hits sit somewhere
        {"id": 21, "module_id": "fx.reverb", "position": [1550.0, 200.0], "parameters": [
            number("Size", 0.22), number("Decay", 0.5), number("Damping", 0.6), number("Pre-Delay", 4.0),
            number("Mix", 0.12), number("Width", 0.6), number("Mod", 0.1)]},
        {"id": 22, "module_id": "output.audio", "position": [1850.0, 200.0], "parameters": [
            number("Volume", 0.8), toggle("Limiter", False), toggle("Character", False)]},
    ]
    connections += [{"from_node": 20, "from_port": "Out L", "to_node": 21, "to_port": "In L"},
                    {"from_node": 20, "from_port": "Out R", "to_node": 21, "to_port": "In R"},
                    {"from_node": 21, "from_port": "Out L", "to_node": 22, "to_port": "Left"},
                    {"from_node": 21, "from_port": "Out R", "to_node": 22, "to_port": "Right"}]
    return {"name": "Soba Break Kit", "version": 5, "nodes": nodes, "connections": connections,
            "midi_mappings": [], "frames": [], "notes": []}


def read_float_wav(path):
    """The frames of a 32-bit float WAV, as render writes them (which the
    wave module can't read), one column a channel."""
    raw = Path(path).read_bytes()
    at, channels, data = 12, None, None
    while at + 8 <= len(raw):
        kind, size = raw[at:at + 4], int.from_bytes(raw[at + 4:at + 8], "little")
        body = raw[at + 8:at + 8 + size]
        if kind == b"fmt ":
            channels = int.from_bytes(body[2:4], "little")
            assert int.from_bytes(body[14:16], "little") == 32, "render writes 32-bit float"
        elif kind == b"data":
            data = np.frombuffer(body, dtype="<f4")
        at += 8 + size + (size & 1)
    return data.reshape(-1, channels)


def render_binary():
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    if not target.is_absolute():
        target = ROOT / target
    exe = target / "release" / ("render.exe" if sys.platform == "win32" else "render")
    if not exe.exists():
        sys.exit(f"no render at {exe}: cargo build --release first")
    return exe


def main():
    KIT.write_text(json.dumps(kit(), indent=2) + "\n", encoding="utf-8")
    loop_seconds = BARS * 4 * 60.0 / TEMPO
    with tempfile.TemporaryDirectory() as scratch:
        rendered = Path(scratch) / "two-passes.wav"
        subprocess.run([str(render_binary()), str(KIT), str(rendered), "--seconds", str(2 * loop_seconds),
                        "--sample-rate", str(RATE)], check=True)
        data = read_float_wav(rendered)
    # The second pass, as one channel, through tape: driven into a soft
    # curve that rounds off the Drums' sharpest transients, as a break
    # lifted from a record already has been. Then peaking at -1 dBFS
    length = round(loop_seconds * RATE)
    loop = data[length:2 * length].mean(axis=1)
    loop /= max(np.abs(loop).max(), 1e-6)
    loop = np.tanh(DRIVE * loop) / np.tanh(DRIVE)
    loop *= 10 ** (-1 / 20) / max(np.abs(loop).max(), 1e-6)
    crest = 20 * np.log10(np.abs(loop).max() / np.sqrt(np.mean(loop ** 2)))
    OUT.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(OUT), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(np.round(loop * 32767).astype("<i2").tobytes())
    print(f"{OUT.relative_to(ROOT)}: {length / RATE:.3f} s, {BARS} bars at {TEMPO:g} BPM, peaks {crest:.1f} dB over its RMS")


if __name__ == "__main__":
    main()
