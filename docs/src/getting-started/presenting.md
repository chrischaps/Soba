# Presenting

Presentation mode is for talking over Soba while it plays: a video, a class, or a talk in front of a room. You write the script ahead of time and split it into **beats**. Each beat is a few moments of the patch moving: the camera turns to a module, a knob sweeps, a note plays. On stage you press one key, a clicker or a footswitch, and the next beat plays while you talk. What to say is in a notes window of its own, so a screen recorder filming Soba's window doesn't pick it up.

Each run is also kept as a **take**, a capture script that films the run again frame by frame, in time with what you said. So you can rehearse it live as many times as you like and film the run you liked exactly.

## Starting a presentation

Open the patch you'll present over, then choose **❓ Help › 🎓 Lessons › 🎬 Present…** and pick the script. From the command line:

```text
soba patches/basic-subtractive.json --present presentations/ep4-filters.txt
```

Nothing plays until you press for the first beat. The status bar shows a **Beat 3/6** chip while you present. Its dot lights red while a beat plays. Click the chip to bring the notes back if you closed them.

In the browser build, **Present…** asks for the script file, and the notes sit in a corner of the page rather than in a window of their own.

## The keys

| Key | What it does |
|---|---|
| `Page Down` or `→` | Plays the next beat. If one is still playing, it finishes first. |
| `Shift` + `Page Down` or `→` | Finishes the beat on show now: knobs and the camera get to where they were going. |
| `Page Up` or `←` | Plays the beat on show again from its start. Pressed within a second of a beat starting, it goes back to the beat before. |
| `Escape` | Stops presenting. The patch stays as the beats left it. |

A presentation clicker sends `Page Down` and `Page Up`, so it works without any set-up. The keys work from the notes window too. They don't reach the beats while a knob's value field or a menu has the keyboard: the notes say so, and a click on the canvas gives the keys back.

You can still turn knobs and move the view by hand between beats. A beat played again puts back the knobs, the view and the transport as they were when it first began.

## The notes

The notes window shows the beat on show and its name, with what you wrote to say in large type, and the **bold** words to lean on in colour. Below that is the next beat, with its first line, so you can lead into it. A beat that moves on by itself shows a bar counting down to it.

The buttons do what the keys do: **⟲ Again**, **Finish ⏭**, **Next ▶** and **Stop**. **● Take** shows how long the take has run, and **Save take…** writes it to a file.

## Writing a script

A script is a text file. It starts with a title and any footswitches, then the beats. Each beat has the lines to say and the cues to play, timed in seconds from the moment the beat starts:

```text
title    The Filter That Sings
advance  cc 64                    # a footswitch on CC 64 plays the next beat

beat Cutoff
say   A lowpass filter keeps the lows and takes the highs away.
say   **Cutoff** is where it starts cutting. Bring it down, and the buzz goes dark.
0     look frame:Tone 0.8
0     cursor on
0     move filter.svf knob:Cutoff 0.6
0.6   note 36 80 5.0
1.0   param filter.svf Cutoff 260 3.6

beat Resonance
say   Now **resonance**. It pushes back right at the cutoff, so that one spot rings.
1.0   param filter.svf Resonance 0.85 3.4
auto  6
```

- **`title`** names the presentation in the notes and the status bar.
- **`beat <name>`** starts a beat. A beat with no name is called by its number.
- **`say`** is a paragraph for the notes. Use one `say` line per paragraph. A beat with nothing to say is fine: the notes then tell you to let it play.
- **`auto <seconds>`** moves on to the next beat by itself that long after this one starts, without a press. Use it for a cold open that runs straight into the first explanation.
- **`#`** starts a comment.

The cues are the ones a capture script uses, so anything Soba can be filmed doing, a beat can do live. These are the ones a presentation uses most:

| Cue | What it does |
|---|---|
| `look filter.svf 0.8` | Glides the camera to frame a module, as `F` frames a selection, over 0.8 s. |
| `look osc.sine 1.4 2.0` | Centres a module at zoom 1.4, over 2 s. |
| `look frame:Tone 0.8` | Frames a frame, by its title. |
| `param filter.svf Cutoff 260 3.6` | Turns a knob to a value over 3.6 s. Frequencies and times glide evenly in octaves. |
| `note 36 80 5.0` | Plays a MIDI note into Note In: note number, velocity, length in seconds. |
| `chord 48,55,60 70 3.0` | Plays several notes at once. |
| `play` / `stop` | Starts or stops the transport. Soba opens with it stopped, so the first beat usually starts with `0 play`. |
| `cursor on` | Draws a pointer over the picture. Real mouse moves don't show up in a take, so this is how a viewer sees where to look. |
| `move filter.svf knob:Resonance 0.6` | Glides that pointer to part of a module (a knob, a jack, a button), wherever the module is drawn. `move off` takes it away. |
| `enter Bells` / `leave` | Dives into a group, or back out. |

A module is named by its kind, like `filter.svf`, and `filter.svf#2` is the second one. The full list of cues and of the parts a `move` can aim at is at the top of `src/app/capture.rs` in the source. A beat can't use the cues that only make sense while recording: `still`, `layout`, `input` and `end`.

If the script has a mistake, Soba won't start it, and the message names the line.

## Footswitches

A USB footswitch or any MIDI controller can step the beats. Name its CC before the first beat:

```text
advance  cc 64          # the next beat
back     cc 65 2        # the beat again, or the one before: CC 65 on channel 2 only
finish   cc 66          # finish the beat on show
```

A footswitch counts as pressed when its value goes from below 64 to 64 or above. CC 64 is what a sustain pedal sends, so a sustain pedal plugged into a MIDI keyboard makes a good one. While you present, Soba keeps these CCs away from everything else, so the patch doesn't hear the pedal as sustain, and no mapping or MIDI thru route does either.

## Takes

Every run is kept as a take. A take is a capture script: each cue the beats played, timed from your first press, with the lines you said written in as comments. It begins two seconds before the first press, setting the view, the transport and any knob you'd turned by hand, so it starts from where you did.

To keep a take, either start with `--take`, which writes it when you stop presenting:

```text
soba patches/basic-subtractive.json --present presentations/ep4-filters.txt --take ep4-take.txt
```

or click **Save take…** in the notes at any point.

A take keeps the script's cues, not your hands. A knob you turn or a view you move by hand during a beat isn't in it, so anything the film needs to show belongs in the script. A beat you played again replaces the first try: the take holds still where you went back, which is where the editor cuts your retake of the narration.

## From rehearsal to film

This is how a video gets made with it:

1. **Write the script** against the patch, one beat per idea, each ending on something to listen to.
2. **Rehearse live.** Size Soba's window to the shape of the film (tall for a Short, wide for a video), because the camera frames modules to fit the window and the take keeps that view. Play the beats, rewrite them and go again until the pacing feels right. Fix the script between runs and present it again: there's no need to restart Soba.
3. **Record the take.** Record your voice as you present (any recorder will do), and present with `--take`. Play a beat again if you fluff a line.
4. **Film the take.** A capture films it with the patch's sound, frame by frame, so nothing stutters however busy the patch is:

   ```text
   soba patches/basic-subtractive.json --capture ep4-take.txt --out target/showcase/ep4 --size 1080x1920 --fps 30
   ```

5. **Line up the voice.** The film's zero is your first press, so put the start of the first beat in your recording at zero, and cut out any retakes where the film holds still.
6. **Encode it.** `python tools/showcase/encode.py ep4 ep4.mp4 --lufs -14` makes the clip, at the loudness video sites play at.

A take plays its cues against the patch as it stands, so if you change the patch or the script, present it again for a new take.

## A worked example

`presentations/ep4-filters.txt` in the source is the pilot for *Synthesis from One Sine*, a video series that teaches synthesis with Soba. It's a Short about the filter, presented over the **Basic Subtractive** example in six beats:

1. **The sound first.** With nothing said, the camera frames the filter and its envelope, and a resonant filter sweeps through one low note. It moves on by itself.
2. **Every harmonic.** The filter opens wide on the oscillator's saw wave, which has every harmonic in it.
3. **Cutoff.** The pointer takes the Cutoff knob down, and the buzz goes dark.
4. **Resonance.** Resonance comes up, and the cutoff starts to ring.
5. **It sings.** The cutoff sweeps up through the note, and the ring picks out its harmonics one at a time.
6. **Play it.** A short phrase, each note's envelope opening the filter.

Open Basic Subtractive, choose **Present…** and pick the script. Then press `Page Down` for each beat: the first runs on into the second by itself.
