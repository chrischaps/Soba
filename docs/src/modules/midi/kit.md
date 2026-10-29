# Kit

**Module ID** `source.kit` · **Category** Output

A Kit is a drum machine, sampler or groovebox as a node. An [Instrument](./instrument.md) plays one voice, but a drum machine doesn't. A Digitakt has a track on each channel, the SP-404MKII's pads are notes, and a volca drum has a channel for each part. Choose one on the Kit's face, and its eight **lanes** take the device's own parts by name: **Track 1** to **Track 8**, **Pad 1** to **Pad 8**, **Part 1** to **Part 6**.

Each lane has a **Gate** and a **Vel** jack, in the same order as a [Trigger Sequencer](../utilities/trigger-sequencer.md)'s outputs. Patch the sequencer's eight lanes straight across, and it plays the device's eight tracks.

The Kit has twelve knobs, as an Instrument does: the device's own controls, by name, each with a jack. It makes no sound of its own. The hardware does.

## Choosing what it plays

Click **Choose a kit** on the face for the list of kit [profiles](../../hardware/instruments.md): the Digitakt, the SP-404MKII, the volca drum, and any kits of your own. (Or type `digitakt` in [Quick add](../../getting-started/interface-overview.md#quick-add) and choose **Kit · Digitakt**.) Choosing one:

- gives each lane one of the device's parts, its first eight in order;
- gives each knob one of its parameters, as an Instrument's does;
- sets **Ch** to the device's channel, and **Port** to the port whose device looks like it;
- and sends every knob to the device.

It's one step to undo. **None**, at the bottom of the list, empties the lanes and the knobs.

## Inputs

| Port | Signal Type | Description |
|------|-------------|-------------|
| **Track 1**, **Pad 13**… (one per lane) | Gate (Green) | Strikes the lane's part as it rises, and lets the note go as it falls |
| **Track 1 Vel**… (one per lane) | Control (Orange) | How hard the lane strikes, 0 to 1, read at the moment its gate rises. Unpatched, the **Velocity** knob says |
| **Filter Freq**, **Decay**… (one per knob) | Control (Orange) | As an [Instrument's](./instrument.md#inputs): 0 to 1 across the knob's travel |

Each lane's jacks are named for its part. An empty lane's are called **Gate 1** and **Vel 1** to **Gate 8** and **Vel 8**, as they are in the patch file. Every input is optional. The module has no outputs.

## Parameters

| Control | Range | Default | Description |
|---------|-------|---------|-------------|
| **Port** | A – H | A | Which device plays, as [MIDI Out's ports](../output/midi-out.md#ports-not-devices) do |
| **Ch** | 1 – 16 | 1 | The channel for parts without one of their own, and for the knobs, unless they turn a track |
| **Velocity** | 0 – 1 | 0.8 | How hard a lane strikes while its Vel jack is unpatched |
| **Slot 1** … **Slot 12** | 0 – 1 | 0.5 | Each knob's travel, as an Instrument's |

**Track**, the lane whose track the knobs turn, is chosen from the pads (see below).

## The display

The top is an Instrument's: the lamp, the device and the channel, then the profile's name and **Send all**. The knobs are at the bottom. Between them:

- **Eight pads**, two rows of four, each named for the part its lane plays. A pad flashes green as its lane strikes, brighter the harder it was struck, and stays softly lit while its gate is open. Each pad has its lane's number in a corner.
- **The Velocity knob**, at the right.

**Click a pad** to choose the part its lane plays. The menu lists every part the profile has, so a lane can play any of the SP-404MKII's sixteen pads, not only the first eight. **Nothing** empties the lane. Hover over a pad to see exactly what it sends: **Pad 13 · note 36 (C2) on channel 1**.

## Tracks and the knobs

Some devices take the same CCs on every track, each on its track's own channel. The Digitakt's Filter Frequency on channel 3 turns track 3's filter. On a device like that, the knobs turn one track at a time. That pad wears an **orange bar**. To turn another track, click its pad and choose **Knobs turn this track**.

Turning the knobs to another track sends nothing. The track keeps its sound until you move a knob, and then only that knob is sent. **Send all** sends every knob to the track the knobs turn.

### A knob on a track of its own

A drum machine's sound is spread across its tracks: the kick's decay, the snare's tune, the hats' filter. So a knob can keep a track of its own, whichever pad has the orange bar. **Right-click a knob** and open **Track**, beside **Holds**: it lists the lanes' tracks. Choose **Track 3**, and that knob turns track 3, on its channel, whatever the Track says. **The Track's**, at the top, puts it back.

A knob on its own track wears a small orange tag under its name, **T3**, wherever its track isn't the one the Track says. Twelve knobs can hold a whole kit this way: four for the kick, four for the snare, four for the hats. **Send all** sends each knob to its own track, so opening the patch brings every track back, not only one. Moving a knob to another track sends it there at once, as giving it another parameter does.

A parameter with a channel of its own still goes there: the Digitakt's Delay and Reverb are on its FX channel, whichever track the knob is on. Choosing the kit again puts every knob back on the Track's. The patch writes each knob's track down by name, as it does a lane's part, so undo, copy and paste keep it, and a patch whose profile this computer doesn't have still sends it to the right channel.

The SP-404MKII is different: its pads are notes, and its knobs are its effects. Its knobs stay on their own channels whichever pad you click.

The Kit [listens to its device](./instrument.md#the-hardware-turns-it-too) as an Instrument does. Turn the Digitakt's filter on the track the knobs turn, and the Kit's knob follows. Turning another track's filter doesn't move it, because that track's CCs come on another channel, unless a knob holds that track's filter as a track of its own: then that knob follows.

## How it works

**A strike.** A lane's gate rising sends a Note On: its part's note, on its part's channel, or on **Ch** if the part has none. A part with only a channel (a volca drum's, which any note plays) is struck with middle C (note 60). The gate falling sends the Note Off. Notes go out in time with the sound, stamped with their sample, as [MIDI Out's](../output/midi-out.md#how-it-works) do. A busy port's budget never holds back a note.

**No stuck notes.** A note is let go when its gate falls, when its jack is unplugged, when its lane is given another part, and when the Clock stops. Two lanes playing the same part share its note: the second strikes it again, and it's let go when the last of them falls.

**The profile is read, not copied.** As an Instrument's knobs do, each lane plays its profile's part of that name, so a note corrected in the profile reaches every patch. The patch writes each lane's part down too, so it still plays on a computer without the profile.

## Playing it

- **The Digitakt's eight tracks from Soba.** A [Clock](../modulation/clock.md) into a [Trigger Sequencer](../utilities/trigger-sequencer.md), its eight lanes and their velocities into a Kit playing the **Digitakt**. Program a beat on the sequencer's grid. A Soba hi-hat from the same sequencer plays in time with the hardware.
- **Pads for the SP-404MKII.** The same sequencer into a Kit playing the **SP-404MKII**. Click a pad to move its lane to any of the sixteen, the bottom row's (13 to 16) say.
- **A filter on one track.** Click a Digitakt pad, choose **Knobs turn this track**, and patch an [LFO](../modulation/lfo.md) into **Filter Freq**. Only that track sweeps.
- **A whole kit in twelve knobs.** On a Digitakt Kit, give knobs 1-4 the kick's Decay, Tune, Filter Freq and Level, each on **Track ▸ Track 1**; 5-8 the snare's on Track 2; 9-12 the hats' on Track 3. Save the patch, and opening it again brings all three back.

## Related

- [Instrument](./instrument.md): a synth, played from Gate and Pitch
- [Instrument Profiles](../../hardware/instruments.md): the kits that ship, and how to make your own (`"kind": "kit"` with its `"parts"`)
- [Trigger Sequencer](../utilities/trigger-sequencer.md): eight lanes to play it with
