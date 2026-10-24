# ZOIA

*Empress Effects · synth · channel 1*

Empress's modular-style multi-effects pedal, whose patches are built from modules on a grid.

> **From the maker's chart:** Empress ZOIA User Manual, firmware 1.13, pp. 26-31 (starred CC, Config menu, MIDI); MIDI module descriptions at https://empresseffects.com/products/zoia ; https://empresseffects.com/cdn/shop/files/ZOIA_Manual_1_dot_13.pdf

## Before you play

ZOIA has no fixed CC map: put a MIDI CC In module in your patch for each knob and set its controller to 102-117 (CC In 1 = 102 ... CC In 16 = 117, CCs the MIDI spec leaves undefined; the manual reserves only CC 60-63 for engage/bypass and buttons). Set Config > Midi Ch (program changes and bypass) and Starred CC ch to the channel used here, and Control Port to MIDI if using the TRS jack. Patches are numbered 0-63. Tempo follows a MIDI Clock In module.

## Parameters

The first 12 are an Instrument's knobs when it's first made.

| | Parameter | Message | Group | Values |
|---|---|---|---|---|
| ● | CC In 1 | CC 102 | FX |  |
| ● | CC In 2 | CC 103 | FX |  |
| ● | CC In 3 | CC 104 | FX |  |
| ● | CC In 4 | CC 105 | FX |  |
| ● | CC In 5 | CC 106 | FX |  |
| ● | CC In 6 | CC 107 | FX |  |
| ● | CC In 7 | CC 108 | FX |  |
| ● | CC In 8 | CC 109 | FX |  |
| ● | CC In 9 | CC 110 | FX |  |
| ● | CC In 10 | CC 111 | FX |  |
| ● | CC In 11 | CC 112 | FX |  |
| ● | CC In 12 | CC 113 | FX |  |
|  | CC In 13 | CC 114 | FX |  |
|  | CC In 14 | CC 115 | FX |  |
|  | CC In 15 | CC 116 | FX |  |
|  | CC In 16 | CC 117 | FX |  |

## Programs

A program change chooses a patch, shown 0 to 63.

## Clock

Sends MIDI clock: no · follows it: yes · starts and stops with it: no
