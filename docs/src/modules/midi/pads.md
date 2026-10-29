# Pads

**Module ID** `input.pads` · **Category** Source

Pads turns the pads of a MIDI pad controller into gates: finger drums, triggers and latches. Each of its eight pads has a **Gate** output, so a pad can strike a [Drum](../sources/drum.md), open an [ADSR](../modulation/adsr.md) or reset a sequencer. It also puts out how hard each pad was hit and how hard the held pads are pressed.

Pads send notes. Played into a [Note In](./note-in.md) they would be pitches: pad 1 a low C, pad 2 a C♯. Pads hears those notes as pads instead, and **claims** them, so a Note In listening on Omni lets them be. Hitting a pad plays its drum and never also plays the synth (see [Claimed notes](#claimed-notes)).

It listens to the devices ticked in **MIDI In** on the toolbar, or to the one chosen under **Device** in its right-click menu (see [Listening to one device](./note-in.md#listening-to-one-device)), or to the one a [MIDI In](./midi-in.md) patched into its purple **MIDI** input names. It reads each hit on its own sample, as Note In reads notes. The [Pad Kit](../../recipes/pad-kit.md) example plays five Drums from it.

## Outputs

| Port | Signal Type | Description |
|------|-------------|-------------|
| **Gate 1** – **Gate 8** | Gate (Green) | Each pad, as **Mode** says: high while held, flipped by each hit, or a short trigger |
| **Velocity** | Control (Orange) | How hard each pad was last hit, 0 to 1: a polyphonic cable with one channel per pad |
| **Hit** | Control (Orange) | How hard the latest hit was, 0 to 1, held until the next |
| **Pressure** | Control (Orange) | How hard the held pads are pressed (aftertouch), 0 to 1: the hardest of them. It falls to 0 when you let go |

**Hit** is the cable for velocity on a kit. Patch it into every Drum's **Accent**: a Drum reads its Accent at the moment its trigger rises, and Hit changes on that same sample, so each Drum hears how hard its own pad was struck. Two pads hit together, as a two-finger hit sends them, are struck a sample apart (about 20 µs), so each still gets its own.

**Pressure** hears polyphonic aftertouch, pad by pad, and channel pressure on the pads' channel. A pad controller sends whichever it is set to.

## Parameters

| Control | Choices | Default | Description |
|---------|---------|---------|-------------|
| **Mode** | Momentary / Latch / Trigger | Momentary | **Momentary**: a pad's gate is high while the pad is held. **Latch**: each hit flips it, open then shut, however long the pad is held. **Trigger**: a 5 ms gate per hit, however long the pad is held |

A pad hit again while its gate is high (held in Momentary, or inside its 5 ms in Trigger) drops its gate for one sample and raises it again, so a drum hears a new hit.

## The face

Eight pads in two rows of four, pad 1 at the top left. A hit flashes its pad in the gate's green, brighter and wider the harder it was struck, and the flash fades over about a third of a second. A pad whose gate is open, held or latched, stays softly lit, and glows a little more the harder you press it. Each pad shows its note in the corner, and its channel too if it has one of its own. Along the top are the channel the pads hear and, unless the Mode is Momentary, the mode.

**Click a pad** to play it. Toward the top of a pad is a hard hit, toward the bottom a soft one. In Momentary mode the gate stays high for as long as you hold the mouse button.

## Learning the pads

When you add Pads, it hears notes 36 to 43 on channel 10. Those are the General MIDI drum notes, which most pad controllers send out of the box, so the pads often work before you learn anything. If yours send something else:

1. Click **learn** at the top right of the face. Pad 1 blinks purple, and the status bar asks you to hit a pad.
2. Hit the first pad on your controller.

Pad 1 takes that note and the channel it came on, and pads 2 to 8 follow on the next seven notes. **Undo** takes a learn back. To give up before hitting anything, press `Escape` or click **cancel**.

**One pad on its own.** If your controller's pads aren't seven notes in a row, right-click a pad and choose **Learn MIDI for this pad**, then hit the pad it should be. That pad keeps its own note and channel even when the row is learned again. **Follow pad 1** in the same menu gives it back to the row.

A pad's menu also sets its **Note** and **Channel** by hand. Pad 1's are the row's: changing them moves every pad that follows it. What the pads hear is saved with the patch.

## Claimed notes

The notes a Pads module listens to are its own. A [Note In](./note-in.md) whose **Ch** is Omni skips them, so a pad never also plays the synth, and the pianos and the sequencers' ears don't light for them either. The [MIDI Monitor](./midi-monitor.md) still shows them.

A Pads set to one **Device** claims only that device's notes. Set it to the pad controller, and another keyboard's note 36 on channel 10 still plays the synth.

A Note In set to a channel hears that channel's notes, claimed or not. To play a melody on the pads as well as drums, set a Note In to the pads' channel.

The computer keys and the on-screen pianos are never claimed: they always play Note In.

## Uses

- **Finger drums.** Each **Gate** into a [Drum](../sources/drum.md)'s **Trig**, and **Hit** into every Drum's **Accent**. That's [Pad Kit](../../recipes/pad-kit.md).
- **A choke.** Patch the closed hat's pad into the open hat's **Choke** as well as the closed hat's **Trig**, as a drummer's foot does.
- **Swells and stabs.** In **Momentary**, a pad's gate into an [ADSR](../modulation/adsr.md) holds a chord as long as you hold the pad. **Pressure** into a filter's **Cutoff** opens it as you lean in.
- **Switches.** In **Latch**, a pad holds its gate open until you hit it again: a drone through a VCA you start and stop by hand, or a [Logic](../utilities/logic.md) module's **B**, letting a sequencer's gates through only while the pad is lit.
- **Triggers.** In **Trigger**, a pad's short gate into a [Step Sequencer](../utilities/sequencer.md)'s **Reset** starts the pattern over, however long the pad is held.
