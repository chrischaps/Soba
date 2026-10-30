# Pad Lights: MiniLab 3

The Arturia MiniLab 3's eight pads have RGB lights, and the computer can set their colours. When Soba plays a MiniLab, each pad lights up in the colour of what it plays in the patch, so the controller itself shows what each pad does.

| A pad that… | Lights |
|---|---|
| plays a [Pad](../modules/midi/pad.md) whose **Gate** goes into a module | in that module's colour, dimly: blue for a [Drum](../modules/sources/drum.md), orange for an [ADSR](../modules/modulation/adsr.md) |
| plays a Pad with nothing on its Gate | Gate green, dimly |
| is learned to a knob, toggle or dropdown (MIDI Learn) | in its module's colour, dimly |
| is learned to a [Scenes](../modules/utilities/scenes.md) pad | in the Scenes' colour, glowing steadily while that scene is the current one |
| plays nothing | dark |

**A hit flashes the pad.** The harder you hit it, the brighter it flashes, and the flash fades as fast as it does on the Pad's face. A pad that's latched, or held down, glows steadily until it's let go.

If a Pad's Gate isn't patched, its **Velocity** cable picks the colour. When two things listen to one pad, the brighter of the two lights it.

## Turning the lights on

The lights go out through a MIDI Out port, like anything else Soba sends to hardware.

1. Open **the Rig** (`Ctrl+Shift+H`).
2. Choose the MiniLab as the device for a free **Out** port. On Windows the MiniLab has two outputs, `Minilab3 MIDI` and `MIDIOUT2 (Minilab3 MIDI)`. Either one works with Soba, and Arturia's own DAW scripts use the second.
3. The pads light up. To turn them off, set the port back to **None**.

Soba sends a pad only when its colour changes. It sends each port at most thirty times a second, so a busy patch can't flood the controller, and it waits whenever a SysEx dump is still going out on that port. Every two seconds it sends all the pads again, so a MiniLab that's unplugged and plugged back in lights up again.

**💡 Test its pad lights**, in the port's device menu, plays a rainbow across the pads for four seconds. It works with an empty patch, so you can check the lights on their own.

## Which pad is which

Soba recognises each pad by the note it sends from the factory. Bank A sends notes 36 to 43 and bank B sends 44 to 51, all on channel 10, which are the notes the Library's **Pads ×8** listens to. A Pad learned from the MiniLab hears those same notes, so it lights the pad you hit.

If you changed the pads' notes in Arturia's MIDI Control Center, Soba won't know which pad is which. Put the new notes in `src/hardware/lights/minilab-3.json`.

## Still to be checked on the device

Arturia doesn't publish the MiniLab 3's SysEx. The colour message Soba sends comes from [people who worked it out by testing](https://gist.github.com/Janiczek/04a87c2534b9d1435a1d8159c742d260):

```text
F0 00 20 6B 7F 42 02 02 16 <pad> <red> <green> <blue> F7
```

Here `<pad>` is `04` to `0B` for bank A and `14` to `1B` for bank B. Red, green and blue each run from 0 to 127. The `02 02` is said to set the colours for DAW mode, `02 01` for Arturia mode and `02 00` for the current User mode.

It hasn't been tried on a real MiniLab yet. If **Test its pad lights** shows no rainbow:

- **Try the other output port.**
- **Switch the MiniLab to DAW mode.** Hold **Shift** and press pad 3 until the display says **DAW**.
- **Edit the message.** Change `02 02` to `02 01` in `src/hardware/lights/minilab-3.json` (the `colour` line), rebuild, and try again. You can add messages Soba should send before the first colour as `enter`, and messages for when the lights stop as `leave`, each as hex bytes.

Once you've seen it work, set `"confirmed": true` in the file, and the Rig stops saying the message is unchecked.
