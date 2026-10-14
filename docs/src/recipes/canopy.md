# Canopy

A song in the manner of Bonobo: four and a quarter minutes of F minor at 116 BPM, on a swung sixteenth. Two little pentatonic loops, a kalimba and a set of bells, run at lengths that don't fit the bar, so the figure they make together is never quite the same twice. Under them come a strummed harp, a Rhodes, a string machine that breathes with the kick, a round sub bass and a broken beat with a real break chopped beneath it. Over them, pitched vocal chops carry the hook and a breathy flute the melody. It starts at dawn with birds and wind, and ends at night the same way.

The patch plays the whole song itself. Three [Arrangers](../modules/utilities/arranger.md) are the score, in sixteen sections. Each part is a [group](../concepts/groups.md) with jacks of its own, so the top level reads as a score: harmony, loops, band, forest and desk. Open any group to see its insides.

> **Load it:** choose **📚 Examples → Canopy** in the toolbar and press **▶ Play**. It needs no keyboard.
> The patch file is [`patches/canopy.json`](https://github.com/chrischaps/Soba/blob/master/patches/canopy.json), and its singer is [`patches/samples/canopy-chops.wav`](https://github.com/chrischaps/Soba/blob/master/patches/samples/canopy-chops.wav).

<iframe class="patch-embed" src="../play/?patch=canopy" title="Canopy, playable in the browser" loading="lazy"></iframe>

*Or play it here: press **▶ Play** in the corner. The full app is a click away under **Open in Soba**. The score's three Arrangers and the mixing desk run along the top, with a Dancer at the end of the desk. Below them, left to right: the harmony (three Chord Sequencers and the Rhodes, Strings and Harp they play), the two loops, the band, and the forest.*

## The song

| Bars | Time | Section | What happens |
|------|------|---------|--------------|
| 1–8 | 0:00 | Dawn | Wind, and birds calling. The kalimba fades in on its twelve-step loop. |
| 9–16 | 0:17 | Canopy | The bells join on their ten-step loop, the strings swell, a shaker starts, and a first faint vocal chop echoes. |
| 17–32 | 0:33 | Groove | A crash, and the beat: kick, rim and swung hats, the sub bass and the Rhodes. The vocal chops take the hook. A drum fill closes the phrase. |
| 33–48 | 1:06 | Groove II | The snare comes in with its ghost notes, the hats go to sixteenths, the harp strums a chord each bar, and the chopped break slips in under the kit. |
| 49–56 | 1:39 | Clearing | The drums fall away. Over held chords, whole-note bass and the strings, the flute plays its melody for the first time, and the birds come back. The tape sways. |
| 57–64 | 1:56 | Rise | The beat builds back over seven bars, under a rising wash of noise, into a fill. |
| 65–80 | 2:12 | Peak | Everything at once: the full kit and break, the busier bass and comping, the chops and the flute. |
| 81–96 | 2:46 | Peak II | The flute goes up an octave, and the chops and the break come forward. |
| 97–112 | 3:19 | Dusk | The kit thins back to the groove and fades, the bass fades out over sixteen bars, and the wind rises. |
| 113–124 | 3:52 | Night, Ring | The kalimba, wind and birds, as at dawn. Then the sequencers stop and the last notes ring out. |

Left running, the song starts again at dawn.

## What it teaches

- **Loops that don't fit the bar.** The kalimba's [Step Sequencer](../modules/utilities/sequencer.md) has 12 steps of sixteenths, three beats against the bar's four. The bells' has 10 steps of eighths, five beats. Neither is reset by the bar, so they drift against it and against each other, and the three line up again only every fifteen bars. Bonobo builds tracks such as *Cirrus* from loops layered this way.
- **One scale over every chord.** Every loop note is from F minor pentatonic (F, A♭, B♭, C, E♭), and every chord in the song contains or colours those notes. So the loops can land anywhere against the harmony and never clash.
- **Three voicings of one progression.** Three [Chord Sequencers](../modules/utilities/chord-sequencer.md) play the same sixteen bars: close for the Rhodes, spread and tied for the strings, and open and **strummed** for the harp, 70 ms between strings, with **Strum dir** on Alt so it strokes up one bar and down the next.
- **Pitched vocal chops from one file.** Twelve sung syllables (*ah*, *oh*, *ya*, *hey*, *mah*…) sit in twelve slots of one sample. Two Step Sequencers step together: one plays the note through the [Sampler](../modules/sources/sampler.md)'s **V/Oct**, and the other picks the syllable through **Start**, where note 60 + *k* starts at *k*/12 of the file. Played a few semitones either side of the sung C5, the voice moves up and down the way a chopped vocal does.
- **A break under a drum machine.** The [Chopper](../modules/sources/chopper.md) plays Soba's own two-bar break in time, 20 BPM faster than it was played, with dice on three of its sixteen steps. A high-pass at 380 Hz takes out its kick, so it adds air and ghost notes without fighting the kit.
- **Strings that breathe with the kick.** The string machine's two sides each run through a **Pump** from the [Library](../concepts/groups.md#the-library), struck by the kick's gate, so the pad dips on each kick and swells back.
- **Birds from a sine.** Each chirp is a sine flicked up nearly an octave by a [Slope](../modules/modulation/slope.md), plus a random offset that a [Sample & Hold](../modules/utilities/sample-hold.md) takes from noise on every chirp, so no two calls match. The drum sequencer's eighth lane plays them, with ratchets for the trills.

## Modules

| Module | Role |
|--------|------|
| [Clock](../modules/modulation/clock.md) | **BPM** 116, **Div** 1/16, **Swing** 56% |
| [Arranger](../modules/utilities/arranger.md) ×3 | The score: 16 sections, 124 bars. **Faders** rides eight mixer channels. **Cues** picks the drum, bass and comping patterns, hits the crash, raises the riser, lifts the flute, stops the sequencers in the Ring and warps the tape. **Voices** rides the chops, flute, wind and birds |
| [Chord Sequencer](../modules/utilities/chord-sequencer.md) ×3 | The progression, a chord a bar, for the Rhodes, the strings and the harp |
| [Step Sequencer](../modules/utilities/sequencer.md) ×7 | The kalimba (12 steps), bells (10 eighths), Rhodes comping, bass, flute (eighths, four chained patterns), and the chops' notes and syllables (four chained one-bar patterns) |
| [Trigger Sequencer](../modules/utilities/trigger-sequencer.md) | The drums and the birds: four patterns, picked by the Kit cue |
| [Chopper](../modules/sources/chopper.md) | `samples/soba-break.wav`, **Beats** 8, **Slices** 16, chance on steps 6, 11 and 15 |
| [Sampler](../modules/sources/sampler.md) | `samples/canopy-chops.wav`, **Mode** Gated, **Root** C5 |
| Electric Piano, String Machine, FM Bell, Mallet, Pump ×2, Wind | From the [Library](../concepts/groups.md#the-library). The Mallet is the harp |
| [Oscillator](../modules/sources/oscillator.md) ×13 | Kalimba, bass, flute and birds, plus the Library voices' own |
| [Drum](../modules/sources/drum.md) ×8 | Kick (F1), snare, rim, a high tom for a conga, closed and open hats, a shaker and the crash |
| [Compressor](../modules/effects/compressor.md) ×4 | The kit's glue, the metals' level, the break and the harp |
| [Stereo Delay](../modules/effects/delay.md) | **Sync** 1/8D, **P-P** and **Tape** on: the echo on the loops, chops and flute |
| [Reverb](../modules/effects/reverb.md) | The room everyone shares |
| [Mixer](../modules/utilities/mixer.md) ×4 | Top (with the echo), Drums, Band and Room (with the reverb), on one chain. An LFO pans the bells slowly across |
| [Tape](../modules/effects/tape.md) | 15 ips, a little saturation, wow and age |
| [Audio Output](../modules/output/audio-output.md) | **Vol** 75%, **Limiter** and **Character** on |
| [Dancer](../modules/visualization/dancer.md) | **Style** Disco, at Size 2.4. It hears the finished mix from the Tape, so it barely moves at dawn and goes all out at the Peak, and the Faders' **Section Trig** gives it a big move at every change of section |

The rest are the voices' own envelopes, VCAs, filters, LFOs and Mixes.

## How it's built

### The score

Three Arrangers count the Clock's sixteenths through the same sixteen sections, from Dawn to Ring. Every mixer channel's **Level** knob is at zero, so an Arranger lane is the fader. **Faders** rides the Kalimba, Bells, Harp, Rhodes, Strings, Bass, Drums and Break. **Cues** picks the patterns: A to D for the drums, two bass lines and three kinds of comping. **Voices** rides the vocal chops, the flute, the wind and the birds.

A one-bar **Fill** closes each long section, playing the drums' D pattern: a snare roll that ratchets up to fours, over a run of congas.

### Harmony

Fm9, D♭maj9, B♭m9 and E♭(add9), two bars each. On the second pass the last four bars become G♭maj7 and Cm7, to turn the progression round. The roots are written low, so the Rhodes' Chord Sequencer's **Root** output moves the bass from F down to B♭ and back. G♭maj7 under the pentatonic loops makes their C a raised eleventh: one bar of brightness in sixteen.

### The voice

```text
[Chops Pitch]     ──> [Sampler V/Oct]     (the note: C5 plays the syllable as sung)
[Syllables Pitch] ──> [Sampler Start]     (note 60 + k starts at k/12: syllable k)
[Chops Gate]      ──> [Sampler Gate]      (Gated, so each chop is as long as its notes)
```

The singer is nobody's recording. Like [Held Vowels](./held-vowels.md)' singer, it comes from the small formant synthesizer in `tools/voice/`, this time [`chops.py`](https://github.com/chrischaps/Soba/blob/master/tools/voice/chops.py), with the formants raised for a lighter voice. The hook is four bars: *oh · ah · ya · ah · oh*, then *mah · hey · ah-oh*, then the first bar again, rising to *hey · lah*, and an *ooh* to end. Every note is pentatonic, so it sits over all six chords. The chops go to the echo more than anything else, and their repeats fill the gaps between them.

## Variations

- **Line them up.** Open the Bells group and set its sequencer to 8 steps: a bar of eighths. The bells now repeat every bar, and they and the kalimba meet every three bars instead of fifteen. Put it back to 10 to hear the drift return.
- **Your own chops.** Open the Vocal Chops group and load any short vocal phrase into the Sampler. Set **Root** to the note it was sung on and leave **Slices** off, and the Syllables sequencer picks twelve places in it.
- **Swing harder.** Turn the Clock's **Swing** to 62%. The kit, the loops and the chops all lean back with it. The break keeps its own swing, because it's sliced in eighths, which swing doesn't move.
