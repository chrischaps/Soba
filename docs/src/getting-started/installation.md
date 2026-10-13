# Installation

Soba runs on Windows, macOS and Linux. You can download a prebuilt release or build it from source with Rust. Or, to hear it first, try it in the browser.

## Try it in the browser

**[Open Soba in the browser](../play/)**: the whole app, nothing to install. It opens on the **First Sound** example. Press **▶ Play**, then play the `Z` to `M` keys, or hold the keys of the Keyboard module's piano with the mouse or a finger.

<iframe class="patch-embed" src="../play/?patch=first-sound" title="First Sound, playable in the browser" loading="lazy"></iframe>

The browser version runs the same engine and modules, and every example is in its **📚 Examples** menu. A few things need the desktop app:

- **MIDI devices and audio input.** In the browser, the computer keyboard and the on-screen pianos play the patch.
- **Recording** to a WAV, and **My Modules**, which keep files in a folder.
- **Low latency.** The browser plays sound through larger buffers, about a tenth of a second behind your playing, and twice that in Firefox. That's fine for listening and exploring, and loose for playing live.

On a phone or tablet, a patch opens zoomed out to fit the screen: pinch to zoom in, and drag the canvas to move around.

Patches go in and out as files: **💾 Save** downloads the patch, and **📂 Open** picks one to upload. Each recipe in this manual can be played on its page.

## Download a release

Each [release on GitHub](https://github.com/chrischaps/Soba/releases) has a download for Windows, macOS (Apple Silicon and Intel) and Linux.

### Windows

Download **`soba-windows-setup.exe`** and run it. It installs Soba for you alone, in `%LOCALAPPDATA%\Programs\Soba`, without asking for an administrator. It adds Soba to the Start Menu, and you can remove it from **Settings → Apps → Installed apps**, like any other app.

For playing live, download **`soba-windows-asio-setup.exe`**, the low-latency build, instead. It runs exactly like the standard one, and can also use an audio interface's ASIO driver: see [Build with ASIO](#build-with-asio-windows-optional) for what that gives you, and skip the building. Installing either one replaces the other.

The first time, Windows may show **"Windows protected your PC"**: the downloads aren't signed yet. Click **More info**, then **Run anyway**.

If you'd rather not install anything, the zips (`soba-windows.zip` and `soba-windows-asio.zip`) hold the same app as one file. Unzip it anywhere and run `soba.exe`.

### macOS

Download the disk image for your Mac: **`soba-macos-apple-silicon.dmg`** for an M-series Mac, or **`soba-macos-intel.dmg`**. Open it and drag **Soba** to **Applications**.

The app isn't signed by Apple yet, so the first time, macOS says it can't check it for malicious software. **Right-click** (or Control-click) Soba in Applications and choose **Open**, then **Open** again. After that it opens normally. On recent versions of macOS, if there's no **Open** button, go to **System Settings → Privacy & Security** and click **Open Anyway** beside the message about Soba.

### Linux

Download `soba-linux.zip`, unzip it, and run `soba`. If it won't start, mark it as a program first: `chmod +x soba`.

## Updates

Once a day, Soba asks GitHub whether a newer release is out. When one is, the dot after the version number in the bottom-right corner lights up blue, and a note says what's new. Click the version number to see the note again.

- **What's New** shows the release notes for every version since yours.
- **Install** downloads the new version, checks it against the release's published checksums (`SHA256SUMS`), puts it in place and restarts. Your patch, including changes you haven't saved, your audio and MIDI devices, and your view of the canvas come back as they were. If the patch has unsaved changes, Soba offers to save them first. Install waits while a recording is running.
- **Later** puts the note away; the dot stays lit. **Skip this version** stops offering that version; a newer one is still offered.
- **Help → Check for Updates…** checks now, and says so if you're up to date.

**What the check sends:** one request to GitHub's public list of releases, saying it comes from `soba` and which version. Nothing about you or your computer, and no usage data. A check that doesn't work, when you're offline say, does nothing visible and tries again next time. To stop checking altogether, turn off **Help → Check for Updates Automatically**; Soba then makes no requests at all.

**Rolling back:** the version you updated from stays beside the new one, and **Help → Roll Back to …** swaps back to it and restarts. The version you left is skipped, so it isn't offered again straight away.

Where Soba can't replace itself, **Install** becomes **Download**, which opens the release page:

- A copy built from source (`cargo run`), which only says that an update exists.
- A copy in a folder it can't write to, such as `Program Files`.
- macOS, until the app is signed: macOS doesn't let a downloaded app replace itself.
- Linux, when a package manager installed it.

Each copy updates to the same kind of download it came from: the low-latency Windows build to the next low-latency build, the standard one to the standard one.

## Build from source

### Install Rust

Install the Rust toolchain with [rustup](https://rustup.rs/):

```bash
# Windows
winget install Rustlang.Rustup

# macOS and Linux
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Check that it worked:

```bash
cargo --version
```

### Install system libraries (Linux only)

Windows and macOS need nothing more: Soba uses WASAPI and CoreAudio, which come with the system. (ASIO on Windows is optional: see [Build with ASIO](#build-with-asio-windows-optional).)

On Linux, install the development packages for ALSA (audio), X11 and keyboard handling. On Debian and Ubuntu:

```bash
sudo apt install libasound2-dev libxcb-render0-dev libxcb-shape0-dev \
  libxcb-xfixes0-dev libxkbcommon-dev libssl-dev
```

On other distributions, install the equivalent ALSA, xcb and xkbcommon development packages (for example `alsa-lib-devel` on Fedora, `alsa-lib` on Arch).

### Build and run

```bash
git clone https://github.com/chrischaps/Soba.git
cd Soba
cargo run --release
```

The first build takes a few minutes. Always use `--release` to play: the debug build is much slower and can't keep up with a busy patch, so you'll hear dropouts. The finished binary is `target/release/soba`.

The app opens on the **First Sound** example. Press **▶ Play**, then play the `Z` to `M` keys on your computer keyboard. If you hear a note, everything is working.

### Build with ASIO (Windows, optional)

For playing a guitar or singing through Soba, Windows Audio's round trip of 60 ms or so is too slow to play against. An audio interface's **ASIO** driver talks to the hardware directly and gets it down to 11–20 ms on a Scarlett 2i2.

<div class="asio-badge"><img src="../images/asio-compatible.svg" alt="ASIO Compatible"><span>ASIO is a registered trademark of Steinberg Media Technologies GmbH</span></div>

The low-latency Windows download (`soba-windows-asio.zip`, see [Download a release](#download-a-release)) has ASIO built in: install your interface's driver (step 1) and choose **ASIO** in the **Output** menu. To build it yourself, it's a build option, off by default:

1. **Install your interface's ASIO driver** from its maker. For a Focusrite Scarlett, that's the Focusrite USB driver from [focusrite.com](https://focusrite.com/downloads). Restart, or unplug the interface and plug it back in, once it's installed: until then the driver may not find the interface.
2. **Install LLVM**, which the build uses to read the ASIO headers:

   ```bash
   winget install LLVM.LLVM
   ```

   Then point the build at it. In a new terminal:

   ```bash
   setx LIBCLANG_PATH "C:\Program Files\LLVM\bin"
   ```

3. **Build with the `asio` feature:**

   ```bash
   cargo run --release --features asio
   ```

   The first build downloads Steinberg's ASIO SDK into your temp folder. To use a copy you already have, set `CPAL_ASIO_DIR` to its folder.

   If the build stops at "Failed to extract ASIO SDK", Windows PowerShell couldn't load its unzip command, which happens when the build runs from PowerShell 7. Build from Git Bash with `env -u PSModulePath cargo run --release --features asio`, or unzip `%TEMP%\asio_sdk.zip` yourself and set `CPAL_ASIO_DIR` to the folder inside.

Then choose **ASIO** at the top of the **Output** menu. See [Audio Input](../modules/sources/audio-input.md#low-latency-with-asio-windows) for choosing a buffer size.

Steinberg licenses the ASIO SDK under the GPLv3 (or its own proprietary terms). Soba's source is MIT, but a binary built with ASIO includes the SDK, so if you share one, the GPLv3 applies to it. The low-latency download is such a binary: it's distributed under the GPLv3, with the GPL, Steinberg's licence for the SDK and a notice in its zip and installer. The exact SDK it was built from (ASIO SDK 2.3.4, pinned by its checksum in `release.yml`) is attached to the same release as `asio-sdk-source.zip`. The other downloads leave ASIO out and stay MIT.

## Command line

Pass a patch file to open it instead of First Sound:

```bash
cargo run --release -- patches/lush-pad.json
```

The `render` tool plays a patch into a WAV file without opening the app or using an audio device, then prints each channel's peak, true peak and RMS level, the mix's loudness, and how hard the patch drives the output limiter:

```bash
cargo run --release --bin render -- patches/fm-synthesis.json out.wav --seconds 5
```

| Option | Default | Effect |
|--------|---------|--------|
| `--seconds N` | 5 | Length of the render |
| `--sample-rate HZ` | 48000 | Sample rate of the file |
| `--block-size N` | 256 | Samples processed per block |
| `--audition` | off | Plays a short phrase into the patch's Keyboard, MIDI Note and Poly MIDI modules |
| `--input FILE.wav` | none | What [Audio Input](../modules/sources/audio-input.md) modules hear, in place of an input device. It must be at the render's sample rate |
| `--cue FILE.txt` | none | Parameter changes to play into the patch as it renders, one per line: `<seconds> param <module>[#n] <parameter> <value>`, as in a capture script. Press a [Looper](../modules/utilities/looper.md)'s footswitches this way |
| `--compare FILE.wav` | none | An earlier render to measure against: prints how far peak, true peak, RMS, loudness and range have moved since |

A patch that waits for a player renders silence unless something inside it plays notes (a Clock or Step Sequencer, say) or you add `--audition`. Audio Input modules render silence unless you give them a file with `--input`. [Samplers](../modules/sources/sampler.md) find their files relative to the patch, as the app does.

Here Lush Pad's filter has been closed from 2500 Hz to 833 Hz since `before.wav`:

```text
rendered 'Lush Pad' (8.00 s @ 48000 Hz) -> after.wav
  L: peak   -4.8 dBFS   true   -4.7 dBTP   rms  -22.2 dBFS
  R: peak   -4.3 dBFS   true   -4.3 dBTP   rms  -21.4 dBFS
  loudness -17.4 LUFS   range 12.3 LU
  limiter in   -4.3 dBFS   took 0.0 dB at most, 0% of the time over 1 dB
against before.wav:
  L: peak   -1.5 dB     true   -1.5 dB     rms   -1.0 dB
  R: peak   -1.2 dB     true   -1.2 dB     rms   -0.9 dB
  loudness  -1.3 LU     range -0.1 LU
```

**True** is the true peak, in dBTP, also after BS.1770-4: how high the waveform goes between the samples, which can be higher than at them. A converter playing the file, or an encoder squeezing it into an MP3 or a video, rebuilds that waveform and clips whatever goes over full scale, so a render whose samples all sit under 0 dBFS can still clip when it's played. A smooth pad reads its sample peak or a tenth above it. Sharp transients go higher: Backbeat's drums read 0.8 dB above their samples. Keep it under −1 dBTP to be safe.

**Loudness** is integrated loudness after ITU-R BS.1770-4, the measure streaming services normalize to. It weights the mix the way the ear does, so it's the number to compare when you want to know whether a change made a patch sound louder. RMS overweights the bass: above, it says the darker pad lost 1 dB, but the ear hears the treble it lost and puts it at 1.3. **Range** is the EBU loudness range, the spread between the quiet and loud passages. It reads `--` on renders shorter than three seconds.

**Limiter** is how hard the patch drives the [Audio Output](../modules/output/audio-output.md)'s limiter: the highest peak going into it, which can be over 0 dBFS, the most it turned the output down, and for how much of the render it took off more than 1 dB. The limiter is there to catch accidents. A patch whose peaks stay under −1 dBFS leaves it idle, as every example does. The examples also sit within a decibel or so of −16 LUFS, so switching between them doesn't jump in volume; aim there to match them.

## Microphone access

Soba opens a microphone or other input only when you choose one in the toolbar's **Input** menu.

- **macOS** asks for permission the first time. If you run Soba from a terminal (`cargo run`), macOS asks on behalf of the terminal app, and the permission belongs to it. If you said no, or the input stays silent, turn it on under **System Settings → Privacy & Security → Microphone**, then quit and reopen the terminal or the app.
- **Windows** lets desktop apps use the microphone unless it's turned off under **Settings → Privacy & security → Microphone** (**Let desktop apps access your microphone**).
- **Linux** has no permission prompt; the input appears if ALSA (or PipeWire's ALSA support) lists it.

To run the test suite, use `cargo test`.

## Troubleshooting

**No sound.** Check that **▶ Play** is pressed: the app opens stopped. Then check the **Output** device menu in the toolbar. It lists every output device, with the system default marked **(Default)**; pick the one you're listening on. **🔄 Refresh** at the bottom of the menu picks up a device you plugged in after starting.

**"Audio unavailable" in the toolbar.** Soba couldn't open an output device. Make sure one is connected and that no other application holds it exclusively, then restart.

**Crackles and dropouts.** Make sure you're running the release build. The toolbar's CPU meter, shown while the patch plays, tells you how close the engine is to its limit. If it's near the top, remove modules or lower the voice count on Poly MIDI and the Oscillator's unison.

**My MIDI controller does nothing.** Choose it in the toolbar's **MIDI In** menu: Soba doesn't connect to a controller until you do. A filled dot (●) before the name means it's connected. Connection errors appear in the status bar at the bottom of the window. See [MIDI](./interface-overview.md#midi).
