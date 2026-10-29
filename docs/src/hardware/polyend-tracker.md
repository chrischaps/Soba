# Polyend Tracker

*Polyend · synth, 8 voices · channel 1*

Polyend's 8-track sample tracker; over MIDI it plays one instrument as a synth and takes CC for its sound, mixer and perform effects.

> **From the maker's chart:** Polyend Tracker Manual v1.7.0, ch. 11 MIDI (11.2 Configuration, 11.4 CC mapping, 11.9 MIDI Synthesizer), ch. 8 Master mixer, https://polyend-website.fra1.digitaloceanspaces.com/wp-content/uploads/2023/08/01084159/Polyend-Tracker-Manual-1.7.0.pdf

## Before you play

Set Config > MIDI: Notes In and CC In to USB or MIDI In jack, and Notes Input Channel / CC In channel to match this profile (each is All or Ch 1-16; the manual gives no factory channel, so 1 is an assumption). Stop the sequencer to play it as a MIDI synthesizer: notes play the last selected instrument. Synth CCs 1-31 and 83-118 act on that instrument; CC 51-62 drive Perform mode effect slots, 71-82 the mixer. The Tracker receives no program change. Filter Type picks one of Disabled, Low-pass, High-pass and Band-pass, so Mutate moves it only with Ctrl+Alt+M; the manual doesn't say which CC 9 values pick which, so it reads as a number.

## Parameters

The first 12 are an Instrument's knobs when it's first made.

| | Parameter | Message | Group | Values |
|---|---|---|---|---|
| ● | Cutoff | CC 1 | Filter |  |
| ● | Resonance | CC 11 | Filter |  |
| ● | Filter Type | CC 9 | Filter | one of several, as a number; Mutate moves it only with Ctrl+Alt+M |
| ● | Overdrive | CC 12 | FX | Mutate leaves it |
| ● | Bit Depth | CC 13 | FX |  |
| ● | Tune | CC 5 | Oscillator | Mutate leaves it |
| ● | Finetune | CC 3 | Oscillator | Mutate leaves it |
| ● | Reverb Send | CC 14 | FX |  |
| ● | Delay Send | CC 15 | FX |  |
| ● | Volume | CC 7 | Amp | Mutate leaves it |
| ● | Vol Atk/Shp | CC 21 | Envelope |  |
| ● | Vol Dcy/Spd | CC 22 | Envelope |  |
|  | Vol Sustain | CC 23 | Envelope |  |
|  | Vol Release | CC 24 | Envelope |  |
|  | Vol Amount | CC 25 | Envelope |  |
|  | Pan Atk/Shp | CC 27 | Envelope |  |
|  | Pan Dcy/Spd | CC 28 | Envelope |  |
|  | Pan Sustain | CC 29 | Envelope |  |
|  | Pan Release | CC 30 | Envelope |  |
|  | Pan Amount | CC 31 | Envelope |  |
|  | Cut Atk/Shp | CC 84 | Envelope |  |
|  | Cut Dcy/Spd | CC 85 | Envelope |  |
|  | Cut Sustain | CC 86 | Envelope |  |
|  | Cut Release | CC 87 | Envelope |  |
|  | Cut Amount | CC 88 | Envelope |  |
|  | WtPos Atk/Shp | CC 103 | Envelope |  |
|  | WtPos Dcy/Spd | CC 104 | Envelope |  |
|  | WtPos Sustain | CC 105 | Envelope |  |
|  | WtPos Release | CC 106 | Envelope |  |
|  | WtPos Amount | CC 107 | Envelope |  |
|  | GrPos Atk/Shp | CC 109 | Envelope |  |
|  | GrPos Dcy/Spd | CC 110 | Envelope |  |
|  | GrPos Sustain | CC 111 | Envelope |  |
|  | GrPos Release | CC 112 | Envelope |  |
|  | GrPos Amount | CC 113 | Envelope |  |
|  | Fine Atk/Shp | CC 115 | Envelope |  |
|  | Fine Dcy/Spd | CC 116 | Envelope |  |
|  | Fine Sustain | CC 117 | Envelope |  |
|  | Fine Release | CC 118 | Envelope |  |
|  | Fine Amount | CC 119 | Envelope |  |
|  | Perf FX 1 | CC 51 | FX |  |
|  | Perf FX 2 | CC 52 | FX |  |
|  | Perf FX 3 | CC 53 | FX |  |
|  | Perf FX 4 | CC 54 | FX |  |
|  | Perf FX 5 | CC 55 | FX |  |
|  | Perf FX 6 | CC 56 | FX |  |
|  | Perf FX 7 | CC 57 | FX |  |
|  | Perf FX 8 | CC 58 | FX |  |
|  | Perf FX 9 | CC 59 | FX |  |
|  | Perf FX 10 | CC 60 | FX |  |
|  | Perf FX 11 | CC 61 | FX |  |
|  | Perf FX 12 | CC 62 | FX |  |
|  | Track 1 Level | CC 71 | Mix | Mutate leaves it |
|  | Track 2 Level | CC 72 | Mix | Mutate leaves it |
|  | Track 3 Level | CC 73 | Mix | Mutate leaves it |
|  | Track 4 Level | CC 74 | Mix | Mutate leaves it |
|  | Track 5 Level | CC 75 | Mix | Mutate leaves it |
|  | Track 6 Level | CC 76 | Mix | Mutate leaves it |
|  | Track 7 Level | CC 77 | Mix | Mutate leaves it |
|  | Track 8 Level | CC 78 | Mix | Mutate leaves it |
|  | Delay Mix | CC 79 | Mix | Mutate leaves it |
|  | Reverb Mix | CC 80 | Mix | Mutate leaves it |
|  | Dry Mix | CC 81 | Mix | Mutate leaves it |
|  | Line In Level | CC 82 | Mix | Mutate leaves it |

## Clock

Sends MIDI clock: yes · follows it: yes · starts and stops with it: yes
