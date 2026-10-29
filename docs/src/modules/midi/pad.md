# Pad

**Module ID** `input.pad` · **Category** Source

A Pad is one pad of a MIDI pad controller, as a gate: a finger drum, a trigger or a latch. Its **Gate** can strike a [Drum](../sources/drum.md), open an [ADSR](../modulation/adsr.md) or reset a sequencer, and it also puts out how hard it was hit and how hard it's pressed.

A pad controller is as many Pads as it has pads, in a [group](../../concepts/groups.md): eight for a MiniLab, sixteen for an SP-404MKII, one for a footswitch. The Library's **Pads ×8** is eight of them, ready to play (see [Pads ×8](#pads-8)). A voice in Soba is built from small parts the same way: an oscillator, a filter and an envelope in a group, not one Voice module.

Pads send notes. Played into a [Note In](./note-in.md) they would be pitches: pad 1 a low C, pad 2 a C♯. A Pad hears its note as a pad instead, and **claims** it, so a Note In listening on Omni lets it be. Hitting a pad plays its drum and never also plays the synth (see [Claimed notes](#claimed-notes)).

It listens to the devices ticked in **MIDI In** on the toolbar, or to the one chosen under **Device** in its right-click menu (see [Listening to one device](./note-in.md#listening-to-one-device)), or to the one a [MIDI In](./midi-in.md) patched into its purple **MIDI** input names. It reads each hit on its own sample, as Note In reads notes. The [Pad Kit](../../recipes/pad-kit.md) example plays five Drums from a group of eight.

## Outputs

| Port | Signal Type | Description |
|------|-------------|-------------|
| **Gate** | Gate (Green) | The pad, as **Mode** says: high while held, flipped by each hit, or a short trigger |
| **Velocity** | Control (Orange) | How hard the pad was last hit, 0 to 1, held until the next hit. It changes on the sample the gate rises |
| **Pressure** | Control (Orange) | How hard the pad is pressed (aftertouch), 0 to 1. It falls to 0 when you let go |

**Velocity** is the cable for a drum's **Accent**. A Drum reads its Accent at the moment its trigger rises, and a Pad's Velocity changes on that same sample, so the drum hears how hard its pad was struck. Two pads hit together each put out their own, on the same sample.

**Pressure** hears polyphonic aftertouch on the Pad's note, and channel pressure on its channel while it's held. A pad controller sends whichever it is set to.

## Parameters

| Control | Choices | Default | Description |
|---------|---------|---------|-------------|
| **Mode** | Momentary / Latch / Trigger | Momentary | **Momentary**: the gate is high while the pad is held. **Latch**: each hit flips it, open then shut, however long the pad is held. **Trigger**: a 5 ms gate per hit, however long the pad is held |

A pad hit again while its gate is high (held in Momentary, or inside its 5 ms in Trigger) drops its gate for one sample and raises it again, so a drum hears a new hit.

Each Pad has its own Mode, so one pad can latch a drone while the pad beside it plays a drum.

## The face

One pad. A hit flashes it in the gate's green, brighter and wider the harder it was struck, and the flash fades over about a third of a second. While its gate is open, held or latched, it stays softly lit, and glows a little more the harder you press it. Its note is in the corner, and along the top are the channel it hears and **learn**.

**Click the pad** to play it. Toward the top is a hard hit, toward the bottom a soft one. In Momentary mode the gate stays high for as long as you hold the mouse button.

## On a group's face

A Pad in a group can put its pad on the group's face: right-click the pad and choose **Show on group**. The pads pinned there sit four to a row under the miniature, so a closed group of eight is a controller to play without opening it, with no controller plugged in.

- **Click a pinned pad** as you'd click the Pad's own: the higher you click, the harder the hit. It writes the Pad's own **Press**, so it's one pad in two places, and a cable from its Gate hears both.
- **It lights with the hardware.** Hit the pad on your controller and the pinned pad flashes, brighter the harder you hit it, as the Pad's own face does.
- **Name it** for what it plays: right-click it on the face and type **Kick** or **Rec** over the name. The name sits in its top corner, in the Pad's blue, and only on the face; its note stays in the other corner.
- Its right-click menu learns it, sets its **Note** and **Channel**, and takes it off the face with **Hide from group**.

Pinned pads are kept by undo, copy and paste, a saved patch and **My Modules**, like any [pinned control](../../concepts/groups.md#the-face). The Library's **Pads ×8** comes with all eight pinned.

## Learning a pad

A new Pad hears note 36 on channel 10: the General MIDI kick, which most pad controllers send from their first pad. If you want another pad:

1. Click **learn** at the top right of the face. The pad blinks purple, and the status bar asks you to hit a pad.
2. Hit the pad on your controller.

The Pad takes that note and the channel it came on. **Undo** takes a learn back. To give up before hitting anything, press `Escape` or click **cancel**. In Map Mode, clicking the pad starts the learn.

Right-click the pad to learn it from the menu, or to set its **Note** and **Channel** by hand. What it hears is saved with the patch.

### A new Pad takes the next note

Add, paste or duplicate a Pad beside others on the same level, and it hears the note after the highest one there, on that Pad's channel. Duplicate the Pad on 40 and the copy is on 41; duplicate a row on 36 to 39 and the copies are on 40 to 43. So a group of Pads built by hand mostly needs no learning at all. A Pad alone on its level keeps the note it came with.

### Learning a group in order

To learn every Pad in a group at once, right-click the group and choose **Learn in order**, then hit your controller's pads one after another. The Pads take them in the order they sit: the top row first, each row left to right.

- The Pad waiting for its pad blinks purple, and says **skip** along its top. The ones still to come show their number, and each one learned shows a tick, its rim staying softly lit until the walk is over.
- On the group's face, each Pad's card in the miniature lights as it's learned, and a row of dots along its foot counts the walk.
- The status bar says how far it has come and what it waits for.

Press `Tab`, or click **Skip**, to leave a Pad as it is and go on to the next. Press `Escape`, or click **Stop**, to stop early: what was learned so far is kept. Hitting a pad that has already been learned in this walk is passed by, so a pad that bounces or is hit twice doesn't take two Pads.

The whole walk is one step: **Undo** takes all of it back.

## Pads ×8

**Pads ×8**, under **Library** ▸ **Controllers** in the add menu, is a group of eight Pads on the General MIDI drum notes, 36 to 43 on channel 10, laid out in two rows of four as most controllers' pads are. Its jacks come in pairs, **Gate 1**, **Vel 1**, **Gate 2**, **Vel 2** and so on, in the order a [Trigger Sequencer](../utilities/trigger-sequencer.md)'s lanes come, so it drops in where a Trigger Sequencer was. Into the Library's **Drum Kit**, patch **Gate 1** into **Kick**, **Gate 3** into **Snare** and **Gate 7** into **Hat**, and **Vel 1** into **Accent**: General MIDI's kick, snare and closed hat.

Its **MIDI** jack goes into every Pad inside. Plug a [MIDI In](./midi-in.md) into it and all eight hear that controller; leave it empty and each hears its own **Device**.

If your controller sends other notes, right-click the group, choose **Learn in order** and hit its pads from the first (see [Learning a group in order](#learning-a-group-in-order)). Its eight pads are on its face, to click (see [On a group's face](#on-a-groups-face)); right-click one there to learn it on its own. To make a bigger pad controller, open the group and duplicate a row of Pads: the copies take the next notes, and their pads come onto the face with them. Right-click each new one's **Gate** and choose **Show on group** to give it a jack.

## Claimed notes

The note a Pad listens to is its own. A [Note In](./note-in.md) whose **Ch** is Omni skips it, so a pad never also plays the synth, and the pianos and the sequencers' ears don't light for it either. The [MIDI Monitor](./midi-monitor.md) still shows it.

A Pad set to one **Device** claims only that device's note. Set it to the pad controller, and another keyboard's note 36 on channel 10 still plays the synth.

A Note In set to a channel hears that channel's notes, claimed or not. To play a melody on the pads as well as drums, set a Note In to the pads' channel.

The computer keys and the on-screen pianos are never claimed: they always play Note In.

## Patches made with Pads

Pads, the module this one replaced, was eight pads in one node, with one **Hit** cable for every drum's Accent. A patch made with it opens with a group named **Pads** in its place, holding eight Pads, each on the note and channel its pad heard, in the same Mode, hearing the same Device. The group's jacks are **Gate 1** to **Gate 8**. A cable from **Hit** now leaves from the **Vel** jack of the pad whose gate feeds the same drum, so each drum still hears its own hit, and **Pressure** from the pad it followed. It sounds as it did, except that two pads hit together now strike on the same sample, not a sample apart.

## Uses

- **Finger drums.** A Pad's **Gate** into a [Drum](../sources/drum.md)'s **Trig**, and its **Velocity** into the Drum's **Accent**. That's [Pad Kit](../../recipes/pad-kit.md).
- **A choke.** Patch the closed hat's Pad into the open hat's **Choke** as well as the closed hat's **Trig**, as a drummer's foot does.
- **Swells and stabs.** In **Momentary**, a Pad's gate into an [ADSR](../modulation/adsr.md) holds a chord as long as you hold the pad. **Pressure** into a filter's **Cutoff** opens it as you lean in.
- **Switches.** In **Latch**, a Pad holds its gate open until you hit it again: a drone through a VCA you start and stop by hand, or a [Logic](../utilities/logic.md) module's **B**, letting a sequencer's gates through only while the pad is lit.
- **Triggers.** In **Trigger**, a Pad's short gate into a [Step Sequencer](../utilities/sequencer.md)'s **Reset** starts the pattern over, however long the pad is held.
- **A footswitch.** One Pad on the note your pedal sends, into a [Looper](../utilities/looper.md)'s **Rec**.
