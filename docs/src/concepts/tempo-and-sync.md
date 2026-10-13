# Tempo and Sync

A patch has one tempo and one beat, and modules that care about time share them. Turn the [Clock](../modules/modulation/clock.md)'s **BPM** and the echoes of a synced [Delay](../modules/effects/delay.md) move, a synced [LFO](../modules/modulation/lfo.md) speeds up, and every sequencer the Clock steps keeps pace. None of them needs a cable from the Clock to know.

## The transport

The patch's first Clock is its **transport**. Before each block of audio, Soba reads three things from it and hands them to every module:

- the **tempo**, in beats per minute;
- the **beat**: how many beats have passed since the Clock started, counted from its downbeat;
- whether it's **playing**: the beat moves while the Clock runs, and holds while it's stopped.

A patch without a Clock has no transport. Synced modules then assume 120 BPM, and a synced LFO keeps its own beat.

If a patch has more than one Clock, the transport is the first one in processing order, which is usually the one nothing else feeds. Other Clocks still pulse at their own settings; they just don't set the patch's tempo.

The tempo doesn't have to stay put. A CV in the Clock's **Tempo** jack bends it, and everything synced bends with it: an [Arranger](../modules/utilities/arranger.md) lane there slows the song into its last chord. See [Tempo CV](../modules/modulation/clock.md#tempo-cv).

For a pulse slower than the beat, such as once every four bars, divide the one Clock with a [Clock Divider](../modules/utilities/divider.md) rather than adding a second, slower Clock. A divider counts the Clock's pulses, so it stays on the beat at any tempo, and it can't become the transport by accident.

## Two kinds of sync

**Following the tempo.** The Delay's **Sync** turns a note length into a time: a 1/8 at 120 BPM is 250 ms. It needs only the tempo.

**Following the beat.** A synced LFO reads where the beat is and works out its phase from that. A 1 bar LFO is at the start of its cycle on every downbeat, a quarter of the way through on beat 2, and so on. Because it reads the beat rather than counting its own cycles, it can't drift, however long it plays, and every synced LFO in the patch moves in step. When the Clock starts from the top, they all start from the top with it.

That's the difference from patching a Clock's **Gate** into an LFO's **Sync** input. A gate only restarts the cycle at each pulse, and between pulses the LFO runs at its own Rate, which you have to set to match.

## Playing in time with other gear

Set the Clock's **Source** to **MIDI** and it follows a MIDI clock master: a DAW, a drum machine, a hardware sequencer. The master sends 24 clock ticks per beat, plus Start, Stop and Continue messages for its transport. The Clock:

- **counts the ticks**, so its beat is exactly the master's, tick by tick;
- **measures the tempo** from their spacing, averaged over the last two beats, so the knob reads the master's BPM and synced modules follow it;
- **starts, stops and continues** when the master does. Start fires the Clock's **Reset** output, so patched sequencers start from step 1 on the downbeat.

The whole patch then plays in the master's time: sequencers step on its grid, LFOs sweep with its bars, and the Delay echoes on its eighth notes. See [Following MIDI clock](../modules/modulation/clock.md#following-midi-clock) for setting it up.

## Precision

The Clock counts its beat precisely enough that every pulse lands within a sample of where it belongs, even after ten minutes, so a slow phrase clock and a fast sixteenth clock started together stay together.

Modules read the transport once per block of audio (a few milliseconds). Within the block, a synced LFO moves on at the tempo it read. When the beat jumps, as when the Clock restarts or a MIDI master suddenly changes tempo, a synced LFO can be up to one block late in following it. On a steady tempo, and from the next beat on, it's exact.
