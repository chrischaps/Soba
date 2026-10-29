# Signal Types

Every jack in Soba has a signal type, and every type has a color. The color of a jack tells you what it sends or expects, and a cable takes the color of the output it comes from, so you can read a patch's signal flow at a glance.

| Type | Color | Range | Carries |
|------|-------|-------|---------|
| **Audio** | <span class="swatch audio"></span>Blue | −1.0 to 1.0 | Sound |
| **Control** | <span class="swatch control"></span>Orange | 0.0 to 1.0, or −1.0 to 1.0 | Modulation and pitch (CV) |
| **Gate** | <span class="swatch gate"></span>Green | 0.0 or 1.0 | Notes, clock pulses, triggers |
| **MIDI** | <span class="swatch midi"></span>Purple | A device and a channel | Which controller a module hears, from a [MIDI In](../modules/midi/midi-in.md); see [MIDI](#midi) |
| **Bus** | <span class="swatch bus"></span>Pale steel | A whole stereo mix and its sends | A Mixer's **Chain Out** into the next Mixer's **Chain In**; see [Bus](#bus) |

Underneath, audio, control and gate signals are all the same thing: a stream of numbers at the audio sample rate. The type describes what the numbers mean, and it decides which jacks a cable may connect (see [Which types connect](#which-types-connect)).

## Audio

Audio is the sound itself: a waveform swinging between −1.0 and 1.0, at your audio device's sample rate (typically 44.1 or 48 kHz). Oscillators make it; filters, the VCA, the Mix, the Mixer and the effects shape it; the [Audio Output](../modules/output/audio-output.md) sends it to your speakers.

Keep audio within ±1.0 and it passes through the output untouched. Louder than that and the output stage's limiter, on by default, catches the peaks before they clip. It's a safety net, though, not a mixing tool. When several voices or oscillators add up, bring the level down with a VCA or the [Mixer](../modules/utilities/mixer.md).

## Control

Control signals, often called CV (control voltage), move a parameter instead of making a sound. They come from envelopes, LFOs, the sequencer and Note In, and they go into jacks such as a filter's **Cutoff** or an oscillator's **FM**.

A control signal is either:

- **Unipolar**, from 0.0 to 1.0. An envelope is unipolar: it rises from nothing to its peak and falls back.
- **Bipolar**, from −1.0 to 1.0. An LFO in bipolar mode swings both ways around the center, which suits vibrato or a sweep around a set cutoff.

Most control signals change slowly, but nothing stops them running at audio rate. Patch an oscillator into another oscillator's **FM** input and you have FM synthesis.

### Pitch: 1 per octave

Pitch travels as a control signal on a 1-per-octave scale, the digital version of the 1 V/octave standard in hardware modular. Each 1.0 is an octave:

| Pitch CV | Note (with the oscillator's tune knobs at 0) |
|----------|-----------|
| −1.0 | C3 |
| 0.0 | C4 (261.63 Hz) |
| 0.5 | F♯4 |
| 1.0 | C5 |
| 2.0 | C6 |

One semitone is 1/12. The **Pitch** outputs of [Note In](../modules/midi/note-in.md) and the [Step Sequencer](../modules/utilities/sequencer.md) both use this scale, so they play an [Oscillator](../modules/sources/oscillator.md) in tune with nothing to calibrate. Both filters take their **Cutoff** CV on the same scale, so a pitch cable patched into Cutoff makes the filter track the keyboard exactly.

## Gate

A gate is either on (1.0) or off (0.0). It says *when*: a key held down, a step in a sequence, a tick of the clock. The [ADSR Envelope](../modules/modulation/adsr.md), for example, starts its attack the moment its **Gate** goes on (the rising edge) and starts its release the moment it goes off (the falling edge).

A short gate is often called a trigger. Modules that only care about the moment a gate begins, such as the clock input of the Step Sequencer or the **Trig** input of [Sample & Hold](../modules/utilities/sample-hold.md), treat both the same way.

Gates come from the **Gate** outputs of Note In, the [Clock](../modules/modulation/clock.md) and the Step Sequencer (which also has an **EOC**, end-of-cycle, gate).

## MIDI

MIDI is the purple signal type. A purple cable says which controller a module hears: it comes from a [MIDI In](../modules/midi/midi-in.md), which names a device and a channel, and goes into the **MIDI** input, the purple jack on the title bar, of a module that listens for MIDI: [Note In](../modules/midi/note-in.md), [Pad](../modules/midi/pad.md), [Control](../modules/midi/control.md), the [Clock](../modules/modulation/clock.md) or the [MIDI Monitor](../modules/midi/midi-monitor.md). Patched, the module hears the MIDI In's device and channel, whatever its own **Device** says. Unpatched, it hears its own Device, as it always has.

The cable doesn't carry the notes themselves. MIDI from your controller still goes straight into the modules that listen for it; the cable only chooses where from. It also runs through a group's jacks, so a group of Pads can have one purple jack, and the controller plugged into it reaches every pad inside. Note In turns notes into ordinary **Pitch**, **Gate** and **Velocity** signals that the rest of the patch understands, and does the same for your computer keyboard.

## Bus

Bus is the pale steel signal type. One Bus cable carries a [Mixer](../modules/utilities/mixer.md)'s whole mix and its sends from its **Chain Out** to the next Mixer's **Chain In**, so Mixers can be chained for more than four channels. It connects only to another Bus: no audio, control or gate jack takes it, and a Bus jack takes nothing else.

## Which types connect

Same-type connections always work. A few cross-type connections work too, and the rest are refused:

| From | To | Allowed? | Why |
|------|----|----------|-----|
| Audio | Control | Yes | Audio-rate modulation, such as FM |
| Control | Audio | Yes | Mix or process a CV like audio, such as an LFO into the [Mix](../modules/utilities/mix.md) |
| Gate | Control | Yes | Use a gate as a 0-or-1 modulation signal |
| Gate | Audio | No | A gate needs an envelope or VCA to become sound |
| Audio or Control | Gate | No | Gate inputs only take gates |
| MIDI | Anything else | No | A MIDI cable chooses a device; it carries no signal |

Allowed cross-type connections pass the signal through unchanged. An audio cable into a control input is the raw waveform; a gate into a control input is exactly 0.0 or 1.0.

A jack that can't take the cable you're dragging shows it before you let go: the ring round it breaks into two halves, one in the cable's colour and one in the jack's, that don't meet. Drop the cable there anyway and a small tag beside the jack explains why, for example *Control cannot connect to Gate*, as does the status bar at the bottom of the window.

## Signal color and category color

Don't confuse the two palettes. Jacks and cables are colored by **signal type**, as above. The bar across the top of each module is colored by the module's **category**: blue for Sources, teal for Filters, and so on. See the [Module Overview](../modules/index.md#categories).

## See also

- [Connections](./connections.md): patching, and reading the signal in a cable
- [Polyphony](./polyphony.md): cables that carry up to eight voices
