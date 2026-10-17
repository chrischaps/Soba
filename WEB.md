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
```

| Address | Opens |
|---|---|
| `/` | The full app on First Sound, with the last session's settings and any autosave |
| `/?patch=lush-pad` | An **embed**: just the canvas and a Play bar, for an iframe. Any example's file name works |
| `/?open=lush-pad` | That example in the full app (the embed's **Open in Soba** link) |
| `/?lesson=first-sound` | A lesson from the manual's Lessons page, in the full app |
| `/#p=1.…` | A **patch link**: the patch inside the address, in the full app, stopped with a Play prompt (see below) |

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

Not yet: short links (`soba.chaps.dev/s/<id>`), designed in
[docs/SHORT_LINKS.md](docs/SHORT_LINKS.md) with a spike Worker in
`packaging/shortlinks/`, and a hosted gallery of shared patches with
listen-first previews rendered by the `render` bin.

This page is the spike report for issue #90: what stood in the way, what
was done about each, and how steady the sound is.

## What changed for the web

One `wasm32` cfg gate keeps the desktop build exactly as it was; the
`WEB` constant in `src/app/mod.rs` hides what the browser can't do.

| Area | Blocker | Done |
|---|---|---|
| **Entry point** | `run_native`, command-line arguments | `main.rs` has a second `main` for wasm32: `eframe::WebRunner` on `index.html`'s canvas, reading `?patch=` / `?open=` |
| **Clocks** | `std::time::Instant` and `SystemTime` panic on wasm32 | `web-time` everywhere (on the desktop it *is* `std::time`) |
| **Audio** | cpal needs a host | cpal's `wasm-bindgen` feature: the WebAudio host. A browser without Web Audio shows "No sound in this browser" instead of cpal's panic |
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

## The AudioWorklet route

cpal's other web host, `audioworklet`, renders in the browser's audio thread,
off the page's thread, so drawing could never delay the sound and buffers
could shrink to 128 frames. It needs wasm threads: a nightly toolchain with
`-Z build-std` and the `atomics` target feature, and a cross-origin isolated
page (`Cross-Origin-Opener-Policy` and `Cross-Origin-Embedder-Policy`
headers) for `SharedArrayBuffer`. The docs site is static hosting that can't
set headers, so it would need the `coi-serviceworker` workaround, and an
isolated page can only be embedded by pages that opt in too.

Not worth it while the WebAudio host plays cleanly everywhere tested. It's
the step to take if the browser build is ever for playing live.

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

- Check Safari and a phone on real hardware.
- Scrolling: over an embed, the wheel and a dragging finger move the patch,
  not the page.
- Recording in the browser: render to memory and download the WAV.
