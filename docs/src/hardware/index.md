# Playing Hardware

Soba can be the brain of a room of hardware. It keeps the time, plays the synths and drum machines, turns their knobs from cables, and changes their patterns and scenes as the song moves on. It also remembers each device's sound in the patch, so a volca that forgets everything when you switch it off gets its sound back when the patch opens.

This page goes through it in the order you'd set a room up. Each step links to the page with the details. The [The Rig](../recipes/the-rig.md) example does all of it in one song.

## 1. Plug in and name the ports

Plug each device into the computer, over USB or through a MIDI interface. Then open **the Rig** (`Ctrl+Shift+H`, or **MIDI In ▸ The Rig…**), which lists every MIDI device Soba can hear or play.

- **Out ports A to H.** Choose a device for each port. Modules play ports, not devices: a node set to port **B** plays whatever device is on B. Lend the patch to a friend and they choose their own devices for its ports. Which device is on each port belongs to this computer, not the patch.
- **Names that survive a reboot.** Windows renumbers devices that share a name (`2- Digitakt`). Soba finds each port's device again by its name without the number.
- **A budget for 5-pin DIN.** A DIN cable carries about a thousand messages a second. Set a port that plays through a MIDI interface to **DIN · 1000/s**, and Soba paces what it sends so a sweep of CCs never chokes the notes.
- **MIDI In.** Open every input you want to hear. They can all be open at once, and each event knows which device it came from.

See [The Rig](../getting-started/interface-overview.md#the-rig) for every column of the panel.

## 2. Keep time

One clock leads, and everything else follows it.

- **Soba leads.** Set a port's **Clock** to **Clock + Transport**, and its device follows the patch's [Clock](../modules/modulation/clock.md): 24 ticks a beat, with Start, Stop and Song Position. **Clock only** sends the tempo, for gear that starts and stops by itself.
- **A device leads.** Set the Clock's **Source** to **MIDI**, and choose the device under **Device** in the Clock's right-click menu.
- **Either.** Set **Source** to **Auto**. Soba leads until a device starts playing, then follows it, and when the device stops Soba carries on from the beat it had reached. See [Leading or following](../modules/modulation/clock.md#leading-or-following-auto).
- **Offsets.** Each device takes its own time to make a sound. A port's **Offset** sends its notes and clock that much later, or earlier, so a slow synth lands with the rest. With the device's audio coming back into your interface, Soba can [measure the delay](../getting-started/interface-overview.md#measuring-a-devices-delay) for you.

[Tempo and Sync](../concepts/tempo-and-sync.md#leading-other-gear) explains how Soba keeps every tick on its sample.

## 3. Play it

| To play | Use |
|---|---|
| Any device, with notes, CCs by number, bend, pressure and program changes | [MIDI Out](../modules/output/midi-out.md) |
| A synth, with its own knobs by name, to turn and patch | [Instrument](../modules/midi/instrument.md) |
| A drum machine's tracks or a sampler's pads, from a Trigger Sequencer | [Kit](../modules/midi/kit.md) |
| A volca fm's whole DX7 voice, bred by ear | [FM Voice](../modules/midi/fm-voice.md) |

An Instrument or a Kit reads its device's **profile**, which names its knobs and says which CCs they send. Soba comes with profiles for the Digitakt, the SP-404MKII, both RC-505s, the MicroFreak, the Polyend Tracker, the volca fm and volca drum, the FANTOM-06 and the ZOIA. You can add your own by turning a device's knobs while Soba listens. See [Instrument Profiles](./instruments.md).

Three things make a device feel like part of the patch:

- **The patch remembers its sound.** Opening the patch sends every knob of every Instrument, and an FM Voice's voice, to its device.
- **The knobs listen back.** Turn the MicroFreak's real **Cutoff** and the node's **Cutoff** turns with it. Save the patch, and it keeps what you did by hand. See [The hardware turns it too](../modules/midi/instrument.md#the-hardware-turns-it-too).
- **Cables reach the hardware.** Patch an LFO into an Instrument's **Cutoff** and the real filter sweeps, up to a thousand times a second.

## 4. Change it with the song

An [Arranger](../modules/utilities/arranger.md) knows when the verse ends. A [Setlist](../modules/midi/setlist.md) passes that on to the room. Each section can move the Digitakt to another pattern, the FANTOM to another scene, the RC-505 to another memory. A row can go a little ahead of the bar line, for gear that queues its next pattern, so every device changes on the downbeat.

The Arranger's lanes can press buttons too. A lane's gate into an Instrument's button presses it by name, such as the RC-505's **Target Trk Rec/Play** (see [Buttons](../modules/midi/instrument.md#buttons)). For a device without a profile, a lane's gate into a MIDI Out's CC jack presses whatever that CC is assigned to.

## 5. Wire the room

With every device plugged into the computer, the computer is your patch bay. In the Rig's **Thru**, a route passes what one MIDI In plays straight to one port, past the patch. Play the MicroFreak from the FANTOM's keys, or drive the volcas from the Tracker, while the patch does something else. A route can pass only some kinds of message, and move them to another channel. See [The Rig](../getting-started/interface-overview.md#the-rig).

## 6. Explore

- **Mutate on hardware.** Select an Instrument and press `Ctrl + M`: its knobs move a small random step, and the device follows. Each press is one undo step. See [Exploring its sounds](../modules/midi/instrument.md#exploring-its-sounds).
- **Scenes on hardware.** A [Scenes](../modules/utilities/scenes.md) module morphs an Instrument's knobs as it does any others, so one fader can carry a synth from one sound to another.
- **Breeding voices.** An [FM Voice](../modules/midi/fm-voice.md) mutates and crosses DX7 voices, and keeps the ones you like in a garden the size of the volca's memory.

## 7. Hear it come back

Most rooms mix the hardware somewhere else, on a desk or a mixer. If you do bring it back into Soba, an [Audio Input](../modules/sources/audio-input.md) can take any channel of a multichannel interface ([Every input of a mixer](../modules/sources/audio-input.md#every-input-of-a-mixer)), so each device can have its own.

## Related

- [The Rig](../recipes/the-rig.md): a whole song for a Digitakt, a MicroFreak, a volca fm and an RC-505
- [Instrument Profiles](./instruments.md): every device Soba knows, and how to teach it yours
- [Tempo and Sync](../concepts/tempo-and-sync.md): one tempo, one beat, and other gear in time with them
