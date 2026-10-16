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

Set **Source** to **Link** instead, and the Clock joins an [Ableton Link](https://www.ableton.com/link/) session on the network. There's no master: Live, Bitwig, phone apps and other copies of Soba all share one tempo and one bar line, and any of them can change the tempo or start and stop the others. The Clock:

- **plays at the session's tempo**, and its **BPM** knob turns to match when another app changes it. Turning the knob changes the tempo for everyone;
- **lines up its bars** with the session's, so beat 1 is everyone's beat 1;
- **starts and stops with the session**. Ticking its **Run** starts every app in it, on the next bar if others are already there.

Where MIDI clock carries the master's time down a cable, Link has every app keep the same time from the network's shared clock. Each app allows for its own audio delay, so they agree on when a beat reaches the speakers, not just when it's computed. See [Playing in a Link session](../modules/modulation/clock.md#playing-in-a-link-session).

## Leading other gear

It works the other way round too. Turn on **Clock** in a [MIDI Out](../modules/output/midi-out.md) and the patch's Clock becomes the master: a DAW, a drum machine or an arpeggiator set to follow MIDI clock plays in Soba's time. MIDI Out:

- **sends Start** when the Clock starts from the top, or on the first downbeat after a count-in;
- **sends 24 ticks per beat**, each placed on the beat to the sample, and sent as the audio beside it reaches your speakers, so the other gear and Soba's own sounds line up;
- **sends Stop** when the Clock stops, and **Continue**, after a Song Position saying where, when it plays on from there.

A follower measures the tempo from the ticks' spacing, so change the Clock's BPM and it follows. Don't send clock back to the gear the Clock follows: two clocks can't lead one another. A Clock playing in a Link session can lead MIDI gear, though. The same MIDI Out can also play the device's notes.

## Precision

The Clock counts its beat precisely enough that every pulse lands within a sample of where it belongs, even after ten minutes, so a slow phrase clock and a fast sixteenth clock started together stay together.

Modules read the transport once per block of audio (a few milliseconds). Within the block, a synced LFO moves on at the tempo it read. When the beat jumps, as when the Clock restarts or a MIDI master suddenly changes tempo, a synced LFO can be up to one block late in following it. On a steady tempo, and from the next beat on, it's exact.
