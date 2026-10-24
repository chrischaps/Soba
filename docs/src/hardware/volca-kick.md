# volca kick

*Korg · synth, mono · channel 1*

A monophonic analogue kick generator with a resonator, played by notes.

> **From the maker's chart:** KORG volca kick MIDI Implementation, Version 1.00 (2016.07.29), https://www.korg.com/us/support/download/manual/0/726/3441/ ; MIDI Implementation Chart https://www.korg.com/us/support/download/manual/0/726/3305/ ; Owner's Manual https://www.korg.com/us/support/download/manual/0/726/3248/

## Before you play

Set the global parameter MIDI RX ShortMessage to On (the factory default) or the CCs are ignored. Clock, Start, Stop and Continue are received only when MIDI Clock Src is Auto (the factory default). There is no MIDI Out: nothing is transmitted. Notes 0-127 with velocity are received. The MIDI channel is set by holding MEMORY while powering on and pressing step button 1-16; the documents do not state the factory channel, so 1 is assumed.

## Parameters

The first 12 are an Instrument's knobs when it's first made.

| | Parameter | Message | Group | Values |
|---|---|---|---|---|
| ● | Pulse Colour | CC 40 | Oscillator |  |
| ● | Pulse Level | CC 41 | Oscillator |  |
| ● | Amp Attack | CC 42 | Envelope |  |
| ● | Amp Decay | CC 43 | Envelope |  |
| ● | Drive | CC 44 | FX |  |
| ● | Tone | CC 45 | Filter |  |
| ● | Res Pitch | CC 46 | Oscillator |  |
| ● | Res Bend | CC 47 | Oscillator |  |
| ● | Res Time | CC 48 | Envelope |  |
| ● | Accent | CC 49 | Amp |  |

## Clock

Sends MIDI clock: no · follows it: yes · starts and stops with it: yes
