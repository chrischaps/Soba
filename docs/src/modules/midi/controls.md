# Controls

**Module ID** `input.controls` · **Category** Source

Controls turns the faders, knobs, wheels and strips of a MIDI controller into cables. Each of its four slots, **A** to **D**, hears one control and puts it out as a Control signal, so you can patch it wherever a cable goes: one fader into a filter's **Cutoff**, another into a VCA, a third through an [Attenuverter](../utilities/attenuverter.md) into an LFO's **Rate**.

[MIDI Learn](../../getting-started/interface-overview.md#midi-learn) can also turn a knob from your controller, but only a knob that's already on a module. Controls is for when the controller is the source: one move feeding several places, a fader scaled and flipped before it arrives, or a control mixed with an LFO.

It listens to the devices ticked in **MIDI In** on the toolbar, or to the one chosen under **Device** in its right-click menu (see [Listening to one device](./note-in.md#listening-to-one-device)), and reads each move on its own sample, as [Note In](./note-in.md) reads notes.

## Outputs

| Port | Signal Type | Description |
|------|-------------|-------------|
| **A**, **B**, **C**, **D** | Control (Orange) | Each slot's control: 0 to 1, or −1 to 1 when its range is bipolar |

## The face

Each slot is a slim fader that moves with the hardware, showing what its cable carries. The fill rises from the bottom, or from the middle for a bipolar slot. The cap warms to orange while the control moves, and cools again once it rests. A slot's letter lights up once a cable leaves it. Under each fader is what the slot hears (`CC 74`, `Bend`, `Mod`, `Press`) and its channel. Hover a fader to read it in full, with the value it's sending now.

When you add a Controls module, its slots already hear a keyboard's own controls, so they work before you learn anything:

| Slot | Hears |
|------|-------|
| **A** | The mod wheel (CC 1) |
| **B** | The pitch bend, from −1 to 1 |
| **C** | Channel pressure |
| **D** | CC 74, which many controllers send from their first knob |

## Learning a control

1. Right-click a slot's fader and choose **Learn MIDI**. Its label blinks purple, and the status bar asks you to move something.
2. Move a fader or a knob, bend the pitch wheel or strip, or press a key down on your controller.

The slot now hears that control, on the channel it came on. Learning a slot again replaces what it heard, and **Undo** takes a learn back. To give up before moving anything, press `Escape`, or right-click the blinking slot and choose **Cancel MIDI Learn**. What the slots hear is saved with the patch.

Learning a slot only teaches the slot. Nothing is mapped, and a note you play while it waits still plays.

## A slot's menu

Right-click a fader to choose what its slot hears by hand:

| Item | Choices | Description |
|------|---------|-------------|
| **Source** | CC / Pitch Bend / Mod Wheel / Pressure | What the slot hears. For a CC, set its number here too |
| **Channel** | Omni / 1 – 16 | Only hear this MIDI channel. Omni hears all of them. A learned slot hears the channel it was learned on |
| **Range** | Auto / 0 to 1 / −1 to 1 | **0 to 1** runs from the bottom of the control's travel to the top. **−1 to 1** puts 0 at the middle, for a bend or for a fader that should push both ways. **Auto** is −1 to 1 for the pitch bend, which rests in the middle, and 0 to 1 for everything else |
| **Hardware** | Absolute / Encoder (64 ±) / Encoder (1 / 127) | For a CC: a knob or fader with ends, or an endless encoder that sends steps. Learning tells them apart from the first few values the control sends |

An **endless encoder** has no ends, so the slot counts its steps instead: each step moves the slot 1% of the way, up or down, from wherever it is, and it stops at the ends. A hundred steps take it from the bottom to the top.

## Parameters

| Control | Range | Default | Description |
|---------|-------|---------|-------------|
| **Slew** | Off – 2 s | 20 ms | How long each output takes to follow its control. A MIDI CC has only 128 steps, and a filter's cutoff swept by one jumps from step to step with a faint zipper. 20 ms smooths the steps away without making the control feel late. Turn it up for slow, gliding moves, or down to Off for a control that should land at once |

## Uses

- **A fader on a filter.** Learn slot **A** from a fader and patch **A** into an SVF Filter's **Cutoff**. The filter follows the fader with no zipper noise at the default **Slew**.
- **The touch strips.** Slot **B** hears the pitch strip and slot **A** the mod strip as soon as the module is added. Patch them into anything: the bend into a filter for a wah, the mod strip into a vibrato's depth.
- **One fader, several places.** Patch one output into as many inputs as you like, and put an [Attenuverter](../utilities/attenuverter.md) in front of any that should move the other way or less.
- **Pressure as expression.** Slot **C** hears how hard you press a held key: patch it into a VCA or a filter to swell a note as you lean in.
