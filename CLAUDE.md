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

## Module Categories (Color-Coded Headers)

- **Blue**: Sources (Oscillators)
- **Green**: Filters
- **Orange**: Utilities (Envelopes, LFOs)
- **Purple**: Output

## Directory Structure

```
src/
├── main.rs              # Entry point, egui app setup
├── lib.rs               # Module declarations
├── app/                 # UI application, theme
├── graph/               # Node graph (egui_node_graph2 integration)
├── widgets/             # Custom controls (knob, fader, displays)
├── engine/              # Audio engine, graph processor, commands
├── dsp/                 # DspModule trait, ports, parameters, signals
├── modules/             # Built-in modules (osc, filter, env, lfo, output)
└── persistence/         # Patch save/load (JSON)
```

## Tech Stack

- **eframe/egui 0.29**: Immediate-mode GUI
- **egui_node_graph2 0.5**: Node editor foundation
- **cpal 0.15**: Cross-platform audio I/O
- **rtrb 0.3**: Lock-free ring buffer for thread communication
- **serde/serde_json**: Patch serialization

## Development Phases

Issues are organized into 6 phases:
1. **Foundation**: Project setup, core types, basic audio, minimal window
2. **Module System**: Registry, oscillator, output, command channel, audio graph
3. **Node Graph UI**: egui_node_graph2 integration, templates, connections
4. **Custom Rendering**: Styled nodes, cables, widgets
5. **Complete Modules**: Filter, envelope, LFO, waveforms
6. **Polish**: Save/load, smoothing, keyboard input, CPU metering

---

## Working Alongside Other Agents

Assume another agent (or Chris) is working in this repo at the same time as you, in this checkout or in a sibling worktree (`../Modular-<n>` on branch `issue-<n>`). Structure the work so that two sessions can't trip over each other:

- **Look before you start.** Run `git status`, `git worktree list` and `git fetch`, and skim `git log origin/master` since your base. An existing worktree or `issue-<n>` branch means someone has claimed issue #n.
- **Work in your own worktree** for anything bigger than a one-line fix: `git worktree add ../Modular-<n> -b issue-<n>`. Leave the main checkout on `master` for whoever is there.
- **Changes you didn't make aren't yours.** Unexpected edits, untracked files or a running build in the working tree probably belong to another session. Don't revert, stash, delete, reformat or commit them. Ask if they block you.
- **Stage by path, never `git add -A`.** Commit only the files you changed, and check `git diff --cached` before committing.
- **Keep diffs narrow in the shared hot spots.** The files most often touched are `src/app/synth_app.rs`, `src/graph/node_data.rs`, `src/graph/module_ui.rs`, `src/graph/templates.rs`, `src/modules/mod.rs`, `src/engine/audio_processor.rs`, `src/graph/fixtures/node_templates.snap`, `docs/src/SUMMARY.md`, `docs/src/modules/index.md` and `README.md`. Add to them rather than reorganizing them. Put new behavior in new files where you can. Don't run repo-wide `cargo fmt` or renames as a side effect.
- **Regenerate shared fixtures last.** Snapshots like `node_templates.snap`, rendered goldens and `Cargo.lock` should come from your rebased tree just before you commit, not hand-merged.
- **Rebase, re-verify, then push.** `git fetch && git rebase origin/master`, rerun `cargo test`, then push. If the push is rejected, repeat. Never force-push `master`.
- **Don't fight over processes or build output.** Use your own `CARGO_TARGET_DIR` (e.g. `target/<your-branch>`) when another build or a running Soba might lock the exe. Kill only processes you started, by PID (never `taskkill /IM`).
- **Clean up after yourself.** Once your branch has landed, remove your worktree and branch (`git worktree remove`, `git branch -d`). Leave other sessions' worktrees and branches alone.

---

## Issue Implementation Workflow

When asked to implement the next issue, follow this workflow:

### 1. Find the Next Issue

```bash
gh issue list --state open --limit 30 --search "-label:deferred"
```

The **lowest issue number** is the next to implement. Issues are ordered by phase and dependency. Issues labeled `deferred` are parked: skip them unless Chris asks for one by number. If the lowest one is already claimed (see `git worktree list` / `git branch -a`), take the next one that doesn't depend on it.

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
- Follow Rust idioms and the patterns established in PLAN.md
- Keep the lock-free constraint in mind for audio code (no allocations in audio thread)
- Use the signal type colors consistently

### 4. Verify the Implementation

Run the verification commands from the issue. At minimum:

```bash
cargo check
cargo build
cargo run  # If the issue adds visible functionality
cargo test # If tests exist
```

For audio-related issues, verify sound output works.

If you changed a module's face (a port, knob, row, display or anything else that can change its size), check the examples still fit around it:

```bash
CARGO_TARGET_DIR=target/capture cargo build --release
python tools/showcase/check_layout.py   # modules inside their frames, clear of notes and each other
```

Fix any example it flags by growing the frame or moving its neighbours. Then re-shoot the recipe images: `python tools/showcase/recipe_images.py --check` lists the stale ones (a face change that keeps the size leaves them stale too), and `python tools/showcase/recipe_images.py <name...>` re-shoots them.

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
git fetch && git rebase origin/master   # then re-run cargo test if anything came in
git push
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

# Render a patch to WAV offline (no audio device) and print peak/RMS levels, plus
# integrated loudness (LUFS, BS.1770-4) and loudness range (LU) of the mix, and the
# peak going into the Audio Output's limiter (and how much it limited)
cargo run --release --bin render -- patch.json out.wav --seconds 5
# ...and how far each has moved since an earlier render: the before/after check
cargo run --release --bin render -- patch.json after.wav --seconds 5 --compare before.wav
# ...playing a short phrase into Keyboard/MIDI nodes (for patches that need a player)
cargo run --release --bin render -- patches/lush-pad.json out.wav --audition
# ...feeding a WAV to Audio Input modules (a capture script's `input file.wav` cue does the same)
cargo run --release --bin render -- patch.json out.wav --input voice.wav
# ...pressing knobs and footswitches from a cue script (`<seconds> param <module> <param> <value>`)
cargo run --release --bin render -- patches/live-looper.json out.wav --seconds 30 --input take.wav --cue cues.txt
# Every example near -16 LUFS with peaks under -1 dBFS into the limiter: renders
# them all at length (a couple of minutes); run before a release, or after an example's levels change
cargo test --release --test example_loudness -- --ignored --nocapture
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
# After a module grows: are every example's modules still inside their frames,
# clear of notes and of each other? (Re-shoot the recipe images it flags)
python tools/showcase/check_layout.py
# Re-shoot every recipe image (docs/src/images/recipe-<name>.png): played, fitted with
# the capture's `fit` cue, cropped and paletted. --check lists the stale ones instead
python tools/showcase/recipe_images.py [name ...]
python tools/showcase/recipe_images.py --check
# Never stop a capture with taskkill /IM: it closes every open copy of Soba. Kill by PID

# Open a patch at launch (otherwise the First Sound example opens)
cargo run -- patches/fm-synthesis.json

# The browser build (see WEB.md): needs `rustup target add wasm32-unknown-unknown`
# and `cargo install --locked trunk`. ?patch=lush-pad embeds an example
trunk serve                # http://127.0.0.1:8080
trunk build --release      # into dist/, published at docs.chaps.dev/soba/play/

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

- **PLAN.md**: Detailed implementation plan with all specifications
- **Claude's Vault**: `~\dev\Claude's Vault\Modular Synth Project - 2026-01-20.md` has project context
