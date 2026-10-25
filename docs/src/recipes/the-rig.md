# The Rig

A song in which Soba conducts a room of hardware. A **Digitakt** changes pattern on the bar, an LFO rides a **MicroFreak**'s filter, a **volca fm** gets its sound back from the patch, and an **RC-505** records four bars and loops them, all beside Soba's own pad and bass. It's in A minor at 100 BPM, Am9, Fmaj9, Cmaj7 and G6 a bar each.

With nothing plugged in it's still a whole song, because every device has a **stand-in**: a Soba voice that plays the device's part. Plug a device in and mute its stand-in, and the real one plays instead.

> **Load it:** choose **📚 Examples → The Rig** and press **▶ Play**. To play the room, choose its devices in the Rig (`Ctrl+Shift+H`, or **MIDI In ▸ The Rig…**) as [below](#setting-up-the-room).
> The patch file is [`patches/the-rig.json`](https://github.com/chrischaps/Soba/blob/master/patches/the-rig.json), built by [`tools/examples/make_the_rig.py`](https://github.com/chrischaps/Soba/blob/master/tools/examples/make_the_rig.py).

## What it teaches

It uses each way Soba has of playing hardware. See [Playing Hardware](../hardware/index.md) for all of them.

- **[Setlist](../modules/midi/setlist.md): the song changes the room.** Each section names the Digitakt's pattern, sent half a second before its bar line so the Digitakt queues it and starts it on the downbeat. At the top it puts the RC-505 on phrase memory 1.
- **[Instrument](../modules/midi/instrument.md): a synth as a node.** The MicroFreak's knobs are on the node by name. A step sequencer plays it, and an LFO is patched into its **Cutoff** jack.
- **[FM Voice](../modules/midi/fm-voice.md): the patch is the volca's memory.** It holds the volca fm's whole DX7 voice and sends it when the patch opens. A second Instrument plays the volca's notes.
- **[MIDI Out](../modules/output/midi-out.md) presses a button.** An [Arranger](../modules/utilities/arranger.md) lane hits **CC 14** on the RC-505's Rec/Play, once to record and once to play.
- **Clock on the ports.** Soba's [Clock](../modules/modulation/clock.md) sends the tempo, Start and Stop to the Digitakt and the RC-505, so their own patterns and loops keep time with the song. Its **Source** is **Auto**: when a device starts playing, Soba follows it.

## The room

| Port | Device | What Soba does with it | Its stand-in |
|---|---|---|---|
| **A** | Digitakt | Nothing plays it: it plays its own patterns. The Setlist chooses them | **Stand-in drums**: four Drums from a Trigger Sequencer, a pattern for each section |
| **B** | MicroFreak | An Instrument plays a step sequencer's line and turns eleven of its knobs. A two-bar LFO rides **Cutoff** between a third and three quarters open | **Stand-in synth**: a saw through a ladder filter that the same LFO opens |
| **C** | volca fm | The FM Voice sends its voice, SOFT KEYS. An Instrument plays three-note chords in the Chorus | **Stand-in keys**: the [Electric Piano](../concepts/groups.md#the-library) from the library, playing the same chords |
| **D** | RC-505 | A MIDI Out presses Rec/Play on the Take's downbeat and again on the Verse's | A [Looper](../modules/utilities/looper.md), pressed at the same moments, taking the same four bars of the MicroFreak |

The MicroFreak's Instrument leaves **Osc Type** off its knobs. Whichever engine you've chosen on the MicroFreak, the patch shapes it rather than replacing it. The volca's Instrument leaves off **Algorithm** and the LFO, because the FM Voice already sets them. Its envelope knobs sit in the middle, which leaves the voice as it is, and its arpeggiator is off.

## The song

| Section | Bars | Digitakt | Soba's drums | MicroFreak | volca fm | RC-505 | Soba's own |
|---|---|---|---|---|---|---|---|
| Intro | 4 | A01 | Hats, a soft kick on the one | | | Phrase memory 1 | The pad swells in |
| Take | 4 | A02 | | A figure, for the loop | | **Rec** | |
| Verse | 8 | A03 | Kick, snare, eighth hats | An answer, longer and higher | | **Rec/Play**: the loop plays | The bass comes in |
| Chorus | 8 | A04 | Fuller, sixteenth hats, an open hat | Climbing, an E or G ringing above each note | Chords | The loop, softer | The bass, full |
| Break | 4 | A05 | A kick and two hats | | | The loop, alone with the pad | A bigger room |
| Verse | 8 | A03 | As the first | As the first | | | As the first |

After the second Verse it goes round from the Chorus for ever: Chorus, Break, Verse. The Take happens once, so the loop recorded there plays on under everything after it.

## Setting up the room

The patch says which port plays what. Which device is on each port is a setting of this computer, so set it once in the Rig (`Ctrl+Shift+H`):

1. **Ports.** Choose the Digitakt for port **A**, the MicroFreak for **B**, the volca fm for **C** and the RC-505 for **D**. If the volca fm goes through a 5-pin MIDI interface, set port C's **Budget** to **DIN · 1000/s**.
2. **Clock.** Set ports **A** and **D** to **Clock + Transport**, so the Digitakt and the RC-505 start, stop and keep time with the song. Set both to follow MIDI clock and not to send it, so Soba leads. To let one lead instead, turn its clock send on and press Play on it: the Clock's **Auto** source follows it, and takes the lead back when it stops.
3. **Each device.** Each profile page lists what to turn on, under **Before you play**:
   - [Digitakt](../hardware/digitakt.md): **Prg Ch Receive** on, with **Prg Chg In Ch** on **1**.
   - [MicroFreak](../hardware/microfreak.md): its MIDI input channel on **1**.
   - [volca fm](../hardware/volca-fm.md): its MIDI channel on **1**.
   - [RC-505](../hardware/rc-505.md): in the Assign page of phrase memory 1, a source of **CC 14** with a target of **TGT TR REC/PLAY**, on the track you want to loop. Set the track's input to whatever you'd like it to record. Soba sends 127 for the press and 0 for the release. The RC-505's manual doesn't give the values it expects, so check that one press records.
4. **Mute the stand-ins.** On the Stand-ins mixer, press the **M** of each device you've plugged in: **1** Digitakt, **2** MicroFreak, **3** volca fm, **4** RC-505.
5. **Line them up.** If a device sounds late against Soba, ping it from its port's **Offset** (see [Measuring a device's delay](../getting-started/interface-overview.md#measuring-a-devices-delay)).

Then open the patch again, or press **Send all** on each Instrument. The MicroFreak gets its knobs and the volca fm its voice. Press **▶ Play**.

## Making it yours

- **Your own patterns.** Click a row in the Setlist to choose another pattern for a section, or **+** to add a row for another device: the FANTOM-06's scenes, the ZOIA's patches, the SP-404MKII's banks.
- **Another sound for the MicroFreak.** Select its Instrument and press `Ctrl + M` to [mutate](../getting-started/interface-overview.md#mutate) its knobs a step. Turn its real knobs and the node's knobs follow, so saving the patch keeps the sound you made by hand.
- **Another voice for the volca.** Press **Mutate** on the FM Voice and the volca plays the child at once. The garden keeps every voice you bred.
- **Your keyboard in the room.** In the Rig's **Thru**, add a route from your keyboard to port B and play the MicroFreak over the song, past the patch. Click the **thru** chip by the keyboard's name under MIDI In so the patch's own modules don't play along.
- **Another device.** Choose another profile on either Instrument, or point a Setlist row at another port.

## Mix and space

| Module | Settings |
|--------|----------|
| [Mixer](../modules/utilities/mixer.md), Stand-ins | Drums 100%, MicroFreak 75% (L 30), keys 65% (R 30), loop from the Arranger's **Loop level** lane (R 15); sends of 6%, 35%, 30% and 20%. Chained into Soba's own |
| [Mixer](../modules/utilities/mixer.md), Soba's own | The pad and the bass from the Arranger's **Pad** and **Bass** lanes; sends of 40% and 4%; **Return** from the **Room** lane; **Master** +1.5 dB |
| [Stereo Delay](../modules/effects/delay.md) | Dotted eighths, **Ping-Pong** and **Tape** on, **Feedback** 34%, **Mix** 35%, **High Cut** 4.2 kHz, **Low Cut** 260 Hz, into the reverb |
| [Reverb](../modules/effects/reverb.md) | **Size** 72%, **Decay** 3.4 s, **Pre-Delay** 22 ms, all wet, in the send |
| [Tape](../modules/effects/tape.md) | Clean: **Saturation** 16% and nothing else. It rounds the first moment of each kick, so the song sits at -16 LUFS without its peaks reaching the limiter |

## Related

- [Playing Hardware](../hardware/index.md): ports, clock, Instruments, Kits, the Setlist and Thru
- [Instrument Profiles](../hardware/instruments.md): the devices Soba knows, and how to add your own
- [Hands On](./hands-on.md): a controller in your hands, rather than a room to conduct
- [Live Looper](./live-looper.md): Soba's own Looper, played live
