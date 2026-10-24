# MIDI Out

**Module ID** `output.midi` · **Category** Output

MIDI Out plays other instruments from your patch: a hardware synth, a drum machine, or a soft synth in a DAW. A **Gate** and a **Pitch** cable become notes, a poly cable becomes chords, and four control cables become CCs. Turn on a port's **Clock** in [the Rig](../../getting-started/interface-overview.md#the-rig) and the patch's [Clock](../modulation/clock.md) goes out on it too, so the gear on that port follows Soba's tempo, starts when it starts and stops when it stops.

Patch a [Step Sequencer](../utilities/sequencer.md) into it and the sequencer plays your synth. Patch a [Chord Sequencer](../utilities/chord-sequencer.md) and it plays chords.

MIDI Out makes no sound of its own. It works in the desktop app, and in the browser in Chrome, Edge and Firefox once you've clicked **🎹 Connect MIDI** (in the device menu on its face, or the toolbar's MIDI In). Safari has no MIDI.

## Inputs

| Port | Signal Type | Description |
|------|-------------|-------------|
| **Pitch** | Control (Orange), poly | V/Oct, 0 = C4 (note 60), rounded to the nearest note. Read as each gate rises |
| **Gate** | Gate (Green), poly | A note starts as a channel's gate rises and ends as it falls |
| **Velocity** | Control (Orange), poly | How hard each note is struck, 0 to 1 (1 to 127). 0.8 with no cable |
| **CC 1** … **CC 4** | Control (Orange) | 0 to 1, sent as 0 to 127 on the lane's CC number whenever the value moves |

Every input is optional. The module has no outputs.

## Parameters

| Control | Range | Default | Description |
|---------|-------|---------|-------------|
| **Port** | A – H | A | Which device plays. Each port's device is a setting of this computer, not of the patch (see below) |
| **Ch** | 1 – 16 | 1 | The MIDI channel the notes and CCs go out on |
| **CC 1** … **CC 4** | 0 – 127 | 1, 74, 71, 10 | The controller number each lane sends. The defaults are the mod wheel, cutoff, resonance and pan, which most synths answer to |

## The display

- **The lamp and device**, top left. Green while notes are going out, and it flashes with each one. Amber while the port has no device, or its device is unplugged. Red when the device won't open, usually because another app has it. **Click the device's name** to choose another.
- **The channel**, top right.
- **A keyboard** of five octaves, C2 to C7, with every note the module is holding lit, so a chord shows its shape. A note beyond either end lights the arrow at that end.
- **Four bars**, one per CC lane, showing the value last sent, with the lane's CC number beneath. A lane with no cable is an empty slot.
- **The clock**, bottom right: four beat lamps that take turns while the port sends clock. The word **clock** under them is dim while the port's clock is off.

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
- when you delete the module or open another patch, and when the audio stops. Soba lets go of every note the module was holding and sends All Notes Off on its channel;
- when you change the channel, the port or the port's device, on the one the notes started on.

**Clock belongs to the port.** Each port's **Clock**, in the Rig, says what it sends of the patch's Clock, with no cable and no module needed:

- **Clock + Transport**: **Start** when the Clock starts from the top (after a count-in, on the first downbeat), **24 ticks a beat** placed on the beat to the sample, **Stop** when the Clock stops, and **Continue** when it plays on, after a Song Position telling the other gear where. Most gear picks up from the next sixteenth;
- **Clock only**: the ticks alone. The gear follows the tempo but starts and stops on its own, as a looper whose loops you start by foot wants;
- **Off**: none of it.

The port sends it once, however many MIDI Outs play it, and a port no MIDI Out plays sends it too: a drum machine needs only its port's clock to follow Soba. Turn a port to **Clock + Transport** while the song plays and its gear comes in on the next sixteenth. A patch with no Clock sends no clock. See [Tempo and Sync](../../concepts/tempo-and-sync.md#leading-other-gear).

Like a port's device, its clock is a setting of this computer. A patch saved when MIDI Out had its own **Send Clock** switch turns that port's clock on as it opens, if it was off, and the load notes say so.

**Offset.** Every device takes its own time to turn MIDI into sound. A port's **Offset**, in the Rig, sends its notes, CCs and clock that many milliseconds later than the sound beside them, up to 50, to line a quick device up with a slow one. A negative offset sends them earlier, but only by as much as your audio device's delay, since a message can't leave before its sound is made. The Rig shows an offset it can't reach in amber.

## Tips

- **One synth per channel.** Two MIDI Outs on the same port, on channels 1 and 2, play two parts on a multitimbral synth or two tracks in a DAW.
- **Line the room up.** Play a Soba hi-hat beside a device's arpeggio, and nudge the device's port **Offset** until they land together. Every MIDI Out on the port moves with it.
- **Velocity from anywhere.** A Step Sequencer's second row, an LFO or a [Sample & Hold](../utilities/sample-hold.md) on Velocity gives a part its accents.
- **CCs for movement.** An [LFO](../modulation/lfo.md) with **Bipolar** off on CC 2 sweeps the synth's own filter. An [ADSR](../modulation/adsr.md) on the same gate opens it with each note.
- **Bipolar sources** go below 0, which sends 0. Use an [Attenuverter](../utilities/attenuverter.md) to lift them into 0 to 1.
