# MIDI Out

**Module ID** `output.midi` · **Category** Output

MIDI Out plays other instruments from your patch: a hardware synth, a drum machine, or a soft synth in a DAW. A **Gate** and a **Pitch** cable become notes, a poly cable becomes chords, a **Bend** and a **Pressure** cable become pitch bend and aftertouch, and four control cables become CCs, each a plain CC, a fine 14-bit one or an NRPN. A **Program**, with its bank, picks the sound the device plays. Turn on a port's **Clock** in [the Rig](../../getting-started/interface-overview.md#the-rig) and the patch's [Clock](../modulation/clock.md) goes out on it too, so the gear on that port follows Soba's tempo, starts when it starts and stops when it stops.

Patch a [Step Sequencer](../utilities/sequencer.md) into it and the sequencer plays your synth. Patch a [Chord Sequencer](../utilities/chord-sequencer.md) and it plays chords.

MIDI Out makes no sound of its own. It works in the desktop app, and in the browser in Chrome, Edge and Firefox once you've clicked **🎹 Connect MIDI** (in the device menu on its face, or the toolbar's MIDI In). Safari has no MIDI.

## Inputs

| Port | Signal Type | Description |
|------|-------------|-------------|
| **Pitch** | Control (Orange), poly | V/Oct, 0 = C4 (note 60), rounded to the nearest note. Read as each gate rises |
| **Gate** | Gate (Green), poly | A note starts as a channel's gate rises and ends as it falls |
| **Velocity** | Control (Orange), poly | How hard each note is struck, 0 to 1 (1 to 127). 0.8 with no cable |
| **Bend** | Control (Orange) | -1 to 1, full throw down to full throw up, sent as Pitch Bend wherever it moves. Unplug it and the bend springs back to the middle |
| **Pressure** | Control (Orange), poly | 0 to 1, sent as aftertouch wherever it moves. A one-channel cable sends Channel Pressure; a poly cable sends each note its own (Poly Pressure), from its voice's channel |
| **CC 1** … **CC 4** | Control (Orange) | 0 to 1, sent on the lane's CC number (0 to 127), or finer in 14-bit or NRPN mode, wherever the value moves |
| **Send Program** | Gate (Green) | Sends the Bank and Program again as it rises: at the top of a song, say, so the device is on the right sound however it was left |

Every input is optional. The module has no outputs.

## Parameters

| Control | Range | Default | Description |
|---------|-------|---------|-------------|
| **Port** | A – H | A | Which device plays. Each port's device is a setting of this computer, not of the patch (see below) |
| **Ch** | 1 – 16 | 1 | The MIDI channel the notes and CCs go out on |
| **CC 1** … **CC 4** | 0 – 127 | 1, 74, 71, 10 | The controller number each lane sends. The defaults are the mod wheel, cutoff, resonance and pan, which most synths answer to |
| **Bend** (Bend Range) | Off, 1 – 24 st | Off | How far a full bend goes. Set, Soba tells the device as the module starts and whenever it changes (RPN 0, Pitch Bend Sensitivity). **Off** leaves the device's own range alone, for gear that sets it on its panel |
| **Prog** (Program) | Off, 0 – 127 | Off | The Program Change to send, as the number goes out. Many manuals count programs from 1: their program 1 is 0 here |
| **Bank** (Bank MSB) | Off, 0 – 127 | Off | The Bank Select MSB (CC 0) sent before the program |
| **LSB** (Bank LSB) | Off, 0 – 127 | Off | The Bank Select LSB (CC 32) sent before the program |

Each lane's **mode**, and its NRPN, are in the right-click menu of its bar on the face (see [CC lanes](#cc-lanes)).

## The display

- **The lamp and device**, top left. Green while notes are going out, and it flashes with each one. Amber while the port has no device, or its device is unplugged. Red when the device won't open, usually because another app has it. **Click the device's name** to choose another.
- **The channel**, top right, and before it the program (**P4**), if one is chosen. It glows as the program goes out.
- **Two wheels**, at the left, as on a keyboard: the bend, from the middle, and the pressure, from the bottom. Hover them for the values.
- **A keyboard** of five octaves, C2 to C7, with every note the module is holding lit, so a chord shows its shape. A note beyond either end lights the arrow at that end.
- **Four bars**, one per CC lane, showing the value last sent, with what the lane sends beneath: **CC 74**, **CC 1+33** for a 14-bit pair, **NRPN 1:20**. A lane with no cable is an empty slot. Right-click a bar to change its mode.
- **The clock**, bottom right: four beat lamps that take turns while the port sends clock. The word **clock** under them is dim while the port's clock is off.
- **An amber ring** round the lamp while the port's [budget](#busy-ports) is full and its CCs are thinning.

## Ports, not devices

A patch doesn't name a device. It names a **port**, A to H, and this computer remembers which device each port plays. Choose port A's device once and every MIDI Out on port A plays it, in this patch and the next. Share the patch and it plays whatever synth is on port A on the other computer. So it travels without either of you editing it.

Click the device's name on the face to choose. The list shows every MIDI output this computer has, and it updates as you plug devices in. In the browser the menu offers **🎹 Connect MIDI** first, until you've allowed it. A port's device stays chosen while it's unplugged and plays again once it's back, even if the system has renumbered it (`2- Digitakt` is now `3- Digitakt`).

To see all eight ports at once, open [the Rig](../../getting-started/interface-overview.md#the-rig) (`Ctrl + Shift + H`, or **The Rig…** in the **MIDI In** menu). It shows each port's device, its clock and offset, how the port is doing, and the last message it sent. It lists the MIDI inputs too, and its **Thru** routes pass an input straight to a port with no MIDI Out at all: a keyboard playing a synth through Soba, with no cables in the patch.

On Windows only one app can open a MIDI port at a time. If the lamp is red, close whatever else has the device (a DAW, or the maker's editor app), and MIDI Out tries again within a couple of seconds. To play a DAW on the same computer, use a virtual MIDI port such as [loopMIDI](https://www.tobias-erichsen.de/software/loopmidi.html): MIDI Out plays the port, and the DAW listens to it.

## How it works

**In time with the sound.** Notes don't go out when the patch works them out. They go out when the audio beside them reaches your speakers, which is your audio device's delay later. So a hardware synth and Soba's own sounds play together, and each note is on time to within a millisecond or two. The browser can't promise to send at a given millisecond, so there Soba hands each message over up to a tenth of a second early, with the moment it's due, and the browser sends it then.

**Chords.** Each channel of a poly cable is a voice. A Chord Sequencer's four-voice cable plays four-note chords, every note starting and ending with its own channel's gate. A one-channel Gate with a poly Pitch strikes the whole chord together. Two voices on the same note keep it sounding until the last of them lets go.

**No stuck notes.** Notes end cleanly:

- when the Clock stops, even mid-chord. A gate still held stays quiet until it rises again;
- when you delete the module or open another patch, and when the audio stops. Soba lets go of every note the module was holding, springs its bend back to the middle, and sends All Notes Off on its channel, unless another MIDI Out plays the same port and channel. Then that one's notes keep sounding;
- when you change the channel, the port or the port's device, on the one the notes started on.

**Clock belongs to the port.** Each port's **Clock**, in the Rig, says what it sends of the patch's Clock, with no cable and no module needed:

- **Clock + Transport**: **Start** when the Clock starts from the top (after a count-in, on the first downbeat), **24 ticks a beat** placed on the beat to the sample, **Stop** when the Clock stops, and **Continue** when it plays on, after a Song Position telling the other gear where. Most gear picks up from the next sixteenth;
- **Clock only**: the ticks alone. The gear follows the tempo but starts and stops on its own, as a looper whose loops you start by foot wants;
- **Off**: none of it.

The port sends it once, however many MIDI Outs play it, and a port no MIDI Out plays sends it too: a drum machine needs only its port's clock to follow Soba. Turn a port to **Clock + Transport** while the song plays and its gear comes in on the next sixteenth. A patch with no Clock sends no clock. See [Tempo and Sync](../../concepts/tempo-and-sync.md#leading-other-gear).

<a id="cc-lanes"></a>**CC lanes.** Each lane sends its cable wherever it moves within a block, up to a thousand times a second, so an LFO sweeps a synth's filter as smoothly as the synth can follow. Right-click a lane's bar for its mode:

- **7-bit**: one CC, 0 to 127. What most gear listens for.
- **14-bit**: two CCs, the lane's number and the one 32 above it (CC 1 and 33, say), coarse half first, for 16,384 steps. MIDI pairs the controllers 0 to 31 this way, so a lane set above 31 sends 31 and 63, and its menu says so.
- **NRPN**: a Non-Registered Parameter, for settings a device has no CC for. Set its number in the menu as the device's manual lists it, MSB and LSB (**NRPN 1:20** is number 148). Each value goes as CC 99 and 98, which choose the parameter, then Data Entry, CC 6 and 38, with 14 bits of value. Gear that reads only 7 bits of an NRPN takes the coarse half.

<a id="busy-ports"></a>**Busy ports.** A USB device takes everything Soba sends. A 5-pin DIN cable carries only about a thousand messages a second, and four fast LFOs into CCs ask for four thousand. Give a port a **Budget** in [the Rig](../../getting-started/interface-overview.md#the-rig) (**DIN · 1000/s**) and it keeps to it: notes, programs and clock always go the moment they're due, and the CC lanes, bend and pressure share what's left. While the port is full, each sends only its newest value and skips one the device already has. Each lane ends where its cable did, and the notes stay in time. SysEx sent to the port keeps to the budget too, whole message by whole message (see [Sending SysEx](../../getting-started/interface-overview.md#sending-sysex)).

**Sounds and programs.** Set **Prog** and the module sends the Program Change as it starts and whenever you change it. With **Bank** or **LSB** set, the Bank Select goes first, MSB then LSB, as most gear wants. Some gear only takes a bank with a program, so the bank alone sends nothing. Bank and program numbers are in each device's manual. A program sends the **Bend Range** again after it, in case the new sound brought its own.

Like a port's device, its clock is a setting of this computer. A patch saved when MIDI Out had its own **Send Clock** switch turns that port's clock on as it opens, if it was off, and the load notes say so.

**Offset.** Every device takes its own time to turn MIDI into sound. A port's **Offset**, in the Rig, sends its notes, CCs and clock that many milliseconds later than the sound beside them, up to 50, to line a quick device up with a slow one. A negative offset sends them earlier, but only by as much as your audio device's delay, since a message can't leave before its sound is made. The Rig shows an offset it can't reach in amber. With the device's audio coming back into an input, the Rig can [measure its delay](../../getting-started/interface-overview.md#measuring-a-devices-delay) and set the offset for you.

## Tips

- **One synth per channel.** Two MIDI Outs on the same port, on channels 1 and 2, play two parts on a multitimbral synth or two tracks in a DAW.
- **Line the room up.** Measure each device from the Rig (the ping beside its port's **Offset**), or play a Soba hi-hat beside its arpeggio and nudge the **Offset** until they land together. Every MIDI Out on the port moves with it.
- **Wheels from your keyboard.** A [Controls](../midi/controls.md) slot hearing your controller's bend, patched into **Bend**, plays the hardware's bend from your keys. Another on its pressure, into **Pressure**, plays aftertouch.
- **The right sound at the top.** Patch a gate that rises at the top of the song into **Send Program**, so a device that was left on another sound starts the song on the right one.
- **Velocity from anywhere.** A Step Sequencer's second row, an LFO or a [Sample & Hold](../utilities/sample-hold.md) on Velocity gives a part its accents.
- **CCs for movement.** An [LFO](../modulation/lfo.md) with **Bipolar** off on CC 2 sweeps the synth's own filter. An [ADSR](../modulation/adsr.md) on the same gate opens it with each note.
- **Bipolar sources** go below 0, which sends 0. Use an [Attenuverter](../utilities/attenuverter.md) to lift them into 0 to 1.
