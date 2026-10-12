"""Speaks the Vocoder example's line with a small formant synthesizer.

    python tools/voice/speak.py patches/samples/soba-speaks.wav
    python tools/voice/speak.py --vowels patches/samples/held-vowels.wav

"Hello. I am Soba. I sing in sines." With --vowels it sings Held Vowels'
line instead: "ohh", "ooh", "ee", "ahh", a bar apart. The voice is made here, from nothing
but a glottal pulse and noise, so the example ships no one's recording. It's
a Klatt-style cascade synthesizer (Klatt, "Software for a cascade/parallel
formant synthesizer", JASA 1980): a pulse train at the speaking pitch,
shaped like the airflow through the vocal folds, runs through a chain of
resonators. The lowest three are the formants, gliding from sound to sound;
the rest are fixed, as in a real tract. Nasals add a pole and a zero;
aspiration ("h") is noise through the same tract; "s" and "z" are noise
band-passed high, alongside it, and a little edge and breath above the
formants keeps the voice from going dull.

Everything runs at twice the output rate and is decimated, so the pulses'
sharp edges don't alias. The phrase is deterministic: the same file every
run.
"""

import sys

import numpy as np
from scipy.io import wavfile
from scipy.signal import butter, resample_poly, sosfilt

OUT_RATE = 44100
RATE = 2 * OUT_RATE

# Formant targets (Hz) and bandwidths, after Klatt's 1980 table for a male
# voice. A pair of tuples is a diphthong, gliding from the first to the second.
VOWELS = {
    "IH": (400, 1800, 2570, 50, 100, 140),
    "EH": (530, 1680, 2500, 60, 90, 200),
    "AE": (620, 1660, 2430, 70, 150, 320),
    "AH": (620, 1220, 2550, 80, 50, 140),
    "OW": ((540, 1100, 2300, 70, 70, 110), (450, 900, 2300, 70, 70, 110)),
    "AY": ((660, 1200, 2550, 100, 70, 200), (400, 1880, 2500, 70, 100, 200)),
}

# The sung vowels, held pure, after Peterson and Barney's 1952 averages for
# men. Sung bandwidths run a little wider than spoken ones.
SUNG_VOWELS = {
    "ahh": (730, 1090, 2440, 90, 110, 170),
    "ohh": (570, 840, 2410, 80, 100, 160),
    "ooh": (300, 870, 2240, 60, 90, 150),
    "ee": (270, 2290, 3010, 60, 110, 180),
}

# Consonants: formants, voicing, aspiration and frication levels, nasal.
#        formants and bandwidths               voice asp   fric  nasal
CONSONANTS = {
    "HH": (None,                                 0.0, 0.55, 0.0, False),
    "L":  ((310, 1050, 2880, 50, 100, 280),      0.75, 0.0, 0.0, False),
    "M":  ((480, 1270, 2130, 40, 200, 200),      0.65, 0.0, 0.0, True),
    "N":  ((480, 1340, 2470, 40, 300, 300),      0.65, 0.0, 0.0, True),
    "NG": ((480, 2000, 2900, 160, 150, 200),     0.6, 0.0, 0.0, True),
    "S":  ((320, 1390, 2530, 200, 80, 200),      0.0, 0.0, 1.0, False),
    "Z":  ((320, 1290, 2530, 70, 60, 180),       0.3, 0.0, 0.6, False),
    "B":  ((200, 900, 2100, 65, 90, 125),        0.12, 0.0, 0.0, False),
}

# The line: (sound, milliseconds, pitch at its start and end in Hz). A
# pitch of None leaves the pitch to glide through.
LINE = [
    ("_", 140, None),
    ("HH", 70, None), ("EH", 100, (128, 136)), ("L", 65, (136, 140)), ("OW", 290, (142, 104)),
    ("_", 170, None),
    ("AY", 210, (122, 134)), ("AE", 85, (132, 128)), ("M", 115, (126, 118)),
    ("_", 60, None),
    ("S", 140, None), ("OW", 175, (146, 136)), ("B", 75, (128, 126)), ("AH", 270, (124, 92)),
    ("_", 260, None),
    ("AY", 200, (118, 130)), ("S", 120, None), ("IH", 95, (140, 138)), ("NG", 100, (136, 132)),
    ("IH", 70, (131, 129)), ("N", 70, (128, 126)), ("S", 130, None),
    ("AY", 290, (134, 98)), ("N", 85, (98, 94)), ("Z", 170, (94, 90)),
    ("_", 280, None),
]

# A stop's burst: how long the closure lasts before it, in ms.
CLOSURE_MS = 58


def smooth(track, ms):
    """A moving average twice over (a triangle), `ms` wide."""
    width = max(1, int(RATE * ms / 1000))
    kernel = np.ones(width) / width
    padded = np.pad(track, width, mode="edge")
    once = np.convolve(padded, kernel, mode="same")
    return np.convolve(once, kernel, mode="same")[width:-width]


def tracks():
    """The synthesizer's parameters, sample by sample."""
    n = sum(int(RATE * ms / 1000) for _, ms, _ in LINE)
    formants = np.zeros((6, n))
    voice, aspiration, frication, burst, nasal = (np.zeros(n) for _ in range(5))
    pitch = np.full(n, np.nan)
    fric_kind = np.zeros(n)  # 1 = "s"/"z", 2 = a stop's burst

    at = 0
    for index, (sound, ms, f0) in enumerate(LINE):
        length = int(RATE * ms / 1000)
        span = slice(at, at + length)
        ramp = np.linspace(0.0, 1.0, length)
        if sound in VOWELS:
            target = VOWELS[sound]
            if isinstance(target[0], tuple):
                start, end = np.array(target[0], float), np.array(target[1], float)
                formants[:, span] = start[:, None] + (end - start)[:, None] * ramp
            else:
                formants[:, span] = np.array(target, float)[:, None]
            voice[span] = 1.0
        elif sound in CONSONANTS:
            target, av, ah, af, is_nasal = CONSONANTS[sound]
            if target is None:
                # "h" takes the shape of the vowel after it
                target = VOWELS[LINE[index + 1][0]]
            formants[:, span] = np.array(target, float)[:, None]
            voice[span] = av
            aspiration[span] = ah
            frication[span] = af
            nasal[span] = 1.0 if is_nasal else 0.0
            fric_kind[span] = 1.0 if af > 0 else 0.0
            if sound == "B":
                closure = int(RATE * CLOSURE_MS / 1000)
                burst[at + closure : at + closure + int(RATE * 0.012)] = 1.0
                voice[at + closure :] = 0.0
        else:
            # Silence keeps the shape it had, so the next sound glides from it
            formants[:, span] = formants[:, at - 1][:, None] if at else np.array(VOWELS["AH"], float)[:, None]
        if f0 is not None:
            pitch[span] = np.linspace(f0[0], f0[1], length)
        at += length

    # A neutral tract before the first sound
    first = np.argmax(formants[0] > 0)
    formants[:, :first] = formants[:, first][:, None]

    # Pitch glides through the gaps; a little drift keeps it human
    known = ~np.isnan(pitch)
    pitch = np.interp(np.arange(n), np.flatnonzero(known), pitch[known])
    rng = np.random.default_rng(3)
    drift = smooth(rng.standard_normal(n), 30.0)
    pitch = smooth(pitch, 40.0) * (1.0 + 0.012 * drift / drift.std())

    formants = np.vstack([smooth(f, 38.0) for f in formants])
    voice = smooth(voice, 14.0)
    aspiration = smooth(aspiration, 12.0)
    frication = smooth(frication, 10.0)
    burst = smooth(burst, 2.0)
    nasal = smooth(nasal, 18.0)
    return formants, pitch, voice, aspiration, frication, burst, nasal


def glottal(pitch):
    """The derivative of a Rosenberg glottal pulse (the airflow, as heard
    through the lips), one per period of `pitch`."""
    phase = np.cumsum(pitch / RATE) % 1.0
    opening, closing = 0.40, 0.16
    flow = np.where(
        phase < opening,
        0.5 * (1.0 - np.cos(np.pi * phase / opening)),
        np.where(phase < opening + closing, np.cos(0.5 * np.pi * (phase - opening) / closing), 0.0),
    )
    return np.diff(flow, prepend=0.0) * RATE / 200.0


def resonate(x, frequency, bandwidth):
    """A Klatt resonator whose frequency and bandwidth move sample by sample."""
    c = -np.exp(-2.0 * np.pi * bandwidth / RATE)
    b = 2.0 * np.exp(-np.pi * bandwidth / RATE) * np.cos(2.0 * np.pi * frequency / RATE)
    a = 1.0 - b - c
    y = np.zeros_like(x)
    y1 = y2 = 0.0
    for i in range(len(x)):
        y0 = a[i] * x[i] + b[i] * y1 + c[i] * y2
        y[i] = y0
        y2, y1 = y1, y0
    return y


def antiresonate(x, frequency, bandwidth):
    """A Klatt antiresonator: a zero where the resonator has a pole."""
    c = -np.exp(-2.0 * np.pi * bandwidth / RATE)
    b = 2.0 * np.exp(-np.pi * bandwidth / RATE) * np.cos(2.0 * np.pi * frequency / RATE)
    a = 1.0 - b - c
    y = np.zeros_like(x)
    x1 = x2 = 0.0
    for i in range(len(x)):
        y[i] = (x[i] - b[i] * x1 - c[i] * x2) / a[i]
        x2, x1 = x1, x[i]
    return y


def speak():
    formants, pitch, voice, aspiration, frication, burst, nasal = tracks()
    n = len(pitch)
    rng = np.random.default_rng(11)
    noise = rng.uniform(-1.0, 1.0, n)

    # The cascade: voicing and aspiration through the tract
    pulses = glottal(pitch)
    breath = noise * (0.5 + 0.5 * (np.cumsum(pitch / RATE) % 1.0 < 0.5))
    source = voice * pulses + 0.25 * aspiration * breath
    ones = np.ones(n)
    # Nasals: a pole and a zero that cancel until the nose opens
    tract = antiresonate(source, 270.0 + 180.0 * nasal, 100.0 * ones)
    tract = resonate(tract, 270.0 * ones, 100.0 * ones)
    # The fixed upper formants, which keep the voice from going dull up high
    tract = resonate(tract, 6500.0 * ones, 1600.0 * ones)
    tract = resonate(tract, 4900.0 * ones, 1000.0 * ones)
    tract = resonate(tract, 3850.0 * ones, 300.0 * ones)
    tract = resonate(tract, 3300.0 * ones, 250.0 * ones)
    for k in (2, 1, 0):
        tract = resonate(tract, formants[k], formants[k + 3])

    # The parallel branch: "s" and "z" high up, a stop's burst lower
    hiss = sosfilt(butter(4, [4200.0, 10500.0], btype="bandpass", fs=RATE, output="sos"), noise)
    pop = sosfilt(butter(2, [400.0, 3500.0], btype="bandpass", fs=RATE, output="sos"), noise)
    fricatives = 0.35 * frication * hiss + 0.6 * burst * pop

    # The edge and breath a real voice keeps above its formants, which a
    # cascade of resonators would roll off entirely
    presence = sosfilt(butter(2, 3500.0, btype="highpass", fs=RATE, output="sos"), pulses + 0.4 * breath)
    speech = tract + fricatives + 0.06 * voice * presence
    speech = resample_poly(speech, 1, 2)

    # A gentle fade at each end, then -16 dBFS RMS over the voiced parts
    fade = int(0.02 * OUT_RATE)
    speech[:fade] *= np.linspace(0.0, 1.0, fade)
    speech[-fade:] *= np.linspace(1.0, 0.0, fade)
    loud = np.abs(speech) > 0.05 * np.abs(speech).max()
    speech *= 10 ** (-16 / 20) / np.sqrt(np.mean(speech[loud] ** 2))
    peak = np.abs(speech).max()
    if peak > 10 ** (-1 / 20):
        speech *= 10 ** (-1 / 20) / peak
    return speech


# Held Vowels' line: one vowel a bar, each sung on the last beat before the
# chord that will hold it, on a note that chord shares with the one before.
# (vowel, pitch in Hz, dBFS RMS). The Sampler starts it on beat 4 of the
# first bar. The open vowels are sung louder, as a singer sings them, and as
# the pad needs: their first formant falls between the chord's notes, where
# a triangle has little to give, and they'd come out quieter than the rest.
SUNG_BPM = 66
SUNG_LINE = [("ohh", 146.83, -12.5), ("ooh", 123.47, -20.0), ("ee", 110.0, -20.0), ("ahh", 146.83, -16.0)]
SUNG_BEATS = 1.4  # how long each vowel lasts, past the downbeat it's held on
SUNG_RATE = OUT_RATE // 2  # vowels need nothing above 11 kHz


def sing():
    """Held Vowels' line: each vowel steady, with a singer's slow vibrato
    and soft edges, a bar apart."""
    bar = 4 * 60.0 / SUNG_BPM
    held = SUNG_BEATS * 60.0 / SUNG_BPM
    length = int(RATE * ((len(SUNG_LINE) - 1) * bar + held + 0.1))
    t = np.arange(length) / RATE
    formants = np.zeros((6, length))
    pitch = np.zeros(length)
    voice = np.zeros(length)
    vibrato = np.zeros(length)
    for k, (vowel, hz, _) in enumerate(SUNG_LINE):
        start = k * bar
        # Each vowel's shape and pitch reach on to the next, so the silence
        # between them glides nowhere
        span = (t >= start) & (t < start + bar)
        formants[:, span] = np.array(SUNG_VOWELS[vowel], float)[:, None]
        pitch[span] = hz
        local = t[span] - start
        # A 70 ms swell in and a 220 ms fall away, raised-cosine both
        rise = 0.5 - 0.5 * np.cos(np.pi * np.clip(local / 0.07, 0.0, 1.0))
        fall = 0.5 + 0.5 * np.cos(np.pi * np.clip((local - held + 0.22) / 0.22, 0.0, 1.0))
        voice[span] = rise * fall
        # Vibrato grows in after the first 250 ms, as a singer's does
        vibrato[span] = np.clip((local - 0.25) / 0.35, 0.0, 1.0)

    drift = smooth(np.random.default_rng(5).standard_normal(length), 40.0)
    pitch = pitch * (1.0 + 0.004 * drift / drift.std() + 0.011 * vibrato * np.sin(2 * np.pi * 5.3 * t))
    formants = np.vstack([smooth(f, 20.0) for f in formants])

    noise = np.random.default_rng(13).uniform(-1.0, 1.0, length)
    pulses = glottal(pitch)
    breath = noise * (0.5 + 0.5 * (np.cumsum(pitch / RATE) % 1.0 < 0.5))
    source = voice * (pulses + 0.04 * breath)
    ones = np.ones(length)
    tract = resonate(source, 6500.0 * ones, 1600.0 * ones)
    tract = resonate(tract, 4900.0 * ones, 1000.0 * ones)
    tract = resonate(tract, 3850.0 * ones, 300.0 * ones)
    tract = resonate(tract, 3300.0 * ones, 250.0 * ones)
    for k in (2, 1, 0):
        tract = resonate(tract, formants[k], formants[k + 3])
    presence = sosfilt(butter(2, 3500.0, btype="highpass", fs=RATE, output="sos"), pulses + 0.4 * breath)
    song = resample_poly(tract + 0.06 * voice * presence, 1, RATE // SUNG_RATE)

    # Each vowel at its own level, measured where it's sung
    for k, (_, _, db) in enumerate(SUNG_LINE):
        part = slice(int(k * bar * SUNG_RATE), int((k * bar + held) * SUNG_RATE))
        loud = np.abs(song[part]) > 0.05 * np.abs(song[part]).max()
        song[part] *= 10 ** (db / 20) / np.sqrt(np.mean(song[part][loud] ** 2))
    peak = np.abs(song).max()
    if peak > 10 ** (-1 / 20):
        song *= 10 ** (-1 / 20) / peak
    return song


def main():
    args = sys.argv[1:]
    if args and args[0] == "--vowels":
        out = args[1] if len(args) > 1 else "held-vowels.wav"
        audio, rate = sing(), SUNG_RATE
    else:
        out = args[0] if args else "soba-speaks.wav"
        audio, rate = speak(), OUT_RATE
    wavfile.write(out, rate, np.round(audio * 32767).astype(np.int16))
    print(f"{out}: {len(audio) / rate:.2f} s, peak {20 * np.log10(np.abs(audio).max()):.1f} dBFS")


if __name__ == "__main__":
    main()
