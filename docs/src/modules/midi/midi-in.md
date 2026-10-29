# MIDI In

**Module ID** `input.midi` · **Category** Source

MIDI In is a MIDI controller as a cable. It names a device and a channel, and every module patched from its purple **MIDI** output hears that device on that channel, whatever its own **Device** says. It makes no sound and has no signal to send: the cable is a choice of where the MIDI comes from, not a stream of it.

Every module that hears MIDI ([Note In](./note-in.md), [Pad](./pad.md), [Control](./control.md), the [Clock](../modulation/clock.md) and the [MIDI Monitor](./midi-monitor.md)) can choose a device of its own, in its right-click menu (see [Listening to one device](./note-in.md#listening-to-one-device)). That suits one module. With several, each keeps its own setting, and moving to another controller means changing them all. Patch them from one MIDI In instead, and the device is chosen in one place.

## Output

| Port | Signal Type | Description |
|------|-------------|-------------|
| **MIDI** | MIDI (Purple) | Patch into a module's **MIDI** input, or a group's, and it hears this device and channel |

A MIDI cable goes only into a MIDI input. One MIDI In can feed as many modules as you like. A module's MIDI input is the purple jack at the left end of its title bar: the controller comes in at its head, and the module grows no taller for it.

## Parameters

| Control | Options | Default | Description |
|---------|---------|---------|-------------|
| **Ch** (Channel) | Omni / 1 – 16 | Omni | The one channel the modules patched from it hear. Omni hears every channel |

The **Device** is chosen on the face. It's saved with the patch by the device's name, as a module's own Device is, so the patch finds it on another computer with the same gear.

## The face

- **The device's name.** Click it to choose: **Any**, or one of the devices ticked in **MIDI In** on the toolbar. A device set on another computer, or not plugged in, shows *(not open)*, and is kept until you choose another.
- **A lamp** beside it, purple while the device is open, grey while it isn't. It flares with each message the device sends on the MIDI In's channel, so a key or pad struck shows on the node it comes in through.
- **Underneath**, the channel, and how many modules hear it.

## What a cable does

A module patched from a MIDI In:

- **hears the MIDI In's device**, in place of its own **Device**. The Device entry of its right-click menu greys while the cable is in, the way a knob greys under a cable, and the device shows after its title;
- **hears only the MIDI In's channel**, if it names one. A Note In or a MIDI Monitor with a **Ch** of its own hears where the two agree: leave it on Omni to hear the MIDI In's channel. MIDI clock and transport have no channel, so a Clock follows the device's clock whatever the Channel;
- **claims notes from that device**, if it's a [Pad](./pad.md): a Note In on Omni skips the pad's note from the MIDI In's device, and only those (see [Claimed notes](./note-in.md#claimed-notes));
- **still hears the computer keys and the piano**, which aren't a device.

Unplug the cable and the module hears its own Device again. Nothing about the module changes while it's patched, so a patch from before MIDI In plays exactly as it did.

## Through a group

The cable runs through a group's jacks. Put eight Pads in a group, give the group a **MIDI** jack, and inside, patch that jack into each Pad's **MIDI** input (the Library's **Pads ×8** comes that way). Plug a MIDI In into the jack outside and every pad inside hears that controller. Swap the MIDI In's device and the whole group follows. It works through a group inside a group too.


A group's MIDI jack with nothing plugged into it outside leaves the modules inside on their own Devices.

## Uses

**Two keyboards, two synths.** Two MIDI Ins, one for each keyboard, each patched into the Note In of its own synth. Point the patch at different keyboards by changing the MIDI Ins alone.

**A split controller.** One controller sending its lower half on channel 1 and its upper half on channel 2: two MIDI Ins on the same device, one on each channel, into a bass Note In and a lead Note In.

**A drum group.** A group of Pads and Drums with one purple jack, so the kit plays from whichever pad controller is plugged into it.

**Watching what a module hears.** Patch the same MIDI In into a [MIDI Monitor](./midi-monitor.md) as well, and the log shows exactly what its modules hear.

## Related modules

- [Note In](./note-in.md): notes as Pitch, Gate and Velocity, and choosing a device
- [Pad](./pad.md), [Control](./control.md): a controller's pads and faders as cables
- [MIDI Monitor](./midi-monitor.md): what a device sends
- [Signal Types](../../concepts/signal-types.md#midi): the purple cable
