# MIDI Out

**Module ID** `output.midi` · **Category** Output

MIDI Out plays other instruments from your patch: a hardware synth, a drum machine, or a soft synth in a DAW. A **Gate** and a **Pitch** cable become notes, a poly cable becomes chords, and four control cables become CCs. Turn on **Send Clock** and the patch's [Clock](../modulation/clock.md) goes out too, so the other gear follows Soba's tempo, starts when it starts and stops when it stops.

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
| **Port** | A – D | A | Which device plays. Each port's device is a setting of this computer, not of the patch (see below) |
| **Ch** | 1 – 16 | 1 | The MIDI channel the notes and CCs go out on |
| **Clock** | On / Off | Off | Sends the Clock as MIDI clock: 24 ticks a beat, with Start, Stop and Continue |
| **CC 1** … **CC 4** | 0 – 127 | 1, 74, 71, 10 | The controller number each lane sends. The defaults are the mod wheel, cutoff, resonance and pan, which most synths answer to |

## The display

- **The lamp and device**, top left. Green while notes are going out, and it flashes with each one. Amber while the port has no device, or its device is unplugged. Red when the device won't open, usually because another app has it. **Click the device's name** to choose another.
- **The channel**, top right.
- **A keyboard** of five octaves, C2 to C7, with every note the module is holding lit, so a chord shows its shape. A note beyond either end lights the arrow at that end.
- **Four bars**, one per CC lane, showing the value last sent, with the lane's CC number beneath. A lane with no cable is an empty slot.
- **The clock**, bottom right: four beat lamps that take turns while clock is going out.

## Ports, not devices

A patch doesn't name a device. It names a **port**, A to D, and this computer remembers which device each port plays. Choose port A's device once and every MIDI Out on port A plays it, in this patch and the next. Share the patch and it plays whatever synth is on port A on the other computer. So it travels without either of you editing it.

Click the device's name on the face to choose. The list shows every MIDI output this computer has, and it updates as you plug devices in. In the browser the menu offers **🎹 Connect MIDI** first, until you've allowed it. A port's device stays chosen while it's unplugged and plays again once it's back.

On Windows only one app can open a MIDI port at a time. If the lamp is red, close whatever else has the device (a DAW, or the maker's editor app), and MIDI Out tries again within a couple of seconds. To play a DAW on the same computer, use a virtual MIDI port such as [loopMIDI](https://www.tobias-erichsen.de/software/loopmidi.html): MIDI Out plays the port, and the DAW listens to it.

## How it works

**In time with the sound.** Notes don't go out when the patch works them out. They go out when the audio beside them reaches your speakers, which is your audio device's delay later. So a hardware synth and Soba's own sounds play together, and each note is on time to within a millisecond or two. The browser can't promise to send at a given millisecond, so there Soba hands each message over up to a tenth of a second early, with the moment it's due, and the browser sends it then.

**Chords.** Each channel of a poly cable is a voice. A Chord Sequencer's four-voice cable plays four-note chords, every note starting and ending with its own channel's gate. A one-channel Gate with a poly Pitch strikes the whole chord together. Two voices on the same note keep it sounding until the last of them lets go.

**No stuck notes.** Notes end cleanly:

- when the Clock stops, even mid-chord. A gate still held stays quiet until it rises again;
- when you delete the module or open another patch, and when the audio stops. Soba lets go of every note the module was holding and sends All Notes Off on its channel;
- when you change the channel, the port or the port's device, on the one the notes started on.

**Clock.** With **Clock** on, MIDI Out sends the Clock's tempo and place, with no cable needed:

- **Start** when the Clock starts from the top. After a count-in, it comes on the first downbeat;
- **24 ticks a beat**, placed on the beat to the sample;
- **Stop** when the Clock stops;
- **Continue** when it plays on from where it stopped, after a Song Position telling the other gear where that is. Most gear picks up from the next sixteenth.

A patch with no Clock sends no clock. See [Tempo and Sync](../../concepts/tempo-and-sync.md#leading-other-gear).

## Tips

- **One synth per channel.** Two MIDI Outs on the same port, on channels 1 and 2, play two parts on a multitimbral synth or two tracks in a DAW.
- **Clock from one module.** Turn **Clock** on in one MIDI Out per device. Two on the same port would send it twice, and the other gear would run at double speed.
- **Velocity from anywhere.** A Step Sequencer's second row, an LFO or a [Sample & Hold](../utilities/sample-hold.md) on Velocity gives a part its accents.
- **CCs for movement.** An [LFO](../modulation/lfo.md) with **Bipolar** off on CC 2 sweeps the synth's own filter. An [ADSR](../modulation/adsr.md) on the same gate opens it with each note.
- **Bipolar sources** go below 0, which sends 0. Use an [Attenuverter](../utilities/attenuverter.md) to lift them into 0 to 1.
