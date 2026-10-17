# Orrery

A song with no score: five minutes of E minor at 112 BPM, from a gong at dawn to a gong at nightfall. Nothing tells the patch when to play what. There's no [Arranger](../modules/utilities/arranger.md), only slow orbits. A **Day** rises for 200 seconds and sets in 100, and each voice waits for the Day to reach its height before it comes in, then leaves as the Day goes down. A **Tide** takes the rhythm away once and brings it back. Two clock gears, five bars and seven, meet every 35 bars, and each meeting is an eclipse.

This is how a modular player builds a piece that plays itself: patterns that move against each other, and slow voltages that decide who is up. The top level is the orrery itself. The clockwork and the orbits are in plain view, and each voice is a [group](../concepts/groups.md) you can open.

> **Load it:** choose **📚 Examples → Orrery** in the toolbar and press **▶ Play**. It needs no keyboard.
> The patch file is [`patches/orrery.json`](https://github.com/chrischaps/Soba/blob/master/patches/orrery.json).

<iframe class="patch-embed" src="https://soba.chaps.dev/?patch=orrery" title="Orrery, playable in the browser" allow="autoplay" loading="lazy"></iframe>

*Or play it here: press **▶ Play** in the corner. The full app is a click away under **Open in Soba**. The clockwork's dividers are at the top left, with the orbits beside them. Below them is "Who is up", one Logic module for each voice, waiting for its height. The voices and the mixing desk come last.*

## The song

There are no sections, only the Day. The names below describe where it is.

| Bars | Time | The Day | What happens |
|------|------|---------|--------------|
| 1–4 | 0:00 | Dawn | An eclipse: a gong, and its ring. As it fades, the Moon begins to sway between two notes. |
| 5–8 | 0:09 | | A low E and B drone, and the steady Twin, which sets the bar. |
| 9–12 | 0:17 | | A heartbeat: a soft kick on beats 1 and 3. |
| 13–16 | 0:26 | | The Sun: a string pad, one chord every four bars. It dips with each kick. |
| 17–20 | 0:34 | | The drifting Twin, one eighth shorter, starts to slip against the steady one. |
| 21–28 | 0:43 | Morning | The bass: a Berlin-school ostinato under the chords. |
| 29–35 | 1:00 | | The full kit: four on the floor, offbeat hats and a shaker. |
| 36 | 1:15 | | An eclipse: the gong and a crash. The Twins snap back into step. |
| 49–68 | 1:43 | Noon | The Day passes halfway. The drums step up to their groove, with clap, ride and open hats, and a fill closes every eighth bar. The Moon now sings in wide arcs. |
| 69–84 | 2:26 | The Tide | The Tide is at its lowest, so the bass and drums rest. An eclipse rings out in the space (2:30), and the Comet arrives over the drone (2:34): a gliding lead that only flies near noon. |
| 85–105 | 3:00 | Afternoon | The rhythm comes back in, with the Comet over it. |
| 106 | 3:45 | | An eclipse. Soon after (3:51), the Comet leaves. |
| 117–128 | 4:09 | Dusk | The Day falls below halfway, and the drums step back down. |
| 129–140 | 4:34 | | The kit goes, then the bass, then the pad and the drifting Twin. The heartbeat is the last pulse, and the Moon narrows back to its two-note sway. |
| 141 | 5:00 | Night | The last eclipse, a gong, rings out. |

Left running, the Day rises again and a second day begins. The Tide is 80 bars round, not 140, so the second day's rest falls somewhere else.

## What it teaches

- **Form from a slow LFO and a few thresholds.** The Day is a triangle [LFO](../modules/modulation/lfo.md), unipolar, so it runs 0 to 1. Each voice has a [Logic](../modules/utilities/logic.md) module whose **Above** is high while the Day is over its **Threshold**: 0.02 for the drone, 0.06 for the heartbeat, 0.2 for the bass, 0.74 for the Comet. That gate rides the voice's fader through a [Slope](../modules/modulation/slope.md), so each one fades in and out. The order voices come in is the order they leave.
- **Read the orbits once a phrase.** A [Sample & Hold](../modules/utilities/sample-hold.md) reads the Day every four bars, from a [Clock Divider](../modules/utilities/divider.md) at ÷64. So every entrance lands on a phrase, not wherever the LFO crosses the line.
- **Gears.** Two dividers count bars, one at ÷5 and one at ÷7, and a Logic **AND** fires only where they meet: every 35 bars. Each meeting strikes a gong and resets both Twins. A smaller pair at ÷5 and ÷7 sixteenths ticks a woodblock where they meet.
- **A melody from two drifting orbits.** Moon A and Moon B are LFOs 7.1 and 4.95 dotted eighths long. Mixed, and sampled every dotted eighth, they never repeat the same way twice. A [Quantizer](../modules/utilities/quantizer.md) snaps them to E minor pentatonic, and the bell is struck only on its **Trig**, when the note changes. The Day sets how wide they swing: two notes at dawn, more than an octave at noon.
- **Phasing.** The Twins play one two-bar figure of sixteen eighths on two [Step Sequencers](../modules/utilities/sequencer.md). One plays all sixteen steps and the other only fifteen, so it slips an eighth on every pass, the way Steve Reich's *Piano Phase* drifts. Panned apart, the two parts slide against each other and lock back together at each eclipse.
- **Set the meter before the cross-rhythms.** The Moon strikes in dotted eighths, three sixteenths apart, which on its own suggests a different bar. So the steady Twin and the heartbeat come in first, leaning on the beats. By the time the Moon and the drifting Twin have something to cross, you already know where the bar is.

## Modules

There are 177 modules of 27 kinds. The ones that do the work:

| Module | Role |
|--------|------|
| [Clock](../modules/modulation/clock.md) | **BPM** 112, **Div** 1/16 |
| [Clock Divider](../modules/utilities/divider.md) ×10 | Eighths, dotted eighths, bars and phrases; the Comet's five-eighth clock; the last bar of every eight, for fills; and two pairs of gears (bars at ÷5 and ÷7, sixteenths at ÷5 and ÷7) |
| [LFO](../modules/modulation/lfo.md) ×7 | The orbits: the Day (triangle), the Sundial (saw) that sets the Day's dusk, the Tide (80 bars), the Breath (13 bars, which moves the bass), Moon A and B; and the Comet's vibrato |
| [Logic](../modules/utilities/logic.md) ×20 | Who is up: one for each voice's height, two for the drums' steps, and the eclipses; two more let only the drums you hear through to Seoul |
| [Sample & Hold](../modules/utilities/sample-hold.md) ×5 | The orbits read once a phrase, and the Moon's and Comet's notes |
| [Quantizer](../modules/utilities/quantizer.md) ×4 | The Moon's and Comet's scale, and two used as steady voltages to set the slowest LFO rates |
| [Chord Sequencer](../modules/utilities/chord-sequencer.md) | Em9, Cmaj9, G6, Dadd9, Em9, Am9, Cmaj7, Bsus4: a chord every four bars, for the pad and the bass's root |
| [Step Sequencer](../modules/utilities/sequencer.md) ×4 | The Twins (16 and 15 steps), and the bass: notes in four patterns, chosen by the Breath, and accents |
| [Trigger Sequencer](../modules/utilities/trigger-sequencer.md) | The drums: a heartbeat (A), a beat (B), the groove (C) and the fill (D), stepped up by the Day |
| [Drum](../modules/sources/drum.md) ×9 | Kick, clap, closed and open hats, shaker, tom and ride; the eclipse's crash; the tick's woodblock |
| String Machine, FM Bell ×2, Pump ×2 | From the [Library](../concepts/groups.md#the-library): the Sun's pad, the Moon and the gong, and the kick's pump on the pad |
| [Ladder Filter](../modules/filters/ladder-filter.md) ×3 | The bass, the Comet and the drone |
| [Noise](../modules/sources/noise.md) | Pink surf that swells with the Tide |
| [Stereo Delay](../modules/effects/delay.md) | **Sync** 1/8D, **P-P** and **Tape** on: the echo, with a hall of its own |
| [Reverb](../modules/effects/reverb.md) ×2 | The hall everyone shares, and the echo's |
| [Mixer](../modules/utilities/mixer.md) ×4 | Sky, Earth, Pulse and Air, on one chain |
| [Tape](../modules/effects/tape.md) | A little wow, saturation and age over the whole mix |
| [Audio Output](../modules/output/audio-output.md) | **Vol** 60%, **Limiter** and **Character** on |
| [Link Out](../modules/output/link-out.md) | The orbits and the drums, sent to the Seoul visualizer |

The rest are the voices' own oscillators, envelopes, VCAs and filters, and the Attenuverters and Mixes that scale the orbits.

## How it's built

### The Day, and its dusk

A day that rises for 200 seconds and falls in 100 isn't a plain triangle, so the Day's rate changes at dusk. Its **Rate** knob is at 0.01 Hz, and a steady −2 V on its rate input takes it two octaves down, to 0.0025 Hz: a 200-second climb. At dusk, a gate adds +1 V, and the Day falls twice as fast as it rose.

That gate comes from the **Sundial**, a saw LFO once round the whole song, through a Logic at 2/3. The Day can't time its own dusk: a cable from its **Phase** back to its own rate would make a loop, and Soba leaves loops out.

The −2 V comes from a [Quantizer](../modules/utilities/quantizer.md). A [Mix](../modules/utilities/mix.md) rounds off anything past about ±1, so it can't sum to −2. The Quantizer's **Transpose** adds its semitones as volts (12 semitones is 1 V) to an Attenuverter's −1 V, and the dusk gate adds its +1 V through the Transpose input.

### Who is up

```text
[Day] ──> [S&H, every phrase] ──> [Logic CV]   Threshold: the voice's height
                                  [Logic Above] ──> [Slope] ──> the voice's fader
[Tide] ─> [S&H] ─> [Logic] ──AND──┘  (for the bass, the heartbeat and the drums)
```

The bass, heartbeat and drums need the Day *and* the Tide, so each of their Logics also takes the Tide's gate on **B** and passes **AND**. The bass and the kit swell with the Day too: they come in quieter in the morning and play out in full at noon.

### The drums climb

The Trigger Sequencer's **Pattern** input picks A, B, C or D from four quarters of its range. Three gates add up into it: past the kit's height, the drums step to B (+0.3); past halfway, to C (+0.25); and in the last bar of every eight, while the Day is past halfway, the fill (+0.3) takes them to D.

### To Seoul

At the top right, a [Link Out](../modules/output/link-out.md) sends the orrery to [Seoul](https://github.com/chrischaps/Seoul), a music visualizer, so the picture follows the song's own orbits rather than guessing at them from the sound. It follows Seoul's lane convention, so any of Seoul's link-aware presets plays along:

| Lane | From | In Seoul |
|------|------|----------|
| **CV 1** | The Day | Energy: the picture brightens toward noon and dims at dusk |
| **CV 2** | The Breath | Color, turning every 13 bars |
| **CV 3** | The Tide | Motion, which slows while the rhythm rests |
| **CV 4**, **CV 5** | Moon A and Moon B | The melody's two orbits, for presets that read them (Obangsaek's white and black) |
| **Gate 1** | The heartbeat and kick, while the heartbeat is up | The kick |
| **Gate 2** | The clap, while the kit is up | The snare |
| **Gate 3** | The closed hats, while the kit is up | The hats |
| **Gate 4** | Each eclipse | A bloom, in Obangsaek |

The Drums group runs its sequencer the whole song and leaves the faders to decide when it's heard, so its gates fire in the silence too. That's why gates 1 to 3 come through Logic **AND**s with the same "who is up" gates the faders follow: Seoul sees only the drums you hear. The Drums group has **Clap** and **Hats** outputs for them, beside its **Kick**.

Link Out makes no sound, so the mix is exactly as it was. Try Seoul's **Han River** (fountains on the kick, fireworks on the clap) or **Hongsalmun** (a stride on every kick), and watch the city go dark through the Tide.

## Variations

- **Line up the Twins.** Open **Twins → Fifteen** and set its sequencer's **Steps** to 16. The two parts now play the figure in unison, and the drift is gone. Set it back to 15 to hear them slide apart again.
- **A different noon.** In "Who is up", the Logic at **Threshold** 0.74 is the Comet's. Raise it to 0.9 and the Comet flies only for the top of the Day, or lower it to 0.5 for a longer afternoon.
- **More eclipses.** In "The clockwork", find the two gears clocked by the bar divider, at ÷5 and ÷7, and set the ÷7 one to ÷3. The gears now meet every 15 bars, and the gong and the Twins' reset come round more than twice as often.
