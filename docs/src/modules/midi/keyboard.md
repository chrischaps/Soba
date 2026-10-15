# Keyboard

**Module ID** `input.keyboard` · **Category** Source

![Keyboard module](../../images/module-keyboard.png)
*The piano display lights the keys you're holding.*

The Keyboard turns your computer keyboard into a monophonic controller. Press a key and it sends the note's pitch as V/Oct, raises a gate for as long as you hold it, and sends a fixed velocity. It needs no setup and no hardware, which makes it the quickest way to hear a patch. The [First Sound](../../getting-started/your-first-patch.md) example that opens with the app is played from it.

It plays one note at a time. To play chords from the same keys, use [Poly MIDI](./poly-midi.md) instead: while a Poly MIDI module is in the patch, the computer keyboard plays it too.

## Outputs

| Port | Signal Type | Description |
|------|-------------|-------------|
| **Gate** | Gate (Green) | High while a key is held. Patch it into an envelope's **Gate** |
| **Pitch** | Control (Orange) | The held note as V/Oct. C4 is 0.0, C5 is +1.0, C3 is −1.0 |
| **Velocity** | Control (Orange) | The **Vel** knob's value, sent with every note |

The **Gate** output lights up on the node while a note is held.

## Parameters

| Control | Range | Default | Description |
|---------|-------|---------|-------------|
| **Oct** (Octave) | −2 to +2 | 0 | Shifts the whole keyboard up or down by octaves |
| **Vel** (Velocity) | 0 – 100% | 100% | The strength sent for every note. A computer key can't tell soft from hard, so this sets it for all of them |
| **Priority** | Last / Lowest / Highest | Last | Which key sounds when several are held: the one pressed most recently, the lowest or the highest (see [Playing](#playing)) |
| **Glide** | Off – 2 s | Off | How long the pitch takes to slide to a new note (see [Glide](#glide)) |
| **Glide** (Glide Mode) | Always / Legato | Legato | Which notes slide: all of them, or only those played while another key is held |

## Key layout

The bottom two letter rows form a piano keyboard starting at C4. The bottom row plays the white keys and the row above it plays the black keys, sitting between them as they would on a piano. The top letter row and the numbers above it are a second piano laid out the same way, an octave higher, as in a tracker:

```text
Black keys:    2   3       5   6   7       9   0
White keys:  Q   W   E   R   T   Y   U   I   O   P
Note:        C5  D   E   F   G   A   B   C6  D   E

Black keys:    S   D       G   H   J       L   ;
White keys:  Z   X   C   V   B   N   M   ,   .   /
Note:        C4  D   E   F   G   A   B   C5  D   E
```

That's C4 to E6, a little over two octaves. The pianos overlap by a few notes: **,** **.** **/** play the same C, D and E as **Q W E**, so a run up the bottom row can carry on along the top. Turn **Oct** to move the whole range.

The piano display plays too. Hold a key with the mouse, or a finger on a touch screen, and it plays like a computer key; slide across the keys and the pitch follows, legato. It's how the Keyboard is played on a phone.

## Playing

The Keyboard plays legato. The first key raises the gate, and while any key is held the gate stays up: pressing another key moves **Pitch** to it without retriggering the envelope, so overlapping your key presses slurs one note into the next. To retrigger, release every key before playing the next one.

When several keys are held, **Priority** picks the one that sounds:

- **Last** plays the key pressed most recently. Release it and the pitch falls back to the most recent key still held, which makes trills between two fingers easy.
- **Lowest** plays the lowest held key, so a held bass note wins over anything played above it.
- **Highest** plays the highest held key, the classic choice for a lead line over a held drone.

A change to **Priority** applies from the next key you press or release. Every held key lights on the piano display, whichever one is sounding.

After you let go, **Pitch** stays on the last note, so the release tail stays in tune. Even a very quick tap holds the gate high for at least 30 ms, so every key press triggers a full envelope.

Keys don't play notes while you hold **Ctrl** or **Alt** (those are shortcuts), while you're typing in a text field, or while the quick-add palette is open. **Space** and **Tab** open the palette, so they're never notes. The app has to have keyboard focus: if nothing happens, click the canvas.

Like everything else, the Keyboard only sounds while the patch is playing. Press **Play** in the toolbar first.

## Glide

**Glide** makes the pitch slide from one note to the next instead of jumping, like the portamento on a classic monosynth. The knob sets how long the slide takes, from Off up to 2 seconds. Most of its travel covers the short times, where glide is most useful: 200 ms is a little past halfway.

- **Even across the keyboard.** The slide moves evenly in pitch, so it sounds the same low or high.
- **The same time for any distance.** An octave leap arrives as quickly as a step to the next semitone.
- **Eases in.** The pitch covers most of the distance early and settles gently onto the note: about two thirds of the way in the first fifth of the time, and within 1% of the note by the end of it.

The **Glide** dropdown picks which notes slide:

| Glide Mode | Which notes slide | Feels like |
|------------|-------------------|------------|
| **Legato** | Only a note played while another key is still held. A note after a gap starts on its own pitch | A Minimoog or a 303: you choose each slide with your fingers |
| **Always** | Every note, from wherever the last one left off | Portamento that's always on |

To hear it, open First Sound and turn **Glide** to about 200 ms. Hold **Z** (C) and tap **B** (G): the pitch slides up to G and, when you let go of B, back down to C. Then play B on its own: in Legato mode it starts on G.

## Patch examples

The smallest playable voice, as in the First Sound example:

```text
[Keyboard Pitch] ──> [Oscillator V/Oct]
[Keyboard Gate] ──> [ADSR Gate]
[Oscillator Out] ──> [VCA In]
[ADSR Out] ──> [VCA CV]
[VCA Out] ──> [Audio Output Mono]
```

Two oscillators following one keyboard, the second a fifth up (**Semi** +7):

```text
[Keyboard Pitch] ──> [Oscillator 1 V/Oct]
                 ──> [Oscillator 2 V/Oct]
```

Turn **Vel** down and patch **Velocity** into the envelope's **Velocity** input to audition how a patch responds to softer playing:

```text
[Keyboard Velocity] ──> [ADSR Velocity]
```

## Related modules

- [Poly MIDI](./poly-midi.md) – chords, from a MIDI keyboard or these same keys
- [MIDI Note](./midi-note.md) – a monophonic voice played from a MIDI device
- [Oscillator](../sources/oscillator.md) – where **Pitch** usually goes
- [ADSR Envelope](../modulation/adsr.md) – where **Gate** usually goes
