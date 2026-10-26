"""Where a filmed take starts in the narration you recorded while presenting.

    python tools/showcase/take_sync.py FILM RECORDING [--track N]
        [--voice FILE] [--voice-track M] [--out voice.wav]

Record two tracks as you present, your mic and Soba's own sound (desktop or
loopback audio): OBS in audio-only mode does it, as do Reaper and Audacity.
Then film the take (`soba patch.json --capture take.txt --out FILM`). The
film's sound and the recording's Soba track are the same sound, so lining
them up lines up your voice: this finds where the film's zero, the first
press, falls in the recording.

FILM is the capture's folder (its audio.wav is read) or any audio file.
RECORDING is anything ffmpeg reads; `--track N` picks its Nth audio track
(from 0), for a recorder that keeps both tracks in one file.

It matches the two sounds' loudness, 5 ms at a time, rather than their
waveforms, so a note that started a block later live still lines up. It
matches several minutes of the film separately and fits a line through
them, so it also says whether the recorder's clock drifted from Soba's.

With `--voice` (a file, or the recording's `--voice-track`), it also writes
`--out`: the voice from the film's zero on, stretched by the drift, ready
to drop on the film's timeline at 0:00. The voice and the Soba track must
come from the same recording, so they share its clock.
"""

import argparse
import subprocess
import sys
from pathlib import Path

import numpy as np

RATE = 8000  # Hz: plenty for a loudness contour
HOP = 40  # samples at RATE: 5 ms
WINDOW = 60.0  # seconds of film matched at a time


def decode(path, track=None, rate=RATE):
    """A file's sound as mono float samples at `rate`."""
    cmd = ["ffmpeg", "-hide_banner", "-loglevel", "error", "-i", str(path)]
    if track is not None:
        cmd += ["-map", f"0:a:{track}"]
    cmd += ["-ac", "1", "-ar", str(rate), "-f", "f32le", "-"]
    out = subprocess.run(cmd, capture_output=True, check=True).stdout
    return np.frombuffer(out, dtype=np.float32).astype(np.float64)


def contour(x):
    """Loudness every 5 ms, in log terms, so quiet and loud passages both count."""
    n = len(x) // HOP
    rms = np.sqrt(np.mean(x[: n * HOP].reshape(n, HOP) ** 2, axis=1))
    return np.log10(rms + 1e-4)


def best_lag(needle, hay):
    """Where `needle` sits in `hay` (in contour steps), and how sure: the
    normalised correlation at that lag."""
    needle = needle - needle.mean()
    norm = np.linalg.norm(needle)
    if norm == 0 or len(hay) < len(needle):
        return None, 0.0
    size = 1 << int(np.ceil(np.log2(len(hay) + len(needle))))
    corr = np.fft.irfft(np.fft.rfft(hay - hay.mean(), size) * np.conj(np.fft.rfft(needle, size)), size)
    corr = corr[: len(hay) - len(needle) + 1]
    # Normalise by the stretch of hay under the needle, so loud passages don't win by being loud
    c = np.concatenate([[0.0], np.cumsum(hay - hay.mean())])
    c2 = np.concatenate([[0.0], np.cumsum((hay - hay.mean()) ** 2)])
    m = len(needle)
    local = np.sqrt(np.maximum(c2[m:] - c2[:-m] - (c[m:] - c[:-m]) ** 2 / m, 1e-12))
    score = corr / (norm * local)
    lag = int(np.argmax(score))
    # Between steps: the top of a parabola through the peak and its neighbours
    fine = float(lag)
    if 0 < lag < len(score) - 1:
        a, b, c = score[lag - 1], score[lag], score[lag + 1]
        if a - 2 * b + c < 0:
            fine += 0.5 * (a - c) / (a - 2 * b + c)
    return fine, float(score[lag])


def clock(seconds):
    sign = "-" if seconds < 0 else ""
    seconds = abs(seconds)
    return f"{sign}{int(seconds // 60)}:{seconds % 60:06.3f}"


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("film", help="the capture's folder, or its audio.wav")
    ap.add_argument("recording", help="the recording that holds Soba's sound")
    ap.add_argument("--track", type=int, help="the recording's audio track with Soba's sound, from 0")
    ap.add_argument("--voice", help="the voice: a file (default: the recording itself, with --voice-track)")
    ap.add_argument("--voice-track", type=int, help="the voice's audio track, from 0")
    ap.add_argument("--out", help="where to write the voice, lined up to the film (WAV)")
    a = ap.parse_args()

    film_path = Path(a.film)
    if film_path.is_dir():
        film_path = film_path / "audio.wav"
    film = contour(decode(film_path))
    rec = contour(decode(a.recording, a.track))
    step = HOP / RATE

    # One match per minute of film that has something in it to match
    span = int(WINDOW / step)
    hits = []
    for start in range(0, max(1, len(film) - span // 2), span):
        piece = film[start : start + span]
        if piece.std() < 0.05:
            continue  # silence, or a steady drone: nothing to pin it by
        lag, score = best_lag(piece, rec)
        if lag is not None and score > 0.5:
            hits.append((start * step, (lag - start) * step, score))
    if not hits:
        sys.exit("Couldn't find the film's sound in the recording. Is --track the one with Soba's sound on it?")

    t = np.array([h[0] for h in hits])
    off = np.array([h[1] for h in hits])
    # A line through the matches, after dropping any that disagree (a retake's stretch can mislead one)
    keep = np.abs(off - np.median(off)) < 0.25
    if keep.sum() >= 2:
        slope, offset = np.polyfit(t[keep], off[keep], 1)
    else:
        slope, offset = 0.0, float(np.median(off))

    print(f"Matched {keep.sum()} of {len(hits)} minutes of the film:")
    for (at, o, score), k in zip(hits, keep):
        print(f"  film {clock(at)}  ->  recording {clock(at + o)}   (match {score:.2f}){'' if k else '  ignored'}")
    print(f"\nThe film's 0:00 is at {clock(offset)} in the recording.")
    drift = slope * 1e6
    over = slope * len(film) * step * 1000
    print(f"Clock drift: {drift:+.0f} ppm, {over:+.0f} ms over the film"
          + ("" if abs(over) < 17 else " (more than half a frame: --out stretches the voice to fit)"))

    if a.out:
        if not a.voice and a.voice_track is None:
            sys.exit("--out needs the voice: --voice FILE, or --voice-track N for the recording's own track")
        voice = a.voice or a.recording
        track = a.voice_track
        rate = 48000
        # Recording time = offset + film time x (1 + slope): play it (1 + slope) times as fast
        filters = [f"aresample={rate}"]
        if offset >= 0:
            filters.append(f"atrim=start={offset:.6f},asetpts=PTS-STARTPTS")
        else:
            filters.append(f"adelay={-offset * 1000:.3f}:all=1")
        if abs(over) >= 1:
            filters.append(f"asetrate={rate * (1 + slope):.6f},aresample={rate}")
        cmd = ["ffmpeg", "-y", "-hide_banner", "-loglevel", "error", "-i", str(voice)]
        if track is not None:
            cmd += ["-map", f"0:a:{track}"]
        cmd += ["-ar", str(rate), "-af", ",".join(filters), "-c:a", "pcm_s24le", a.out]
        subprocess.run(cmd, check=True)
        print(f"\nWrote {a.out}: the voice from the film's 0:00 on. Put it at 0:00 on the film's timeline.")


if __name__ == "__main__":
    main()
