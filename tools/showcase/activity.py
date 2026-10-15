"""What is sounding when: each part's level through a song, from soloed stems.

    python tools/showcase/activity.py patch.json stems/ [--out DIR] [--sections Dawn@1,Noon@49,...]

`stems/` holds one WAV per part, rendered from the start of the song (the
name of the file is the part's name). Writes activity.json (each part's RMS
per beat, in dB, and the song's sections, read from the patch's first
Arranger and its Clock, or for a song with no Arranger, from `--sections`:
names at the bars they start on, counted from 1) and activity.png, a heat strip of every part across
the song with the sections marked.

A film imports it to aim the camera at what is playing:

    act = Activity.load("activity.json")
    act.level("flute", 99.3, 115.9)       # dB over a span (its quietest beat by default)
    act.check([(t0, t1, "flute"), ...])   # holds whose subject isn't carrying the moment
"""

import argparse
import json
import struct
import sys
from pathlib import Path

import numpy as np

FLOOR = -90.0


def read_wav(path):
    """A WAV as float frames x channels, and its rate (16-bit or float)."""
    data = Path(path).read_bytes()
    pos, fmt = 12, None
    while pos < len(data):
        cid, size = data[pos:pos + 4], struct.unpack("<I", data[pos + 4:pos + 8])[0]
        body = data[pos + 8:pos + 8 + size]
        if cid == b"fmt ":
            _, ch, rate, _, _, bits = struct.unpack("<HHIIHH", body[:16])
            fmt = (ch, rate, bits)
        elif cid == b"data":
            ch, rate, bits = fmt
            x = np.frombuffer(body, dtype=np.float32 if bits == 32 else np.int16).astype(np.float64)
            if bits == 16:
                x /= 32768
            return x.reshape(-1, ch), rate
        pos += 8 + size + (size & 1)
    raise ValueError(f"{path}: no audio")


def sections_of(patch, given=None):
    """[(name, start seconds, bars)] from the patch's first Arranger, timed
    by its Clock (Tempo, and a 1/16 clock at the Arranger's Steps a bar).
    `given` ([(name, first bar)]) names them instead, in 4/4 bars, where the
    song has no Arranger (Orrery); the last runs to the end of the song."""
    nodes = patch["nodes"]
    clock = next(n for n in nodes if n["module_id"] == "util.clock")
    arranger = next((n for n in nodes if n["module_id"] == "seq.arranger"), None)
    if given is not None or arranger is None:
        tempo = next(p["value"] for p in clock["parameters"] if p["name"] == "Tempo")
        bar = 4 * 60.0 / tempo
        given = given or [("", 1)]
        ends = [b for _, b in given[1:]] + [None]
        return [(name, round((b - 1) * bar, 4), (end - b) if end else 1) for (name, b), end in zip(given, ends)], tempo, bar
    param = {p["name"]: p["value"] for p in arranger["parameters"]}
    tempo = next(p["value"] for p in clock["parameters"] if p["name"] == "Tempo")
    bar = param["Steps"] / 4 * 60.0 / tempo
    labels = arranger.get("labels", {})
    out, t = [], 0.0
    for k in range(1, int(param["Sections"]) + 1):
        bars = int(param[f"Length {k:02}"])
        out.append((labels.get(f"Section {k}", f"Section {k}"), round(t, 4), bars))
        t += bars * bar
    return out, tempo, bar


class Activity:
    def __init__(self, data):
        self.data = data
        self.beat = data["beat"]
        self.parts = {k: np.array(v) for k, v in data["parts"].items()}

    @classmethod
    def load(cls, path):
        return cls(json.loads(Path(path).read_text()))

    def _span(self, t0, t1):
        a = max(0, int(t0 / self.beat))
        b = max(a + 1, int(np.ceil(t1 / self.beat)))
        return a, b

    def level(self, part, t0, t1, how="min"):
        """A part's level over a span, in dB: its quietest beat ("min"),
        loudest ("max") or the energy mean ("mean")."""
        a, b = self._span(t0, t1)
        v = self.parts[part][a:b]
        if how == "mean":
            return float(10 * np.log10(np.mean(10 ** (v / 10)) + 1e-12))
        return float(v.min() if how == "min" else v.max())

    def loudest(self, t0, t1):
        """Parts by their mean level over a span, loudest first."""
        return sorted(((self.level(p, t0, t1, "mean"), p) for p in self.parts), reverse=True)

    def check(self, holds, within=12.0, audible=-45.0):
        """Each hold (t0, t1, part or [parts]) whose subject isn't carrying
        the moment: quieter than `audible` on some beat, or more than
        `within` dB under the loudest part. Returns [(hold, why)]."""
        bad = []
        for t0, t1, subject in holds:
            subjects = [subject] if isinstance(subject, str) else list(subject)
            top_db, top = self.loudest(t0, t1)[0]
            best = max(subjects, key=lambda p: self.level(p, t0, t1, "mean"))
            quiet = self.level(best, t0, t1, "min")
            mean = self.level(best, t0, t1, "mean")
            if quiet < audible:
                bad.append(((t0, t1, subject), f"{best} drops to {quiet:.0f} dB"))
            elif mean < top_db - within:
                bad.append(((t0, t1, subject), f"{best} {mean:.0f} dB under {top} {top_db:.0f} dB"))
        return bad


def measure(patch_path, stems, given=None):
    patch = json.loads(Path(patch_path).read_text(encoding="utf-8"))
    sections, tempo, bar = sections_of(patch, given)
    beat = 60.0 / tempo
    parts = {}
    for wav in sorted(Path(stems).glob("*.wav")):
        x, rate = read_wav(wav)
        hop = beat * rate
        n = int(len(x) / hop)
        ms = [np.mean(x[int(i * hop):int((i + 1) * hop)] ** 2) for i in range(n)]
        parts[wav.stem] = [round(max(FLOOR, 10 * np.log10(m + 1e-12)), 2) for m in ms]
    return {"tempo": tempo, "beat": beat, "bar": bar,
            "sections": [{"name": n, "start": t, "bars": b} for n, t, b in sections],
            "parts": parts}


def heat_strip(data, path):
    """Every part across the song, one row each, brighter where louder; the
    sections are thin lines, named along the top."""
    from PIL import Image, ImageDraw
    parts = data["parts"]
    names = list(parts)
    beats = max(len(v) for v in parts.values())
    cell, row, left, top = 3, 22, 86, 26
    img = Image.new("RGB", (left + beats * cell + 8, top + row * len(names) + 8), (16, 17, 24))
    d = ImageDraw.Draw(img)

    def colour(db):
        # Night to canopy-green to sunlight: -60 dB is dark, -12 dB is bright
        u = min(1.0, max(0.0, (db + 60) / 48))
        stops = [(16, 17, 24), (24, 70, 72), (88, 170, 110), (240, 214, 120)]
        k = min(len(stops) - 2, int(u * (len(stops) - 1)))
        f = u * (len(stops) - 1) - k
        return tuple(round(a + (b - a) * f) for a, b in zip(stops[k], stops[k + 1]))

    for r, name in enumerate(names):
        y = top + r * row
        d.text((6, y + 5), name, fill=(200, 200, 210))
        for i, v in enumerate(parts[name]):
            d.rectangle([left + i * cell, y + 2, left + (i + 1) * cell - 1, y + row - 3], fill=colour(v))
    for s in data["sections"]:
        x = left + round(s["start"] / data["beat"]) * cell
        d.line([x, top - 4, x, img.height - 6], fill=(120, 120, 140))
        if s["bars"] > 1:
            d.text((x + 3, 6), s["name"], fill=(170, 170, 185))
    img.save(path)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("patch")
    ap.add_argument("stems")
    ap.add_argument("--out", default=".")
    ap.add_argument("--sections", help="Name@bar,...: the sections, where the patch has no Arranger")
    a = ap.parse_args()
    given = None
    if a.sections:
        given = [(name, int(b)) for name, b in (s.rsplit("@", 1) for s in a.sections.split(","))]
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    data = measure(a.patch, a.stems, given)
    (out / "activity.json").write_text(json.dumps(data))
    heat_strip(data, out / "activity.png")
    act = Activity(data)
    for s in data["sections"]:
        t1 = s["start"] + s["bars"] * data["bar"]
        top = ", ".join(f"{p} {db:.0f}" for db, p in act.loudest(s["start"], t1)[:4])
        print(f"{s['start']:7.2f}  {s['name']:10} {top}")
    print(out / "activity.png")


if __name__ == "__main__":
    sys.exit(main())
