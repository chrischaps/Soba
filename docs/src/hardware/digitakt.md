# Digitakt

*Elektron · kit, 8 voices · channel 1*

Eight-track sample-based drum machine and sequencer.

> **From the maker's chart:** Elektron Digitakt User Manual, OS 1.53 (2026-09), Appendix B: MIDI and sections 8.4 and 15.5, https://www.elektron.se/wp-content/uploads/2026/09/Digitakt-User-Manual_ENG_OS1.53_260909.pdf

## Before you play

In Settings > MIDI Config > Port Config turn Receive CC/NRPN and Receive Notes on. Tracks 1-8 are on their default channels 1-8 (notes 0-7 trigger tracks 1-8 only then). Program changes need Prg Ch Receive on in MIDI Sync; the channel is set by Prg Chg In Ch. The Delay and Reverb parameters go to the FX Control channel, not a track's: set FX Control Ch to 9 in Settings > MIDI Config > Channels (the manual gives no factory setting). The compressor and the external-in mixer (also in B.7) are left out, because the manual doesn't say which channel they listen on. Sample Select, Play Mode, Filter Type, SRR Routing, the LFOs' Mult, Dest, Wave and Trig mode, Filter Trig, LFO Trig and Delay Pingpong each pick one of several (Filter Type: Off, 2-pole Lowpass, 2-pole Highpass, EQ 1-5), so Mutate moves them only with Ctrl+Alt+M. Appendix B gives their CCs but not which values pick which, so they read as numbers.

## Parameters

The first 12 are an Instrument's knobs when it's first made. Each part takes them on its own channel, except those with a channel of their own.

| | Parameter | Message | Group | Values |
|---|---|---|---|---|
| ● | Filter Freq | CC 74 | Filter |  |
| ● | Resonance | CC 75 | Filter |  |
| ● | Tune | CC 16 | Sample | Mutate leaves it |
| ● | Sample Select | CC 19 | Sample | one of several, as a number; Mutate moves it only with Ctrl+Alt+M |
| ● | Bit Reduction | CC 18 | Sample |  |
| ● | Overdrive | CC 81 | Amp | Mutate leaves it |
| ● | Delay Send | CC 82 | FX |  |
| ● | Reverb Send | CC 83 | FX |  |
| ● | Decay | CC 80 | Amp |  |
| ● | Env Depth | CC 77 | Filter |  |
| ● | Level | CC 7 | Amp | Mutate leaves it |
| ● | Pan | CC 10 | Amp | Mutate leaves it |
|  | Sample Level | CC 23 | Sample | Mutate leaves it |
|  | Play Mode | CC 17 | Sample | one of several, as a number; Mutate moves it only with Ctrl+Alt+M |
|  | Param E | CC 20 | Sample |  |
|  | Param F | CC 21 | Sample |  |
|  | Param G | CC 22 | Sample |  |
|  | Filter Type | CC 76 | Filter | one of several, as a number; Mutate moves it only with Ctrl+Alt+M |
|  | Filter Attack | CC 70 | Filter |  |
|  | Filter Decay | CC 71 | Filter |  |
|  | Filter Sus | CC 72 | Filter |  |
|  | Filter Release | CC 73 | Filter |  |
|  | Filter Delay | CC 86 | Filter |  |
|  | Filter Base | CC 84 | Filter |  |
|  | Filter Width | CC 85 | Filter |  |
|  | SRR | CC 87 | Filter |  |
|  | SRR Routing | CC 88 | Filter | one of several, as a number; Mutate moves it only with Ctrl+Alt+M |
|  | Amp Attack | CC 78 | Amp |  |
|  | Amp Hold | CC 79 | Amp |  |
|  | LFO1 Speed | CC 102 | LFO |  |
|  | LFO1 Mult | CC 103 | LFO | one of several, as a number; Mutate moves it only with Ctrl+Alt+M |
|  | LFO1 Fade | CC 104 | LFO |  |
|  | LFO1 Dest | CC 105 | LFO | one of several, as a number; Mutate moves it only with Ctrl+Alt+M |
|  | LFO1 Wave | CC 106 | LFO | one of several, as a number; Mutate moves it only with Ctrl+Alt+M |
|  | LFO1 Phase | CC 107 | LFO |  |
|  | LFO1 Trig | CC 108 | LFO | one of several, as a number; Mutate moves it only with Ctrl+Alt+M |
|  | LFO1 Depth | CC 109/61 (14-bit) | LFO |  |
|  | LFO2 Speed | CC 112 | LFO |  |
|  | LFO2 Mult | CC 113 | LFO | one of several, as a number; Mutate moves it only with Ctrl+Alt+M |
|  | LFO2 Fade | CC 114 | LFO |  |
|  | LFO2 Dest | CC 115 | LFO | one of several, as a number; Mutate moves it only with Ctrl+Alt+M |
|  | LFO2 Wave | CC 116 | LFO | one of several, as a number; Mutate moves it only with Ctrl+Alt+M |
|  | LFO2 Phase | CC 117 | LFO |  |
|  | LFO2 Trig | CC 118 | LFO | one of several, as a number; Mutate moves it only with Ctrl+Alt+M |
|  | LFO2 Depth | CC 119/63 (14-bit) | LFO |  |
|  | Track Level | CC 95 | Mix | Mutate leaves it |
|  | Mute | CC 94 | Mix | Mutate leaves it |
|  | Trig Note | CC 3 | Voice | Mutate leaves it |
|  | Trig Velocity | CC 4 | Voice | Mutate leaves it |
|  | Trig Length | CC 5 | Voice |  |
|  | Filter Trig | CC 13 | Voice | one of several, as a number; Mutate moves it only with Ctrl+Alt+M |
|  | LFO Trig | CC 14 | Voice | one of several, as a number; Mutate moves it only with Ctrl+Alt+M |
|  | Delay Time | CC 85 (ch 9) | FX |  |
|  | Delay Pingpong | CC 86 (ch 9) | FX | one of several, as a number; Mutate moves it only with Ctrl+Alt+M |
|  | Delay Width | CC 87 (ch 9) | FX |  |
|  | Delay Feedback | CC 88 (ch 9) | FX | Mutate leaves it |
|  | Delay Highpass | CC 89 (ch 9) | FX |  |
|  | Delay Lowpass | CC 90 (ch 9) | FX |  |
|  | Delay to Reverb | CC 91 (ch 9) | FX |  |
|  | Delay Volume | CC 92 (ch 9) | FX | Mutate leaves it |
|  | Reverb Predelay | CC 24 (ch 9) | FX |  |
|  | Reverb Decay | CC 25 (ch 9) | FX |  |
|  | Reverb Shelf Hz | CC 26 (ch 9) | FX |  |
|  | Reverb Shelf dB | CC 27 (ch 9) | FX |  |
|  | Reverb Highpass | CC 28 (ch 9) | FX |  |
|  | Reverb Lowpass | CC 29 (ch 9) | FX |  |
|  | Reverb Pre/Post | CC 30 (ch 9) | FX | Mutate leaves it |
|  | Reverb Volume | CC 31 (ch 9) | FX | Mutate leaves it |

## Parts

A [Kit](../modules/midi/kit.md)'s eight lanes play the first eight when it's first made, and its knobs turn whichever track you choose. A part without a note is struck with middle C (60).

| Part | Note | Channel |
|---|---|---|
| Track 1 | 0 | 1 |
| Track 2 | 1 | 2 |
| Track 3 | 2 | 3 |
| Track 4 | 3 | 4 |
| Track 5 | 4 | 5 |
| Track 6 | 5 | 6 |
| Track 7 | 6 | 7 |
| Track 8 | 7 | 8 |

## Programs

A program change chooses a pattern, shown A01 to H16, a letter a bank.

## Clock

Sends MIDI clock: yes · follows it: yes · starts and stops with it: yes
