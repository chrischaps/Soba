# SP-404MKII

*Roland · kit, 16 voices · channel 1*

Roland's sampler and effector with 16 pads per bank, ten banks and five FX buses.

> **From the maker's chart:** Roland SP-404MK2 Reference Manual (applicable to version 4.00), MIDI implementation chart (v4.04, Apr. 4, 2024), p. 268-273, https://static.roland.com/assets/media/pdf/SP-404MK2_v4_reference_eng02_W.pdf

## Before you play

Pad notes and channel are MIDI mode A, bank A on channel 1: pads 1-4 are notes 48-51 (the bottom row is 13-16, notes 36-39), and bank B..J are channels 2..10 with the same notes (the chart's note-map layout); set the MIDI mode in UTILITY > SYSTEM > MIDI. In MIDI mode B the notes run across banks instead (12-91). The chart's default channel entry is ambiguous. The FX CCs address a bus by channel: 1-4 are buses 1-4 and 5 is Input FX, so one profile channel (1 = bus 1) reaches one bus; CC 83 picks the effect by number (see the manual's effect list). CC 7 and 8 work in DJ mode, buttons 85-89 in Looper mode on channel 1. Program change PC 0-15 picks pattern 1-16 of the bank given by the channel (example in the chart: PC 15 on channel 4 is Bank D pattern 16). Chart does not list a Bank Select.

## Parameters

The first 12 are an Instrument's knobs when it's first made.

| | Parameter | Message | Group | Values |
|---|---|---|---|---|
| ● | FX Ctrl 1 | CC 16 | FX |  |
| ● | FX Ctrl 2 | CC 17 | FX |  |
| ● | FX Ctrl 3 | CC 18 | FX |  |
| ● | FX Ctrl 4 | CC 80 | FX |  |
| ● | FX Ctrl 5 | CC 81 | FX |  |
| ● | FX Ctrl 6 | CC 82 | FX |  |
| ● | FX On | CC 19 | FX | Off, On; Mutate leaves it |
| ● | FX Number | CC 83 | FX | Mutate leaves it |
| ● | DJ Ch Volume | CC 7 | Mix | Mutate leaves it |
| ● | DJ X-Fade | CC 8 | Mix | Mutate leaves it |

## Parts

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

A program change chooses a pattern, shown A01 to H16, a letter a bank.

## Clock

Sends MIDI clock: yes · follows it: yes · starts and stops with it: yes
