# Strike

**Module ID** `output.strike` · **Category** Output

A Strike is one drum hit out to hardware: one pad, track or part of a drum machine or sampler. A [Kit](./kit.md) has eight lanes, the width of a [Trigger Sequencer](../utilities/trigger-sequencer.md). The hardware has as many parts as it has: sixteen pads a bank on an SP-404MKII, six parts on a volca drum. Use a Strike for each part and the kit is as wide as the device.

Patch a device's purple **MIDI** output into a Strike's **MIDI** input, and the Strike plays that device. The device is an [Instrument](./instrument.md), a Kit or [MIDI Out](../output/midi-out.md). The Strike takes the device's port and channel, and you choose its part by name from the device's profile: **Kick**, **Track 3**, **Pad 13**. It makes no sound of its own. The hardware does.

## Inputs

| Port | Signal Type | Description |
|------|-------------|-------------|
| **MIDI** | MIDI (Purple) | The device it plays, from an Instrument's, Kit's or MIDI Out's **MIDI** output, or a group's jack that leads to one. Its jack is on the left end of the title bar |
| **Gate** | Gate (Green) | Strikes the part as it rises, and lets the note go as it falls |
| **Vel** | Control (Orange) | How hard it strikes, 0 to 1, read at the moment its gate rises. Unpatched, the **Velocity** knob sets it |

The module has no outputs.

## Parameters

| Control | Range | Default | Description |
|---------|-------|---------|-------------|
| **Velocity** | 0 – 1 | 0.8 | How hard it strikes while **Vel** is unpatched |

Its **Port**, **Channel** and **Note** are set on the face (see below). They're saved with the patch like any control.

## The face

- **The part**, in large letters: the part it strikes, or **Note 36** when it plays its own note. Click it for the device's parts, then choose one. Hover a part to see its note and channel. **Its own note** at the bottom plays the **Note** under it, on the Strike's channel, for a device without a profile.
- **A lamp** that flares as it strikes, brighter the harder it's hit, and stays lit while it holds its note.
- **Underneath**: the note, the port and channel, and the device it's patched from (**note 36 · A · ch 1 · SP-404MKII**).

Unpatched, the same menu chooses the **Port** and **Channel**. Patched, they're the device's, and the menu says so.

## What the cable does

The purple cable says which device the Strike plays. It doesn't carry a stream. Each frame, the editor follows the cable back to the device, through any groups' jacks on the way, and copies the device's **Port**, **Channel** and profile onto the Strike. Change the device's port, or choose another profile, and every Strike patched from it follows. The copy belongs to the same undo step as the change.

A cable always runs out of a device, as it runs out of a [MIDI In](./midi-in.md). One device can be patched into as many Strikes and [CC Outs](./cc-out.md) as you like.

Unplug the cable and the Strike keeps the port, channel and part it had. They're its own to change again.

## How it works

**A strike.** The gate rising sends a Note On. It plays the part's note on the part's channel, or on the Strike's channel if the part has none. A part with only a channel is struck with middle C (note 60): any note plays a volca drum's part. The gate falling sends the Note Off. Struck again while its note still sounds, it lets the note go first.

**In time.** Each Strike sends through a ring of its own to the MIDI sender, every message stamped with the sample it was struck on. The sender sends what's due from every ring at once, in the order it's due. Sixteen Strikes hit on one step go out together, in jack order, and a hit one sample later goes after them. A busy port's budget never holds back a note.

**Letting go.** The Clock stopping, the gate unpatched, another part chosen and the module deleted all let its note go.

## Playing it

- **A whole SP-404 bank.** Add **SP-404 Bank A** from the Library's **Hardware** section: an Instrument playing the SP-404MKII, and sixteen Strikes on pads 1 to 16. Patch two [Trigger Sequencers](../utilities/trigger-sequencer.md) into **Gate 1–8** and **Gate 9–16**.
- **The volca drum's six parts.** **volca drum Kit**, in the same section, strikes each part on its own channel, 1 to 6.
- **The Digitakt's tracks, beside its knobs.** **Digitakt Kit** gives each track a Strike, and its Instrument's twelve knobs are the Digitakt's own controls.
- **A ninth lane for a Kit.** Patch a Kit's **MIDI** output into a Strike, and give the Strike a part none of the Kit's lanes play.
- **Two Strikes on one pad.** A sequencer's accent lane and its main lane both into Strikes on the kick, each with its own velocity. Both notes go, in order, on the device's channel.

## Related

- [Kit](./kit.md): eight lanes in one node
- [CC Out](./cc-out.md): one of the device's knobs, as its own node
- [Instrument](./instrument.md): the device, with its knobs
- [Instrument Profiles](../../hardware/instruments.md): the parts each kit has
