# Hands On

A patch to play with both hands on an Arturia MiniLab 3, set up for the controller as it comes from the box. A bassline plays itself in D Dorian at 96 BPM. The keys play a lead over it, pad bank A plays a drum kit, and pad bank B runs a Looper and four scenes. All eight encoders, all four faders and both touch strips have a job. The patch is 41 modules, sixteen of them the pads in two groups.

> **Load it:** choose **📚 Examples → Hands On**, pick the MiniLab in **MIDI In**, press **▶ Play** and play. Then teach Soba the controller once (**MIDI In ▸ Teach Your Controller…**) and press **Ctrl+H**: a sketch of the MiniLab appears along the bottom, naming what each control does.
> The patch file is [`patches/hands-on.json`](https://github.com/chrischaps/Soba/blob/master/patches/hands-on.json).

## What it teaches

It uses each way Soba has of putting a controller to work:

- **[Note In](../modules/midi/note-in.md) for the keys.** It's on Omni, yet it never plays the pads' notes: sixteen [Pads](../modules/midi/pad.md) claim both banks.
- **[Pads](../modules/midi/pad.md) as gates.** Each bank is a group of eight Pads: **Pads A** and **Pads B**. Bank A strikes five Drums, each as hard as its pad was hit, through its **Vel** jacks. Bank B's first three pads are cables into the [Looper](../modules/utilities/looper.md)'s **Rec**, **Stop** and **Undo** jacks.
- **[Controls](../modules/midi/controls.md) as cables.** The mod strip, two faders and the pitch strip become Control signals, patched like any other: one fader feeds three inputs at once.
- **MIDI mappings saved with the patch.**
  - The encoders turn knobs, and pick each one up where it stands.
  - The main encoder steps a dropdown, and its click presses a footswitch.
  - A pad flips a toggle, and four pads recall scenes.
  - All of these are in the patch, so they come with it.
- **Pressure.** Press into a held drum pad and its aftertouch lets the closed hat ring longer.

## Your MiniLab

| Control | Sends | Does |
|---|---|---|
| Keys | notes, channel 1 | The lead: a detuned saw through a ladder filter, four voices |
| Pitch strip | pitch bend | Bends the lead (Note In) and the bass (Controls **D** into its **Exp FM**) by 2 semitones |
| Mod strip | CC 1 | Controls **A**: opens the lead's filter, up to 3 octaves |
| Encoder 1 | CC 74 | Lead **Cutoff** |
| Encoder 2 | CC 71 | Lead **Res** |
| Encoder 3 | CC 76 | Lead envelope **Release** |
| Encoder 4 | CC 77 | Lead **Detune** |
| Encoder 5 | CC 93 | Echo **Feedback** |
| Encoder 6 | CC 18 | Room **Decay** |
| Encoder 7 | CC 19 | Bass **Res** |
| Encoder 8 | CC 16 | The Clock's **Swing** |
| Fader 1 | CC 82 | Controls **B**, the wash: more of the lead and the drums into the echoes, and more echoes back |
| Fader 2 | CC 83 | Controls **C**: opens the bass's filter |
| Fader 3 | CC 85 | The kit's level |
| Fader 4 | CC 17 | The lead's level |
| Main encoder | CC 114 | Steps the lead's **Wave**: sine, saw, square, triangle |
| Main encoder click | CC 115 | Clears the loop (the Looper's **Clear** footswitch) |
| Pads, bank A | notes 36 – 43, channel 10 | **1** kick, **3** snare, **4** clap, **6** tom, **7** closed hat |
| Pads, bank B | notes 44 – 51, channel 10 | **1** Rec, **2** Stop, **3** Undo, **4** Reverse, **5 – 8** scenes A – D |

The encoders and faders 3 and 4 **pick up** their knobs: a knob waits where it is until the hardware reaches it, so turning one never makes the sound jump. While it waits, a purple tick on the knob's rim shows where the hardware is.

## Scenes

The four scenes set the knobs the encoders turn, apart from the lead's Cutoff. The mod strip's cable drives that one, and a scene leaves any knob with a cable in it to the cable.

| Pad | Scene | Sounds like |
|---|---|---|
| B5 | A, home | Where the patch opens |
| B6 | B, dub | A more resonant lead with long tails, long echoes, a big room and a heavier swing |
| B7 | C, tight | A wider lead with short tails, short echoes and a small room |
| B8 | D, space | The widest lead, slow tails and a nine-second room, played straight |

Recall one and every encoder waits to pick its knob up again from where the scene left it.

## Loops

The Looper records everything the mixer plays, echoes included, in time with the Clock. Hit B1 and it records four bars from the next downbeat, then closes the loop by itself and plays it. Hit B1 again to overdub a layer, and once more to stop overdubbing. B3 takes the last layer back, and a second B3 brings it back. B2 stops the loop, and B2 again plays it from the top. B4 plays it backwards, and clicking the main encoder clears it.

## Mix and space

| Module | Settings |
|--------|----------|
| [Mixer](../modules/utilities/mixer.md) | Kit 80%, kick 90%, bass 75%, lead 70%; sends of 10% from the kit, 5% from the bass and 30% from the lead, which fader 1 adds to; **Master** +1.5 dB |
| [Stereo Delay](../modules/effects/delay.md) | Dotted eighths, **Ping-Pong** and **Tape** on, **Feedback** 35%, **High Cut** 3.5 kHz, **Low Cut** 180 Hz, all wet in the send |
| [Reverb](../modules/effects/reverb.md) | **Size** 60%, **Decay** 3.2 s, **Mix** 45%, after the delay |

## Another controller?

The mappings are the MiniLab's factory map, but none of them is fixed:

- **Map Mode** (**Ctrl+L**): click a knob, move a control on your controller, and that knob is yours. The [Controller Map](../getting-started/interface-overview.md#midi-learn) (**MIDI In ▸ Controller Map…**) lists every mapping, to change or clear.
- **If an encoder steps oddly,** your controller's encoders send steps rather than positions. Choose **Hardware ▸ Encoder** on that knob's right-click menu, or in the Controller Map.
- **The pads:** right-click **Pads A** or **Pads B**, choose **Learn in order** and hit your pads from the first. To learn just one, right-click its pad on the group's face and choose **Learn MIDI**.
- **Let the encoders follow you.** Teach Soba your controller and press **Ctrl+Shift+L**: the encoders turn whatever module you select, not just the knobs mapped here.

## Related

- [Pad Kit](./pad-kit.md): the drum half on its own
- [Live Looper](./live-looper.md): looping a live input
- [MIDI Learn](../getting-started/interface-overview.md#midi-learn): mappings, pickup and encoders
- [Controls](../modules/midi/controls.md) and [Pad](../modules/midi/pad.md)
