# Breakdown

Every dance track has a moment where it drops away: the drums thin out, the filter closes and rings, the echoes bloom into the space the beat left. Then it all comes back. On a hardware synth that's a performance with both hands on a dozen knobs at once. Here it's one knob.

This patch is the [Rhythmic Sequence](./rhythmic-sequence.md) acid line with a [Scenes](../modules/utilities/scenes.md) module added. Scene **A** is the verse, as the patch was. Scene **B** is its breakdown. A slow LFO turns **Morph** from one to the other and back every 32 seconds, and every knob the two scenes hold glides with it, on screen and in the sound.

> **Load it:** choose **📚 Examples → Breakdown** in the toolbar and press **▶ Play**. It needs no keyboard.
> The patch file is [`patches/breakdown.json`](https://github.com/chrischaps/Soba/blob/master/patches/breakdown.json).

<iframe class="patch-embed" src="../play/?patch=breakdown" title="Breakdown, playable in the browser" loading="lazy"></iframe>

*Or play it here: press **▶ Play** in the corner. The full app is a click away under **Open in Soba**. The Scenes module and its LFO are in the amber frame at the bottom: watch the light move from pad A to pad B.*

## What it teaches

- **One gesture, many knobs.** A scene is a snapshot of every knob in the patch. Morph glides them all together, each round its own travel.
- **The space between.** Morph at 30% is neither the verse nor the breakdown, and nobody set it on purpose. It's often the best sound in the patch.
- **Switches flip halfway.** The oscillator's **Voices** goes from one to three as Morph passes 50%, so the line suddenly thickens into a chord-like smear.
- **Cables win.** The filter's **Cutoff** and the hats' **Level** are driven by envelopes, so the scenes leave them be. The scenes move how far the envelope opens the filter (**Cutoff CV**) instead.

## The two scenes

| Module | Knob | A (verse) | B (breakdown) |
|--------|------|-----------|---------------|
| Oscillator | **Voices** | 1 | 3 |
| | **Detune** | 40% | 25% |
| SVF Filter | **Res** | 0.70 | 0.90 |
| | **Cutoff CV** | 2.0 | 0.7 |
| Distortion | **Drive** | 30% | 10% |
| | **Mix** | 70% | 15% |
| Stereo Delay | **Feedback** | 30% | 62% |
| | **Mix** | 25% | 50% |
| | **High Cut** | 10 kHz | 2.5 kHz |
| ADSR (hats) | **Dec** / **Rel** | 45 / 40 ms | 220 / 200 ms |
| SVF Filter (hats) | **Res** | 0.55 | 0.25 |

The High Cut glides by octaves, as its knob turns: halfway, it's at 5 kHz, not 6.25.

## Modules

As [Rhythmic Sequence](./rhythmic-sequence.md#modules), plus:

| Module | Settings |
|--------|----------|
| [LFO](../modules/modulation/lfo.md) | **Rate** 0.03125 Hz (32 seconds), **Wave** Sine, **Phase** 270°, **Bipolar** off, so it starts at 0 and lingers at each end |
| [Scenes](../modules/utilities/scenes.md) | **From** A, **To** B, scenes as above. The LFO's **Out** into **Morph** |

## Play with it

1. **Ride it yourself.** Unplug the LFO from **Morph** and turn the knob. Stop halfway and listen.
2. **Jump.** Click pad **A** or **B** to snap every knob to a scene. Each click is one undo step.
3. **Make a third.** With Morph wherever you like, click pad **C** to keep it. Set **To** to C and morph from the verse to your new scene.
4. **Let Mutate find one.** Select the Bass frame's modules, hold `Ctrl + M` until something surprises you, then press `Ctrl + Shift + M` to keep it in the next free pad.
5. **Put it on the downbeat.** Swap the LFO for an [Arranger](../modules/utilities/arranger.md) lane: a section at 0 for the verse, a **Ramp** to 1 over a four-bar build, a section at 1 for the breakdown.
