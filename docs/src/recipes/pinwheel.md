# Pinwheel

A drum kit that plays the picture as well as the sound. This is [Roll Call](./roll-call.md)'s groove with a [Link Out](../modules/output/link-out.md) beside it, sending three LFOs, a swell on every kick, the bar and the drums' gates to [Seoul](https://github.com/chrischaps/Seoul), a music visualizer. Seoul's **Obangsaek** preset is a paper pinwheel in Korea's five cardinal colours, and in this patch it turns once a bar on the Clock, its colours breathe with the LFOs, and the kick, snare and hats each throw a bloom of colour into the wind.

> **Load it:** choose **📚 Examples → Pinwheel** in the toolbar and press **▶ Play**. Then start Seoul and pick **Obangsaek**.
> The patch file is [`patches/pinwheel.json`](https://github.com/chrischaps/Soba/blob/master/patches/pinwheel.json).

<iframe class="patch-embed" src="https://soba.chaps.dev/?patch=pinwheel" title="Pinwheel, playable in the browser" allow="autoplay" loading="lazy"></iframe>

*Or hear it here: press **▶ Play** in the corner. The browser plays the drums, but only the desktop app sends to Seoul. The kit is Roll Call's; the **Picture** frame below the mix is what's new.*

## What it teaches

- **Cables play the picture.** Link Out's lanes go to Seoul as `cv1` to `cv8` and `gate1` to `gate4`, and a Seoul preset decides what each one moves. Choosing what goes into each lane is composing the picture.
- **The Clock leads both.** Link Out sends the Clock's tempo and place in the bar with no cable, so the pinwheel turns on the patch's own bar, never on a guess.
- **Gates arrive as edges.** A trigger lasts a fraction of a frame, yet every hit makes a bloom.
- **A preset for the link still plays without it.** Close Soba and Obangsaek goes back to following what it hears.

## The lanes

| Lane | From | In Obangsaek |
|------|------|--------------|
| **CV 1** | LFO, sine over 4 bars | Blue, the east |
| **CV 2** | LFO, triangle over 2 bars | Red, the south |
| **CV 3** | Slope on the kick: up at once, down over 0.4 s | Yellow, the centre, and the pin at the middle |
| **CV 4** | LFO, sine over 1 bar | White, the west |
| **CV 5** | Clock Divider's **Count**: a ramp through the bar | Black, the north |
| **Gate 1** | Kick lane | A bloom |
| **Gate 2** | Snare lane | A bloom |
| **Gate 3** | Closed hat lane | A bloom |
| **Gate 4** | Clock Divider's **Trig**: every downbeat | A bloom |

The LFOs have **Bipolar** off, so they send 0 to 1, and Obangsaek reads each as how much of its colour to show. Their phases are a third of a turn apart, so the colours take turns rather than swelling together.

## Modules

Roll Call's fourteen, with these added:

| Module | Settings |
|--------|----------|
| [Clock Divider](../modules/utilities/divider.md) | **Div** 16, **Length** 1: one count a 1/16 step, so a trigger every bar |
| [Slope](../modules/modulation/slope.md) | **Rise** 4 ms, **Fall** 0.4 s, **Shape** -0.5 |
| [LFO](../modules/modulation/lfo.md) (blue) | **Tempo** 4 bars, Sine, **Bipolar** off |
| [LFO](../modules/modulation/lfo.md) (red) | **Tempo** 2 bars, Triangle, **Phase** 120°, **Bipolar** off |
| [LFO](../modules/modulation/lfo.md) (white) | **Tempo** 1 bar, Sine, **Phase** 240°, **Bipolar** off |
| [Link Out](../modules/output/link-out.md) | **Port** 9137 |

## How it's built

```text
[Clock Gate]  ──> [Clock Divider Clock]
[Clock Reset] ──> [Clock Divider Reset]
[Trigger Sequencer Gate 1] ──> [Slope Trig]
[LFO (blue) Out]  ──> [Link Out CV 1]
[LFO (red) Out]   ──> [Link Out CV 2]
[Slope Out]       ──> [Link Out CV 3]
[LFO (white) Out] ──> [Link Out CV 4]
[Clock Divider Count] ──> [Link Out CV 5]
[Trigger Sequencer Gate 1, 2, 3] ──> [Link Out Gate 1, 2, 3]
[Clock Divider Trig] ──> [Link Out Gate 4]
```

Nothing in the **Picture** frame reaches the Audio Output, so the patch sounds just as Roll Call does.

## Seoul's side

Seoul listens on port 9137 unless its `seoul.toml` says otherwise. Link Out's lamp turns green once Seoul is listening, and amber, with *no Seoul*, while nothing is. Seoul's F1 panel shows the link: eight CV columns, four gate lamps, and *conf link* beside the tempo while the Clock plays.

The picture gets the lanes sooner than Seoul hears the sound, which goes the long way through your audio device. If a bloom comes before its drum, raise `delay_ms` under `[link]` in `seoul.toml` until they land together. 40 ms is a good start.

## Try this

**Another preset.** The gates follow Seoul's lane convention (kick, snare, hats, downbeat), so the patch plays more than Obangsaek. Try **Han River**, where the kick surges the fountain and the snare sets off fireworks, or **Hongsalmun**, where each kick is a stride through the gates. In **Gayageum**, each CV lane bends one of its strings and each gate plucks one.

**A colour from the music.** Patch an [Audio Input](../modules/sources/audio-input.md)'s **Follow** into a lane in place of an LFO, and sing the colour in.

**Faster colours.** Set the LFOs' **Tempo** to 1/2 or 1/4 and the pinwheel's colours flicker on the beat.

**Stop and start.** Turn the Clock's **Run** off: the pinwheel stops following the bar and Seoul goes back to finding the beat by ear. Turn it on again and it locks back on.

## Related

- [Link Out](../modules/output/link-out.md) – the lanes, the port and the lamp
- [Roll Call](./roll-call.md) – the groove this is built on
- [LFO](../modules/modulation/lfo.md) – tempo sync and phase
- [Clock Divider](../modules/utilities/divider.md) – counting steps into bars
