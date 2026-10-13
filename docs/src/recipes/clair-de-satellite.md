# Clair de Satellite

A song in the manner of the French duo Air: four minutes of A minor at 88 BPM, with a Rhodes, a string machine, a round bass, soft drums, tape echo, and a vocoder that sings on the chords. It opens on a satellite beeping over tape hiss and ends the same way. In between, a robot voice sings *Satellite… passing over you…*, and near the end it asks *can you hear me…* The band falls away for one bar, and a plain human voice answers: *still?*

The patch plays the whole song itself. Three [Arrangers](../modules/utilities/arranger.md) are the score, in 19 sections. Each part of the band is a [group](../concepts/groups.md) with jacks of its own, so the top level reads like a score: harmony, band, voice and desk. Open any group to see its insides.

> **Load it:** choose **📚 Examples → Clair de Satellite** in the toolbar and press **▶ Play**. It needs no keyboard.
> The patch file is [`patches/clair-de-satellite.json`](https://github.com/chrischaps/Soba/blob/master/patches/clair-de-satellite.json).

<iframe class="patch-embed" src="../play/?patch=clair-de-satellite" title="Clair de Satellite, playable in the browser" loading="lazy"></iframe>

*Or play it here: press **▶ Play** in the corner. The full app is a click away under **Open in Soba**. The score's three Arrangers run along the top, with the mixing desk at the right. Below them are the harmony (two Chord Sequencers, the Rhodes and the String Machine), the band (each part a group) and the voice.*

## The song

| Bars | Section | What happens |
|------|---------|--------------|
| 1–8 | Signal | A satellite beeps over tape hiss, each beep falling a little in pitch as it passes. The Rhodes fades in on the verse chords. |
| 9–16 | Drift | The strings swell, the bass enters on whole notes and the drums on brushes, as the chorus chords lift into F major. The satellite fades. |
| 17–24 | Verse | The groove, and the voice: *Satellite · passing over you · Satellite · every night, every night*. |
| 25–32 | Chorus | A reedy whistle plays the hook while the strings open up. |
| 33–48 | Verse II, Chorus II | Again, with FM bells, and the verse ends *can you hear me?* |
| 49–56 | Bridge | The drums drop out. The voice alternates *Clair de satellite* and *can you hear me?* over held chords, and the satellite returns. |
| 57–64 | Build, Encore, Breath | A riser climbs under *I'm sending you signals · from so far away · Satellite*. The robot asks *can you hear me…*. Then the band falls away, and the plain voice answers: *still?* |
| 65–72 | Solo | A crash, and a Minimoog-style lead takes the verse melody. |
| 73–80 | Last Chorus | Everything at once, the Rhodes comping harder. |
| 81–88 | Outro | The first verse returns as the drums and bass thin out and the satellite comes back. |
| 89–93 | Last Light, Ring | One last Am9. The sequencers stop, the chord rings out, and the tape sags as the signal dies. |

Left running, the song starts again on the satellite.

## What it teaches

- **Chords on one cable.** Two [Chord Sequencers](../modules/utilities/chord-sequencer.md) play the same sixteen-bar progression: close and voice-led for the Rhodes, spread and tied for the strings. The slash basses (Am/G#, Am7/G, D9/F#) make the walking line on the **Bass** output.
- **A rhythm from one module, chords from another.** The Rhodes takes its notes from a Chord Sequencer and its rhythm from a [Step Sequencer](../modules/utilities/sequencer.md)'s gate. A mono gate into a polyphonic voice strikes every note of the chord.
- **A vocoder that can be understood.** A vocoder reads words best from a carrier that is low, dense and held. Here that's the strings' spread chord, with its lows filtered off. The voice is brightened and compressed on its way in, and the vocoder's attack and release are kept short. A carrier up in a melody's register, an octave or two higher, loses most of the words.
- **Many lines in one sample.** The voice's eight lines are eight [slices](../modules/sources/sampler.md#slices) of one file. A Step Sequencer that steps every two bars picks a line by note: C4 sings the first, C#4 the second, and so on.
- **Groups as instruments.** Each part is a group with named jacks: Bass takes **Clock**, **Reset**, **Pattern** and **Bass note**, and gives **Out**. A few knobs are pinned to each group's face.
- **Slides.** The Solo's lead glides into chosen notes, the way a Minimoog player slurs a phrase.
- **The song on tape.** The whole mix runs through [Tape](../modules/effects/tape.md), and an Arranger lane on its **Wow** input warps it in the Ring.

## Modules

| Module | Role |
|--------|------|
| [Clock](../modules/modulation/clock.md) | **BPM** 88, **Div** 1/16, **Swing** 56% |
| [Arranger](../modules/utilities/arranger.md) ×3 | The score: 19 sections, 93 bars. **Faders** rides eight mixer channels, **Cues** picks patterns and strikes and warps, **Voices** brings in the vocoder and the plain voice |
| [Chord Sequencer](../modules/utilities/chord-sequencer.md) ×2 | The progression, a chord a bar, for the Rhodes and the strings |
| [Step Sequencer](../modules/utilities/sequencer.md) ×6 | The Rhodes' comping and the bass riff (sixteenths), the lead and hook (eighths, four chained patterns), the bells (quarters) and the lyrics (a step every two bars) |
| [Trigger Sequencer](../modules/utilities/trigger-sequencer.md) | The drums and the satellite's beeps: four patterns, picked by the Kit cue |
| [Clock Divider](../modules/utilities/divider.md) ×3 | Eighths, quarters, and every two bars |
| Electric Piano, String Machine, FM Bell | From the [Library](../concepts/groups.md#the-library). The String Machine has a second output for stereo, and the bell a **Root** jack so it follows the chords |
| [Oscillator](../modules/sources/oscillator.md) ×12 | Bass, lead, hook, satellite and the vocoder's carrier, plus the Library voices' own |
| [Drum](../modules/sources/drum.md) ×8 | Kick (A1), snare, rim, tom, closed and open hats, a shaker and the crash |
| [Compressor](../modules/effects/compressor.md) ×4 | The kit's glue, the metals' level, the voice into the vocoder and the plain voice |
| [Sampler](../modules/sources/sampler.md) ×2 | The sung lines, and *still?* |
| [Vocoder](../modules/effects/vocoder.md) | **Bands** 24, attack 2 ms, release 35 ms, **Sibilance** 60%, **Unvoiced** 50% |
| [Slope](../modules/modulation/slope.md) | The satellite's Doppler fall |
| [3-Band EQ](../modules/effects/eq.md) | The voice's lows cut and highs lifted, before the vocoder |
| [Mixer](../modules/utilities/mixer.md) ×4 | Top (with the tape echo), Drums, Band and Room (with the hall), on one chain. Room's **Master** −3.7 dB sets the level onto the tape |
| [Stereo Delay](../modules/effects/delay.md) | **Sync** 1/8D, **P-P** and **Tape** on |
| [Reverb](../modules/effects/reverb.md) | The hall everyone shares |
| [Tape](../modules/effects/tape.md) | 15 ips, a little saturation, wow and age |
| [Audio Output](../modules/output/audio-output.md) | **Vol** 80%, **Limiter** and **Character** on |

The rest are the voices' own envelopes, VCAs, filters, LFOs and Mixes.

## How it's built

### The score

Three Arrangers count the Clock's sixteenths through the same nineteen sections, from Signal to Ring. **Faders** has a lane per mixer channel: EP, Strings, Bass, Drums, Lead, Hook, Bells and Space. Each channel's **Level** knob is at zero, so the lane is the fader. **Cues** picks the patterns for the drums, the Rhodes' comping and the bass (A to D in quarters of the lane), hits the crash, raises the riser, holds the chords in the Ring, and warps the tape. **Voices** rides the vocoder and, on the Breath alone, the plain voice.

A one-bar **Fill** closes most phrases. The Build is split into Build, Encore and Breath, so the reveal lands on its own bar: the robot's last line plays in Encore, and Breath drops the drums, bass and hook and dips the Rhodes and strings, so the one plain word is heard clearly.

### Harmony

The verse is a line cliché: Am9, Am/G#, Am7/G, D9/F#, then Fmaj9, Em7, Dm9 and E7. The chorus lifts to Fmaj7, G11, Em7, Am7, Dm9, G13, Cmaj9 and E7. The roots are written in octave 2, so the **Bass** output is a bass line and the bass voice only adds a riff over it: the note, its octave, and slides between them.

The strings' Chord Sequencer ties every step, so the String Machine's swell never restarts and the chords change under one long breath. The Rhodes' is untied, and its gate comes from the comping sequencer.

### The voice

```text
[Lyrics Pitch] ──> [Sampler V/Oct]          (8 slices: note 60 + k sings line k + 1)
[Lyrics Gate]  ──> [Sampler Gate]           (Gated: one line, then stop)
[Sampler L] ──> [EQ] ──> [Compressor] ──> [Vocoder Modulator]
[Strings' Chord Sequencer Pitch] ──> [Saw] ──> [Chorus] ──> [SVF high-pass] ──> [Vocoder Carrier]
[Voices Gate 2] ──> [Still Sampler Gate]    (the Breath)
```

The voice is the author's, cloned with ElevenLabs and saying each line once. The vocoder sings it on the strings' chord, voiced low and wide, where the harmonics sit close enough together to trace every vowel. The words and the chord move independently, as on Air's own vocoder songs.

## Variations

- **Sing it yourself.** Patch an [Audio Input](../modules/sources/audio-input.md)'s **L** into the Voice group's modulator path, in place of the Sampler, and sing over the chords. Wear headphones.
- **A smaller throat.** Raise the vocoder's **Formant**, pinned to the Voice group's face, by a quarter octave.
- **All robot.** The *still?* Sampler feeds the vocoder too. Mute the Room mixer's fourth strip, and give the Voices Arranger's Vocoder lane a Hold in Encore instead of its ramp to zero: the robot answers itself.
