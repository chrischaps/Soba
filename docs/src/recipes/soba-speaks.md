# Soba Speaks

A patch that talks. Every two bars a voice says *"Hello. I am Soba. I sing in sines."* You don't hear the voice itself. You hear a chord of saws wearing its vowels and consonants, through a [Vocoder](../modules/effects/vocoder.md). The chords fall a step a bar, Fmaj9, Em7, Dm9, Cmaj7, and a soft pad of the same chords carries on between the words. The line is there twice, spoken by a person and by a formant synthesizer, and a mute picks which one the chords wear. A third mute hands the line to you.

> **Load it:** choose **📚 Examples → Soba Speaks** in the toolbar and press **▶ Play**. It needs no keyboard.
> The patch file is [`patches/soba-speaks.json`](https://github.com/chrischaps/Soba/blob/master/patches/soba-speaks.json), and its voices are [`patches/samples/soba-speaks-chris.wav`](https://github.com/chrischaps/Soba/blob/master/patches/samples/soba-speaks-chris.wav) and [`patches/samples/soba-speaks.wav`](https://github.com/chrischaps/Soba/blob/master/patches/samples/soba-speaks.wav).

<iframe class="patch-embed" src="https://soba.chaps.dev/?patch=soba-speaks" title="Soba Speaks, playable in the browser" allow="autoplay" loading="lazy"></iframe>

*Or play it here: press **▶ Play** in the corner. The full app is a click away under **Open in Soba**. The voices stand in a column on the left, beside the Mixer that picks one. The vocoder and its room run along the top, and the chords run along the bottom. One oscillator feeds both the vocoder's **Carrier** and the dark pad.*

## What it teaches

- **Carrier and modulator.** A vocoder has two inputs, and they do different jobs. The **Modulator** says *what* is said: the voice's changing spectrum. The **Carrier** says *what it's said with*: its pitch, its chord, its tone.
- **A sample that plays itself.** A Sampler in One-Shot mode, fired by a Clock Divider every two bars, makes a recorded line part of the song.
- **Mutes as a switch.** Three voices wait on a Mixer, and its mutes choose the one the vocoder hears. Every source keeps playing, so a switch lands mid-word.
- **A whole chord on one cable.** The Chord Sequencer sends five voices down one polyphonic cable. The Vocoder sums them into one carrier, so the voice speaks in harmony.
- **One oscillator, two jobs.** The same saw chord goes raw into the vocoder, which needs its highs, and filtered dark into a pad underneath.
- **Stereo bands.** The Vocoder's odd bands lean left and its even bands lean right. Two Mixer strips, panned apart, keep the spread.

## Modules

| Module | Settings |
|--------|----------|
| [Clock](../modules/modulation/clock.md) | **BPM** 76, **Div** 1 (a pulse every bar) |
| [Clock Divider](../modules/utilities/divider.md) | **Div** 2 (every other bar) |
| [Sampler](../modules/sources/sampler.md) | `samples/soba-speaks-chris.wav` (Chris's voice), **Mode** One-Shot, **Atk** 2 ms, **Rel** 50 ms, **Level** 100%, **Thresh** −40 dB (for recording your own line) |
| [Sampler](../modules/sources/sampler.md) | `samples/soba-speaks.wav` (the synthesized voice), the same but **Level** 56% |
| [Audio Input](../modules/sources/audio-input.md) | Defaults: for your own voice, live and recorded |
| [Mixer](../modules/utilities/mixer.md) (the voices) | **Level 1** to **3** 100%, pans centred, **Mute 2** and **Mute 3** on |
| [Chord Sequencer](../modules/utilities/chord-sequencer.md) | **Steps** 4, **Gate** 100%, **Voices** 5, **Range** 4, **Voicing** Close, **Voice lead** on, **Bass voice** on. Chords Fmaj9 (F3), Em7 (E3), Dm9 (D3), Cmaj7 (C3) |
| [Oscillator](../modules/sources/oscillator.md) | **Wave** Saw, **Voices** 2, **Detune** 10%, **Spread** 50% |
| [SVF Filter](../modules/filters/svf-filter.md) | **Cutoff** 700 Hz, **Res** 10% |
| [VCA](../modules/utilities/vca.md) | **Level** 50% |
| [ADSR Envelope](../modules/modulation/adsr.md) | **Atk** 400 ms, **Dec** 200 ms, **Sus** 100%, **Rel** 1.5 s, **Vel** 0% |
| [Vocoder](../modules/effects/vocoder.md) | **Bands** 16, **Attack** 3 ms, **Release** 35 ms, **Formant** 0, **Sibil** 35%, **Unvoic** 40%, **Width** 60%, **Mix** 100% |
| [Mixer](../modules/utilities/mixer.md) (the mix) | **Level 1** and **Level 2** 28%, **Pan 1** L 80, **Pan 2** R 80. **Level 3** 40%, **Width 3** 60%, **Master** −1.2 dB |
| [Stereo Delay](../modules/effects/delay.md) | **Sync** 1/8D, **FB** 35%, **Mix** 22%, **HiCut** 4 kHz, **LoCut** 300 Hz, **P-P** on, **Tape** on |
| [Reverb](../modules/effects/reverb.md) | **Size** 75%, **Decay** 4 s, **PreD** 25 ms, **Mix** 30%, **Mod** 30% |
| [Audio Output](../modules/output/audio-output.md) | **Vol** 80%, **Limiter** on |

## How it's built

### The voice

```text
[Clock Gate] ──> [Clock Divider Clock]
[Clock Divider Trig] ──> [Sampler Gate]          (both Samplers)
[Sampler L] ──> [Mixer Ch 1]                     (Chris's voice)
[Sampler L] ──> [Mixer Ch 2]                     (the synthesized voice, muted)
[Audio Input L] ──> [Mixer Ch 3]                 (your voice, muted)
               ──> [Sampler In L]               (Chris's Sampler, to record your line)
[Mixer Out L] ──> [Vocoder Modulator]
```

The Clock pulses once a bar. The Clock Divider passes every second pulse to both Samplers, which play their recordings from the start in One-Shot mode, to the end whatever the gate does. Each line lasts about four seconds, a bar and a quarter at 76 BPM, and its last word, *sines*, lands near the change to the second chord.

The voice you hear first belongs to Chris, who made Soba. It was recorded for this example with [`tools/voice/record.py`](https://github.com/chrischaps/Soba/blob/master/tools/voice/record.py). The script counts in at the patch's tempo, and trims the take to the words. A real voice gives the vocoder what a machine has to fake: breath before the words, a pitch that wanders and falls, consonants that are never quite the same twice.

The other voice isn't anyone's recording. It was made by a small formant synthesizer, [`tools/voice/speak.py`](https://github.com/chrischaps/Soba/blob/master/tools/voice/speak.py). A pulse train at a speaking pitch, shaped like the airflow through the vocal folds, runs through a chain of resonators. The lowest three glide from sound to sound: they're the formants that make an "ah" an "ah". The "s" sounds are filtered noise. To a vocoder, that's all a voice is: formants that move, and hiss. Unmute **Ch 2** and mute **Ch 1** on the voice Mixer to hear how close it gets.

The Audio Input also feeds the top Sampler's **In L**, so it can record your line in place of Chris's. Its **Thresh** is at −40 dB: press **Rec** and it waits for you to speak, so the take starts on your first word and the Clock Divider's gate finds it there every two bars.

The voice Mixer only switches, so every strip sits at 100% and centre, and only **Out L** goes on to the vocoder. The synthesized voice's Sampler sits at 56%, 5 dB down, because two voices at one level dry don't come out at one level through a vocoder. A deep voice carries much of its weight below 200 Hz, under most of the bands, while the synthesized one is all mid-range formants. At 56% they come out of the vocoder at one loudness.

### The chords

```text
[Clock Gate] ──> [Chord Sequencer Clock]
[Chord Sequencer Pitch] ──> [Oscillator V/Oct]
[Oscillator Out] ──> [Vocoder Carrier]
                 ──> [SVF Filter In]
[SVF Filter LowPass] ──> [VCA In]
[Chord Sequencer Gate] ──> [ADSR Gate]
[ADSR Out] ──> [VCA CV]
```

The Chord Sequencer steps once a bar through four chords that fall by step: Fmaj9, Em7, Dm9, Cmaj7. With **Voice lead** on, each chord moves to the nearest notes of the next, so the harmony glides down instead of jumping. Five voices of two detuned saws each give the vocoder a carrier rich in highs, all the way up through its top bands.

The same chord goes through the SVF Filter at 700 Hz into a VCA that swells with each chord. That's the pad underneath: dark enough not to compete with the words, and still there when the voice is silent.

### The vocoder and the room

```text
[Vocoder Out L] ──> [Mixer Ch 1]   (panned L 80)
[Vocoder Out R] ──> [Mixer Ch 2]   (panned R 80)
[VCA Out] ──> [Mixer Ch 3]
[Mixer Out L/R] ──> [Stereo Delay] ──> [Reverb] ──> [Audio Output]
```

The Vocoder splits the voice into 16 bands, from 100 Hz to 6.4 kHz. Each band's level opens the matching band of the chord. **Attack** 3 ms and **Release** 35 ms are fast enough for speech, so each syllable starts and stops cleanly. **Sibilance** lets the voice's own "s" sounds through on top, and **Unvoiced** puts noise in the carrier's place while the voice hisses, so the consonants stay readable.

A five-note chord is loud, so the vocoder's strips sit at 28%. The ping-pong echo answers each phrase from the other side, and the reverb puts the voice and the pad in one room.

## Try this

- **Record your own line.** Pick your microphone under **Input** in the toolbar and press **Rec** on the top Sampler. It waits for a sound: say the line, *"Hello. I am Soba. I sing in sines,"* or any line of your own, then press **Rec** again. The Sampler plays your take from the next two bars on, in Chris's place, and keeps it beside the patch. **Ctrl+Z** brings Chris back. Wear headphones, or the take hears the speakers. Then click the take's name under the waveform and choose **Trim to the sound** and **Normalize RMS to −16 dBFS**: the same trim and level `record.py` gave Chris's take, so yours sits in the vocoder as his does.
- **Speak into it.** On the voice Mixer, mute **Ch 1** and unmute **Ch 3**, pick your microphone under **Input** in the toolbar, and talk or sing. Wear headphones, or the mic hears the speakers. Sustained vowels make the chords bloom. A whisper comes through as hiss.
- **Two voices at once.** Unmute **Ch 2** as well as **Ch 1**. The person and the synthesizer start together and drift apart word by word, and the vocoder wears both at once.
- **Robot.** Set **Bands** to 8 and **Release** to 20 ms.
- **Choir.** Set **Bands** to 24 and **Release** to 600 ms. The words smear into a choir that holds each vowel.
- **Change the throat.** Turn **Formant** up to +0.5 oct for a small, bright voice, or down to −0.5 oct for a giant. The pitch stays the chord's. Patch a slow [LFO](../modules/modulation/lfo.md) into the **Formant** input and the voice keeps changing size.
- **A whisper.** Patch [Noise](../modules/sources/noise.md) (white) into **Carrier** in place of the oscillator. The voice comes back with no pitch at all.
- **Hear what the vocoder hears.** Bypass the Vocoder (**Ctrl+B**) and you hear the raw saw chord it's been shaping all along.
