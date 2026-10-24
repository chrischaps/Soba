# Pad Kit

A drum kit you play with your fingers, on a pad controller. A [Pads](../modules/midi/pads.md) module hears the controller's eight pads, and five of them strike a [Drum](../modules/sources/drum.md) each: kick, snare, clap, tom and closed hat. Each drum sounds as hard as its pad was hit. A Mixer and a little room finish it. The patch is 9 modules.

> **Load it:** choose **📚 Examples → Pad Kit** in the toolbar, pick your controller in **MIDI In**, press **▶ Play** and hit the pads. No controller? Click the pads on the module's face: toward the top of a pad is a hard hit.
> The patch file is [`patches/pad-kit.json`](https://github.com/chrischaps/Soba/blob/master/patches/pad-kit.json).

## What it teaches

- **Pads as gates.** Each pad has its own **Gate**, so a pad is a trigger like a sequencer step, ready for anything with a **Trig** or a **Gate** input.
- **One cable of velocity for the whole kit.** **Hit** carries how hard the latest pad was struck, and changes on the very sample that pad's gate rises. Patched into every Drum's **Accent**, it gives each drum its own hit: a ghost note is soft and dark, a backbeat full.
- **Notes that belong to the pads.** The pads send notes. Pads claims them, so a [Note In](../modules/midi/note-in.md) on Omni added to this patch for a bassline won't also play the drums' notes as pitches.
- **Learning a controller.** Pads hears the General MIDI drum notes, 36 to 43 on channel 10, which most pad controllers send out of the box. If yours sends something else, click **learn** on the face and hit your first pad.

## The pads

The drums sit where General MIDI puts them, so a drum part written for any GM kit plays the same drums here.

| Pad | Note | Drum |
|-----|------|------|
| 1 | 36 (C2) | Kick |
| 2 | 37 (C♯2) | (free) |
| 3 | 38 (D2) | Snare |
| 4 | 39 (D♯2) | Clap |
| 5 | 40 (E2) | (free) |
| 6 | 41 (F2) | Tom |
| 7 | 42 (F♯2) | Closed hat |
| 8 | 43 (G2) | (free) |

Pads 2, 5 and 8 are free. Add a Drum, set its **Type** (Rim on pad 2 is GM's side stick), and patch the pad's **Gate** into its **Trig** and **Hit** into its **Accent**.

## Modules

| Module | Settings |
|--------|----------|
| [Pads](../modules/midi/pads.md) | **Mode** Momentary; notes 36 – 43 on channel 10 |
| [Drum](../modules/sources/drum.md) (kick) | **Type** Kick, **Tune** -2 st, **Decay** 80%, **Tone** 30%, **Snap** 35%, **Level** 78% |
| [Drum](../modules/sources/drum.md) (snare) | **Type** Snare, **Decay** 50%, **Tone** 48%, **Snap** 45%, **Level** 85% |
| [Drum](../modules/sources/drum.md) (clap) | **Type** Clap, **Decay** 45%, **Tone** 45%, **Snap** 35%, **Level** 80% |
| [Drum](../modules/sources/drum.md) (tom) | **Type** Tom, **Tune** -3 st, **Decay** 50%, **Tone** 30%, **Snap** 30%, **Level** 75% |
| [Drum](../modules/sources/drum.md) (closed hat) | **Type** Closed Hat, **Decay** 40%, **Tone** 50%, **Snap** 35%, **Level** 75% |
| [Mixer](../modules/utilities/mixer.md) | Snare 100% centre, clap 90% at L 15, tom 70% at L 30, hat 90% at R 25; the kick on **Return L**; **Master** +6 dB |
| [Reverb](../modules/effects/reverb.md) | **Size** 32%, **Decay** 0.6 s, **Damp** 65%, **PreD** 8 ms, **Mix** 16% |
| [Audio Output](../modules/output/audio-output.md) | **Vol** 86% |

The kit is tuned warm rather than bright. **Snap** sits low on every drum, so a hit is a thud and a crack, not a click, and the kick and tom are tuned down a little for weight. A hard hit is still louder and brighter than a soft one: that's **Accent**, which Hit sets.

## How it's built

```text
[Pads Gate 1] ──> [Kick Trig]
[Pads Gate 3] ──> [Snare Trig]
[Pads Gate 4] ──> [Clap Trig]
[Pads Gate 6] ──> [Tom Trig]
[Pads Gate 7] ──> [Closed Hat Trig]
[Pads Hit]    ──> [every Drum's Accent]
[each Drum Out] ──> [Mixer] ──> [Reverb] ──> [Audio Output]
```

**Gates.** In Momentary mode, a pad's gate is high while the pad is held. A Drum only cares when its trigger rises, so how long you hold a pad doesn't matter: each hit lasts as long as the drum's **Decay**. Hitting a pad again while it's held still strikes the drum, since the gate drops for one sample first.

**Hit.** One cable from **Hit** fans out to all five **Accent** inputs. A Drum reads its Accent at the moment its trigger rises, and Hit has just changed to that pad's velocity, so every drum hears its own. Two pads struck together are struck a sample apart (about 20 µs, far too short to hear), so a kick and a snare hit at once each still get their own.

**Velocity** carries the same thing differently: one channel per pad, on a polyphonic cable. That suits a polyphonic module, which plays a voice per channel. For a kit of separate Drums, Hit is the cable to use.

## Try this

- **Add a closed hat choke.** Add an Open Hat Drum on pad 8, and patch pad 7's **Gate** into its **Choke** too, so the closed hat cuts it short.
- **Latch a drone.** Set **Mode** to Latch and patch a free pad's gate into a VCA under a held note: one hit starts it, the next stops it. (Mode is for the whole module: put a second Pads module in the patch for a mix of modes.)
- **Pressure.** If your pads send aftertouch, put an [SVF Filter](../modules/filters/svf-filter.md) between the tom and the Mixer and patch **Pressure** into its **Cutoff**: hold the tom's pad and lean on it to open the filter as the tom rings.
- **Drums and keys together.** Add a Note In and a synth voice. With **Ch** on Omni the keys play the synth while the pads play the drums, on one controller.

## Related

- [Pads](../modules/midi/pads.md) – learning the pads, the three modes and claimed notes
- [Drum Machine](./drum-machine.md) – the same Drums, played by sequencers
- [Drum](../modules/sources/drum.md) – what each drum's knobs do
