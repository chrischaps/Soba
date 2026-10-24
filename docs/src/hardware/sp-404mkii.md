# SP-404MKII

*Roland · kit, 16 voices · channel 1*

Roland's sampler and effector with 16 pads per bank, ten banks and five FX buses.

> **From the maker's chart:** Roland SP-404MK2 Reference Manual (applicable to version 4.00), MIDI implementation chart (v4.04, Apr. 4, 2024), p. 268-273, https://static.roland.com/assets/media/pdf/SP-404MK2_v4_reference_eng02_W.pdf

## Before you play

Pad notes and channel are MIDI mode A, bank A on channel 1: pads 1-4 are notes 48-51 (the bottom row is 13-16, notes 36-39), and bank B..J are channels 2..10 with the same notes (the chart's note-map layout); set the MIDI mode in UTILITY > SYSTEM > MIDI. In MIDI mode B the notes run across banks instead (12-91). The chart's default channel entry is ambiguous. The FX CCs choose their bus by channel (1-4 are buses 1-4, 5 is Input FX); FX Number picks the effect by number (see the manual's effect list, p. 251). The DJ CCs work in DJ mode, the buttons in Looper mode. Program changes 0-15 pick a pattern, and the channel picks its bank (PC 15 on channel 4 is D16). Chart does not list a Bank Select.

## Parameters

The first 12 are an Instrument's knobs when it's first made.

| | Parameter | Message | Group | Values |
|---|---|---|---|---|
| ● | Bus 1 Ctrl 1 | CC 16 (ch 1) | FX |  |
| ● | Bus 1 Ctrl 2 | CC 17 (ch 1) | FX |  |
| ● | Bus 1 Ctrl 3 | CC 18 (ch 1) | FX |  |
| ● | Bus 1 Ctrl 4 | CC 80 (ch 1) | FX |  |
| ● | Bus 1 Ctrl 5 | CC 81 (ch 1) | FX |  |
| ● | Bus 1 Ctrl 6 | CC 82 (ch 1) | FX |  |
| ● | Bus 2 Ctrl 1 | CC 16 (ch 2) | FX |  |
| ● | Bus 2 Ctrl 2 | CC 17 (ch 2) | FX |  |
| ● | Bus 2 Ctrl 3 | CC 18 (ch 2) | FX |  |
| ● | Bus 2 Ctrl 4 | CC 80 (ch 2) | FX |  |
| ● | Bus 2 Ctrl 5 | CC 81 (ch 2) | FX |  |
| ● | Bus 2 Ctrl 6 | CC 82 (ch 2) | FX |  |
|  | Bus 3 Ctrl 1 | CC 16 (ch 3) | FX |  |
|  | Bus 3 Ctrl 2 | CC 17 (ch 3) | FX |  |
|  | Bus 3 Ctrl 3 | CC 18 (ch 3) | FX |  |
|  | Bus 3 Ctrl 4 | CC 80 (ch 3) | FX |  |
|  | Bus 3 Ctrl 5 | CC 81 (ch 3) | FX |  |
|  | Bus 3 Ctrl 6 | CC 82 (ch 3) | FX |  |
|  | Bus 4 Ctrl 1 | CC 16 (ch 4) | FX |  |
|  | Bus 4 Ctrl 2 | CC 17 (ch 4) | FX |  |
|  | Bus 4 Ctrl 3 | CC 18 (ch 4) | FX |  |
|  | Bus 4 Ctrl 4 | CC 80 (ch 4) | FX |  |
|  | Bus 4 Ctrl 5 | CC 81 (ch 4) | FX |  |
|  | Bus 4 Ctrl 6 | CC 82 (ch 4) | FX |  |
|  | Input FX Ctrl 1 | CC 16 (ch 5) | FX |  |
|  | Input FX Ctrl 2 | CC 17 (ch 5) | FX |  |
|  | Input FX Ctrl 3 | CC 18 (ch 5) | FX |  |
|  | Input FX Ctrl 4 | CC 80 (ch 5) | FX |  |
|  | Input FX Ctrl 5 | CC 81 (ch 5) | FX |  |
|  | Input FX Ctrl 6 | CC 82 (ch 5) | FX |  |
|  | Bus 1 FX On | CC 19 (ch 1) | FX | Off, On; Mutate leaves it |
|  | Bus 1 FX Number | CC 83 (ch 1) | FX | Mutate leaves it |
|  | Bus 2 FX On | CC 19 (ch 2) | FX | Off, On; Mutate leaves it |
|  | Bus 2 FX Number | CC 83 (ch 2) | FX | Mutate leaves it |
|  | Bus 3 FX On | CC 19 (ch 3) | FX | Off, On; Mutate leaves it |
|  | Bus 3 FX Number | CC 83 (ch 3) | FX | Mutate leaves it |
|  | Bus 4 FX On | CC 19 (ch 4) | FX | Off, On; Mutate leaves it |
|  | Bus 4 FX Number | CC 83 (ch 4) | FX | Mutate leaves it |
|  | Input FX On | CC 19 (ch 5) | FX | Off, On; Mutate leaves it |
|  | Input FX Number | CC 83 (ch 5) | FX | Mutate leaves it |
|  | DJ CH1 Volume | CC 7 (ch 1) | Mix | Mutate leaves it |
|  | DJ CH2 Volume | CC 7 (ch 2) | Mix | Mutate leaves it |
|  | DJ Pattern Vol | CC 7 (ch 3) | Mix | Mutate leaves it |
|  | DJ X-Fade | CC 8 (ch 1) | Mix | Mutate leaves it |

## Parts

A [Kit](../modules/midi/kit.md)'s eight lanes play the first eight when it's first made. A part without a note is struck with middle C (60).

| Part | Note | Channel |
|---|---|---|
| Pad 1 | 48 | 1 |
| Pad 2 | 49 | 1 |
| Pad 3 | 50 | 1 |
| Pad 4 | 51 | 1 |
| Pad 5 | 44 | 1 |
| Pad 6 | 45 | 1 |
| Pad 7 | 46 | 1 |
| Pad 8 | 47 | 1 |
| Pad 9 | 40 | 1 |
| Pad 10 | 41 | 1 |
| Pad 11 | 42 | 1 |
| Pad 12 | 43 | 1 |
| Pad 13 | 36 | 1 |
| Pad 14 | 37 | 1 |
| Pad 15 | 38 | 1 |
| Pad 16 | 39 | 1 |

## Buttons

A gate presses a momentary button while it's open, and flips a toggle each time it opens.

| Button | CC | Kind | On / off |
|---|---|---|---|
| Looper Delete | 87 | momentary | 127 / 0 |
| Looper Rec | 88 | momentary | 127 / 0 |
| Looper Resample | 89 | momentary | 127 / 0 |
| Looper Exit | 85 | momentary | 127 / 0 |
| Looper Pitch/Speed | 86 | momentary | 127 / 0 |

## Programs

A program change chooses a pattern, shown A01 to J16, a letter a bank.
Each bank is on its own channel: A on channel 1 to J on channel 10.

## Clock

Sends MIDI clock: yes · follows it: yes · starts and stops with it: yes
