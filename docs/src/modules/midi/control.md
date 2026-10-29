# Control

**Module ID** `input.control` · **Category** Source

A Control turns one fader, knob, wheel or strip of a MIDI controller into a cable. It hears one control and puts it out as a Control signal, so you can patch it wherever a cable goes: a fader into a filter's **Cutoff**, a knob into a VCA, the mod wheel through an [Attenuverter](../utilities/attenuverter.md) into an LFO's **Rate**.

A controller is as many Controls as it has faders and knobs to patch, in a [group](../../concepts/groups.md): four for a MiniLab 3's faders, nine for a controller with nine, one for an expression pedal. The Library has two ready to use: **MiniLab 3 Faders** and **Controls ×4** (see [The Library's Controls](#the-librarys-controls)). It works the way a [Pad](./pad.md) does: one Pad is one pad, and eight of them make a pad controller.

[MIDI Learn](../../getting-started/interface-overview.md#midi-learn) can also turn a knob from your controller, but only a knob that's already on a module. A Control is for when the controller is the source: one move feeding several places, a fader scaled and flipped before it arrives, or a control mixed with an LFO.

It listens to the devices ticked in **MIDI In** on the toolbar, or to the one chosen under **Device** in its right-click menu (see [Listening to one device](./note-in.md#listening-to-one-device)), or to the one a [MIDI In](./midi-in.md) patched into its purple **MIDI** input names. It reads each move on its own sample, as [Note In](./note-in.md) reads notes.

## Outputs

| Port | Signal Type | Description |
|------|-------------|-------------|
| **Out** | Control (Orange) | The control: 0 to 1, or −1 to 1 when its range is bipolar |

## The face

One slim fader that moves with the hardware, showing what its cable carries. The fill rises from the bottom, or from the middle when it's bipolar. The cap warms to orange while the control moves, and cools again once it rests. Beside the fader is what it hears (`CC 74`, `Bend`, `Mod`, `Press`) and the value it's sending, lit orange once a cable leaves it. Along the top are the channel it hears and **learn**. Hover the fader to read it in full.

A new Control hears the mod wheel, so it works with any keyboard before you learn anything.

### On a group's face

In a group, right-click the fader and choose **Show on group**, and it's on the group's face too, moving there with the hardware. Faders on a face sit in a row, up to six across, each under its name. Right-click one there to rename it, learn it, choose what it hears, or **Hide from group**. The Library's groups and old Controls patches come with every fader on the face, each named for its jack.

## Learning a control

1. Click **learn** at the top right of the face. The fader blinks purple, and the status bar asks you to move something.
2. Move a fader or a knob, bend the pitch wheel or strip, or press a key down on your controller.

The Control now hears that control, on the channel it came on. If it was an endless encoder, the next few steps tell it so, and it counts steps from then on. Learning it again replaces what it heard, and **Undo** takes a learn back. To give up before moving anything, press `Escape` or click **cancel**. In Map Mode, clicking the fader starts the learn.

Learning a Control only teaches the Control. Nothing is mapped, and a note you play while it waits still plays.

### Learning a group in order

To learn every Control in a group at once, right-click the group and choose **Learn in order**, then move your controller's faders and knobs one after another. The Controls take them in the order they sit: the top row first, each row left to right. The one waiting blinks purple and says **skip**, the ones to come show their number, and each one learned shows a tick. Press `Tab` to leave one as it is, or `Escape` to stop, keeping what was learned. A control already learned in the walk is passed by, so a fader you're still moving doesn't teach the next Control too. The whole walk is one **Undo**.

It's the same walk a group of Pads takes (see [Learning a group in order](./pad.md#learning-a-group-in-order)), and a group holding both learns its pads and faders in one go.

## Its menu

Right-click the fader to learn it from the menu, or to choose what it hears by hand:

| Item | Choices | Description |
|------|---------|-------------|
| **Source** | CC / Pitch Bend / Mod Wheel / Pressure | What it hears. For a CC, set its number here too |
| **Channel** | Omni / 1 – 16 | Only hear this MIDI channel. Omni hears all of them. A learned Control hears the channel it was learned on |
| **Range** | Auto / 0 to 1 / −1 to 1 | **0 to 1** runs from the bottom of the control's travel to the top. **−1 to 1** puts 0 at the middle, for a bend or for a fader that should push both ways. **Auto** is −1 to 1 for the pitch bend, which rests in the middle, and 0 to 1 for everything else |
| **Hardware** | Absolute / Encoder (64 ±) / Encoder (1 / 127) | For a CC: a knob or fader with ends, or an endless encoder that sends steps. Learning tells them apart from the first few values the control sends |

An **endless encoder** has no ends, so the Control counts its steps instead: each step moves it 1% of the way, up or down, from wherever it stands, and it stops at the ends. A hundred steps take it from the bottom to the top. Telling a Control that was a fader that it's an encoder doesn't move it: the next step goes on from where it is.

## Parameters

| Control | Range | Default | Description |
|---------|-------|---------|-------------|
| **Slew** | Off – 2 s | 20 ms | How long the output takes to follow its control. A MIDI CC has only 128 steps, and a filter's cutoff swept by one jumps from step to step with a faint zipper. 20 ms smooths the steps away without making the control feel late. Turn it up for slow, gliding moves, or down to Off for a control that should land at once |

## The Library's Controls

Both are under **Library** ▸ **Controllers** in the add menu, their faders on their faces. Each has a **MIDI** jack that goes into every Control inside: plug a [MIDI In](./midi-in.md) into it and they all hear that controller, or leave it empty and each hears its own **Device**.

| Group | Controls | Jacks |
|-------|----------|-------|
| **MiniLab 3 Faders** | An Arturia MiniLab 3 as it comes from the box: its four faders on CC 82, 83, 85 and 17, the pitch strip and the mod strip | **Fader 1** – **Fader 4**, **Bend**, **Mod** |
| **Controls ×4** | A keyboard's own controls: the mod wheel, the pitch bend (−1 to 1), the pressure and CC 74, which many controllers send from their first knob | **A**, **B**, **C**, **D** |

For another controller, add **Controls ×4**, right-click it, choose **Learn in order** and move four of its controls. Double-click the group to open it: duplicate a Control for each fader more, right-click each one's **Out** and choose **Show on group** to give it a jack, and its fader's menu to put it on the face.

## Patches made with Controls

Controls, the module this one replaced, was four slots, **A** to **D**, in one node. A patch made with it opens with a group named **Controls** in its place, holding four Controls, each hearing what its slot heard, with the **Slew** they shared, hearing the same Device, its fader on the group's face. The group's jacks are **A** to **D**, so every cable stays where it was, and it sounds exactly as it did.

## Uses

- **A fader on a filter.** Learn a Control from a fader and patch **Out** into an SVF Filter's **Cutoff**. The filter follows the fader with no zipper noise at the default **Slew**.
- **The touch strips.** A Control on the pitch bend and one on the mod wheel: the bend into a filter for a wah, the mod strip into a vibrato's depth.
- **One fader, several places.** Patch one output into as many inputs as you like, and put an [Attenuverter](../utilities/attenuverter.md) in front of any that should move the other way or less.
- **Pressure as expression.** A Control on **Pressure** hears how hard you press a held key: patch it into a VCA or a filter to swell a note as you lean in.
- **An expression pedal.** One Control, learned from the pedal, into a VCA's **CV** for a volume pedal or a filter for a wah.
