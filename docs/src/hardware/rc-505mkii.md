# RC-505mkII

*BOSS (Roland) · synth · channel 1*

BOSS five-track loop station with rhythm, input and track FX.

> **From the maker's chart:** BOSS RC-505mkII Parameter Guide (Version 1.1 and later), ASSIGN and MIDI, p. 23-28, https://static.roland.com/assets/media/pdf/RC-505mk2_Parameter_eng02_W.pdf; Owner's Manual 'Controlling Devices via MIDI' p. 20-21, https://static.roland.com/assets/media/pdf/RC-505mk2_eng01_W.pdf

## Before you play

The manuals list no factory-default CC assigns, so set them in MENU > ASSIGN (16 slots, stored per memory, so WRITE): for each button, SW ON, SOURCE = MIDI CC# as listed (CC 1-31 and 64-95 are valid), TARGET = the matching function (TRK1-5 REC/PLY for CC 14-18, ALL ST/STP for 19, CUR.TRK UN/RED for 20, CUR.TRK CLEAR for 21, CUR.TRK STOP for 22, TAP TEMPO for 23, INPUT FX for 24, CUR.TRK INC for 25, CUR.TRK DEC for 26). The manual doesn't say whether a CC source acts on press or on value, so the on 127 / off 0 pair is an assumption. Set MIDI > RX CH CTL to the Soba channel (the manual gives no factory number). Memories 1-99 by program change 1-99 (100-128 ignored; Bank Select ignored). Rhythm notes use RX CH RHYTHM (default 10), not covered here.

## Buttons

A gate presses a momentary button while it's open, and flips a toggle each time it opens.

| Button | CC | Kind | On / off |
|---|---|---|---|
| Track 1 Rec/Play | 14 | momentary | 127 / 0 |
| Track 2 Rec/Play | 15 | momentary | 127 / 0 |
| Track 3 Rec/Play | 16 | momentary | 127 / 0 |
| Track 4 Rec/Play | 17 | momentary | 127 / 0 |
| Track 5 Rec/Play | 18 | momentary | 127 / 0 |
| All Start/Stop | 19 | momentary | 127 / 0 |
| Cur Trk Undo/Redo | 20 | momentary | 127 / 0 |
| Cur Trk Clear | 21 | momentary | 127 / 0 |
| Cur Trk Stop | 22 | momentary | 127 / 0 |
| Tap Tempo | 23 | momentary | 127 / 0 |
| Input FX On/Off | 24 | momentary | 127 / 0 |
| Cur Trk Inc | 25 | momentary | 127 / 0 |
| Cur Trk Dec | 26 | momentary | 127 / 0 |

## Programs

A program change chooses a memory, shown 1 to 99.

## Clock

Sends MIDI clock: yes · follows it: yes · starts and stops with it: yes
