# Soba in the browser

The same app, compiled to WebAssembly and drawn on a canvas, so a page can
hand a visitor a patch to play instead of a video of one. It's published
at **soba.chaps.dev**, and every recipe page in the manual
(docs.chaps.dev/soba) embeds its patch from there.

```bash
rustup target add wasm32-unknown-unknown
cargo install --locked trunk
trunk serve                 # http://127.0.0.1:8080, rebuilt on save
trunk build --release       # into dist/
python tools/web/build.py   # into dist/, with the threaded build too: what soba.chaps.dev serves
```

The published site holds two builds of the app (see
[Sound on the audio thread](#sound-on-the-audio-thread)): a threaded one
that renders sound in an AudioWorklet, which the full app runs in Chrome,
Edge and Firefox, and the plain one `trunk` makes, which renders it on the
page's own thread, for embeds, Safari and anywhere else. `trunk serve` is
the plain one alone.

| Address | Opens |
|---|---|
| `/` | The full app on First Sound, with the last session's settings and any autosave |
| `/?patch=lush-pad` | An **embed**: just the canvas and a Play bar, for an iframe. Any example's file name works |
| `/?open=lush-pad` | That example in the full app (the embed's **Open in Soba** link) |
| `/?lesson=first-sound` | A lesson from the manual's Lessons page, in the full app |
| `/#p=1.…` | A **patch link**: the patch inside the address, in the full app, stopped with a Play prompt (see below) |
| `/s/<id>` | A **short patch link**: `404.html` turns it into `/#s=<id>`, and the app fetches the patch (see below) |

## Where it's published

`.github/workflows/docs.yml` builds the manual and the app once CI passes on
a master commit that changes either. The app (`dist/`) goes first, to the `chrischaps/soba.chaps.dev`
repo, served by GitHub Pages under the domain in its `CNAME` file (a DNS
`CNAME` record points `soba` at `chrischaps.github.io`); the workflow
pushes with the `SOBA_DEPLOY_KEY` secret. The manual goes to
`chrischaps/docs.chaps.dev` at `/soba/` as before, with `DOCS_DEPLOY_KEY`.
Its embeds are iframes from soba.chaps.dev with `allow="autoplay"`, so the
sound may start from a click inside them in every browser.

Until #371 the app was at **docs.chaps.dev/soba/play/**. That address is
now `packaging/web/moved.html`, which forwards to soba.chaps.dev keeping
the query and the fragment, so `?patch=`, `?open=`, `?lesson=` and
`#p=` patch links made before the move still open. A browser keeps the
autosave, settings and tab-only files per site, so a visitor arriving
plainly with an unsaved autosave at the old address is told so instead,
and offered `play/stay/`: a copy of the app that still reads the old
site's data, to save the patch from before carrying on.

## Embeds

An embed keeps nothing: it doesn't restore or autosave a session, so playing
with a recipe never leaves an autosave for the full app to offer back.

## Patch links

**Share** (on the desktop too) copies a link that holds the whole patch:
its compact JSON, deflated and written in base64url into the address's
fragment, `soba.chaps.dev/#p=1.<data>` (`persistence/patch_link.rs`). A fragment never
reaches the server, so a static host is all it needs, and nothing
is stored anywhere. The `1.` is the link format's version: a newer one
opened by an older page says to reload, and every later build keeps
reading version 1.

- **Opening one** happens before the session is restored, and the fragment
  is taken out of the address (`history.replaceState`), so reloading picks
  up the session rather than opening the link again over it. The patch
  starts stopped with a note offering **Play**, and opens with nothing
  unsaved, so it's never autosaved over the visitor's own work until they
  change it. A crash's autosave is still offered as usual. A link pasted
  into the address bar of a tab that's already open only changes the
  fragment, which doesn't reload the page: `index.html` notes the
  `hashchange` and the app opens it, asking first about unsaved changes.
  Pasting a link onto the canvas does the same, on the desktop too.
- **Samples:** the examples' samples are named by key
  (`example:samples/…`) and fetched as an example's are. A patch that plays
  the user's own files (from disk, dropped in or recorded in the tab) can't
  go by link: Share names the files and offers Save As, which on the web
  downloads the zip with them.
- **Length:** a link is the address plus about 1.1 K characters for First
  Sound. Measured over the examples (`cargo test --lib patch_link --
  --nocapture` lists them): the recipes and short songs come to 1.1–6.5 K,
  From One Sine 16 K, Orrery 20 K, Clair de Satellite 29 K and Canopy 34 K.
  Browsers take addresses of megabytes; it's chats that may cut the longest
  short, which Share's note mentions over 30 K. Every example round-trips
  exactly, layout included, so nothing is dropped to save space.

### Short links

**Share** copies `soba.chaps.dev/s/<id>` when it can (#376, designed in
[docs/SHORT_LINKS.md](docs/SHORT_LINKS.md)). The link service, a Cloudflare
Worker with a D1 database in `packaging/shortlinks/`, keeps the long link's
payload (`1.<data>`) under an ID worked out from the patch, at
`soba-links.chaps.workers.dev`, an address nobody sees
(`persistence/short_link.rs`).

- **Sharing** posts the payload and copies the short link. The note says
  the patch is kept on soba.chaps.dev and offers **Copy the long link
  instead**. If the service doesn't answer in 4 seconds, or refuses, the
  long link is copied with a quiet word, never an error.
- **Opening one:** GitHub Pages serves `packaging/web/404.html` (copied in by
  `docs.yml`) for `/s/<id>`, which has no file. It replaces the address with
  `/#s=<id>`, and the app fetches the payload and opens it as a long
  link, with the same fragment tidying, Play note and unsaved-changes
  question. At launch nothing else opens meanwhile, and First Sound opens if
  the patch can't be had. A short link pasted onto the canvas or into the
  address bar works too, on the desktop as well (`ureq`, on a thread).
- **The service:** `npx wrangler dev` in `packaging/shortlinks/` runs it
  locally, and `npm test` sweeps every example through it.
  `SOBA_LINKS_URL=http://127.0.0.1:8787 cargo test --lib short_link --
  --ignored` checks the app's side against it. `npx wrangler deploy`
  publishes it. That's done by hand, as it changes rarely.

Not yet: a hosted gallery of shared patches with listen-first previews
rendered by the `render` bin.

This page is the spike report for issue #90: what stood in the way, what
was done about each, and how steady the sound is.

## What changed for the web

One `wasm32` cfg gate keeps the desktop build exactly as it was; the
`WEB` constant in `src/app/mod.rs` hides what the browser can't do.

| Area | Blocker | Done |
|---|---|---|
| **Entry point** | `run_native`, command-line arguments | `main.rs` has a second `main` for wasm32: `eframe::WebRunner` on `index.html`'s canvas, reading `?patch=` / `?open=` |
| **Clocks** | `std::time::Instant` and `SystemTime` panic on wasm32 | `web-time` everywhere (on the desktop it *is* `std::time`) |
| **Audio** | cpal needs a host | cpal's `wasm-bindgen` feature: the WebAudio host. A browser without Web Audio shows "No sound in this browser" instead of cpal's panic. The threaded build uses cpal's `audioworklet` host instead, where the page can have it (see below) |
| **Autoplay** | Browsers hold an `AudioContext` until a gesture, and Safari only releases it *inside* the gesture | `index.html` notes every context and resumes it from the first pointer or key event; the status bar says "Click to start sound" until then |
| **F1** | Chrome, Edge and Firefox open their own help on `F1`, and eframe only stops the browser's default for Tab, Space, Backspace and the arrows | `index.html` calls `preventDefault` on `F1` itself, so it only toggles the Keyboard Shortcuts sheet |
| **MIDI** | The scanner thread holds a non-`Send` Web MIDI handle; Web MIDI needs a permission prompt and Safari has none; MIDI Out's sender is a thread that wakes every millisecond | **Connect MIDI** (the toolbar's MIDI In, and a MIDI Out's device menu) asks for Web MIDI from a click, without sysex (`engine/web_midi.rs`). Its `statechange` stands in for the scanner. Inputs feed the desktop's queues, so MIDI Note, Poly MIDI, the Monitor, MIDI Learn, MIDI clock in and step-record work unchanged. MIDI Out's sender runs on a `setTimeout` (`link/web_sender.rs`) and hands each message over up to 100 ms early, stamped for when its sample is heard; the browser keeps to the stamp. Safari says it has no Web MIDI. See below |
| **Threads** | The recorder's writer thread, the MIDI scanner and sender | Recording is hidden (it writes to a folder); the scanner isn't started, and MIDI Out is sent from a timer |
| **Files** | `rfd`'s dialogs are async on the web, and there's no file system | **Save** downloads the patch; **Open** uploads one (`app/web.rs`). Examples were already `include_str!`. Recent files and the recordings folder are hidden; **My Modules** can't save (it needs a folder). Takes, trims and dropped files are kept in IndexedDB too (`persistence/tab_store.rs`), so a reload or a recovered autosave finds them. The first visit that finds no other copy of the app open lets go of the files its patch doesn't name |
| **Samples** | The examples' WAVs were compiled in, so every visitor downloaded every example's samples (1.7 MB of the wasm, after wasm-opt drops their silences) | trunk copies `patches/samples/` beside the page, and a sample is fetched the first time a patch plays it (`persistence/example_fetch.rs`). Meanwhile the Sampler says "Loading…" and an embed's Play bar "loading samples…". The desktop build still compiles them in |
| **Theme** | eframe follows the browser's light/dark preference, and a light browser started the theme from egui's light style | `apply_theme` pins egui to Dark first |
| **Phones** | First Sound is played from computer keys | The Keyboard and Poly MIDI modules' pianos play when held with the mouse or a finger, sliding legato (on the desktop too) |
| **Small screens** | The node graph zooms only with the wheel and pans only with the middle button, so a phone saw the left of a patch and no way past it | A patch opened in the browser zooms out to fit (`fit_level`). Pinch zooms about the fingers, two fingers pan, and one finger on empty canvas pans instead of drawing a selection box (`follow_gestures`, `follow_touch_pan`). A trackpad pinch or `Ctrl`+scroll zooms about the pointer, on the desktop too |
| **Adding on a phone** | The add menu opened on a right-click and the palette on `Space` or `Tab`: a touch screen has neither | A finger held still on empty canvas for half a second opens the add menu there (`app/long_press.rs`), unless it moved first and became a pan. The menu's categories open on a tap, and their submenus open to the menu's left where the screen ends first. A **+** on the toolbar opens the palette in the middle of the view |

### cpal's WebAudio scheduling, and a fix in the page

cpal's WebAudio host renders on the page's main thread: two
`AudioBufferSourceNode`s take turns, and as one ends its `ended` event renders
the next buffer and schedules it to start where the other leaves off. If the
page is busy past that moment, the buffer is scheduled in the past, and cpal
never catches up: every later buffer is scheduled early too, starting over the
one still playing. A single hiccup becomes garbled sound for good.

`index.html` wraps `AudioBufferSourceNode.start`: when a buffer would start
in the past, the schedule moves on by however late it was (plus 10 ms), so a
hiccup is one short dropout and then clean sound, a little later. The same
wrapper counts what the measurements below report (`sobaAudioStats()` in
the console).

The status bar shows the same thing to a player (#374). On the web, the CPU
meter reads the page's load rather than the DSP's: for each buffer, the time
its `ended` event waited for the page (a frame still drawing) plus the time it
took to render, over the buffer's length. At 100% a buffer comes out late.
Render time alone never shows that: a heavy graph's frames can cause a dropout
while the sound costs 20%. Beside the meter, the count of late buffers so far
turns amber for a few seconds after each one. Both come from
`sobaAudioLoad()` in `index.html`, read by `src/app/web.rs`.

## How steady is it

Measured with Playwright driving each browser headless, playing each patch
for 30 seconds: the first half untouched, the second half with the canvas
dragged back and forth so the whole graph repaints every frame. Lush Pad is
the heaviest example (8 voices of 5-voice unison saws, chorus and reverb),
played with three keys held.

A **late** buffer is a dropout. **Render** is the time the app takes to
make one buffer, on the page's thread.

| Browser | Buffer | Lush Pad | Generative Ambient | First Sound | Render (Lush Pad) |
|---|---|---|---|---|---|
| Chrome 154 | 2048 | 0 late | 0 late | 0 late | 3.7 ms avg, 10 ms max of 43 ms |
| Edge 154 | 2048 | 0 late | 0 late | 0 late | 3.7 ms avg, 9 ms max |
| Firefox 155 | 2048 | **15 late** (one every few seconds, after the first 13 s) | 0 late | 0 late | 4.5 ms avg |
| Firefox 155 | **4096** | 0 late (and 0 in a 60 s run) | 0 late | 0 late | 8.7 ms avg, 14 ms max of 93 ms |
| WebKit 26.6 (Playwright, Windows) | n/a | no Web Audio in this build: the app runs and says so | | | |

So the sound isn't short of CPU: rendering takes a tenth of the buffer or
less. Firefox's drops came a millisecond or so late each time, the look of
its `ended` events arriving late rather than of the page being busy. The
web build therefore asks Firefox for 4096-frame buffers and leaves Chrome,
Edge and Safari at the default 2048 (`web_buffer` in
`engine/audio_engine.rs`).

The cost is latency. From a key to the sound is about two buffers plus the
browser's own output delay: around 100 ms in Chrome, 200 ms in Firefox.
That's fine for exploring a patch and loose for playing live.

**Not measured here:** Safari, and a real phone. Playwright's WebKit on
Windows has no Web Audio, and its phone emulation fakes the pixel ratio
(it drew the UI twice the size; a real 2× scale factor draws it correctly).
Both need a look on real hardware: an iPhone or iPad, and Safari on a Mac.

### The heavy examples

Lush Pad turned out not to be the heaviest. Canopy, Clair de Satellite and
Orrery cost about three times as much natively, and in the browser they
**drop buffers** (#349: `tools/web/steadiness.mjs` in Chrome and
Playwright's Firefox, six runs over three builds). Each cell is the range
of late buffers in 30 seconds, then of the time to render one:

| Browser | Buffer | Lush Pad | Canopy | Clair de Satellite | Orrery |
|---|---|---|---|---|---|
| Chrome | 2048 (43 ms) | 0 late; 4–7 ms | 7–424 late; 11–21 ms | 0–363; 10–19 ms | 5–321; 9–17 ms |
| Firefox | 4096 (93 ms) | 0 late; 7–15 ms | 1–113; 22–67 ms | 0–179; 20–101 ms | 0–69; 19–34 ms |

Rendering alone takes a quarter to a half of each buffer, sometimes more,
and drawing a graph this size shares the thread with it, so a long frame
or two misses the next buffer. The full app now renders elsewhere (see
"Sound on the audio thread" below); embeds are steadied on the page's
thread instead ("Frames wait for the sound").

The browser build is compiled with wasm's 128-bit SIMD (`+simd128`, in
`.cargo/config.toml`) and `wasm-opt -O3`. Every browser that runs Soba has
had SIMD since 2021–23 (Safari since 16.4), and LLVM vectorizes the DSP
loops with it. These runs couldn't say how much it gains: the same build
varied up to 2× from run to run on a machine busy with other work, more
than the difference between builds. So the ranges above span all three
builds, and a quiet machine should measure it again:

```bash
(cd tools/web && npm install && npx playwright install firefox)
trunk build --release
node tools/web/steadiness.mjs --dist dist --browsers chrome,firefox
```

## Sound on the audio thread

The heavy examples drop out on the page's thread however fast the DSP gets,
because a long enough frame always finds a buffer's deadline. So the full
app renders its sound somewhere drawing can't reach: in an AudioWorklet, on
the browser's own audio thread (#375). That's cpal's `audioworklet` host,
behind Soba's `web-worklet` feature.

It takes three things the plain build doesn't have:

- **wasm threads.** The page and the worklet run one wasm module on one
  shared memory, so the engine's queues (rtrb) and atomics work between them
  as they do between the desktop's threads. That needs std rebuilt with the
  `atomics` feature: a nightly toolchain with `-Z build-std`, pinned in
  `tools/web/build.py` and the workflows, plus linker flags for the shared,
  imported memory (2 GB at most) and wasm-bindgen's per-thread setup.
  `build.py` sets them all and builds into `target/web-threads/`.
- **A cross-origin isolated page,** for `SharedArrayBuffer`. That takes two
  response headers, COOP and COEP, and GitHub Pages can't send them. So
  `packaging/web/isolate-sw.js`, a service worker, adds them to the page as
  it's served. The first visit registers it and loads the page once more,
  through it, before the app starts: a patch link's fragment survives that.
  `sobaIsolate()` in `index.html` does it, once a tab, so a browser that
  won't isolate this way isn't reloaded again (nor is a hard reload, which
  bypasses the worker). `?patch=` embeds aren't isolated: the page around
  one isn't, so it couldn't be.
- **A clock in the worklet.** web-time's `Instant` reads `performance`,
  which an AudioWorklet's scope has in neither Chrome nor Firefox, so the
  engine would panic there. `index.html` loads a module into the worklet
  before cpal's that stands one in from `Date`, set to agree with the page's
  `timeOrigin + now()`. MIDI stamped on the page lands where it should, to a
  millisecond, which is all `Date` resolves. The same module times each
  render for `sobaAudioStats()` and the status bar's meter.

`build.py` runs `trunk` twice, puts both builds in `dist/`, and swaps
trunk's loader for one that runs the threaded build on an isolated page and
the plain one anywhere else: embeds, Safari (until someone hears it on real
hardware) and browsers without service workers (Firefox's private windows).
The plain build is exactly what it was, so those play as before.

On the Rust side, the audio callback was already lock-free. The one lock
the two threads share, the `AudioProcessor`'s, is only ever `try_lock`ed by
the callback, and the UI takes it only once the stream is gone. But a
worklet's context closes a moment after its stream drops, and a browser's
page thread may not wait on a lock at all (it traps), so the UI side spins
on `try_lock` in the threaded build (`lock_processor`). The allocator's lock
on wasm already spins.

Embeds on docs.chaps.dev stay on the plain build, on the page's thread
(steadied there as below). Isolating them would take the docs site isolating its own pages
too, and every iframe opting in.

### How steady it is there

The same runs as above (`tools/web/steadiness.mjs --isolated`, which serves
the COOP and COEP headers itself). In the worklet there's no `ended` event
to be late, so a **late** buffer is one the browser played silence for:
Chrome counts them (`AudioContext.playbackStats.underrunEvents`), and
Firefox, which doesn't, by the audio clock falling behind the wall clock
(each dropout leaves it further behind for good). **Render** is one 128-frame
quantum, 2.7 ms at 48 kHz, timed by `Date` to the millisecond, so the mean
is fair and the 95th percentile is a whole number.

Each cell is late buffers while still, then while dragging, and the mean
and 95th percentile render. Below it, the plain build in the same session,
for comparison (its renders are a whole buffer each):

| Browser | Build | Lush Pad | Canopy | Clair de Satellite | Orrery |
|---|---|---|---|---|---|
| Chrome 155 | threaded, 128 frames | 0, 0; 0.2 / 1 ms | 0, 0; 1.0 / 2 ms | 0, 0; 0.7 / 1 ms | 0, 0; 0.7 / 1 ms |
| Firefox 155 | threaded, 128 frames | 0, 0; 0.2 / 1 ms | 0, 0; 0.8 / 2 ms | 0, 0; 0.7 / 1 ms | 0, 0; 0.7 / 1 ms |
| Chrome 155 | plain, 2048 | 0, 0; 3.6 / 4.8 ms | 1, 2; 11.5 / 13 ms | 0, 1; 10.0 / 11 ms | 0, 2; 9.8 / 11 ms |
| Firefox 155 | plain, 4096 | 0, 0; 6.8 / 8 ms | 0, 0; 22.6 / 24 ms | 0, 0; 19.8 / 21 ms | 0, 0; 19.0 / 20 ms |

This run of the plain build dropped far fewer than #349's above, on a
quieter machine, which is the point: on the page's thread the sound is as
steady as the page is idle. In the worklet, dragging the whole graph around
changes nothing. A frame costs a little more there than on the page's
thread (Canopy 7.8 µs against 5.6 µs in Chrome), the overhead of a callback
per 128 frames and of atomics, but that's still under 40% of the audio
thread.

Do the counters see a dropout? A copy of the build whose worklet stalls for
60 ms every 4 seconds, playing Orrery for 30 seconds: Firefox's clock
counted 7 gaps, and in Chrome the clock counted 7 too, while Chrome's own
count was 38, as it counts every device callback it fills with silence,
several to a stall. The status bar's count shows Chrome's.

With the buffers this small, the delay from a key to the sound is a
128-frame quantum plus the browser's and device's own output latency, where
the plain build adds two of its buffers: about 85 ms in Chrome and 170 ms
in Firefox gone. (The output latency itself is the device's. On the
machine these were measured on, a bare AudioContext playing a sine reports
220–240 ms in both browsers, so that's no number to quote for Soba.)

Not tested: Safari (the plain build still), a phone, and the docs site's
embeds isolated.

## Frames wait for the sound

An embed can't use the worklet, so it keeps rendering on the page's thread,
in the `ended` event of the buffer before. A frame of a big patch takes
30–45 ms to draw in Chrome and Firefox, headless or not, and a frame still
drawing when a buffer ends makes the next one late. Two changes steady it
(#387):

- **Frames wait for a buffer.** `index.html` wraps `requestAnimationFrame`:
  when a frame comes while the next buffer is due sooner than a recent long
  frame took to draw, it's held until that buffer has been rendered, then
  drawn in the long gap after. A frame is never held more than 30 ms past
  the buffer's end, in case that end never comes. In a worklet nothing
  plays through `AudioBufferSourceNode`, so nothing is held.
- **Embeds take 4096-frame buffers everywhere** (`web_buffer`), as Firefox
  already did. At 2048 a 43 ms buffer leaves about 30 ms after its render,
  less than a heavy frame, so no frame fits between buffers. 4096 leaves
  about 65 ms, for about 45 ms more delay, which an embed, made for
  listening, can spare.

`tools/web/steadiness.mjs` in both browsers, headless, as embeds; each cell
is late buffers while still, then while dragging, and frames drawn a second
while dragging. `--no-hold` is the same build with frames drawn as they
come:

| Browser | Frames | Lush Pad | Canopy | Clair de Satellite | Orrery |
|---|---|---|---|---|---|
| Chrome, 2048 (before) | as they come | 0, 0 | 106, 136 | 47, 122 | 79, 143 |
| Chrome, 4096 | as they come | 0, 0; 60 fps | 17, 3; 19 fps | 0, 0; 24 fps | 0, 3; 17 fps |
| Chrome, 4096 | **held** | 0, 0; 54 fps | **0, 0**; 12 fps | **0, 0**; 13 fps | **0, 0**; 12 fps |
| Firefox, 4096 (before) | as they come | 0, 0 | 1, 41 | 0, 3 | 0, 1 |
| Firefox, 4096 | **held** | 0, 0; 53 fps | **0, 0**; 12 fps | **0, 0**; 14 fps | **0, 0**; 12 fps |

The price is the picture: a heavy patch draws at 12–14 frames a second
while it plays, against 17–24 drawing as they come, because the gap after
each buffer fits one long frame. Holding at 2048 alone helped (Chrome's
Canopy went from 214 late to 6) but couldn't fit Orrery's frames while
dragging (67 late), hence the bigger buffer too. Not measured: Safari, and
a phone, whose frames are slower still.

## MIDI devices

Chrome, Edge and Firefox have Web MIDI; Safari doesn't. A page may use it
only once the visitor allows it, so nothing is asked until **🎹 Connect
MIDI** is clicked: Firefox first offers a small site add-on that grants it.
midir's own Web MIDI backend wasn't used: it asks as soon as it's touched,
always with sysex (which Chrome words as "control and reprogram your MIDI
devices"), doesn't hear `statechange`, and sends without a timestamp.

- **In:** each input's `midimessage` is stamped on arrival and goes into
  the same audio and UI queues as the desktop's. A device unplugged while
  MIDI In listens to it lets go, releasing its notes.
- **Out:** `MidiSender` is the desktop's, ticked by a timer that sets
  itself for when there's next work. A frame being drawn holds the timer
  up, so it sends each message up to 100 ms before it's due, with
  `MIDIOutput.send(bytes, timestamp)`. The stamp ties `web_time`'s
  `Instant` to `performance.now()` once, so messages due together keep
  their order. A note let go (the patch stopped, a module deleted) is
  stamped after the last message already on its way, so it can't land
  before a Note On the browser is still holding. Measured against a stand-in
  output in Chrome: a Step Sequencer with Send Clock at 120 BPM put its
  ticks 20.83 ms apart (0.14 ms spread), and stopping left no note held.
- **A hidden tab** stops drawing frames but keeps its timers while it plays
  sound, so MIDI Out carries on. The stale rule (250 ms without a block)
  counts from when blocks were last read, so a slow timer doesn't release
  notes that are still playing.
- **Ports A–D** are remembered through eframe's storage, as on the desktop.

## Next steps

- Check Safari and a phone on real hardware, and then let Safari have the
  threaded build (`sobaIsolate()` in `index.html` holds it back).
- Scrolling: over an embed, the wheel and a dragging finger move the patch,
  not the page.
- Recording in the browser: render to memory and download the WAV.
