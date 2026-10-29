# Connections

A cable carries a signal from one module's output to another module's input. Patching is the whole of Soba: the modules decide what can happen, and the cables decide what does.

## Patching a cable

Outputs are on the right edge of a module and inputs are on the left. To connect two modules:

1. Press on an output jack and drag. A cable follows the pointer.
2. Release over an input jack on another module.

You can also start from an empty input and drag back to an output.

To unplug a cable, press on the input end and drag it away. The cable comes off the jack and follows the pointer: drop it on another input to re-patch it, or on empty canvas to remove it. Deleting a module removes all of its cables.

Patching and unpatching are recorded in the undo history, so **Ctrl + Z** puts back a cable you pulled by mistake.

## The rules

- **An output can feed any number of inputs.** Each one receives the full signal, with no loss of level.
- **An input takes one cable.** Patching a second cable into an occupied input replaces the first.
- **Signal types must be compatible.** Audio and control connect to each other freely, and a gate can drive a control input, but a gate input only accepts gates. The full table is on [Signal Types](./signal-types.md#which-types-connect). If a cable can't connect, the jack's ring breaks in two as the cable nears it, and a cable dropped there anyway is removed with a small tag beside the jack, and the status bar, explaining why.
- **A module can't patch into itself, and signals don't loop.** Soba processes the patch in one direction, from sources to the output, so a cable that would feed a module's output back into its own input, directly or through other modules, is refused, and the tag at the jack and the status bar name the modules round the loop. A patch file holding such a cable opens without it and says which. The [Mixer](../modules/utilities/mixer.md)'s **Return** jacks and the [Clock](../modules/modulation/clock.md#tempo-cv)'s **Tempo** jack are the exceptions: they hear a loop one audio block late. For echoes and feedback, use the [Delay](../modules/effects/delay.md)'s **Feedback** knob.

## Empty jacks

An input with nothing patched in isn't necessarily zero. Each jack has a resting value chosen so the module does something sensible on its own. A [VCA](../modules/utilities/vca.md) with nothing in its **CV** jack plays at full level, for example, so you can hear its input before you add an envelope.

The stereo effects work the same way. Patch only their left input and they treat the signal as mono, feeding both sides, so a mono voice into the Reverb still gets a stereo tail.

## Knobs with jacks

Many parameters have both a knob and an input jack of the same name, such as **Cutoff** on the filters or **Time** on the Delay. With nothing patched, the knob sets the value. Patch a cable in and the signal modulates the parameter *around the knob*: the knob sets the center, and the cable moves the value up and down from there.

How far a cable moves the parameter depends on the parameter, and each module page gives the scale. On both filters, for instance, the Cutoff CV is in octaves: at the default **CV Amt** of 1, +1.0 doubles the cutoff and −1.0 halves it. With Cutoff at 1 kHz, a bipolar LFO sweeps the filter from 500 Hz to 2 kHz. Some inputs have a knob of their own that sets how far they reach, as **CV Amt** does here.

A small dot above the knob shows that a cable is patched in: orange once the signal is arriving, green while it's connected but hasn't reported a value yet. You can keep turning the knob to move the center while the cable plays.

A knob mapped to a MIDI controller shows a purple **M** badge instead of the dot.

## Reading the signal in a cable

While the patch plays, each cable shows what its signal has been doing over the last few seconds. The signal travels from the output to the input at a steady pace, so the end of the cable nearest the output is *now* and points further along are moments ago.

- **Brightness is strength.** A cable lights up with its signal and goes dark when it's silent. A note becomes a packet of light that runs down the wire. The packet's length is how long the note lasted, and an audio cable's light fades out as the note releases.
- **Marks point the way.** Chevrons ride the light from output to input, so you can read a patch's direction even in a still frame. Each mark keeps the brightness of the moment it left the output.
- **Control cables draw their shape.** On orange cables the light swings to one side of the cable or the other with the signal's value. An LFO shows its waveform traveling down the wire, an envelope its rise and fall, a sequencer its steps.
- **Gates are on or off.** A green cable shows bright packets with hard edges, one per gate.

When you press **Stop**, the last of the signal drains out of the cables rather than vanishing.

To change the marks, open **Cables** in the toolbar and choose **Chevrons**, **Dots** or **Comets**. Soba remembers your choice.

Polyphonic cables are drawn as a bundle of strands, one per voice. See [Seeing polyphony](./polyphony.md#seeing-polyphony).

## Laying out a patch

A patch is easier to read, and easier to come back to, when it follows a few habits:

- Run the audio path from left to right, ending at the Audio Output.
- Put modulation sources (envelopes, LFOs, the clock) above or below the audio path, so their cables reach across it rather than tangling with it.
- Keep each voice's modules together.

```text
               [LFO] ──┐
                       ▼ Cutoff
[Oscillator] ──> [SVF Filter] ──> [VCA] ──> [Audio Output]
                                    ▲ CV
               [ADSR] ──────────────┘
```

## When there's no sound

Work backward from the output:

1. **Is anything patched into the Audio Output?** The **Mono** input is the simplest place to start.
2. **Is the transport running?** Press **Play** in the toolbar.
3. **Is the VCA opening?** If its **CV** jack is patched to an envelope, the envelope needs a gate. Play a note.
4. **Is the envelope getting a gate?** Its Gate cable should light green when you play.
5. **Follow the light.** A cable that stays dark carries silence. The last lit cable before the dark ones points to the module to check.

## See also

- [Signal Types](./signal-types.md): what each color carries, and which types connect
- [Polyphony](./polyphony.md): one cable, up to eight voices
- [Interface Overview](../getting-started/interface-overview.md): every editing action and shortcut
