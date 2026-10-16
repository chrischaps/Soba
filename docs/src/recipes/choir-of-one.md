# Choir of One

One singer, recorded once, singing twelve syllables on a single note. This patch turns that singer into a choir. A [Cloud](../modules/sources/cloud.md) breaks the recording into grains a quarter of a second long, and a [Chord Sequencer](../modules/utilities/chord-sequencer.md) sends four voices down one cable, so the Cloud grows a cloud of grains for each: Dmaj9, Bm9, Gmaj9 and Asus4, two bars each. A slow LFO drifts through the syllables, so the choir's vowel keeps changing, and nobody has to breathe.

> **Load it:** choose **📚 Examples → Choir of One** in the toolbar and press **▶ Play**. It needs no keyboard.
> The patch file is [`patches/choir-of-one.json`](https://github.com/chrischaps/Soba/blob/master/patches/choir-of-one.json), and its singer is Canopy's [`patches/samples/canopy-chops.wav`](https://github.com/chrischaps/Soba/blob/master/patches/samples/canopy-chops.wav).

<iframe class="patch-embed" src="../play/?patch=choir-of-one" title="Choir of One, playable in the browser" loading="lazy"></iframe>

*Or play it here: press **▶ Play** in the corner. The full app is a click away under **Open in Soba**. The chords are in the amber frame, the drift in the violet one and the Cloud in the rose one. Watch its sparks: four clouds of them, gathered round the line where Position is, drifting along the syllables as the LFO turns.*

## What it teaches

- **Granular time.** A grain is a short read of a recording under a smooth window. Played over and over from about the same place, overlapping, it holds a moment of sound for as long as you want it, without slowing anything down.
- **Pitch from the grains.** Each grain is read faster or slower, tape-style, so the same syllable sings any note. The recording's own C5 becomes every note of every chord.
- **A cloud per voice.** A polyphonic cable carries a chord as one channel per note. The Cloud grows a cloud for each channel, out of one pool of grains.
- **Spray.** A little scatter in where and when grains start is what turns a buzzing loop into a choir. None at all, and you hear the grains as a pulse.

## How it's patched

```text
[Clock Gate] ──> [Clock Divider Clock]
[Clock Reset] ──> [Clock Divider Reset]  [Chord Sequencer Reset]
[Clock Divider Trig] ──> [Chord Sequencer Clock]   (every two bars)
[Chord Sequencer Pitch] ──> [Cloud Pitch]          (four voices on one cable)
[LFO Out] ──> [Attenuverter In]
[Attenuverter Out] ──> [Cloud Position]            (the drift)
[Cloud L / R] ──> [Reverb In L / R] ──> [Audio Output Left / Right]
```

The Clock ticks once a beat, at 60 BPM, and the Clock Divider counts eight beats to each chord. The Chord Sequencer voices each chord in four notes and moves each note as little as it can to the next chord, so the choir's parts glide rather than jump.

The syllables were sung on C5. The Cloud plays them as recorded at 0 V, and the Chord Sequencer's C4 is 0 V, so the choir sounds an octave above where its chords are written: from about D3 up to D5.

The LFO takes 40 seconds to go round. The Attenuverter turns it down to a third, so Position wanders between about 18% and 82% of the file: through most of the syllables, but never quite to either end.

## Modules

| Module | Settings |
|--------|----------|
| [Clock](../modules/modulation/clock.md) | **Tempo** 60 BPM, **Div** 1/4 |
| [Clock Divider](../modules/utilities/divider.md) | **Div** 8: a chord every two bars |
| [Chord Sequencer](../modules/utilities/chord-sequencer.md) | Dmaj9 (D2), Bm9 (B1), Gmaj9 (G1), Asus4 (A1). **Voices** 4, **Range** 2, **Voice lead** on, **Gate** 100% |
| [LFO](../modules/modulation/lfo.md) | **Rate** 0.025 Hz, **Wave** Sine, **Bipolar** on |
| [Attenuverter](../modules/utilities/attenuverter.md) | **Amt** 0.32 |
| [Cloud](../modules/sources/cloud.md) | `canopy-chops.wav`. **Pos** 50%, **Size** 240 ms, **Dens** 14 /s, **Spray** 12%, **Pitch** 0, **Spread** 70%, **Level** 80% |
| [Reverb](../modules/effects/reverb.md) | **Size** 90%, **Decay** 7 s, **PreD** 40 ms, **Mix** 40% |

## Play with it

1. **Blur it.** Turn **Spray** up past 50%. Grains come from all over the file at once, and the syllables smear into one long breath.
2. **Shimmer.** Turn **Size** down to 15 ms and **Dens** up to 120. The choir turns to glass.
3. **Hear the grains.** Turn **Spray** to 0 and **Dens** down to 4. Each grain is a separate syllable now, four voices singing one together, four times a second.
4. **Freeze yourself.** Patch an [Audio Input](../modules/sources/audio-input.md) into **In L**, set **Pos** to 95% and sing a long note. Click **Freeze**, and the choir sings what you just sang.
5. **In time.** Patch the Clock's **Gate** into **Trig**: the cloud now starts a burst of grains on every beat.
