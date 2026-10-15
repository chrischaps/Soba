# Link Out

**Module ID** `output.link` · **Category** Output

Link Out plays [Seoul](https://github.com/chrischaps/Seoul), a music visualizer, from your patch. Seoul already hears Soba through your speakers, but from the sound alone it can only guess what the patch means. Link Out tells it: whatever you patch into its eight **CV** lanes and four **Gates** goes to Seoul with the patch's Clock, and Seoul's presets read them as `cv1` to `cv8` and `gate1` to `gate4`.

The cables decide what the picture follows. The LFO that sweeps your filter can sweep Seoul's zoom too, the kick's trigger can flash it, and a preset that turns on the beat turns on your Clock's beat exactly, not one Seoul has guessed at.

Link Out makes no sound and changes none. It works in the desktop app. In the browser it shows what it would send but sends nothing.

## Inputs

| Port | Signal Type | Description |
|------|-------------|-------------|
| **CV 1** … **CV 8** | Control (Orange) | Sent to Seoul as `/soba/cv/1` … `/soba/cv/8`, as they are: 0 to 1, or -1 to 1. An empty lane sends 0 |
| **Gate 1** … **Gate 4** | Gate (Green) | Sent to Seoul as `/soba/gate/1` … `/soba/gate/4`. Every rising edge gets there, however short the trigger |

Every input is optional. The module has no outputs.

## Parameters

| Control | Range | Default | Description |
|---------|-------|---------|-------------|
| **Port** | 9137 – 9140 | 9137 | The port Seoul listens on. 9137 unless Seoul's `seoul.toml` names another |

## The display

- **The lamp and port**, top left. Green and breathing while Link Out is sending and Seoul is listening. Amber, with *no Seoul*, while nothing listens on that port: start Seoul, or pick the port its `seoul.toml` names.
- **The tempo**, top right: the Clock's BPM, or *no Clock*.
- **Eight columns**, one per CV lane, filling up from the middle line for a positive value and down for a negative one. A lane with no cable is an empty slot.
- **Four lamps** for the gates, lit while a gate is high and flashing on each rising edge, so even a 1 ms trigger shows.
- **The beat in the bar**, bottom right, as the Clock draws it.

## How it works

**The Clock.** Link Out sends the patch's tempo and its place in the bar, from the first [Clock](../modulation/clock.md), with no cable needed. While the patch plays, Seoul locks to it: every preset that moves on the beat moves on this one, and Seoul's tempo reads exact. Stop the Clock and Seoul goes back to finding the beat by ear. A patch without a Clock still sends its lanes and gates.

**Gates as edges.** Seoul counts a gate's rising edges, so a trigger is never lost between two of its frames. Link Out sends every edge it sees, even a trigger that rises and falls between two sends, and a gate that falls and rises again between sends is sent low and then high, so both count.

**Seoul's side.** The values travel as OSC to `127.0.0.1` on the chosen port, about 200 times a second. Nothing leaves your computer. Seoul's `docs/LINK.md` has the whole protocol, so anything that speaks OSC can listen in, and its `[link] delay_ms` setting lines the picture up with the sound you hear.

**More than one.** Two Link Outs on the same port share it: each sends the lanes it has cables in. On different ports they play two copies of Seoul.

**Latency.** Seoul gets the lanes within a few milliseconds, sooner than it hears the sound, which goes the long way round through your audio device. Set Seoul's `delay_ms` by ear until a gate's flash lands with the kick.

## One patch, many presets

Seoul's link-aware presets agree on what each lane does, so a patch wired this way plays all of them:

| Lane | Plays |
|------|-------|
| **Gate 1** | the kick |
| **Gate 2** | the snare |
| **Gate 3** | the hats |
| **Gate 4** | a downbeat, or any accent |
| **CV 1** | energy: a brighter, busier picture |
| **CV 2** | color |
| **CV 3** | motion |

A gate takes over from the drum Seoul hears only once it has played, and the CV lanes only add to what a preset does, so leave any lane empty and the preset follows the music there instead. Han River, Jongno Rain, Hongsalmun, Window Within and Cheonggyecheon Lanterns follow the convention, and Gayageum and Obangsaek give each lane a part of their own.

## Tips

- **A preset should look good without Soba.** Seoul's link presets blend between what they hear and what Link Out sends, so a lane with no cable falls back to the music.
- **Unipolar for brightness, bipolar for motion.** An [LFO](../modulation/lfo.md) with **Bipolar** off gives 0 to 1, which suits a colour's weight or a glow. Bipolar suits a swing either way, such as a pan or a tilt.
- **Envelopes make good lanes.** A [Slope](../modulation/slope.md) or [ADSR](../modulation/adsr.md) on the kick's gate sends a swell that rises and falls with each hit.
- **Mark the bar.** A [Clock Divider](../utilities/divider.md) at 16 on a 1/16 Clock gives a gate on every downbeat, and its **Count** is a ramp through the bar.

See [Pinwheel](../../recipes/pinwheel.md) for a patch built for the link.
