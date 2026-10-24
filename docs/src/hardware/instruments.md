# Instrument Profiles

To MIDI, every synth is just numbers: "CC 74 on channel 3". A **profile** puts a device's numbers into its own words. It names the device, says which CC is its Cutoff, says what each value of its oscillator type means, which note plays its kick, and how its programs are numbered ("A05", "Scene 12"). It also says whether the device keeps time with MIDI clock.

Profiles are what the Instrument module and the Setlist read. A profile on its own sends nothing: it describes the device for the modules that do.

## The profiles that ship

Each one was built from its maker's official MIDI implementation chart, and its page says which chart and which version.

| Device | Maker | What it is |
|---|---|---|
| [Digitakt](./digitakt.md) | Elektron | A kit: eight sample tracks, a channel each |
| [SP-404MKII](./sp-404mkii.md) | Roland | A kit: sixteen pads a bank, as notes |
| [RC-505](./rc-505.md) | BOSS | A loop station: buttons you assign to CCs |
| [RC-505mkII](./rc-505mkii.md) | BOSS | The same for the mkII, which has sixteen assigns |
| [MicroFreak](./microfreak.md) | Arturia | A paraphonic synth with 20 CCs |
| [Polyend Tracker](./polyend-tracker.md) | Polyend | Played as a MIDI synth, with its mixer and perform effects |
| [volca fm2](./volca-fm2.md) | Korg | A six-voice FM synth |
| [volca sample2](./volca-sample2.md) | Korg | A kit: ten sample parts, a channel each |
| [volca drum](./volca-drum.md) | Korg | A kit: six parts, a channel each |
| [volca kick](./volca-kick.md) | Korg | A kick synth, played by notes |
| [FANTOM-06](./fantom-06.md) | Roland | A workstation with sixteen zones, and scenes chosen by program change |
| [ZOIA](./zoia.md) | Empress | Sixteen CC lanes for the CC In modules in your patches |

Where a chart left something out, such as a factory channel or what an oscillator type's values are, the profile doesn't guess. It says so under **Before you play**, and the knob shows plain numbers.

## Your own profiles

Your own profiles go in **Documents/Soba/Instruments**, one JSON file each. Soba reads the folder each time it looks for a profile. If one of yours has the same name as a shipped profile (in any case), yours replaces it. That's how you'd add the parameters the shipped one leaves out, or swap its first twelve knobs for the twelve you play most.

A file that isn't a profile is skipped, and Soba tells you why. That covers a CC past 127, two parameters with the same name, and a kit with no parts.

### Listening to an instrument

A device that sends its knobs as CCs can teach Soba its own profile. The Digitakt, the MicroFreak and many others do this.

1. Open the device in **MIDI In**.
2. Choose **MIDI In ▸ Listen to an Instrument…**.
3. Give the profile a name.
4. Turn each knob on the device, one at a time. Each knob shows up as a row the moment it moves, with its lamp glowing and a bar showing where it stands. Type its name into the row; the newest row takes your typing.
5. Click **Save Profile**.

Turn the knobs in the order you want them on the node, because the first twelve become an Instrument's knobs. A knob you don't name keeps its number ("CC 83"). A knob you moved by mistake can be dropped with its ✕. If one knob sends on a different channel from the rest, it's left out, because a profile's parameters share a single channel.

### The format

A profile is a small JSON file, like a patch:

```json
{
  "name": "MicroFreak",
  "maker": "Arturia",
  "port": "MicroFreak",
  "kind": "synth",
  "channels": [1],
  "voices": { "paraphonic": 4 },
  "parameters": [
    { "name": "Cutoff", "cc": 23, "group": "Filter" },
    { "name": "Mode", "cc": 80, "values": ["Mono", "Poly"] },
    { "name": "Fine", "cc": 1, "lsb": 33, "mutate": false },
    { "name": "Deep", "nrpn": 1234, "range": [0, 100] }
  ],
  "parts": [{ "name": "Kick", "note": 36, "channel": 10 }],
  "buttons": [{ "name": "Rec", "cc": 80, "mode": "momentary" }],
  "programs": { "what": "Pattern", "style": "lettered", "letters": "ABCDEFGH", "per_bank": 16 },
  "clock": { "sends": true, "receives": true, "transport": true },
  "notes": "Turn on Receive CC in the device's MIDI settings."
}
```

- **port**: part of the name the device's MIDI port shows up as. Soba uses it to suggest the profile for that port.
- **kind**: `synth` is one instrument played with notes. `kit` is a drum machine, sampler or groovebox whose **parts** each have a note, a channel, or both.
- **parameters**: one per CC, in the order an Instrument's knobs take them. Each is one of three kinds:
  - a CC (`"cc"`),
  - a 14-bit pair (`"cc"` plus `"lsb"`),
  - an NRPN (`"nrpn"`).

  A parameter can also have:
  - **range**: the values it takes, when they aren't all of them.
  - **values**: for a stepped parameter, its steps. Plain labels share the range evenly, and each is sent from the middle of its share, so the device lands on the right step however it divides the range. A step written as `{ "label": "Poly", "value": 64 }` sends exactly that value.
  - **curve**: `"log"`, for times and frequencies.
  - **default**: where it starts.
  - **mutate**: `false` keeps Mutate away from a volume or a tuning.

  **group** (Oscillator, Sample, Voice, Filter, Envelope, LFO, Arp, FX, Amp, Mix, Global) colours the knob after the matching module category: oscillators blue, filters teal, envelopes and LFOs orange, effects cyan, and the rest grey.
- **cc_per_part**: each part takes the same CCs on its own channel, a track a channel.
- **buttons**: CCs a gate presses (`momentary`) or flips (`toggle`), with their `on` and `off` values (127 and 0 unless given).
- **programs**: what program changes choose and how the device numbers them. The style is either `numbered` (from `first`, with `count`) or `lettered`, like the Digitakt's A01 to H16. It can also give a `channel`, and `banks` reached by Bank Select (`msb`, `lsb`).
- **clock**: whether the device sends MIDI clock, follows it, and starts and stops with it.
