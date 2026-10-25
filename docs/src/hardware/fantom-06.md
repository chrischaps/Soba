# FANTOM-06

*Roland · synth · channels 1–16*

Roland's 61-key workstation synthesizer, sixteen zones each playing its own tone.

> **From the maker's chart:** Roland FANTOM-06/07/08 MIDI Implementation, Version 1.00, Feb 1, 2022, https://static.roland.com/assets/media/pdf/FANTOM-06_07_08_MIDI_Imple_eng01_W.pdf (linked from roland.com FANTOM-06 support page)

## Before you play

Zone channels 1-16 are assumed (each zone's Receive Channel is a setting; the document gives no factory default). Scenes: Bank Select 85/LSB 0 = A001-A128, LSB 1 = B001-B128 (the document elides further banks); scenes are presumably selected on the Scene Control Channel (SYSTEM, 1-16 or OFF; the MIDI document does not say so outright, so check the Reference Manual), and Receive Program Change/Bank Select must be On in SYSTEM:MIDI. Tones per zone: Bank Select MSB 87 (User LSB 0, Preset B 64, C 68, D 69, E 78, CMN 85, A 92), MSB 89 SN-A (User 0, Preset 65), MSB 91 VTW (User 0, Preset 65), MSB 86 drum kits (User LSB 0, Preset A 64, CMN 65); program 1-128, sent on the zone's channel. Clock in needs Sync Mode EXTERNAL; Start/Stop/Continue also work in REMOTE; clock out needs Sync Output On. Polyphony and the Scene Control Channel's factory default are not stated in Roland's spec page, MIDI Implementation or Reference Manual.

## Parameters

The first 12 are an Instrument's knobs when it's first made. Each part takes them on its own channel.

| | Parameter | Message | Group | Values |
|---|---|---|---|---|
| ● | Cutoff | CC 74 | Filter | an offset: 64 is no change; Mutate leaves it |
| ● | Resonance | CC 71 | Filter | an offset: 64 is no change; Mutate leaves it |
| ● | Attack | CC 73 | Envelope | an offset: 64 is no change; Mutate leaves it |
| ● | Decay | CC 75 | Envelope | an offset: 64 is no change; Mutate leaves it |
| ● | Release | CC 72 | Envelope | an offset: 64 is no change; Mutate leaves it |
| ● | Modulation | CC 1 | Voice |  |
| ● | Vibrato Rate | CC 76 | LFO | an offset: 64 is no change; Mutate leaves it |
| ● | Vibrato Depth | CC 77 | LFO | an offset: 64 is no change; Mutate leaves it |
| ● | Vibrato Delay | CC 78 | LFO | an offset: 64 is no change; Mutate leaves it |
| ● | Reverb Send | CC 91 | FX |  |
| ● | Chorus Send | CC 93 | FX |  |
| ● | Portamento | CC 5 | Voice |  |
|  | Expression | CC 11 | Mix | Mutate leaves it |
|  | Pan | CC 10 | Mix | Mutate leaves it |
|  | Volume | CC 7 | Mix | Mutate leaves it |
|  | Breath | CC 2 | Voice |  |
|  | Foot | CC 4 | Voice |  |
|  | GP Control 1 | CC 16 | Global |  |
|  | GP Control 2 | CC 17 | Global |  |
|  | GP Control 3 | CC 18 | Global |  |
|  | GP Control 4 | CC 19 | Global |  |
|  | GP Control 5 | CC 80 | Global |  |
|  | GP Control 6 | CC 81 | Global |  |
|  | GP Control 7 | CC 82 | Global |  |
|  | GP Control 8 | CC 83 | Global |  |

## Parts

| Part | Note | Channel |
|---|---|---|
| Zone 1 | – | 1 |
| Zone 2 | – | 2 |
| Zone 3 | – | 3 |
| Zone 4 | – | 4 |
| Zone 5 | – | 5 |
| Zone 6 | – | 6 |
| Zone 7 | – | 7 |
| Zone 8 | – | 8 |
| Zone 9 | – | 9 |
| Zone 10 | – | 10 |
| Zone 11 | – | 11 |
| Zone 12 | – | 12 |
| Zone 13 | – | 13 |
| Zone 14 | – | 14 |
| Zone 15 | – | 15 |
| Zone 16 | – | 16 |

## Buttons

A gate presses a momentary button while it's open, and flips a toggle each time it opens.

| Button | CC | Kind | On / off |
|---|---|---|---|
| Hold | 64 | momentary | 127 / 0 |
| Portamento | 65 | toggle | 127 / 0 |
| Sostenuto | 66 | momentary | 127 / 0 |
| Soft | 67 | momentary | 127 / 0 |
| Legato | 68 | toggle | 127 / 0 |

## Programs

A program change chooses a scene, shown 1 to 128.
Bank Select reaches Scene A (MSB 85, LSB 0), Scene B (MSB 85, LSB 1).

## Clock

Sends MIDI clock: yes · follows it: yes · starts and stops with it: yes
