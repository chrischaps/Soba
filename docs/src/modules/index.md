# Module Overview

Soba has 40 modules in six categories. Each category has its own header color, so you can tell what a module is for at a glance across a crowded patch. The categories match the right-click menu and the quick-add palette, and the modules are listed here in the order the menu shows them.

The **ID** is the name a patch file uses to refer to the module.

## Categories

### Source

<span class="swatch bar source"></span>Blue header. Modules that start a sound or a note: the oscillator, the noise source, the drum voice, the sampler and the chopper, the audio input, and the modules that turn your playing into pitch and gate signals.

| Module | ID | What it does |
|--------|----|--------------|
| [Oscillator](./sources/oscillator.md) | `osc.sine` | Band-limited VCO with a tune section, hard sync, through-zero FM, a sub-oscillator and unison |
| [Noise](./sources/noise.md) | `source.noise` | White, pink and brown noise, and a smooth random voltage |
| [Drum](./sources/drum.md) | `source.drum` | An analog-style drum voice: kick, snare, tom, clap, hats, cymbal, rim, cowbell or shaker, with accent and choke |
| [Sampler](./sources/sampler.md) | `source.sampler` | Plays a WAV, MP3 or Ogg file: one-shots from a gate, an instrument across the keyboard, or its slices by note |
| [Chopper](./sources/chopper.md) | `source.chopper` | Plays a sliced break in time with the Clock, at any tempo, rearranged step by step |
| [Audio Input](./sources/audio-input.md) | `source.audio_input` | A microphone, guitar or line input, with an envelope follower and a gate |
| [Keyboard](./midi/keyboard.md) | `input.keyboard` | Play notes from your computer keyboard |
| [MIDI Note](./midi/midi-note.md) | `input.midi_note` | One voice of pitch, gate, velocity and aftertouch from a MIDI controller |
| [Poly MIDI](./midi/poly-midi.md) | `input.poly_midi` | Up to eight voices from a MIDI controller, for chords |

### Filter

<span class="swatch bar filter"></span>Teal header. Modules that shape a sound's tone by removing or emphasizing frequencies. Both can be [bypassed](../getting-started/interface-overview.md#the-module-menu).

| Module | ID | What it does |
|--------|----|--------------|
| [SVF Filter](./filters/svf-filter.md) | `filter.svf` | 12 dB/octave multimode filter with lowpass, highpass, bandpass and notch outputs |
| [Ladder Filter](./filters/ladder-filter.md) | `filter.ladder` | Moog-style 24 dB/octave lowpass, saturating and oversampled |

### Modulation

<span class="swatch bar modulation"></span>Orange header. Sources of movement: control signals that change other modules' parameters over time.

| Module | ID | What it does |
|--------|----|--------------|
| [ADSR Envelope](./modulation/adsr.md) | `mod.adsr` | Attack, decay, sustain and release, with exact stage times, curve shaping, velocity and CV on every stage |
| [LFO](./modulation/lfo.md) | `mod.lfo` | Low-frequency oscillator for cyclic modulation, free or locked to the beat |
| [Slope](./modulation/slope.md) | `mod.slope` | Rises and falls at set rates: slew and glide, a trigger envelope, or a cycling LFO with skew |

### Effect

<span class="swatch bar effect"></span>Cyan header. Processors for finished sound. Every effect can be [bypassed](../getting-started/interface-overview.md#the-module-menu).

| Module | ID | What it does |
|--------|----|--------------|
| [Stereo Delay](./effects/delay.md) | `fx.delay` | Echoes with feedback filtering, ping-pong, tempo sync and tape mode |
| [Reverb](./effects/reverb.md) | `fx.reverb` | Stereo feedback-delay-network reverb with a modulated tail |
| [3-Band EQ](./effects/eq.md) | `fx.eq` | Low shelf, parametric mid and high shelf |
| [Distortion](./effects/distortion.md) | `fx.distortion` | Oversampled soft clip, hard clip, wavefolder, tube and bit crush |
| [Chorus](./effects/chorus.md) | `fx.chorus` | Stereo chorus and flanger |
| [Tape](./effects/tape.md) | `fx.tape` | A tape machine for a bus: wow, flutter, saturation, age and hiss |
| [Slicer](./effects/slicer.md) | `fx.slicer` | Chops a held sound into a rhythm in time with the Clock: sixteen steps, a step filter and ping-pong |
| [Compressor](./effects/compressor.md) | `fx.compressor` | Dynamics compressor with sidechain and a gain-reduction output |
| [Vocoder](./effects/vocoder.md) | `fx.vocoder` | Puts a voice's spectrum on a synth: 8 to 24 bands, formant shift, sibilance and unvoiced noise, Freeze and carrier Flatten |

### Utility

<span class="swatch bar utility"></span>Gray header. The plumbing of a patch: levels, timing, sequencing, looping, and tools for seeing what a signal is doing.

| Module | ID | What it does |
|--------|----|--------------|
| [Clock](./modulation/clock.md) | `util.clock` | Steady gate pulses at a tempo; sets the patch tempo, and can follow MIDI clock |
| [VCA](./utilities/vca.md) | `util.vca` | Voltage-controlled amplifier: sets a signal's level from a CV |
| [Attenuverter](./utilities/attenuverter.md) | `util.attenuverter` | Scale, invert and offset a control signal |
| [Mix](./utilities/mix.md) | `util.mix` | Add up to four signals, audio or CV, into one mono sum |
| [Mixer](./utilities/mixer.md) | `util.mixer` | Four-channel stereo console with pan, width, sends, mute and solo |
| [Sample & Hold](./utilities/sample-hold.md) | `util.sample_hold` | Capture a signal's value on each trigger and hold it |
| [Quantizer](./utilities/quantizer.md) | `util.quantizer` | Snap a pitch to the nearest note of a scale |
| [Clock Divider](./utilities/divider.md) | `util.divider` | Fire once every so many clocks, and hold a gate open for a phrase |
| [Logic](./utilities/logic.md) | `util.logic` | Combine gates with AND, OR, XOR and NOT, and turn a control voltage into a gate |
| [Looper](./utilities/looper.md) | `util.looper` | Record, overdub and undo live layers, like a looper pedal, in time with the Clock |
| [Oscilloscope](./visualization/oscilloscope.md) | `util.oscilloscope` | Draw up to two signals as waveforms |
| [Dancer](./visualization/dancer.md) | `util.dancer` | A stick figure who dances to the patch, on the beat and as hard as the music is loud |
| [Step Sequencer](./utilities/sequencer.md) | `seq.step` | 16 steps of pitch, gate and velocity, in four chained patterns |
| [Chord Sequencer](./utilities/chord-sequencer.md) | `seq.chord` | 16 chords, each a root, a type and a slash bass, voice-led and played on polyphonic cables, in four patterns played in a chain |
| [Trigger Sequencer](./utilities/trigger-sequencer.md) | `seq.trigger` | Eight lanes of drum hits in four chained patterns, with accents, probability and ratchets |
| [Arranger](./utilities/arranger.md) | `seq.arranger` | A song's timeline: named sections of bars, moving eight lanes of automation by jumps, ramps and hits |
| [MIDI Monitor](./midi/midi-monitor.md) | `util.midi_monitor` | Show incoming MIDI messages |

### Output

<span class="swatch bar output"></span>Purple header. Where the patch meets your speakers.

| Module | ID | What it does |
|--------|----|--------------|
| [Audio Output](./output/audio-output.md) | `output.audio` | Stereo master output with volume, DC blocking and a peak limiter |

Header colors describe what a module *is*. The colors of its jacks and cables describe the *signals* it handles, which is a separate palette: see [Signal Types](../concepts/signal-types.md).

## Polyphonic modules

The Oscillator, Noise, Drum, both filters, the ADSR Envelope, Slope, the VCA, the Attenuverter, Mix, Sample & Hold and the Quantizer run one voice per channel when a polyphonic cable reaches them. Everything else treats a polyphonic cable as one signal. Poly MIDI and the Chord Sequencer are where polyphonic cables start. See [Polyphony](../concepts/polyphony.md).

## Common signal chains

The classic subtractive voice: an oscillator, a filter to shape its tone, and a VCA opened by an envelope.

```text
[Keyboard Pitch]     ──> [Oscillator V/Oct]
[Oscillator Out]     ──> [SVF Filter In]
[SVF Filter LowPass] ──> [VCA In]
[VCA Out]            ──> [Audio Output Mono]
[Keyboard Gate]      ──> [ADSR Gate]
[ADSR Out]           ──> [VCA CV]
```

Add space with effects after the VCA, so they process the shaped note:

```text
[VCA Out]                  ──> [Stereo Delay In L]
[Stereo Delay Out L / R]   ──> [Reverb In L / R]
[Reverb Out L / R]         ──> [Audio Output Left / Right]
```

Drive a voice from a sequence instead of a keyboard:

```text
[Clock Gate]           ──> [Step Sequencer Clock]
[Step Sequencer Pitch] ──> [Oscillator V/Oct]
[Step Sequencer Gate]  ──> [ADSR Gate]
```

## Choosing a module

| To… | Reach for |
|-----|-----------|
| Make a tone | [Oscillator](./sources/oscillator.md) |
| Make drums, wind or breath | [Noise](./sources/noise.md) through a filter and an envelope |
| Play a microphone or guitar through the patch | [Audio Input](./sources/audio-input.md) |
| Let a live sound or drum loop move or trigger the patch | [Audio Input](./sources/audio-input.md)'s **Follow** and **Gate** |
| Play notes | [Keyboard](./midi/keyboard.md), [MIDI Note](./midi/midi-note.md), or [Poly MIDI](./midi/poly-midi.md) for chords |
| Slide from note to note | **Glide** on [Keyboard](./midi/keyboard.md#glide), [MIDI Note](./midi/midi-note.md#glide) or [Poly MIDI](./midi/poly-midi.md#glide); a [Slope](./modulation/slope.md) on any pitch, a sequencer's included |
| Darken or brighten a sound | [SVF Filter](./filters/svf-filter.md) or [Ladder Filter](./filters/ladder-filter.md) |
| Give each note a shape in time | [ADSR Envelope](./modulation/adsr.md) into a [VCA](./utilities/vca.md) |
| Play a whole envelope from a short trigger | [Slope](./modulation/slope.md) |
| Smooth a stepped or jumpy signal | [Slope](./modulation/slope.md) |
| Add slow, repeating movement | [LFO](./modulation/lfo.md), or a cycling [Slope](./modulation/slope.md) for a swell that drops away |
| Tame or flip a modulation signal | [Attenuverter](./utilities/attenuverter.md) |
| Play a pattern | [Clock](./modulation/clock.md) into the [Step Sequencer](./utilities/sequencer.md) |
| Play a drum break in time, at any tempo, and rearrange it | [Clock](./modulation/clock.md) into the [Chopper](./sources/chopper.md) |
| Program a drum kit, with fills | [Clock](./modulation/clock.md) into the [Trigger Sequencer](./utilities/trigger-sequencer.md), a lane for each [Drum](./sources/drum.md) |
| Arrange a whole song: parts that enter, fade and leave | [Clock](./modulation/clock.md) into the [Arranger](./utilities/arranger.md), a lane on each [Mixer](./utilities/mixer.md) **Level** |
| Make random changes | [Noise](./sources/noise.md) into [Sample & Hold](./utilities/sample-hold.md) for steps, or its **Random** output for glides |
| Keep random notes in key | [Quantizer](./utilities/quantizer.md) |
| Combine signals | [Mix](./utilities/mix.md) |
| Place sounds left and right | [Mixer](./utilities/mixer.md) |
| Add space and depth | [Stereo Delay](./effects/delay.md), [Reverb](./effects/reverb.md), [Chorus](./effects/chorus.md) |
| Turn a pad or a drone into a rhythm | [Slicer](./effects/slicer.md) |
| Add grit or warmth | [Distortion](./effects/distortion.md), [Tape](./effects/tape.md) |
| Make a synth talk or sing | [Vocoder](./effects/vocoder.md), with a voice from a [Sampler](./sources/sampler.md) or [Audio Input](./sources/audio-input.md) |
| Balance tone and dynamics | [3-Band EQ](./effects/eq.md), [Compressor](./effects/compressor.md) |
| See a signal | [Oscilloscope](./visualization/oscilloscope.md), [MIDI Monitor](./midi/midi-monitor.md) |
| Watch the music dance | [Dancer](./visualization/dancer.md) |
| Hear the result | [Audio Output](./output/audio-output.md) |
