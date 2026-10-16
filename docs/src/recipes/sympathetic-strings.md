# Sympathetic Strings

Shout into a piano with the sustain pedal down and the piano answers: every string your voice shares a note with starts to ring. This patch is built on that. It has no oscillators. Every note you hear is a [Resonator](../modules/sources/resonator.md), a physical model of a string or a glass, and every one is set ringing by a drum.

A Shaker plays sixteenths into a chord of four strings, and they hum the chord. A Rim strikes a glass tuned to E5. A fifth string plucks a slow pentatonic line over a soft kick.

> **Load it:** choose **📚 Examples → Sympathetic Strings** in the toolbar and press **▶ Play**. It needs no keyboard.
> The patch file is [`patches/sympathetic-strings.json`](https://github.com/chrischaps/Soba/blob/master/patches/sympathetic-strings.json).

<iframe class="patch-embed" src="../play/?patch=sympathetic-strings" title="Sympathetic Strings, playable in the browser" loading="lazy"></iframe>

*Or play it here: press **▶ Play** in the corner. The full app is a click away under **Open in Soba**. Time is on the left, then the lanes and the Chord Sequencer, then the drums, then the three Resonators in the amber frame. Watch the strings swing on their faces and the glass bloom into rings on each rim hit.*

## What it teaches

- **A drum is an exciter.** Patched into a Resonator's **In**, a drum stops being a drum and becomes the hand that plays an instrument. The Resonator gives the sound only its own ringing. The shaker's hiss and the rim's click are gone, and what's left is the string and the glass.
- **Pitch from a model.** The Resonator rings at its **Tune** and **V/Oct** whatever plays it, so a drum with no pitch plays a chord.
- **A chord on one cable.** The Chord Sequencer's **Pitch** carries four notes, so the Resonator runs four strings, and one shaker rings all four.
- **Brightness is the hand.** It sets how hard the pick or the mallet is, and how much of a drum's top gets in. The strings' 60% lets the shaker's grain in softly. The plucked line's 45% is a thumb, not a pick.

## Modules

| Module | Settings |
|--------|----------|
| [Clock](../modules/modulation/clock.md) | **BPM** 92, **Div** 1/16, **Gate** 50%, **Swing** 54% |
| [Step Sequencer](../modules/utilities/sequencer.md) (kick) | Steps 1, 8 and 11 |
| [Step Sequencer](../modules/utilities/sequencer.md) (rim) | Steps 4, 7, 10, 14 and 16, at velocities from 70 to 120 |
| [Step Sequencer](../modules/utilities/sequencer.md) (shaker) | Every step. 127 on the offbeats (3, 7, 15), 100 and 80 between, and 70 where the kick lands |
| [Step Sequencer](../modules/utilities/sequencer.md) (line) | A4, C5, E5, D5, C5, A4, G4, E4 on steps 1, 3, 6, 7, 9, 11, 12 and 14 |
| [Chord Sequencer](../modules/utilities/chord-sequencer.md) | **Steps** 4: Am9, Fmaj9, Cadd9, Gsus4. **Voices** 4, **Voicing** Open, **Voice Leading** on, **Bass Voice** off. Clocked by the kick lane's **EOC**, so it changes chord each bar |
| [Drum](../modules/sources/drum.md) (kick) | **Type** Kick, **Tune** -3 st, **Decay** 70%, **Tone** 30%, **Snap** 45% |
| [Drum](../modules/sources/drum.md) (rim) | **Type** Rim, **Decay** 40%, **Tone** 60% |
| [Drum](../modules/sources/drum.md) (shaker) | **Type** Shaker, **Decay** 35%, **Tone** 10%, **Level** 100% |
| [Resonator](../modules/sources/resonator.md) (glass) | **Mode** Body, **Material** Glass, **Tune** +16 st (E5), **Decay** 55%, **Damp** 30%, **Bright** 55%, **Pos** 35% |
| [Resonator](../modules/sources/resonator.md) (strings) | **Mode** String, **Decay** 72%, **Damp** 60%, **Bright** 60%, **Pos** 22%, **Level** 100% |
| [Resonator](../modules/sources/resonator.md) (line) | **Mode** String, **Tune** -12 st, **Decay** 60%, **Damp** 45%, **Bright** 45%, **Pos** 12%, **Level** 100% |
| [Mixer](../modules/utilities/mixer.md) | Kick 50%, glass 40%, strings 100% with **Width** 60%, line 100% at L 35. **Master** +6 dB |
| [Reverb](../modules/effects/reverb.md) | **Size** 60%, **Decay** 0.7 s, **Damp** 50%, **PreD** 18 ms, **Mix** 28% |

## How it's built

### The strings that hum

```text
[Shaker lane Gate]     ──> [Drum (Shaker) Trig]
[Drum (Shaker) Out]    ──> [Resonator (strings) In]
[Chord Sequencer Pitch] ─> [Resonator (strings) V/Oct]
[Chord Sequencer Gate]  ─> [Resonator (strings) Gate]
[Chord Sequencer Velocity] ─> [Resonator (strings) Strike]
```

Every bead of the shaker goes into all four strings at once. Each string keeps only what's near its own note and its partials, and rings that on. With **Decay** at 72% a string rings for about five seconds, so the sixteenths never let it fall silent. The chord hums for as long as the shaker plays, louder on the accented offbeats.

When the chord changes, each string retunes, and the Chord Sequencer's gate gives the new chord a soft strum (velocity 38) so it starts together. **Damping** at 60% keeps the strings warm. The shaker's high grain dies in a few passes round the loop, and the notes go on.

### The glass

```text
[Rim lane Gate]  ──> [Drum (Rim) Trig]
[Drum (Rim) Out] ──> [Resonator (glass) In]
```

The rim's crack is all click and no pitch. Through a Body made of Glass, tuned to E5, each hit rings the glass's modes: E5, and above it partials at 2.32 and 4.25 times that, which no string has. The rim lane's velocities pass straight through, since a harder click rings the glass harder. E is in three of the four chords. Over the Gsus4 it's the sixth, a colour rather than a clash.

### The plucked line

```text
[Line lane Pitch]    ──> [Resonator (line) V/Oct]
[Line lane Gate]     ──> [Resonator (line) Gate]
[Line lane Velocity] ──> [Resonator (line) Strike]
```

The fifth Resonator is plucked by a gate, as a string on its own would be. **Tune** at -12 puts the line an octave under the lane's notes. **Position** at 12% plucks it near the bridge, for a thin, koto-like edge. A softer step is quieter and smoother, with less of the pick in it, so the line has a touch to it.

### Levels

The shaker's accents land on the offbeats and give way where the kick lands. A string excited from **In** answers each hit with a quick burst before it settles, so the patch keeps those bursts from piling up on the kick. The glass is the steadiest voice, so it carries most of the loudness. The whole mix sits at -16 LUFS with its peaks under -1 dBFS.

## Variations

**Another glass.** Set the glass's **Material** to **Bell** for a church bell an octave under, **Bar** for a glockenspiel, or **Wood** for a marimba-like knock. Turn **Inharm** to 0 and any of them rings in tune like a string.

**A bowed bed.** Swap the Shaker for a [Noise](../modules/sources/noise.md) module, Pink, through a slow LFO into a [VCA](../modules/utilities/vca.md), into the strings' **In**. The chord swells and breathes like a bowed string section.

**Your voice.** Patch an [Audio Input](../modules/sources/audio-input.md) into the strings' **In** and sing. The chord hums along with you, loudest when you land on one of its notes.

**Harder hands.** Turn the strings' **Brightness** to 90% and their **Damping** to 20%. The shaker's grain gets in and rings, and the chord turns metallic, like a hammered dulcimer.

**Pluck in the middle.** Move the line's **Position** to 100%. The pluck goes hollow and round, because a string plucked in its middle has no even partials.

## Related

- [Resonator](../modules/sources/resonator.md): both models, every knob, and the display
- [Drum Machine](./drum-machine.md): the same lanes playing the drums themselves
- [Chord Sequencer](../modules/utilities/chord-sequencer.md): chords on one cable, voice-led
