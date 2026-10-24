# volca sample2

*Korg · kit, 10 voices · channels 1–10*

A ten-part digital sample sequencer with 200 sample slots, a pitch and amp envelope per part and a shared reverb.

> **From the maker's chart:** KORG volca sample (2nd generation) MIDI Implementation, Revision 1.00 (2022.11.01), https://www.korg.com/us/support/download/manual/0/867/4951/ ; MIDI Implementation Chart (Multi channel), Date Nov. 11 2020 v1.00, https://www.korg.com/us/support/download/manual/0/867/4821/ ; MIDI Implementation Chart (Single channel) https://www.korg.com/us/support/download/manual/0/867/4820/

## Before you play

Factory default is multi-channel mode: Part 1-10 on MIDI channels 1-10, each triggered by note 48 and taking the same CCs on its own channel. Reverb Mix (CC 91) is received on channel 1 only. A single-channel mode (all parts on one global channel, selected by choosing a MIDI channel on the device) instead triggers the parts with notes 48-57 and uses different CC numbers per part (Part 1 Level 7, Pan 10; Part 2 Level 14, Pan 15, Speed 16, Amp Attack 17, Amp Decay 18, Hi Cut 19; and so on in blocks of six, with the rest by NRPN); this profile does not describe it. Sample number (CC 3 MSB 0-1 and CC 35 LSB 0-99, samples 0-199) is left out because it is not a 14-bit pair. Pan 0 and 1 both mean full left. Loop, Reverb and Reverse read 0-63 as Off and 64-127 as On. Clock, Start, Stop and Continue are received only when MIDI Clock Src is CL.At (the Auto setting); the txt lists Timing Clock, Start and Stop as transmitted. No Program Change is documented, so patterns cannot be selected by MIDI.

## Parameters

The first 12 are an Instrument's knobs when it's first made. Each part takes them on its own channel.

| | Parameter | Message | Group | Values |
|---|---|---|---|---|
| ● | Speed | CC 43 | Sample |  |
| ● | Pitch EG Int | CC 44 | Sample |  |
| ● | Pitch EG Att | CC 45 | Envelope |  |
| ● | Pitch EG Dec | CC 46 | Envelope |  |
| ● | Amp Attack | CC 47 | Envelope |  |
| ● | Amp Decay | CC 48 | Envelope |  |
| ● | Hi Cut | CC 42 | Filter |  |
| ● | Start | CC 40 | Sample |  |
| ● | Length | CC 41 | Sample |  |
| ● | Level | CC 7 | Mix | Mutate leaves it |
| ● | Pan | CC 10 | Mix | Mutate leaves it |
| ● | Reverb Mix | CC 91 | FX | Mutate leaves it |
|  | Chromatic | CC 49 | Sample |  |
|  | Trig Delay | CC 50 | Sample |  |
|  | Loop | CC 68 | Sample | Off, On |
|  | Reverb On | CC 70 | FX | Off, On |
|  | Reverse | CC 75 | Sample | Off, On |

## Parts

| Part | Note | Channel |
|---|---|---|
| Part 1 | 48 | 1 |
| Part 2 | 48 | 2 |
| Part 3 | 48 | 3 |
| Part 4 | 48 | 4 |
| Part 5 | 48 | 5 |
| Part 6 | 48 | 6 |
| Part 7 | 48 | 7 |
| Part 8 | 48 | 8 |
| Part 9 | 48 | 9 |
| Part 10 | 48 | 10 |

## Clock

Sends MIDI clock: yes · follows it: yes · starts and stops with it: yes
