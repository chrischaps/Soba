# CC Out

**Module ID** `output.cc_out` · **Category** Output

A CC Out is one controller out to hardware: a CC, or one of the device's own knobs by name. [MIDI Out](../output/midi-out.md) has four CC jacks and an [Instrument](./instrument.md) has twelve knobs. Use as many CC Outs as the patch needs.

Patch a device's purple **MIDI** output into a CC Out's **MIDI** input, and the CC Out plays that device. The device is an Instrument, a [Kit](./kit.md) or MIDI Out. The CC Out takes the device's port and channel, and you can choose one of the device's parameters by name: **Cutoff**, **Decay**, **Bus 1 Ctrl 3**. It goes out as the device's profile says, as a CC, a 14-bit pair or an NRPN, across the parameter's own range. Without a parameter it sends a plain 7-bit CC, its **CC** number.

## Inputs

| Port | Signal Type | Description |
|------|-------------|-------------|
| **MIDI** | MIDI (Purple) | The device it plays, from an Instrument's, Kit's or MIDI Out's **MIDI** output, or a group's jack that leads to one. Its jack is on the left end of the title bar |
| **In** | Control (Orange) | 0 to 1 across what it sends, in place of the **Value** knob while patched: an [LFO](../modulation/lfo.md) here sweeps the real thing |

The module has no outputs.

## Parameters

| Control | Range | Default | Description |
|---------|-------|---------|-------------|
| **Value** | 0 – 1 | 0.5 | What it sends, across its range, while **In** is unpatched |

Its **Port**, **Channel** and **CC** are set on the face (see below). They're saved with the patch like any control.

## The face

- **The parameter**, in large letters: the one it sends, or **CC 74**. Click it for the device's parameters, grouped as on an Instrument's knob menu. Choosing one moves **Value** to where that parameter starts. **A plain CC** at the bottom sends the **CC** under it.
- **A bar** on the left: the value it last sent.
- **Underneath**: that value as the device reads it, the port and channel, and the device it's patched from (**64 · A · ch 1 · MicroFreak**).

Unpatched, the same menu chooses the **Port** and **Channel**. Patched, they're the device's, and the menu says so. A parameter with a channel of its own goes there whatever the device's channel is, as the Digitakt's delay goes to its FX channel.

## How it works

It sends where its value changes, at the sample it changes, and no more often than once a millisecond. That's [MIDI Out's lanes](../output/midi-out.md#how-it-works) again: a slow LFO sends a value now and then, and a still knob sends nothing. A busy port's budget thins it before it holds back a note. Given a new parameter, it sends its value again. A relative parameter, one that nudges the device's value, is taken to be where the device already is.

The cable works as a [Strike's](./strike.md#what-the-cable-does) does: the editor copies the device's port, channel and profile onto it each frame. Unplugged, it keeps what it had.

## Playing it

- **More than four CCs into a DAW.** A MIDI Out on the DAW's port, patched into six CC Outs on CCs 20 to 25, each swept by its own LFO.
- **A knob the Instrument hasn't room for.** An Instrument already holds twelve of a Digitakt's parameters. Patch its **MIDI** into a CC Out and choose **LFO1 Depth** for a thirteenth.
- **One sweep on two devices.** One LFO into two CC Outs, each patched from a different Instrument, each on that device's cutoff.

## Related

- [Strike](./strike.md): one drum hit out, the same way
- [Instrument](./instrument.md): twelve knobs of a device in one node
- [MIDI Out](../output/midi-out.md): notes, bend, four CCs and the clock
