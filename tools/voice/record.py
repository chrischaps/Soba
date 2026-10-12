"""Records a person saying the Vocoder example's line, ready for the Sampler.

    python tools/voice/record.py patches/samples/soba-speaks-chris.wav
    python tools/voice/record.py out.wav --from take.wav

"Hello. I am Soba. I sing in sines." A bar of clicks at the patch's 76 BPM
counts you in, and the take starts on the next downbeat, the moment the
Sampler fires. Say the line at an easy pace: it has two bars (6.3 s), and
lands best when "sines" falls near the second chord, about 3.2 s in. Then
the take plays back, and you keep it or go again. Headphones keep the clicks
out of the take, though the take only starts once they've stopped.

`--from` cleans up a take recorded elsewhere instead (any WAV, FLAC or MP3).

Either way the take is made ready the same way as tools/voice/speak.py's
line, so the patch hears both at one level:

- mono, 44.1 kHz, rumble below 70 Hz removed;
- trimmed to the words, from where the sound leading into the first began
  (keeping a breath, not a bumped stand) to 300 ms after the last, so "Hello" starts on the beat and "sines" can breathe out;
- the room between words turned down by up to 12 dB, gently, so breaths
  stay but the vocoder doesn't play the room's hiss as a chord;
- -16 dBFS RMS over the voiced parts, peaks kept under -1 dBFS by a gentle
  limiter (up to 6 dB of it; a louder pop lowers the whole take instead).

Recording needs `pip install sounddevice`; cleaning up a file doesn't.
"""

import argparse
import sys
import tempfile
import time
from pathlib import Path

import numpy as np
import soundfile as sf
from scipy.ndimage import minimum_filter1d
from scipy.signal import butter, resample_poly, sosfilt

OUT_RATE = 44100
BPM = 76
BAR = 4 * 60 / BPM
TAKE_SECONDS = 2 * BAR + 0.5

# One level measurement every 10 ms
FRAME = OUT_RATE // 100
TAIL = 0.30
TARGET_RMS_DB = -16.0
PEAK_CEILING_DB = -1.0
# How far the room between words is turned down, and how far above the
# room's own level a sound must be to count as voice
ROOM_CUT_DB = 12.0
# Peaks this far over the ceiling are limited; beyond it the take is lowered
LIMIT_DB = 6.0
VOICE_ABOVE_ROOM_DB = 10.0
# How long a sound lasts before it counts as words, not a thump or a click
WORD_FRAMES = 8
# How far before the first word the take may start
LEAD_IN_FRAMES = 30


def db(x):
    return 20 * np.log10(np.maximum(x, 1e-9))


def frame_levels(x):
    """RMS level in dB of each 10 ms frame."""
    frames = len(x) // FRAME
    power = np.mean(x[: frames * FRAME].reshape(frames, FRAME) ** 2, axis=1)
    return 10 * np.log10(np.maximum(power, 1e-18))


def follow(levels, attack_frames, release_frames):
    """An envelope over frame levels (dB) that rises in `attack_frames` and
    falls in `release_frames`, so a gate opens fast and closes slowly."""
    out = np.empty_like(levels)
    state = levels[0]
    up, down = 1 / attack_frames, 1 / release_frames
    for i, level in enumerate(levels):
        state += (level - state) * (up if level > state else down)
        out[i] = state
    return out


def limit(x, ceiling):
    """A look-ahead peak limiter: the gain dips smoothly over 3 ms before
    each peak that would pass `ceiling`, and recovers over about 60 ms."""
    width = int(0.003 * OUT_RATE)
    needed = np.minimum(1.0, ceiling / np.maximum(np.abs(x), 1e-9))
    gain = minimum_filter1d(needed, 2 * width + 1)
    gain = np.convolve(np.pad(gain, width, mode="edge"), np.ones(width) / width, mode="same")[width:-width]
    release = 1 - np.exp(-1 / (0.06 * OUT_RATE))
    smooth = np.empty_like(gain)
    state = 1.0
    for i, g in enumerate(gain):
        state = g if g < state else state + (g - state) * release
        smooth[i] = state
    # The average can't dip quite far enough on the sharpest edges
    return np.clip(x * smooth, -ceiling, ceiling)


def runs(mask, bridge):
    """(start, end) of each run of True in `mask`, joined across gaps shorter
    than `bridge` frames."""
    edges = np.flatnonzero(np.diff(np.concatenate([[0], mask.astype(int), [0]])))
    out = []
    for start, end in zip(edges[::2], edges[1::2]):
        if out and start - out[-1][1] < bridge:
            out[-1] = (out[-1][0], end)
        else:
            out.append((start, end))
    return out


def words(levels, loudest, room):
    """The first and last frames of the words. A word is a sound within 30 dB
    of the loudest, and well over the room, that lasts WORD_FRAMES, so a lone thump or lip smack
    doesn't count. The take then starts where the sound leading into the
    first word began: walking back from the word while the level keeps
    falling, it keeps the breath and the rising "h", and stops in the dip
    before anything earlier, such as a thump."""
    spoken = runs(levels > max(loudest - 30, room + VOICE_ABOVE_ROOM_DB), 3)
    sustained = [(a, b) for a, b in spoken if b - a >= WORD_FRAMES] or spoken
    first, last = sustained[0][0], sustained[-1][1] - 1
    quietest = i = first
    while i > max(0, first - LEAD_IN_FRAMES) and levels[i - 1] < levels[quietest] + 3:
        i -= 1
        if levels[i] < levels[quietest]:
            quietest = i
    return quietest, last


def prepare(take, rate):
    """The take made ready for the Sampler, and a report of what was done."""
    if take.ndim > 1:
        take = take.mean(axis=1)
    if rate != OUT_RATE:
        g = np.gcd(rate, OUT_RATE)
        take = resample_poly(take, OUT_RATE // g, rate // g)
    take = sosfilt(butter(2, 70, "highpass", fs=OUT_RATE, output="sos"), take)

    levels = frame_levels(take)
    if len(levels) < 10:
        raise ValueError("the take is too short to hold a line")
    room = np.percentile(levels, 10)
    loudest = np.percentile(levels, 99.5)
    threshold = max(room + VOICE_ABOVE_ROOM_DB, loudest - 45)
    voiced = levels > threshold
    if not voiced.any():
        raise ValueError("no voice found: is the mic muted, or the wrong input?")

    first, last = words(levels, loudest, room)
    start = first * FRAME
    end = min(len(take), (last + 1) * FRAME + int(TAIL * OUT_RATE))

    # Turn the room down between words: the cut eases in over 15 dB below
    # the threshold, opens in 10 ms and closes over 120 ms
    envelope = follow(levels, 1, 12)
    depth = np.clip((threshold - envelope) / 15.0, 0.0, 1.0)
    gain_db = np.repeat(-ROOM_CUT_DB * depth, FRAME)
    gain_db = np.pad(gain_db, (0, len(take) - len(gain_db)), mode="edge")
    take = take * 10 ** (gain_db / 20)

    take = take[start:end]
    fade_in, fade_out = int(0.005 * OUT_RATE), int(0.08 * OUT_RATE)
    take[:fade_in] *= np.linspace(0, 1, fade_in)
    take[-fade_out:] *= np.linspace(1, 0, fade_out) ** 2

    voiced_samples = np.repeat(voiced, FRAME)[start:end]
    voiced_samples = np.pad(voiced_samples, (0, len(take) - len(voiced_samples)))
    rms = np.sqrt(np.mean(take[voiced_samples] ** 2))
    take *= 10 ** (TARGET_RMS_DB / 20) / rms
    # Plosives and sharp consonants peak far above speech's average. Up to
    # LIMIT_DB of them is limited away; anything beyond lowers the whole take
    ceiling = 10 ** (PEAK_CEILING_DB / 20)
    over = db(np.abs(take).max()) - PEAK_CEILING_DB
    held_back = max(0.0, over - LIMIT_DB)
    take *= 10 ** (-held_back / 20)
    take = limit(take, ceiling)
    voiced_rms = db(np.sqrt(np.mean(take[voiced_samples] ** 2)))

    report = {
        "length": len(take) / OUT_RATE,
        "signal_to_room": loudest - room,
        "voiced_rms": voiced_rms,
        "peak": db(np.abs(take).max()),
        "limited": min(max(over, 0.0), LIMIT_DB),
        "held_back": held_back,
    }
    return take, report


def describe(out, report):
    print(f"{out}: {report['length']:.2f} s, voice {report['voiced_rms']:.1f} dBFS RMS, "
          f"peak {report['peak']:.1f} dBFS")
    print(f"  the voice stands {report['signal_to_room']:.0f} dB over the room")
    if report["limited"] > 0:
        print(f"  the loudest peaks were limited by up to {report['limited']:.1f} dB")
    if report["held_back"] > 0:
        print(f"  a peak held the level {report['held_back']:.1f} dB under -16 dBFS RMS "
              "(a popped 'b'? try a little further from the mic, or off to one side)")
    if report["signal_to_room"] < 35:
        print("  the room is loud: a quieter room or a closer mic will vocode more cleanly")
    if report["length"] > 2 * BAR:
        print(f"  longer than two bars ({2 * BAR:.1f} s): the next line would cut it off")


def click(rate, accent):
    t = np.arange(int(0.04 * rate)) / rate
    pitch = 1760 if accent else 1320
    return 0.3 * np.sin(2 * np.pi * pitch * t) * np.exp(-t / 0.008)


def count_in(sd, rate):
    beat = 60 / BPM
    bar = np.zeros(int(4 * beat * rate))
    for i in range(4):
        tone = click(rate, i == 0)
        at = int(i * beat * rate)
        bar[at:at + len(tone)] += tone
    sd.play(bar, rate)
    for i in range(4, 0, -1):
        print(f"  {i}", end="", flush=True)
        time.sleep(beat)
    sd.wait()
    print("  speak")


def record(device):
    import sounddevice as sd

    info = sd.query_devices(device, "input")
    print(f"Recording from {info['name']}.")
    print('The line: "Hello. I am Soba. I sing in sines."')
    input("Press Enter, then say it on the bar after the count-in. ")
    count_in(sd, OUT_RATE)
    # An interface's mic may be on either of its first two inputs: record
    # both and keep the louder
    channels = min(2, info["max_input_channels"])
    take = sd.rec(int(TAKE_SECONDS * OUT_RATE), OUT_RATE, channels=channels, device=device)
    sd.wait()
    return take[:, np.argmax(np.abs(take).max(axis=0))], sd


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("out", help="the WAV to write, e.g. patches/samples/soba-speaks-chris.wav")
    parser.add_argument("--from", dest="source", help="clean up this recording instead of recording")
    parser.add_argument("--device", help="the input device's name or number (default: the system's)")
    args = parser.parse_args()

    while True:
        if args.source:
            take, rate = sf.read(args.source, always_2d=False)
            sd = None
        else:
            take, sd = record(int(args.device) if args.device and args.device.isdigit() else args.device)
            rate = OUT_RATE
            # Kept as recorded, so a take can be cleaned up again with --from
            raw = Path(tempfile.gettempdir()) / f"soba-take-{time.strftime('%H%M%S')}.wav"
            sf.write(raw, take, OUT_RATE, subtype="FLOAT")
            print(f"  as recorded: {raw}")
        try:
            ready, report = prepare(np.asarray(take, dtype=np.float64), rate)
        except ValueError as e:
            sys.exit(f"{args.out}: {e}")
        if sd is None:
            break
        describe("The take", report)
        sd.play(ready, OUT_RATE)
        sd.wait()
        if input("Keep it? [Y/n] ").strip().lower() not in ("n", "no"):
            break

    sf.write(args.out, ready, OUT_RATE, subtype="PCM_16")
    describe(args.out, report)


if __name__ == "__main__":
    main()
