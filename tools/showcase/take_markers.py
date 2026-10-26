"""A take's beats as editor markers, and its chapters for YouTube.

    python tools/showcase/take_markers.py TAKE [--script SCRIPT]
        [--edl markers.edl] [--fps 30] [--start 01:00:00:00]

Prints each beat where it starts in the film, and every retake: a beat you
played again (Page Up) leaves the film holding still from where its first
try began, and the voice there is the line you fluffed, so that stretch is
the cut. Times are the film's, from the first press at 0:00.

With `--script` (the presentation the take was presented from), its
section comments, `# --- Step 1: A buzz (the oscillator) ---`, become
YouTube chapters at the beat after each. Chapters are in the film's time:
move them by whatever you cut before them.

With `--edl`, it writes the markers as an EDL DaVinci Resolve reads
(Timeline > Import > Timeline Markers from EDL): a blue marker at each
beat, and a red one spanning each retake's cut. `--start` is the
timeline's first timecode (Resolve's is 01:00:00:00) and `--fps` its frame
rate.
"""

import argparse
import re
import sys
from pathlib import Path

BEAT = re.compile(r"^# Beat (\d+) · (.*), at (\d+):(\d+(?:\.\d+)?)$")
AGAIN = re.compile(r"^# Played again: the film holds still from (\d+):(\d+(?:\.\d+)?),")
SECTION = re.compile(r"^# -+ (.+?) -+$")


def seconds(m, s):
    return int(m) * 60 + float(s)


def clock(t):
    return f"{int(t // 60)}:{int(t % 60):02d}"


def read_take(path):
    """The take's beats, in order: (number, name, start, retake's cut from or None)."""
    beats = []
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        if m := BEAT.match(line):
            beats.append([int(m[1]), m[2], seconds(m[3], m[4]), None])
        elif (m := AGAIN.match(line)) and beats:
            beats[-1][3] = seconds(m[1], m[2])
    return beats


def read_sections(path):
    """The script's section titles, each against the beat that follows it."""
    sections, pending = {}, None
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        if m := SECTION.match(line):
            pending = m[1]
        elif line.startswith("beat ") and pending:
            sections[line[5:].strip()] = pending
            pending = None
    return sections


def timecode(t, fps, start):
    h, m, s, f = (int(x) for x in start.split(":"))
    frames = round(t * fps) + ((h * 60 + m) * 60 + s) * fps + f
    f = frames % fps
    s = frames // fps
    return f"{s // 3600:02d}:{s // 60 % 60:02d}:{s % 60:02d}:{f:02d}"


def edl(beats, fps, start, title):
    lines = [f"TITLE: {title}", "FCM: NON-DROP FRAME", ""]
    n = 0

    def event(t, name, colour, length):
        nonlocal n
        n += 1
        a, b = timecode(t, fps, start), timecode(t + 1 / fps, fps, start)
        lines.append(f"{n:03d}  001      V     C        {a} {b} {a} {b}  ")
        lines.append(f" |C:{colour} |M:{name} |D:{max(1, length)}")
        lines.append("")

    for number, name, at, again_from in beats:
        if again_from is not None:
            event(again_from, f"Cut: the first try of {name}", "ResolveColorRed", round((at - again_from) * fps))
        event(at, f"{number} · {name}", "ResolveColorBlue", 1)
    return "\n".join(lines)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("take", help="a take written by --take or Save take…")
    ap.add_argument("--script", help="the presentation script, for chapter titles")
    ap.add_argument("--edl", help="write the markers here, for DaVinci Resolve")
    ap.add_argument("--fps", type=int, default=30)
    ap.add_argument("--start", default="01:00:00:00", help="the timeline's first timecode")
    a = ap.parse_args()
    sys.stdout.reconfigure(encoding="utf-8")  # beat names have a ·

    beats = read_take(a.take)
    if not beats:
        raise SystemExit(f"{a.take} has no beats: is it a take?")

    print("Beats, in the film's time:")
    for number, name, at, again_from in beats:
        if again_from is not None:
            print(f"  {clock(again_from)}-{clock(at)}  CUT: the first try of {name} (the film holds still)")
        print(f"  {at:7.1f}s  {number:>2} · {name}")
    retakes = sum(1 for b in beats if b[3] is not None)
    if retakes:
        print(f"\n{retakes} retake{'s' if retakes > 1 else ''} to cut.")

    if a.script:
        sections = read_sections(a.script)
        chapters = [(at, sections[name]) for _, name, at, _ in beats if name in sections]
        if chapters:
            # YouTube wants the first chapter at 0:00
            chapters[0] = (0.0, chapters[0][1])
            print("\nChapters (film time: move each by what you cut before it):")
            for at, title in chapters:
                print(f"{clock(at)} {title}")

    if a.edl:
        Path(a.edl).write_text(edl(beats, a.fps, a.start, Path(a.take).stem), encoding="utf-8")
        print(f"\nWrote {a.edl}: Timeline > Import > Timeline Markers from EDL, in Resolve.")


if __name__ == "__main__":
    main()
