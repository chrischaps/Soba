# volca fm2

*Korg · synth, 6 voices · channel 1*

A compact six-voice, six-operator FM synthesizer with a built-in arpeggiator, chorus and reverb.

> **From the maker's chart:** KORG volca fm2 MIDI Implementation, Revision 1.00 (2022.11.17), https://www.korg.com/us/support/download/manual/0/892/4952/ ; MIDI Implementation Chart (Oct. 27 2021, v1.0) https://www.korg.com/us/support/download/manual/0/892/4865/ ; Owner's Manual https://www.korg.com/us/support/download/manual/0/892/4866/

## Before you play

Set the global parameter MIDI Short Message to On (the factory default) or none of the CCs or Pitch Bend are received. Clock, Start, Stop and Continue are received only while MIDI Clock Src is Auto (the factory default), not Internal. The MIDI implementation (txt) lists Timing Clock, Start, Stop and Active Sensing as transmitted. The MIDI channel is set by holding MEMORY while powering on and pressing a keyboard button 1-16; the documents do not state the factory channel, so 1 is assumed. CC 40 is Octave or Transpose depending on the Transpose Note setting. No Program Change is received (programs are changed from the panel or by DX7 bulk data). The implementation starts Arp Type's Fall 2 at 58 in decimal but 0x40 (64) in hex, so Soba sends 66, inside it either way.

## Parameters

The first 12 are an Instrument's knobs when it's first made.

| | Parameter | Message | Group | Values |
|---|---|---|---|---|
| ● | Algorithm | CC 48 | Voice |  |
| ● | Mod Attack | CC 42 | Envelope |  |
| ● | Mod Decay | CC 43 | Envelope |  |
| ● | Carrier Attack | CC 44 | Envelope |  |
| ● | Carrier Decay | CC 45 | Envelope |  |
| ● | LFO Rate | CC 46 | LFO |  |
| ● | LFO Pitch | CC 47 | LFO |  |
| ● | Chorus | CC 93 | FX |  |
| ● | Reverb | CC 91 | FX |  |
| ● | Arp Type | CC 49 | Arp | Off, Rise 1, Rise 2, Rise 3, Fall 1, Fall 2, Fall 3, Random 1, Random 2, Random 3 |
| ● | Arp Div | CC 50 | Arp | 1/12, 1/8, 1/4, 1/3, 1/2, 2/3, 1/1, 3/2, 2/1, 3/1, 4/1 |
| ● | Transpose | CC 40 | Voice | Mutate leaves it |
|  | Velocity | CC 41 | Amp | Mutate leaves it |

## Clock

Sends MIDI clock: yes · follows it: yes · starts and stops with it: yes
