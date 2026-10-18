# Soba - Claude Instructions

## Project Overview

**Soba** is a **node-based modular audio synthesizer** in Rust (it was called Modular Synth until issue #111; the vault's older notes use that name). Unlike VCV Rack's hardware skeuomorphism, this uses a clean node-graph approach (like Blender nodes) while retaining rich visual feedback.

**Key characteristics:**
- Node-graph UI (not virtual hardware)
- Color-coded signal types and modules
- Lock-free audio architecture (UI and audio threads communicate via ring buffers)
- Learning project for Rust

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│  PRESENTATION: egui + egui_node_graph2                  │
│  - Node graph editor with custom rendering              │
│  - Custom widgets (knobs, faders, waveform displays)    │
└───────────────────────────┬─────────────────────────────┘
                            │ Lock-free ring buffer (rtrb)
┌───────────────────────────┴─────────────────────────────┐
│  ENGINE: Audio graph processor (audio thread)           │
│  - Topological sort for processing order                │
│  - Pre-allocated buffers (no allocations in audio)      │
└───────────────────────────┬─────────────────────────────┘
                            │ cpal audio callback
┌───────────────────────────┴─────────────────────────────┐
│  AUDIO I/O: cpal stream to hardware                     │
└─────────────────────────────────────────────────────────┘
```

## Signal Types (Color-Coded)

| Type    | Color  | Range                | Purpose            |
|---------|--------|----------------------|--------------------|
| Audio   | Blue   | -1.0 to 1.0          | Sample streams     |
| Control | Orange | 0.0-1.0 or -1.0-1.0  | Modulation CV      |
| Gate    | Green  | 0.0 or 1.0           | On/off triggers    |
| MIDI    | Purple | Structured events    | Note/CC data       |
| Bus     | Steel  | Stereo pair + send   | A whole mix, Mixer to Mixer (connects only to Bus) |

## Module Categories (Color-Coded Headers)

- **Blue**: Sources (oscillators, noise, samplers)
- **Teal**: Filters (filters, EQs)
- **Orange**: Modulation (envelopes, LFOs, sequencers)
- **Cyan**: Effects (delay, reverb)
- **Gray**: Utilities (mixers, VCAs)
- **Purple**: Output

These are `ModuleCategory::color` (`src/dsp/module_trait.rs`). Signal colours live in `SignalType::color` and `theme::signal`: use those rather than a hand-typed near-copy, with `gamma_multiply` for a softer tone.

## Directory Structure

```
src/
├── main.rs              # Entry point, egui app setup
├── lib.rs               # Module declarations
├── app/                 # UI application, theme, capture, session
├── graph/               # Node graph (egui_node_graph2 integration), module faces and displays
├── widgets/             # Custom controls (knob, fader, displays)
├── engine/              # Audio engine, graph processor, commands
├── dsp/                 # DspModule trait, ports, parameters, signals
├── modules/             # Built-in modules; the registry is in modules/mod.rs
├── persistence/         # Patch save/load (JSON), format versions
├── link/                # Link Out to Seoul (OSC and MIDI)
├── ableton_link.rs      # Ableton Link tempo sync (the `ableton-link` feature)
├── hosted.rs            # Running inside a DAW as the CLAP plugin
└── bin/render.rs        # Offline patch-to-WAV renderer
plugin/                  # The CLAP plugin
docs/                    # The mdBook user manual
tools/                   # claim.py, nightly.py, showcase/ capture and picture tools
```

## Tech Stack

- **eframe/egui 0.32**: Immediate-mode GUI. Menus go through `widgets::menu`, which keeps them open until an item calls `ui.close()`
- **egui_node_graph2 0.7 (our fork)**: Node editor foundation, from `github.com/chrischaps/egui_node_graph2` (a git dependency, checked out beside this repo at `../egui_node_graph2`). Cable glow, jacks and node drawing hooks live there
- **egui-baseview (our fork)**: The plugin's glue between its baseview window and egui, from `github.com/chrischaps/egui-baseview` (branch `soba`, pinned by rev in `plugin/Cargo.toml`, checked out beside this repo at `../egui-baseview`). Upstream stops at egui 0.33, so each egui step past that lands on the `soba` branch first
- **cpal 0.18**: Cross-platform audio I/O (ASIO behind the `asio` feature)
- **midir 0.11**: MIDI in and out
- **rtrb 0.3**: Lock-free ring buffer for thread communication
- **serde/serde_json**: Patch serialization

## Development Phases

Issues carry a `phase:<n>-<name>` label; `gh label list --search phase` lists them all (fourteen so far, from `phase:1-foundation` to `phase:14-make-music`). Audit issues (`audit`) have no phase. A commit's `[Phase X]` tag names the phase of the issue it closes, or `[Polish]`/`[Docs]` for work outside one.

---

## Working Alongside Other Agents

Assume another agent (or Chris) is working in this repo at the same time as you: in this checkout, in a sibling worktree (`../Modular-<n>` on branch `issue-<n>`), or on another machine. Structure the work so that two sessions can't trip over each other:

- **Look before you start.** Run `git status`, `git worktree list` and `git fetch`, and skim `git log origin/master` since your base. Someone has claimed issue #n if any of these exist: a local worktree or `issue-<n>` branch (this machine), a branch `origin/issue-<n>`, or the `claimed` label on the issue (another machine).
- **Claim before you start, and work in your own worktree** for anything bigger than a one-line fix: `python tools/claim.py <n>` makes `../Modular-<n>` on `issue-<n>` and publishes the claim (see the script for how). If it says the issue is claimed already, take another one. Leave the main checkout on `master` for whoever is there.
- **Changes you didn't make aren't yours.** Unexpected edits, untracked files or a running build in the working tree probably belong to another session. Don't revert, stash, delete, reformat or commit them. Ask if they block you.
- **Stage by path, never `git add -A`.** Commit only the files you changed, and check `git diff --cached` before committing.
- **Keep diffs narrow in the shared hot spots.** The files most often touched are `src/app/synth_app.rs`, `src/graph/node_data.rs`, `src/graph/module_ui.rs`, `src/graph/templates.rs`, `src/modules/mod.rs`, `src/engine/audio_processor.rs`, `src/graph/fixtures/node_templates.snap`, `docs/src/SUMMARY.md`, `docs/src/modules/index.md` and `README.md`. Add to them rather than reorganizing them. Put new behavior in new files where you can. Don't run repo-wide `cargo fmt` or renames as a side effect.
- **Regenerate shared fixtures last.** Snapshots like `node_templates.snap`, rendered goldens and `Cargo.lock` should come from your rebased tree just before you commit, not hand-merged.
- **Rebase, re-verify, then push.** `git fetch && git rebase origin/master`, rerun `cargo test`, then push. If the push is rejected, repeat. Never force-push `master`.
- **Don't fight over processes or build output.** Use your own `CARGO_TARGET_DIR` (e.g. `target/<your-branch>`) when another build or a running Soba might lock the exe. Kill only processes you started, by PID (never `taskkill /IM`).
- **Clean up after yourself.** Once your branch has landed, remove your worktree and branch (`git worktree remove`, `git branch -d`) and release the claim (`git push origin --delete issue-<n>`). If you stop before it lands, either push your work to the branch (`git push --force-with-lease origin issue-<n>`) so whoever picks it up can carry on, or give the issue up with `python tools/claim.py <n> --release`. Leave other sessions' worktrees and branches alone.

---

## Issue Implementation Workflow

When asked to implement the next issue, follow this workflow:

### 1. Find the Next Issue

```bash
gh issue list --state open --limit 100 --search "-label:deferred -label:claimed"
```

The **lowest issue number** is the next to implement. Issues are ordered by phase and dependency. Issues labeled `deferred` are parked: skip them unless Chris asks for one by number. Issues labeled `claimed` are taken (`gh issue list --label claimed` lists them). If the lowest one is already claimed on this machine (see `git worktree list` / `git branch`), take the next one that doesn't depend on it. Then claim yours: `python tools/claim.py <n>`. An open `ci-failure` or `nightly-failure` issue comes before all of them, because master is broken.

### 2. Read the Issue Details

```bash
gh issue view <issue_number>
```

Understand:
- What files need to be created/modified
- Acceptance criteria
- Dependencies on other issues
- Testing instructions

### 3. Implement the Issue

- Read existing code before modifying
- Follow Rust idioms and the patterns in the code around you (PLAN.md is the original plan, kept for history; the code has moved on)
- Keep the lock-free constraint in mind for audio code (no allocations in audio thread)
- Use the signal type colors consistently

### 4. Verify the Implementation

Check what you touched, not the whole suite. The rest runs elsewhere and doesn't wait for you (#272):

- **On every push, GitHub CI** runs the full `cargo test` (against `Cargo.lock`, `--locked`), `cargo clippy` with warnings denied, and the ASIO, wasm and macOS builds. A failure on master opens a `ci-failure` issue.
- **Every night, `tools/nightly.py`** runs the slow checks that need this machine's windows: the loudness of every example, `render --bench` on five heavy examples (failing on a render 15% slower than the last run that passed; `--only perf --rebaseline` accepts one slower on purpose), `check_layout.py`, `zoom_check.py` (`zoom_jitter.py` on every patch something it holds has changed), and the recipe, manual and module pictures. It re-shoots stale pictures and pushes them as a `[Docs]` commit of their own. A failing step opens a `nightly-failure` issue.

So the full `cargo test`, the layout and zoom checks and the picture re-shoots are no longer part of a ticket. At minimum:

```bash
cargo check
cargo +1.99.1 clippy --all-targets -- -D warnings   # CI fails on any warning, rustc's or clippy's, on the Rust it pins (ci.yml's RUST_TOOLCHAIN)
cargo test --lib <module>      # the unit tests of what you changed, e.g. cargo test --lib clock
cargo test --test <file>       # and the tests/<file>.rs that covers it, if one does
cargo run                      # if the issue adds visible functionality
```

- **The whole `cargo test`:** run it only when the change reaches across the engine (the audio graph, the command channel, persistence), where no filter names everything it touched.
- **Templates:** a change to them means regenerating the snapshot with `UPDATE_SNAPSHOTS=1 cargo test --lib test_templates_match_snapshot`. Review its diff.
- **Audio:** for audio-related issues, verify sound output works.

If you changed a module's face, the nightly run finds an example that no longer fits around it, or a face that steps as the view zooms. A face is a port, knob, row, display or anything else that can change its size. To see it now, commit and run `python tools/nightly.py --ref HEAD --only layout,zoom` from your worktree. It builds and films in `../Modular-nightly`, files no issues and pushes nothing.

- **An example it flags:** fix it by growing the frame or moving its neighbours.
- **Steady sizes:** epaint draws fonts at whole pixels, so anything sized by its text steps as the view zooms (#255). Size rows with `steady_text` (`src/graph/steady_text.rs`) or in patch units (`ROW * zoom`), not from `galley.size()` or `ui.label`.

### 5. Commit the Changes

```bash
git add <the files you changed>   # not -A: other agents may have work in the tree
git status  # Review what's being committed
git commit -m "$(cat <<'EOF'
[Phase X] Issue title

- Bullet point of what was done
- Another change

Closes #<issue_number>

Co-Authored-By: Claude <noreply@anthropic.com>
EOF
)"
git fetch && git rebase origin/master   # then re-run your step 4 tests if anything came in
git push
git push origin --delete issue-<n>      # releases the claim
```

### 6. Close the Issue

```bash
gh issue close <issue_number> --comment "Completed!

<Brief summary of what was implemented>

Verified: cargo build ✓, cargo run ✓"
```

---

## Common Commands

```bash
# Build and run
cargo build
cargo run
cargo check

# Run with release optimizations (for audio performance testing)
cargo run --release

# Render a patch to WAV offline (no audio device) and print peak, true peak (dBTP) and RMS levels, plus
# integrated loudness (LUFS, BS.1770-4) and loudness range (LU) of the mix, and the
# peak going into the Audio Output's limiter (and how much it limited)
cargo run --release --bin render -- patch.json out.wav --seconds 5
# ...and how far each has moved since an earlier render: the before/after check
cargo run --release --bin render -- patch.json after.wav --seconds 5 --compare before.wav
# ...timed: processing only, best of 3 (--runs N), with the five heaviest nodes in ns/sample
cargo run --release --bin render -- patches/canopy.json out.wav --seconds 20 --bench
# ...playing a short phrase into Keyboard/MIDI nodes (for patches that need a player)
cargo run --release --bin render -- patches/lush-pad.json out.wav --audition
# ...feeding a WAV to Audio Input modules (a capture script's `input file.wav` cue does the same)
cargo run --release --bin render -- patch.json out.wav --input voice.wav
# ...pressing knobs and footswitches from a cue script (`<seconds> param <module> <param> <value>`)
cargo run --release --bin render -- patches/live-looper.json out.wav --seconds 30 --input take.wav --cue cues.txt
# Every example near -16 LUFS with peaks under -1 dBFS into the limiter: renders
# them all at length (a couple of minutes); the nightly run does this, or run it after an example's levels change
cargo test --release --test example_loudness -- --ignored --nocapture
# The slow checks against origin/master, in ../Modular-nightly at low priority: loudness, layout,
# zoom and the pictures (stale ones re-shot and pushed). Failures open nightly-failure issues
python tools/nightly.py                      # skips if master passed last time; --force runs anyway
python tools/nightly.py --only layout,zoom   # some of: tests, loudness, idle, perf, layout, zoom, images
python tools/nightly.py --ref HEAD --only layout   # your commit instead: no issues, no push
powershell -File tools/nightly_schedule.ps1  # run it every night at 03:00 (-At, -Unregister)
# The real input device, end to end (needs a mic; the patch is muted)
cargo test --release --test live_input -- --ignored --nocapture

# Film the app with sound: a frame-stepped capture plays a cue script through the real
# input paths (see src/app/capture.rs). The showcase kit wraps it; build into
# target/capture so a running copy of the app doesn't lock the exe
CARGO_TARGET_DIR=target/capture cargo build --release
python tools/showcase/shots.py --list            # shots for the chaps.dev page
python tools/showcase/shots.py hero --preview    # frame 0 only, to aim from
python tools/showcase/encode.py hero out.mp4 --loop 2 --fps 30 --crf 27 --poster 0.5
# Captures are lossless by default (~20 MB/s at 1080p30). A long shot passes
# `--crf 12 --preset fast` to soba --capture (a shot sets crf= in shots.py): ~4-5 MB/s
python tools/showcase/shots.py from-one-sine     # the whole 4:41 song, ~10 min to film, 1.3 GB
# A film: a whole song, the camera diving into whichever group is playing (Canopy's
# two cuts). tools/showcase/FILMS.md walks through re-shooting one and starting the next:
# stems.py + activity.py (what plays when), --converge (aim the dives), film,
# check_film.py (review), encode.py. What a film keeps is in tools/showcase/films/<film>/
python tools/showcase/shots.py canopy --converge
python tools/showcase/check_film.py canopy
# Does everything scale smoothly as the view zooms? (#255: text-sized parts step)
python tools/showcase/zoom_jitter.py patches/canopy.json
# ...every example, six at once, skipping those nothing they hold has changed since they passed
python tools/showcase/zoom_check.py   # --all sweeps them all; --plan says which it would
# After a module grows: are every example's modules still inside their frames,
# clear of notes and of each other? (Re-shoot any recipe image of an example it flags)
python tools/showcase/check_layout.py
# Re-shoot every recipe image (docs/src/images/recipe-<name>.png): Live Looper's whole
# patch, From One Sine's mid-song moments and Held Vowels' frozen faces. A recipe with
# an embed has no whole-patch still. --check lists the stale ones instead
python tools/showcase/recipe_images.py [name ...]
python tools/showcase/recipe_images.py --check
python tools/showcase/recipe_images.py --refresh   # all three tools: --check, saving only the stale ones
# --check and --refresh film only the pictures whose patch, modules or shared drawing code changed
# since they last matched, six at a time (tools/showcase/picture_check.py). --all films every one
# The same for the interface pictures, the tutorial's finished patch and a chord played into
# the Chord Sequencer (interface-*.png, tutorial-complete-patch.png, chord-sequencer-play.png),
# each posed by a short cue script
python tools/showcase/manual_images.py [name ...]   # --check lists the stale ones
# ...and the module close-ups (module-<name>.png), each posed by a small patch in
# tools/showcase/modules/. Only the Sampler so far: #199 brings the rest across
python tools/showcase/module_images.py [name ...]   # --check lists the stale ones
# Never stop a capture with taskkill /IM: it closes every open copy of Soba. Kill by PID

# Open a patch at launch (otherwise the First Sound example opens)
cargo run -- patches/fm-synthesis.json

# The browser build (see WEB.md): needs `rustup target add wasm32-unknown-unknown`
# and `cargo install --locked trunk`. ?patch=lush-pad embeds an example
trunk serve                # http://127.0.0.1:8080
trunk build --release      # into dist/, published at soba.chaps.dev

# GitHub CLI
gh issue list --state open
gh issue view <number>
gh issue close <number> --comment "Done"

# Git
git status
git diff
git add <paths>   # stage by path; see "Working Alongside Other Agents"
git commit -m "message"
git push
```

## Important Constraints

1. **No allocations in audio thread** - All buffers must be pre-allocated
2. **Lock-free communication** - Use rtrb ring buffers between UI and audio
3. **Type-safe connections** - Signal types must be validated when connecting ports
4. **Real-time safe** - Audio callback must never block

## Design Philosophy: Inputs vs Knobs

This synthesizer follows a clear separation between **inputs** and **knobs**:

### Inputs (Left Side Ports)
- **Purpose**: Connection points for external signals from other modules
- **Display**: Label only, no inline widget (except Toggle/Select which need special UI)
- **Behavior**: Receive signals from connected modules

### Knobs (Bottom Section)
- **Purpose**: Manual user controls for parameter values
- **Display**: Rotary knob with value readout
- **Behavior**: User can drag to adjust value

### Exposed Parameters (Both Input AND Knob)
Some parameters can be controlled both manually and externally. These are called "exposed" parameters:

1. **When disconnected**: The knob controls the value normally
2. **When connected**: The external signal takes over:
   - Knob becomes read-only (dimmed, non-interactive)
   - Knob position animates to show the incoming signal value
   - Visual indicator (orange dot) shows active external control
3. **When disconnected again**: Returns to manual control

This mirrors real analog modular synthesizers where patching a cable into a parameter jack overrides the manual control.

### Implementation
- `KnobParam::knob_only(...)` - Parameter with knob only, no input port
- `KnobParam::exposed(...)` - Parameter with both input port AND knob
- Signal feedback from audio engine enables knob animation when connected

## Visual Design Notes

From the concept image Chris provided:
- Modules have rounded rectangles with colored header bars
- Ports are metallic circular jacks on module edges
- Cables are bezier curves with signal-type coloring and subtle glow
- Knobs are 3D-style with value displays
- Background is dark with subtle grid pattern

## References

- **PLAN.md**: The original implementation plan (historical: the code is the reference now)
- **FUTURE_CUSTOMIZATIONS.md**: The original case for forking egui_node_graph2 (historical: the fork is done)
- **docs/**: The user manual, one page per module
- **Claude's Vault**: `~\OneDrive\Documents\Claude's Vault`; `Modular Synth Project - 2026-01-20.md` has the early project context, and the `Soba ...` notes the later work
