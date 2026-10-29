# MicroFreak

*Arturia · synth, paraphonic, 4 voices · channel 1*

A four-voice paraphonic hybrid synth with a digital oscillator of 22 types, an analog filter and a poly-pressure keyboard.

> **From the maker's chart:** Arturia MicroFreak User Manual v5.0.1 (7/6/2023), Appendix D: CC# Values (p.141-142), https://dl.arturia.net/products/microfreak/manual/microfreak_Manual_5_0_1_EN.pdf

## Before you play

Set Utility > MIDI > Input Chan to match the channel Soba sends on (the manual does not state the factory input channel; output defaults to all channels). The knobs' CC send is a Utility setting; reception of CCs is the same CC list. Clock source is chosen in Utility (USB, MIDI or Sync). Set Local Control to match your use. Osc Type, LFO Sync Rate and Arp Sync Rate pick one of several (an oscillator engine, a division), so Mutate moves them only with Ctrl+Alt+M and a scene flips them at halfway. The manual names the 22 oscillator types in order (BasicWaves, SuperWave, Wavetable, Harmo, KarplusStr, V.Analog, Waveshaper, Two Op.FM, Formant, Chords, Speech, Modal, Noise, Bass, SawX, Harm, WaveUser, Sample, Scan Grains, Cloud Grains, Hit Grains, Vocoder), the LFO's synced divisions (8/1 to 1/32) and the arp's (1/1 to 1/32t), but not the CC value each starts at, so they read as numbers until the bands are heard from the device.

## Parameters

The first 12 are an Instrument's knobs when it's first made.

| | Parameter | Message | Group | Values |
|---|---|---|---|---|
| ● | Cutoff | CC 23 | Filter |  |
| ● | Resonance | CC 83 | Filter |  |
| ● | Osc Type | CC 9 | Oscillator | one of several, as a number; Mutate moves it only with Ctrl+Alt+M |
| ● | Osc Wave | CC 10 | Oscillator |  |
| ● | Osc Timbre | CC 12 | Oscillator |  |
| ● | Osc Shape | CC 13 | Oscillator |  |
| ● | Filter Amount | CC 26 | Filter |  |
| ● | Attack | CC 105 | Envelope |  |
| ● | Decay | CC 106 | Envelope |  |
| ● | Sustain | CC 29 | Envelope |  |
| ● | Glide | CC 5 | Voice |  |
| ● | Spice | CC 2 | Oscillator |  |
|  | Cyc Env Amt | CC 24 | Envelope |  |
|  | Cyc Env Rise | CC 102 | Envelope |  |
|  | Cyc Env Fall | CC 103 | Envelope |  |
|  | Cyc Env Hold | CC 28 | Envelope |  |
|  | LFO Rate | CC 93 | LFO |  |
|  | LFO Sync Rate | CC 94 | LFO | one of several, as a number; Mutate moves it only with Ctrl+Alt+M |
|  | Arp Rate | CC 91 | Arp |  |
|  | Arp Sync Rate | CC 92 | Arp | one of several, as a number; Mutate moves it only with Ctrl+Alt+M |

## Buttons

A gate presses a momentary button while it's open, and flips a toggle each time it opens.

| Button | CC | Kind | On / off |
|---|---|---|---|
| Keyboard Hold | 64 | toggle | 127 / 0 |

## Clock

Sends MIDI clock: yes · follows it: yes · starts and stops with it: yes
