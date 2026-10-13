"""Sings Canopy's vocal chops: twelve short syllables on one note, a slot each.

    python tools/voice/chops.py patches/samples/canopy-chops.wav

A lighter voice than Held Vowels' (formants raised by FORMANT_SCALE, as a
woman's are) on C5, so the Sampler's Root is 72 and the chops can be played
a few semitones either side without turning to chipmunk or treacle. Each
syllable sits at the head of a SLOT-second slot: syllable k starts at k/12 of
the file, so a Step Sequencer's Pitch (note 60 + k) picks it through Start.

Uses tools/voice/speak.py's Klatt resonators and glottal pulse."""
import os
import sys

import numpy as np
from scipy.io import wavfile
from scipy.signal import butter, resample_poly, sosfilt

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import speak  # noqa: E402

RATE = speak.RATE
OUT_RATE = 22050
SLOT = 8.0 / 12        # twelve slots in eight seconds: note 60 + k is k/12, slot k
PITCH = 523.25           # C5
FORMANT_SCALE = 1.17

# Formants (F1, F2, F3, B1, B2, B3)
V = {
    'ah': (730, 1090, 2440, 90, 110, 170),
    'oh': (570, 840, 2410, 80, 100, 160),
    'oo': (300, 870, 2240, 60, 90, 150),
    'ee': (270, 2290, 3010, 60, 110, 180),
    'eh': (530, 1840, 2480, 70, 100, 170),
    'ih': (390, 1990, 2550, 60, 100, 170),
    'm':  (480, 1270, 2130, 40, 200, 200),
    'l':  (310, 1050, 2880, 50, 100, 280),
    'n':  (480, 1340, 2470, 40, 300, 300),
}

# Each syllable: a list of (shape, seconds, voicing, aspiration, pitch bend in semitones).
# Shapes glide into each other over GLIDE ms.
SYLLABLES = [
    ('ah',    [('ah', 0.40, 1, 0, 0)]),
    ('oh',    [('oh', 0.40, 1, 0, 0)]),
    ('ya',    [('ee', 0.07, 1, 0, -1), ('ah', 0.36, 1, 0, 0)]),
    ('hey',   [('eh', 0.05, 0, 1, 0), ('eh', 0.18, 1, 0, 0), ('ih', 0.20, 1, 0, 0)]),
    ('mah',   [('m', 0.10, 0.7, 0, -1), ('ah', 0.36, 1, 0, 0)]),
    ('lah',   [('l', 0.07, 0.8, 0, 0), ('ah', 0.36, 1, 0, 0)]),
    ('ooh',   [('oo', 0.58, 1, 0, 0)]),
    ('ah-oh', [('ah', 0.24, 1, 0, 0), ('oh', 0.34, 1, 0, -2)]),
    ('nah',   [('n', 0.08, 0.7, 0, 0), ('ah', 0.36, 1, 0, 0)]),
    ('ee',    [('ee', 0.40, 1, 0, 0)]),
    ('mm',    [('m', 0.45, 0.8, 0, 0)]),
    ('oo-ah', [('oo', 0.22, 1, 0, -1), ('ah', 0.36, 1, 0, 0)]),
]
GLIDE = 45


def syllable(parts):
    n = int(RATE * SLOT)
    t = np.arange(n) / RATE
    formants = np.zeros((6, n))
    voice = np.zeros(n)
    asp = np.zeros(n)
    bend = np.zeros(n)
    at = 0
    for shape, secs, av, ah, st in parts:
        k = int(RATE * secs)
        f = np.array(V[shape], float)
        f[:3] *= FORMANT_SCALE
        formants[:, at:at + k] = f[:, None]
        voice[at:at + k] = av
        asp[at:at + k] = ah
        bend[at:at + k] = st
        at += k
    end = at
    formants[:, end:] = formants[:, end - 1][:, None]
    bend[end:] = bend[end - 1]
    formants = np.vstack([speak.smooth(f, GLIDE) for f in formants])
    bend = speak.smooth(bend, 60)
    # Soft edges: a 25 ms swell, and a 120 ms fall at the end of the sound
    rise = 0.5 - 0.5 * np.cos(np.pi * np.clip(t / 0.025, 0, 1))
    fall = 0.5 + 0.5 * np.cos(np.pi * np.clip((t - end / RATE + 0.12) / 0.12, 0, 1))
    voice = speak.smooth(voice, 10) * rise * fall
    asp = speak.smooth(asp, 10) * rise
    vib = np.clip((t - 0.3) / 0.3, 0, 1)
    drift = speak.smooth(np.random.default_rng(7).standard_normal(n), 40.0)
    pitch = PITCH * 2 ** (bend / 12) * (1 + 0.003 * drift / drift.std() + 0.009 * vib * np.sin(2 * np.pi * 5.4 * t))

    noise = np.random.default_rng(17).uniform(-1, 1, n)
    pulses = speak.glottal(pitch)
    breath = noise * (0.5 + 0.5 * (np.cumsum(pitch / RATE) % 1.0 < 0.5))
    source = voice * (pulses + 0.07 * breath) + 0.3 * asp * breath
    ones = np.ones(n)
    tract = speak.resonate(source, 7000.0 * ones, 1600.0 * ones)
    tract = speak.resonate(tract, 5200.0 * ones, 1000.0 * ones)
    tract = speak.resonate(tract, 4300.0 * ones, 300.0 * ones)
    tract = speak.resonate(tract, 3700.0 * ones, 250.0 * ones)
    for k in (2, 1, 0):
        tract = speak.resonate(tract, formants[k], formants[k + 3])
    presence = sosfilt(butter(2, 3800.0, btype='highpass', fs=RATE, output='sos'), pulses + 0.4 * breath)
    out = tract + 0.05 * (voice + asp) * presence
    out = resample_poly(out, 1, RATE // OUT_RATE)
    # Each at -15 dBFS RMS where it's sounding
    loud = np.abs(out) > 0.05 * np.abs(out).max()
    return out * 10 ** (-15 / 20) / np.sqrt(np.mean(out[loud] ** 2))


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else 'canopy-chops.wav'
    audio = np.concatenate([syllable(p) for _, p in SYLLABLES])
    peak = np.abs(audio).max()
    if peak > 10 ** (-1 / 20):
        audio *= 10 ** (-1 / 20) / peak
    wavfile.write(out, OUT_RATE, np.round(audio * 32767).astype(np.int16))
    print(f'{out}: {len(audio) / OUT_RATE:.2f} s, {len(SYLLABLES)} syllables, '
          f'peak {20 * np.log10(np.abs(audio).max()):.1f} dBFS')


if __name__ == '__main__':
    main()
