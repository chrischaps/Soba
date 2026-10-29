# volca fm

*Korg · synth, 3 voices · channel 1*

Korg's original 2016 three-voice, six-operator digital FM synthesizer in a volca, with a step sequencer and arpeggiator.

> **From the maker's chart:** KORG volca fm MIDI Implementation Rev. 1.01 (2016.4.07) and volca fm MIDI Implementation Chart Ver. 1.01 (2016.04.07, Model: volca fm), plus volca fm Owner's Manual (volca_fm_OM_EFGSCJ5), https://www.korg.com/us/support/download/product/0/558/

## Before you play

The volca fm has a MIDI In only (no MIDI Out), and takes no Program Change. The manual does not state the factory MIDI channel (channel 1 is assumed): set it by holding MEMORY while powering on and pressing keyboard button 1-16. Hold FUNC while powering on and set global parameter 7 'MIDI RX ShortMessage' to On (the factory default), or notes, CCs and pitch bend are ignored; clock, Start and Stop are followed only while parameter 6 'MIDI Clock Src' is Auto (the default). Transpose steps shown apply when 'TRANSPOSE NOTE' is On; with it Off, CC 40 gives octave-range transposition of -36 to +36 semitones instead. Modulator/Carrier Attack and Decay run -63 to +63 around the voice's own setting (64 is no change). Korg's hex and decimal columns disagree in a few of the step tables, so Soba sends each step from the middle of its band, inside it whichever column is right. Algorithm's bands are the implementation's table *7 (CC 48 value to algorithm), shown 1-32. That table reads 32 for CC 125-127 although algorithms run 0-31, so Algo 32 is sent at 124, inside the band of 121-124 the table gives it.

## Parameters

The first 12 are an Instrument's knobs when it's first made.

| | Parameter | Message | Group | Values |
|---|---|---|---|---|
| ● | Algorithm | CC 48 | Oscillator | Algo 1, Algo 2 … Algo 32; Mutate moves it only with Ctrl+Alt+M |
| ● | Mod Attack | CC 42 | Envelope | starts at 64 |
| ● | Mod Decay | CC 43 | Envelope | starts at 64 |
| ● | Carrier Attack | CC 44 | Envelope | starts at 64 |
| ● | Carrier Decay | CC 45 | Envelope | starts at 64 |
| ● | LFO Rate | CC 46 | LFO |  |
| ● | LFO Pitch Dep | CC 47 | LFO |  |
| ● | Arp Type | CC 49 | Arp | Off, Rise 1, Rise 2, Rise 3, Fall 1, Fall 2, Fall 3, Random 1, Random 2, Random 3; Mutate moves it only with Ctrl+Alt+M |
| ● | Arp Div | CC 50 | Arp | 1/12, 1/8, 1/4, 1/3, 1/2, 2/3, 1/1, 3/2, 2/1, 3/1, 4/1; Mutate moves it only with Ctrl+Alt+M |
| ● | Transpose | CC 40 | Voice | -3, -2, -1, 0, +1, +2, +3; Mutate leaves it |
| ● | Velocity | CC 41 | Amp | 1–127; Mutate leaves it |

## Clock

Sends MIDI clock: no · follows it: yes · starts and stops with it: yes
