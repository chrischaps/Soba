# Pad Kit

A drum kit you play with your fingers, on a pad controller. A group of eight [Pads](../modules/midi/pad.md), one for each of the controller's pads, strikes five [Drums](../modules/sources/drum.md): kick, snare, clap, tom and closed hat. Each drum sounds as hard as its pad was hit. A Mixer and a little room finish it. The patch is 16 modules, eight of them the pads.

> **Load it:** choose **📚 Examples → Pad Kit** in the toolbar, pick your controller in **MIDI In**, press **▶ Play** and hit the pads. No controller? Click the pads on the face of the **Pads** group: toward the top of a pad is a hard hit.
> The patch file is [`patches/pad-kit.json`](https://github.com/chrischaps/Soba/blob/master/patches/pad-kit.json).

## What it teaches

- **Pads as gates.** Each pad is a [Pad](../modules/midi/pad.md) module with its own **Gate**, so a pad is a trigger like a sequencer step, ready for anything with a **Trig** or a **Gate** input.
- **A controller as a group.** Eight Pads in a group are the controller's pads, with a **Gate** and a **Vel** jack for each one the kit uses. Open the group to see them, two rows of four as the pads sit.
- **Velocity for each drum.** Each Pad's **Velocity**, out through its group's **Vel** jack, carries how hard it was struck, and changes on the very sample its gate rises. Patched into its Drum's **Accent**, it gives the drum its own hit: a ghost note is soft and dark, a backbeat full.
- **Notes that belong to the pads.** The pads send notes. The Pads claim them, so a [Note In](../modules/midi/note-in.md) on Omni added to this patch for a bassline won't also play the drums' notes as pitches.
- **Learning a controller.** The Pads hear the General MIDI drum notes, 36 to 43 on channel 10, which most pad controllers send out of the box. If yours sends something else, right-click the group, choose **Learn in order** and hit your pads from the first: each Pad takes the next.

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

Pads 2, 5 and 8 are free. Add a Drum, set its **Type** (Rim on pad 2 is GM's side stick), and patch the pad's **Gate** into its **Trig** and its **Velocity** into its **Accent**: inside the group, right-click each and choose **Show on group** to give it a jack.

## Modules

| Module | Settings |
|--------|----------|
| Eight [Pads](../modules/midi/pad.md), in a group | **Mode** Momentary; notes 36 – 43 on channel 10, one each |
| [Drum](../modules/sources/drum.md) (kick) | **Type** Kick, **Tune** -2 st, **Decay** 80%, **Tone** 30%, **Snap** 35%, **Level** 78% |
| [Drum](../modules/sources/drum.md) (snare) | **Type** Snare, **Decay** 50%, **Tone** 48%, **Snap** 45%, **Level** 85% |
| [Drum](../modules/sources/drum.md) (clap) | **Type** Clap, **Decay** 45%, **Tone** 45%, **Snap** 35%, **Level** 80% |
| [Drum](../modules/sources/drum.md) (tom) | **Type** Tom, **Tune** -3 st, **Decay** 50%, **Tone** 30%, **Snap** 30%, **Level** 75% |
| [Drum](../modules/sources/drum.md) (closed hat) | **Type** Closed Hat, **Decay** 40%, **Tone** 50%, **Snap** 35%, **Level** 75% |
| [Mixer](../modules/utilities/mixer.md) | Snare 100% centre, clap 90% at L 15, tom 70% at L 30, hat 90% at R 25; the kick on **Return L**; **Master** +6 dB |
| [Reverb](../modules/effects/reverb.md) | **Size** 32%, **Decay** 0.6 s, **Damp** 65%, **PreD** 8 ms, **Mix** 16% |
| [Audio Output](../modules/output/audio-output.md) | **Vol** 86% |

The kit is tuned warm rather than bright. **Snap** sits low on every drum, so a hit is a thud and a crack, not a click, and the kick and tom are tuned down a little for weight. A hard hit is still louder and brighter than a soft one: that's **Accent**, which each pad's Velocity sets.

## How it's built

```text
[Pads Gate 1, Vel 1] ──> [Kick Trig, Accent]
[Pads Gate 3, Vel 3] ──> [Snare Trig, Accent]
[Pads Gate 4, Vel 4] ──> [Clap Trig, Accent]
[Pads Gate 6, Vel 6] ──> [Tom Trig, Accent]
[Pads Gate 7, Vel 7] ──> [Closed Hat Trig, Accent]
[each Drum Out] ──> [Mixer] ──> [Reverb] ──> [Audio Output]
```

**Gates.** In Momentary mode, a pad's gate is high while the pad is held. A Drum only cares when its trigger rises, so how long you hold a pad doesn't matter: each hit lasts as long as the drum's **Decay**. Hitting a pad again while it's held still strikes the drum, since the gate drops for one sample first.

**Velocity.** Each drum hears its own pad's **Vel**. A Drum reads its Accent at the moment its trigger rises, and the Pad's Velocity changes on that same sample, so a kick and a snare hit at once each get their own, on the same sample.

Inside the group, each Pad's **Gate** and **Velocity** run to the group's **Outputs**, and its purple **MIDI** jack hangs from the group's **MIDI** input. Plug a [MIDI In](../modules/midi/midi-in.md) into that and all eight Pads hear the controller it names.

## Try this

- **Add a closed hat choke.** Add an Open Hat Drum on pad 8, and patch pad 7's **Gate** into its **Choke** too, so the closed hat cuts it short.
- **Latch a drone.** Open the group, set a free Pad's **Mode** to Latch and patch its gate into a VCA under a held note: one hit starts it, the next stops it. Every Pad has its own Mode, so the drums play on beside it.
- **Pressure.** If your pads send aftertouch, put an [SVF Filter](../modules/filters/svf-filter.md) between the tom and the Mixer and patch the tom Pad's **Pressure** into its **Cutoff**: hold the tom's pad and lean on it to open the filter as the tom rings.
- **Drums and keys together.** Add a Note In and a synth voice. With **Ch** on Omni the keys play the synth while the pads play the drums, on one controller.

## Related

- [Pad](../modules/midi/pad.md) – learning a pad, the three modes, Pads ×8 and claimed notes
- [Drum Machine](./drum-machine.md) – the same Drums, played by sequencers
- [Drum](../modules/sources/drum.md) – what each drum's knobs do
