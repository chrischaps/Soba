# FM Voice

**Module ID** `output.fm_voice` · **Category** Output

The volca fm speaks DX7. A DX7 voice has six operators, 32 ways to wire them and 155 numbers, and it's famously hard to program by hand. It's easy to wander through by ear, though. An FM Voice holds one voice and sends it to the volca every time it changes. **Mutate** makes a child of it, and **Cross** makes a child of it and another voice. You hear each child as soon as it's made.

The voices bred so far grow in the node's **garden**. Each one is a click away from playing again, and the patch keeps them all.

The FM Voice makes no sound of its own. The volca plays the voice.

## Setting it up

1. Plug the volca fm's MIDI In into one of the Rig's ports (Ctrl+Shift+H), and choose that port as the FM Voice's **Port**. Clicking the device's name at the top of the face also chooses a port's device.
2. Play the volca: from its own keys, a [MIDI Out](../output/midi-out.md), or an [Instrument](./instrument.md) with the volca fm's profile. The voice goes to the volca's edit buffer, the program it's playing.
3. Press **Mutate**.

The volca takes a voice on any channel, so **Channel** matters only for a DX7. A voice sent to the volca plays from its edit buffer, and the volca doesn't save it on its own. To keep one on the volca, write it to a program there, or send the whole garden to its memories (see [Files and banks](#files-and-banks)). The patch keeps the voices either way, and opening the patch sends the playing voice again.

## Knobs

| Control | Range | Description |
|---------|-------|-------------|
| **Port** | A – H | Which device the voice goes to, as [MIDI Out's ports](../output/midi-out.md#ports-not-devices) do |
| **Ch** | 1 – 16 | The channel the voice is addressed to. A volca fm takes it on any; a DX7 only on its own |
| **Format** | volca fm, DX7 | What it's sent to: see [volca fm or DX7](#volca-fm-or-dx7) |
| **Amount** | 0 – 1 | How far **Mutate** moves a child from its parent. A little is a neighbour; all the way is a wander |

The module has no inputs or outputs.

## Breeding

- **Mutate** makes a child of the playing voice and plays it. At a low **Amount**, a few numbers move a little. At a high one, most of them move far.
- **Cross** makes a child of the playing voice and the **picked** one. Shift+click a voice in the garden to pick it, or right-click it and choose **Pick it to cross with**. Each operator comes whole from one parent or the other. The algorithm and feedback come from one parent, and the pitch envelope and the LFO each from one. The picked voice stays picked, so you can cross the same pair again and get a different child.

Both keep the voice about as loud as its parents and in the same key, as [Mutate](../../getting-started/interface-overview.md#mutate) keeps a patch's loudness:

- A **carrier** is an operator you hear. Its output level, the peak of its envelope and its ratio stay the same. Only a hair of its detune moves.
- The algorithm changes only to one whose carriers are the same operators.
- In a cross, each carrier takes its loudness and pitch from the parent whose algorithm the child takes. So a modulator at 14 times the note never turns up as an operator you hear.

What moves is timbre: the **modulators'** levels and ratios, every envelope's rates, the keyboard scaling, the feedback and the LFO. A fixed-frequency operator stays fixed, and the transpose stays.

Every child is named after its family. A mutant takes its parent's name with its generation at the end (**SOFT KEY 3**). A cross takes half of each parent's name (**SOFT BELL**).

Each breed is one undo step, so Ctrl+Z walks back to the parent, and the parent is sent again.

## The garden

The garden holds up to **32** voices, one DX7 bank's worth, in the order a bank saves them. A new FM Voice starts with four seeds:

| Seed | What it is |
|------|------------|
| **SOFT KEYS** | An electric piano: three pairs of operators (algorithm 5), a round body, a struck tine at 14 times the note and a detuned shimmer, brighter played harder |
| **GLASS BELL** | A bell: each carrier struck by a modulator at an inharmonic ratio, 3.5 and 5, ringing long over a low hum |
| **WOOD BASS** | A plucked bass (algorithm 1): a half-ratio body bitten by its modulator's quick decay, with a woody knock above |
| **INIT VOICE** | The DX7's blank page: operator 1 alone, a plain sine |

Each voice is a chip. A brighter chip is a brighter voice: its modulators drive harder.

- The **playing** voice is ringed in white. Lines run from it to the voices it came from. Hover over another chip to see its parents too.
- The **picked** voice is ringed in green.
- A **kept** voice has an amber dot.
- **Click** a chip to play it, and **Shift+click** to pick it. **Right-click** a chip to play, pick, keep or weed it out.

Once the garden holds 32 voices, a new one takes the place of the oldest voice that isn't playing, picked or kept. A faint stroke through a chip marks the next to go. **Keep** (or **Kept**, to let it go) saves the playing voice from weeding. If every voice but the playing one is kept, the garden can't grow until you let one go.

## Files and banks

The **⋯** menu at the right of the buttons:

| Item | What it does |
|------|--------------|
| **Open .syx…** | Plants a DX7 single voice, a bank of 32, or a file of several, after the voices growing. To make room the oldest go, except kept and picked ones. A bank of 32 opened whole is a garden of its own |
| **Save voice as .syx…** | Saves the playing voice as a DX7 single voice, which Dexed and a DX7 read too |
| **Save garden as a bank…** | Saves every voice as one DX7 bank of 32, in the garden's order, with INIT VOICE filling any places left over |
| **Send garden to port's memories…** | Sends the garden as a bank. A volca fm writes a bank over **all 32 of its programs**, so it asks first. Save the volca's own programs first if you want them back |
| **Start over with the seeds** | The four seeds again, and the voices kept |

Opening a file says in the status bar how many voices it planted.

## volca fm or DX7

A DX7 single voice ends with a checksum. The volca fm reads that byte as which operators are on, and a checksum switches some of them off. So with **Format** on **volca fm**, a voice ends with all six operators on. With **DX7**, it ends with the checksum, for a DX7, Dexed or a volca fm2. A saved `.syx` file is always a DX7 file. Banks carry a checksum for both.

This comes from Volcako, an editor written for the volca fm. Korg's chart says only that the volca takes DX7 bulk data, and Volcako finds it takes it on any channel.

## Pacing

A voice is 163 bytes, about 55 ms down a DIN cable. The FM Voice sends one no more often than every 120 ms, and never while its port still has SysEx to send. Breed faster than that and only the newest voice goes. A bank is 4,104 bytes, about a second and a half. The Rig's **Last sent** column shows a dump on its way.

In the browser, SysEx waits until the page is allowed to send it. Send a `.syx` file from the Rig once to ask.

## The display

- **Along the top**: a lamp that flashes purple as a voice goes, then the port and its device. **Send** sends the voice again.
- **The voice's name**, its algorithm (**ALG 5**) and feedback (**FB 5**), and where it came from: **A seed**, **From a file**, **Mutant of SOFT KEYS (1)** or **Cross of SOFT KEYS (1) and GLASS BELL (2)**, with its generation and its place in the garden.
- **The algorithm**, drawn the way the DX7 draws it. The carriers sit along the bottom in the blue of audio, joined to the output line. The modulators stack above them in the orange of modulation. Each operator glows brighter the louder it is. The feedback loop brightens as the feedback rises.
- **Each operator** beside it, 1 to 6: its ratio (**×3.50**, or a fixed frequency in amber), its envelope's shape and its output level. Hover over one for its numbers.

## Ideas

- **A walk**: start from a seed, Mutate at a low Amount, and keep going while you like what you hear. Ctrl+Z when you don't.
- **Two families**: grow a bell and a bass apart, then cross across them.
- **A set of sounds for a song**: keep the voices you love, let the garden weed the rest, and send the garden to the volca's memories at the end of the session.
