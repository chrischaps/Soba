# Instrument

**Module ID** `source.instrument` · **Category** Output

An Instrument is a piece of hardware as a node. Choose **MicroFreak** on its face and its twelve knobs become the MicroFreak's own: **Cutoff**, **Resonance**, **Osc Type**, **Osc Wave**. Each knob has a jack beside it with the same name. Patch an [LFO](../modulation/lfo.md) into **Cutoff** and the real filter sweeps. A [Scene](../utilities/scenes.md) can morph the knobs, and the patch remembers where every one of them is.

That last part matters most for gear that can't store a sound. A volca forgets its sound when you switch it off. With an Instrument, **the patch becomes its preset memory**: open the patch and Soba sends every knob back to the device.

The Instrument also plays notes, as [MIDI Out](../output/midi-out.md) does, from a **Gate** and a **Pitch** cable, and presses the device's [buttons](#buttons): an RC-505's **Rec/Play**, a FANTOM's **Hold**. It makes no sound of its own: the hardware does.

## Choosing what it plays

Click **Choose an instrument** on the face (or the name that's there) for the list of [profiles](../../hardware/instruments.md): the ones that ship with Soba, then your own. Choosing one:

- gives each knob one of the profile's parameters, its first twelve in order;
- gives each button one of its buttons, its first eight in order;
- sets each knob to the parameter's default, if the maker's chart gives one, or else to the middle;
- sets **Ch** to the device's factory channel;
- sets **Port** to the port whose device looks like it (a port playing "MicroFreak MIDI" for the MicroFreak), if [the Rig](../../getting-started/interface-overview.md#the-rig) has one;
- and sends every knob to the device.

It's one step to undo. **None**, at the bottom of the list, empties the knobs and the buttons.

Or skip the face: in [Quick add](../../getting-started/interface-overview.md#quick-add), type a device's name or its maker (`freak`, `korg`) and choose **Instrument · MicroFreak**. The node arrives already playing it, chosen just as above, and one undo takes it away again.

## Inputs

| Port | Signal Type | Description |
|------|-------------|-------------|
| **Pitch** | Control (Orange), poly | V/Oct, 0 = C4 (note 60). Read as each gate rises |
| **Gate** | Gate (Green), poly | A note starts as a channel's gate rises and ends as it falls. A poly cable plays chords |
| **Velocity** | Control (Orange), poly | How hard each note is struck, 0 to 1. 0.8 with no cable |
| **Bend** | Control (Orange) | -1 to 1, sent as Pitch Bend wherever it moves. Unplugged, it springs back to the middle |
| **Pressure** | Control (Orange), poly | 0 to 1, sent as aftertouch: Channel Pressure from a one-channel cable, each note's own from a poly cable |
| **Cutoff**, **Resonance**… (one per knob) | Control (Orange) | 0 to 1 across the knob's travel. Patched, the cable turns the knob, and the knob follows it |
| **Target Trk Rec/Play**, **Hold**… (one per button) | Gate (Green) | Presses its button while the gate is high, or flips a toggle each time it rises. Beside its button, under the knobs |

Each knob's jack is named for the parameter the knob holds. An empty knob's jack is called **Slot 1** to **Slot 12**, as it is in the patch file. Each button's jack is named for its button, and is **Button 1** to **Button 8** in the patch file. A button that presses nothing has no jack. Every input is optional. Its one output is **MIDI** (purple), on the right end of its title bar: patch it into a [Strike](./strike.md) or [CC Out](./cc-out.md) and that plays this device, on its port and channel, with its profile's parts and knobs to choose from by name.

## Parameters

| Control | Range | Default | Description |
|---------|-------|---------|-------------|
| **Port** | A – H | A | Which device plays, as [MIDI Out's ports](../output/midi-out.md#ports-not-devices) do: each port's device is a setting of this computer |
| **Ch** | 1 – 16 | 1 | The MIDI channel the notes go out on, and every knob without a channel of its own |
| **Slot 1** … **Slot 12** | 0 – 1 | 0.5 | Each knob's travel. The knob shows what it sends, as the device names it |

**Press 1** to **Press 8**, in the patch file, are the buttons held down on the face. They have no control of their own.

## The display

- **The lamp and device**, top left. Green while messages are going out, and it flashes with each one. Amber while the port has no device, or its device is unplugged. Red when the device won't open. **Click the device's name** to choose another. The channel is at the right.
- **The profile's name**, beneath, with its maker. Click it to choose another. A name with a **?** is a profile this computer doesn't have (see below).
- **Send all**, at the right, sends every knob to the device again.
- **Twelve knobs**, four to a row. Each is named for its parameter and coloured by its group: an oscillator's blue, a filter's teal, an envelope's orange, an effect's cyan. Each reads out what it sends, as the device names it: **Saw**, **+12**, **74**.

- **Buttons**, under the knobs, one to a row, for a device that has them. See [Buttons](#buttons).

**Right-click a knob** to change what it holds. **Holds** lists the profile's parameters by group. **Nothing** empties the knob. The menu's heading says where the knob sends: **Cutoff · CC 23**, or **Delay Time · CC 85 ch 9** for a parameter on a channel of its own.

The menu's **Value** field is in the device's numbers too, so you can send an exact value: type **100** into Cutoff and CC 23 goes out at 100. For a parameter with named values, type the name (**Fall 2**, in any case, or just enough of its start to pick one out) or choose it from **Steps**. For one that nudges, type an offset: **+12** or **−5**. **Reset to** names the chart's default, and double-clicking the knob goes there too.

## Buttons

Some of a device's controls are buttons rather than knobs: a looper's **Rec/Play**, a synth's **Hold**. A profile lists them with the CC each sends, and whether it's **momentary** or a **toggle**. The Instrument gives each of the first eight a row under its knobs, with a Gate jack beside it named for the button.

- **A momentary button** is pressed while its gate is high: its on value (usually 127) goes out as the gate rises, and its off value (usually 0) as it falls. Patch a lane of an [Arranger](../utilities/arranger.md) into the RC-505's **Target Trk Rec/Play**, and the lane's gate presses it.
- **A toggle** flips each time its gate rises: on, then off at the next rise. The FANTOM-06's **Portamento** is one.
- **Click a button** to press it by hand. It's held while you hold the mouse down, and a toggle flips once a click.
- **It lights while it's on**, as a [Pad](./pad.md) does: a momentary button while it's held, a toggle while it's flipped on. The row's right end says what it sends: **CC 14**, or **CC 65 · toggle**.
- **It lets go.** When the Clock stops, a button that's on is turned off, a toggle too, so a looper isn't left recording or a Hold left down. One still held stays off until its gate falls and rises again. Unplugging its jack lets it go too, as does moving the Instrument to another channel.

**Right-click a button** to change what it presses. **Presses** lists the profile's buttons, all of them: the RC-505mkII has thirteen. **Nothing** empties it.

A device that's all buttons and no knobs, like either RC-505, shows its buttons in place of the knobs, and its empty knobs' jacks go too.

Each button presses its profile's button of that name, as each knob plays its profile's parameter, so a CC corrected in the profile reaches the patch. A button sends only as it changes: **Send all** doesn't press anything.

## Send all

The device comes back to the patch whenever every knob is sent:

- when you open the patch;
- when you choose a profile;
- when you click **Send all**. Do this after you've switched the device on after the patch, or turned it by hand while Soba wasn't listening to it (see below).

Each knob sends its value as a CC (or a 14-bit pair, or an NRPN, as its profile says). The port's [budget](../output/midi-out.md#busy-ports) paces them on a DIN port, so twelve knobs at once don't swamp it.

Knobs that **nudge** rather than set are left out. The FANTOM-06's Cutoff is one of these: its middle means no change, and every value sent moves the sound a little further. Turn one of these knobs and it sends as you turn it. Send all leaves it alone.

## The hardware turns it too

Most synths send their knobs as CCs as you turn them. The MicroFreak and the Digitakt both do. Open the device's MIDI input (**MIDI In** in the toolbar), and the Instrument listens to it. Turn the MicroFreak's real **Cutoff**, and the node's **Cutoff** turns with it. Save the patch, and the patch keeps the sound you made by hand.

- **It hears its own device, and nothing else.** The Instrument listens to the MIDI input whose name matches its port's device, as [the Rig](../../getting-started/interface-overview.md#the-rig) matches them, on the channels its knobs send on. A keyboard sending the same CC on the same channel doesn't move it.
- **Nothing goes back.** The device already has the value you turned it to, so the knob doesn't send it again.
- **An echo doesn't fight you.** Some devices send back every value they're sent. A value Soba sent that knob in the last 0.15 seconds is taken as an echo and ignored, so the knob doesn't jump back while you drag it.
- **A patched knob stays the cable's.** A faint purple tick on its rim shows where the hardware's knob was turned to. The tick fades after a moment, because the cable soon moves the device on from there.
- **Undo takes back a turn of the hand.** The moves of one turn are one step, the same as a mouse drag. The step ends after a second of stillness. Undoing it sends the knob's old value to the device.

A stepped knob goes to the step nearest the value it hears. Knobs that nudge (the FANTOM-06's) send no value of their own back, so they don't listen.

## Exploring its sounds

[Mutate](../../getting-started/interface-overview.md#mutate) works on hardware too. Select the Instrument and press `Ctrl + M`: its knobs move a small random step, and the device follows. Press it again to wander further, or hold it to go a long way. Each press is one undo step, so `Ctrl + Z` walks back to the sound you liked, sending the old values as it goes. Think of the MicroFreak as a partner suggesting the sound next door, not a menu to dig through.

- **What stays put:** a knob a cable drives, an empty knob, any parameter the profile marks `"mutate": false` (a volume, a tuning, a MIDI setting) or whose name says it's loud (a level, drive or feedback), and knobs that nudge, since undo couldn't take a nudge back.
- **Named values** (an arpeggio type, a clock division, the volca fm's Algorithm) move only with `Ctrl + Alt + M`, and then by whole steps, so the device always lands on one of its own values. Ten presses of `Ctrl + M` never trade an FM algorithm for another.
- **Choices without names** move only with `Ctrl + Alt + M` too. These are parameters the profile marks `"selects": true` because the maker's chart doesn't say which values pick which: the MicroFreak's Osc Type, or the Digitakt's Filter Type. They read as numbers, and a nudge could otherwise carry the MicroFreak from one oscillator engine to the next.
- **Keeping one:** `Ctrl + Shift + M` keeps the knobs in a [Scenes](../utilities/scenes.md) module. Scenes morph an Instrument's knobs as they do any other knobs. A knob of named values, or one that selects, holds until halfway, then switches once, rather than stepping through every value in between. Knobs that nudge aren't kept.

## How it works

**Knobs.** A knob sends when you turn it. A patched knob sends wherever its cable moves within a block, up to a thousand times a second, so an LFO sweeps a filter as smoothly as the device can follow. A stepped knob (an oscillator type, an arpeggio mode) shares its travel evenly among its steps, and sends each step's own value from the maker's chart, so it lands on the right step whatever numbers the device uses.

**Buttons.** A button's press goes out on the sample its gate rises, as a note does, and only when it changes: a gate held high for a minute sends one on and one off.

**Notes.** Notes, bend and pressure play as [MIDI Out](../output/midi-out.md#how-it-works) plays them: in time with the sound, chords from poly cables, and no stuck notes. When the Clock stops, a channel changes or the port changes, every note is let go.

**A new port** hears every knob afresh: move the Instrument to port B, and port B's device gets the whole sound.

**The profile is read, not copied.** Each knob plays its profile's parameter of that name, and each button its button. Correct a CC in your profile and every patch that uses it sends the corrected one. The patch also writes each knob's parameter down, so it still plays on a computer that doesn't have the profile. Its name shows with a **?**, and its knobs send what the patch wrote. Add the profile to that computer's **Documents/Soba/Instruments** folder and it reads the profile again.

## Playing it

- **A filter sweep on real hardware.** An [LFO](../modulation/lfo.md), slow, into **Cutoff**. Turn the LFO's **Rate** up for a wobble.
- **A sequencer playing the device.** A [Clock](../modulation/clock.md) into a [Step Sequencer](../utilities/sequencer.md), its **Gate** and **Pitch** into the Instrument's. A second sequencer lane into **Osc Type** changes the oscillator step by step.
- **A looper that records on the bar.** Choose **RC-505**, and patch an [Arranger](../utilities/arranger.md) lane's gate into **Target Trk Rec/Play**: a one-bar gate at the top of the verse starts recording, and the next one plays it back. [The Rig](../../recipes/the-rig.md) does this.
- **Saved sounds for a volca.** Choose **volca fm**, set the knobs, and save the patch. Next time, open the patch and the volca plays that sound again.

## Related

- [Instrument Profiles](../../hardware/instruments.md): what a profile is, the ones that ship, and how to make your own by listening to a device
- [Kit](./kit.md): a drum machine's tracks or pads as eight lanes, for a Trigger Sequencer
- [MIDI Out](../output/midi-out.md): for gear without a profile, or for CCs by number
- [The Rig](../../getting-started/interface-overview.md#the-rig): which device each port plays
