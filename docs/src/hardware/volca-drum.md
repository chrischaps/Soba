# volca drum

*Korg · kit, 6 voices · channels 1–6*

A six-part digital percussion synthesizer, each part two oscillators with pitch and amp envelopes, plus a waveguide resonator.

> **From the maker's chart:** KORG volca drum MIDI Implementation Chart (Split channel), Date 2019.2.20 v1.00, https://www.korg.com/us/support/download/manual/0/809/4259/ ; (Single channel) https://www.korg.com/us/support/download/manual/0/809/4260/ ; Owner's Manual https://www.korg.com/us/support/download/manual/0/809/4195/

## Before you play

Factory default is split-channel mode: Part 1-6 on MIDI channels 1-6, any note triggers the part (sounds do not map to note numbers) and the CCs are per part on its channel. Set the global parameter MIDI RX ShortMessage to On (the factory default) or the CCs are ignored. Clock, Start, Stop and Continue are received only when MIDI Clock Src is Auto (the factory default); nothing is transmitted. Single-channel mode (chosen by setting a MIDI channel with REC held while powering on; all six parts on that one channel) uses other CC numbers per part: Part 1 Select 14, Level 15, Mod Amt 16, Mod Rate 17, Pitch 18, EG Attack 19, EG Release 20, Send 103, Pan 109, each acting on both layers; the later parts continue in blocks (Part 2 from 23, Send 104, Pan 110, up to Part 6 at 89-100, Send 108, Pan 114), and the waveguide stays on 116-119. No Program Change is documented. The chart marks Program Change as received, and the Owner's Manual keeps 16 programs on the step buttons; which program change picks which isn't stated, so program change 0 is taken as program 1. Select and WG Model (the waveguide model) pick one of several, so Mutate moves them only with Ctrl+Alt+M; the chart doesn't say which values pick which, so they read as numbers.

## Parameters

The first 12 are an Instrument's knobs when it's first made. Each part takes them on its own channel.

| | Parameter | Message | Group | Values |
|---|---|---|---|---|
| ● | Level 1 | CC 17 | Mix | Mutate leaves it |
| ● | Level 2 | CC 18 | Mix | Mutate leaves it |
| ● | Pitch 1 | CC 26 | Oscillator |  |
| ● | Pitch 2 | CC 27 | Oscillator |  |
| ● | Mod Amt 1 | CC 29 | Oscillator |  |
| ● | Mod Amt 2 | CC 30 | Oscillator |  |
| ● | Mod Rate 1 | CC 46 | Oscillator |  |
| ● | Mod Rate 2 | CC 47 | Oscillator |  |
| ● | EG Attack 1 | CC 20 | Envelope |  |
| ● | EG Attack 2 | CC 21 | Envelope |  |
| ● | EG Release 1 | CC 23 | Envelope |  |
| ● | EG Release 2 | CC 24 | Envelope |  |
|  | Select 1 | CC 14 | Oscillator | one of several, as a number; Mutate moves it only with Ctrl+Alt+M |
|  | Select 2 | CC 15 | Oscillator | one of several, as a number; Mutate moves it only with Ctrl+Alt+M |
|  | Select 1+2 | CC 16 | Oscillator | one of several, as a number; Mutate moves it only with Ctrl+Alt+M |
|  | Level 1+2 | CC 19 | Mix | Mutate leaves it |
|  | Pitch 1+2 | CC 28 | Oscillator |  |
|  | Mod Amt 1+2 | CC 31 | Oscillator |  |
|  | Mod Rate 1+2 | CC 48 | Oscillator |  |
|  | EG Attack 1+2 | CC 22 | Envelope |  |
|  | EG Rel 1+2 | CC 25 | Envelope |  |
|  | Bit Reduction | CC 49 | FX |  |
|  | Fold | CC 50 | FX |  |
|  | Drive | CC 51 | FX | Mutate leaves it |
|  | Dry Gain | CC 52 | Mix | Mutate leaves it |
|  | Pan | CC 10 | Mix | Mutate leaves it |
|  | Send | CC 103 | FX |  |
|  | WG Model | CC 116 | FX | one of several, as a number; Mutate moves it only with Ctrl+Alt+M |
|  | WG Decay | CC 117 | FX |  |
|  | WG Body | CC 118 | FX |  |
|  | WG Tune | CC 119 | FX |  |

## Parts

A [Kit](../modules/midi/kit.md)'s eight lanes play the first eight when it's first made, and its knobs turn whichever track you choose, or each a track of its own. A part without a note is struck with middle C (60).

| Part | Note | Channel |
|---|---|---|
| Part 1 | – | 1 |
| Part 2 | – | 2 |
| Part 3 | – | 3 |
| Part 4 | – | 4 |
| Part 5 | – | 5 |
| Part 6 | – | 6 |

## Programs

A program change chooses a program, shown 1 to 16.

## Clock

Sends MIDI clock: no · follows it: yes · starts and stops with it: yes
