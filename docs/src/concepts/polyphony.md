# Polyphony

A cable in Soba can carry up to **eight channels** at once, one per voice. Patch a chord through a single chain of modules and every note gets its own oscillator, its own filter and its own envelope. The result is a chord of separate voices, each shaping itself, rather than one voice playing a blend of notes.

This is the approach VCV Rack takes. You don't build eight copies of your voice. You build one, and the cables carry the voices through it.

## Where voices come from

The [Note In](../modules/midi/note-in.md) module is where polyphony starts when you play. Its **Voices** knob (1 to 8) sets how many channels its **Pitch**, **Gate** and **Velocity** cables carry, and each note you play is given a channel of its own. It plays from your computer keyboard as well as from MIDI.

In a patch that plays itself, the [Chord Sequencer](../modules/utilities/chord-sequencer.md) starts it instead. It sends a progression's chords down the same **Pitch**, **Gate** and **Velocity** cables, one channel per note, so a voice built for Note In plays it unchanged.

## Polyphonic modules

These modules run one voice per channel of their widest input:

| Module | Each voice has its own |
|--------|-----------|
| [Oscillator](../modules/sources/oscillator.md) | Phase, sync and unison stack |
| [Noise](../modules/sources/noise.md) | Random stream, so no two voices hiss alike |
| [Drum](../modules/sources/drum.md) | Hit, so fast notes ring over each other instead of cutting each other off |
| [SVF Filter](../modules/filters/svf-filter.md) | Filter state and resonance |
| [Ladder Filter](../modules/filters/ladder-filter.md) | Filter state and resonance |
| [ADSR Envelope](../modules/modulation/adsr.md) | Stage and level |
| [Slope](../modules/modulation/slope.md) | Level, so a chord glides voice by voice |
| [VCA](../modules/utilities/vca.md) | Gain |
| [Attenuverter](../modules/utilities/attenuverter.md) | Scaling |
| [Mix](../modules/utilities/mix.md) | Sum |
| [Sample & Hold](../modules/utilities/sample-hold.md) | Held value |
| [Quantizer](../modules/utilities/quantizer.md) | Note, and its Trig, so a chord comes out as a chord in the scale |

Their outputs carry as many channels as their widest input. The knobs are shared: turning **Cutoff** moves every voice's cutoff.

## Seeing polyphony

A polyphonic cable is drawn as a **bundle of strands**, one per channel, in a dark sheath that narrows where it plugs into a jack. The bundle widens as the channel count grows, so you can tell mono from poly at a glance at any zoom.

Each strand lights with its own voice. Hold a chord and light runs along the strands of the notes you're playing while the rest stay dark. When you let go, each note's release fades down its own strand. The glow around the bundle grows with the number of voices playing, so a chord glows brighter than a single note. See [Reading the signal in a cable](./connections.md#reading-the-signal-in-a-cable).

Output labels show the channel count too: **Out ×8** means that output carries eight channels. The count comes from the running patch, so press **Play** to see it. Once shown, it stays while the transport is stopped.

## Mixing mono and poly

A **mono cable into a polyphonic module** is shared by every voice. One LFO into a polyphonic filter's **Cutoff** sweeps all the voices together. To move each voice on its own, use a polyphonic source such as the envelope or Note In's **Velocity**.

A **polyphonic cable into a mono module** is folded down to one channel:

- **Audio inputs** hear every channel summed. That's why a polyphonic voice can go straight into an effect or the Audio Output.

The [Mixer](../modules/utilities/mixer.md) is the exception. It hears each voice of a polyphonic cable apart, and its per-channel **Width** knobs fan them out across the stereo field, so a chord opens up instead of sitting in the middle. With **Width** at 0% it sums them like any mono module.
- **Control and gate inputs** take the first channel only, since adding CVs together rarely means anything useful.

Voices add up, so a four-note chord is roughly four times as loud as one note. Leave headroom with the VCA's **Level** or the Mixer.

## A polyphonic voice

```text
[Note In Pitch]      ──> [Oscillator V/Oct]
[Note In Gate]       ──> [ADSR Gate]
[Note In Velocity]   ──> [ADSR Velocity]
[Oscillator Out]     ──> [SVF Filter In]
[ADSR Out]           ──> [SVF Filter Cutoff]
[SVF Filter LowPass] ──> [VCA In]
[ADSR Out]           ──> [VCA CV]
[VCA Out]            ──> [Reverb In L]
[Reverb Out L / R]   ──> [Audio Output Left / Right]
```

Every module up to the VCA runs one copy per voice. The Reverb hears the voices summed, as a single stereo instrument.

## Cost

Each voice is a full copy of the module, so eight voices use about eight times the CPU of one. If a patch strains your computer, turn down **Voices** on the Note In module. With mono cables, a polyphonic module runs a single voice and costs no more than before.
