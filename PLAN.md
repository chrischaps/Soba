# Soba - Implementation Plan

> **Historical.** This is the plan the project started from (January 2026, as Modular Synth), kept for its reasoning. The code has moved on: the versions below are now egui/eframe 0.30, cpal 0.18 and our fork of egui_node_graph2 (0.7, a git dependency); there is no `registry.rs` (modules register in `src/modules/mod.rs`); and the work runs to fourteen phases, not six (the `phase:*` issue labels). Read the code, `CLAUDE.md` and the manual in `docs/` for how things are now.

## Overview

A node-based modular synthesizer in Rust with a polished visual interface inspired by the provided concept art. Unlike VCV Rack's hardware skeuomorphism, this takes a cleaner node-graph approach while retaining rich visual feedback (waveform displays, meters, custom widgets).

## Technology Stack

| Component | Choice | Rationale |
|-----------|--------|-----------|
| Language | Rust | Learning opportunity, excellent for real-time audio |
| Audio I/O | `cpal` | Cross-platform, well-maintained |
| GUI Framework | `egui` + `eframe` | Immediate mode, good for custom rendering |
| Node Graph | `egui_node_graph2` | Node editor foundation (will need customization) |
| Lock-free Comms | `rtrb` | Real-time safe ring buffer |
| Serialization | `serde` + `serde_json` | Patch save/load |

## Architecture Layers

```
┌─────────────────────────────────────────────────────────┐
│  PRESENTATION: egui application, custom node renderer   │
│  - Node graph editor (egui_node_graph2 + custom)        │
│  - Module parameter panels                              │
│  - Custom widgets (knobs, faders, displays)             │
└───────────────────────────┬─────────────────────────────┘
                            │ Lock-free ring buffer (rtrb)
                            │ Commands: AddNode, Connect, SetParam...
┌───────────────────────────┴─────────────────────────────┐
│  ENGINE: Audio graph processor (runs in audio thread)   │
│  - Topological sort for processing order                │
│  - Buffer management (pre-allocated)                    │
│  - Module lifecycle management                          │
└───────────────────────────┬─────────────────────────────┘
                            │ cpal audio callback
┌───────────────────────────┴─────────────────────────────┐
│  AUDIO I/O: cpal stream to hardware                     │
└─────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────┐
│  MODULES: DspModule trait implementations               │
│  - Oscillator, Filter, Envelope, LFO, Output            │
└─────────────────────────────────────────────────────────┘
```

## Signal Type System

Four signal types with color coding (matching concept art):

| Type | Purpose | Color | Value Range |
|------|---------|-------|-------------|
| Audio | Sample streams | Blue | -1.0 to 1.0 |
| Control | Modulation CV | Orange | 0.0 to 1.0 (unipolar) or -1.0 to 1.0 (bipolar) |
| Gate | On/off triggers | Green | 0.0 or 1.0 |
| MIDI | Note/CC data | Purple | Structured events |

**Connection Rules:**
- Same types always connect
- Audio ↔ Control (implicit, for audio-rate modulation)
- Gate → Control (on/off as modulation)
- MIDI remains separate (must go through converter modules)

## Module Protocol

Core trait that all modules implement:

```rust
pub trait DspModule: Send + 'static {
    fn info(&self) -> &ModuleInfo;
    fn ports(&self) -> &[PortDefinition];
    fn parameters(&self) -> &[ParameterDefinition];
    fn prepare(&mut self, sample_rate: f32, max_block_size: usize);
    fn process(&mut self, inputs: &[&SignalBuffer], outputs: &mut [SignalBuffer],
               params: &[f32], context: &ProcessContext);
    fn reset(&mut self);
}
```

**Port Definition** - Declares inputs/outputs with type, name, default value

**Parameter Definition** - Declares controls with:
- Range (min/max/default)
- Display mode (Linear, Logarithmic, Discrete, Toggle)
- Unit string ("Hz", "ms", "dB", etc.)

**UI Control Types** (for module builders):
- Rotary knob (continuous)
- Fader/slider (vertical or horizontal)
- Dropdown selector (discrete choices)
- Toggle button
- Waveform display (read-only visualization)
- Level meter (read-only)

## Initial Module Set (5 modules)

| Module | Category | Inputs | Outputs | Key Parameters |
|--------|----------|--------|---------|----------------|
| **Oscillator** | Source | Freq CV, FM | Audio Out | Waveform (Sine/Saw/Square/Tri), Pitch, Tune |
| **SVF Filter** | Filter | Audio In, Cutoff CV, Res CV | LP/HP/BP Out | Cutoff, Resonance, Drive |
| **ADSR Envelope** | Modulation | Gate | Envelope Out | Attack, Decay, Sustain, Release |
| **LFO** | Modulation | Rate CV, Sync | Multi Out | Rate, Waveform |
| **Audio Output** | Output | L, R | (to speakers) | Master Volume |

## Visual Design (per concept image)

**Module Appearance:**
- Rounded rectangle with colored header bar
- Module type icon in header
- Dark body with subtle inner glow
- Ports as metallic circular jacks on edges
- Color theme per category (blue=source, green=filter, orange=utility, purple=output)

**Cables:**
- Bezier curves with color matching signal type
- Subtle glow/shadow for depth
- Smooth anti-aliasing

**Widgets:**
- 3D-style rotary knobs with value display
- Vertical faders with track and thumb
- Waveform/spectrum displays with grid
- VU meters with segmented LEDs

**Background:**
- Dark grid pattern (subtle)

## File Structure

```
soba/
├── Cargo.toml
├── src/
│   ├── main.rs
│   ├── lib.rs
│   ├── app/                    # UI application
│   │   ├── mod.rs
│   │   ├── synth_app.rs        # Main egui app
│   │   └── theme.rs            # Colors, styles
│   ├── graph/                  # Node graph integration
│   │   ├── mod.rs
│   │   ├── data_types.rs       # SignalType wrapper
│   │   ├── node_templates.rs   # Module → node mapping
│   │   └── custom_renderer.rs  # Custom node/cable drawing
│   ├── widgets/                # Custom UI controls
│   │   ├── mod.rs
│   │   ├── knob.rs
│   │   ├── fader.rs
│   │   ├── waveform_display.rs
│   │   └── vu_meter.rs
│   ├── engine/                 # Audio engine
│   │   ├── mod.rs
│   │   ├── audio_engine.rs     # cpal integration
│   │   ├── audio_graph.rs      # Graph processor
│   │   ├── commands.rs         # EngineCommand enum
│   │   └── buffer_pool.rs      # Pre-allocated buffers
│   ├── dsp/                    # Module system
│   │   ├── mod.rs
│   │   ├── module_trait.rs     # DspModule trait
│   │   ├── port.rs
│   │   ├── parameter.rs
│   │   ├── signal.rs           # SignalBuffer, SignalType
│   │   └── registry.rs         # ModuleRegistry
│   ├── modules/                # Built-in modules
│   │   ├── mod.rs
│   │   ├── oscillator.rs
│   │   ├── filter.rs
│   │   ├── envelope.rs
│   │   ├── lfo.rs
│   │   └── output.rs
│   └── persistence/            # Save/load
│       ├── mod.rs
│       └── patch.rs
```

## Implementation Phases

### Phase 1: Foundation
1. Set up Rust project with dependencies
2. Implement `SignalType`, `SignalBuffer`, core data structures
3. Define `DspModule` trait and supporting types
4. Create basic `AudioEngine` with cpal (just output silence, then sine test tone)
5. Create minimal egui window

**Verification:** Window opens, test tone plays through speakers

### Phase 2: Module System
1. Implement `ModuleRegistry`
2. Create `SineOscillator` module (simplest complete module)
3. Create `AudioOutput` module
4. Wire up command channel (rtrb) between UI and engine
5. Implement `AudioGraph` with topological sort

**Verification:** Can create oscillator in code, hear output

### Phase 3: Node Graph UI
1. Integrate `egui_node_graph2`
2. Map modules to node templates
3. Implement connection validation (type checking)
4. Sync graph changes to engine via commands

**Verification:** Can add/remove/connect nodes visually, hear result

### Phase 4: Custom Rendering
1. Create custom node renderer (colored headers, styled bodies)
2. Implement custom cable rendering (bezier, colored)
3. Build widget library (knob, fader)
4. Add waveform display widget

**Verification:** UI matches concept art aesthetic

### Phase 5: Complete Module Set
1. Implement SVF Filter
2. Implement ADSR Envelope
3. Implement LFO
4. Add remaining oscillator waveforms

**Verification:** Can build subtractive synth patch (osc→filter→output with envelope)

### Phase 6: Polish
1. Patch save/load (JSON serialization)
2. Parameter smoothing (anti-zipper)
3. Keyboard input for note triggers
4. CPU metering

**Verification:** Can save patch, reload it, play notes

## Key Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| Real-time safety violations (allocations in audio thread) | Pre-allocate all buffers; use `rtrb` for lock-free comms; review audio callback carefully |
| Custom egui rendering complexity | Start with functional defaults, add visual polish incrementally |
| Feedback loops in graph | Detect cycles, insert 1-block delay or reject connection |
| Cross-platform audio differences | Test on Windows early (your platform); query device capabilities |

## Dependencies (Cargo.toml)

```toml
[dependencies]
eframe = "0.29"
egui = "0.29"
egui_node_graph2 = "0.5"
cpal = "0.15"
rtrb = "0.3"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
```

## Verification Strategy

After each phase:
1. **Audio test** - Does it make sound correctly?
2. **UI test** - Can you interact with the graph?
3. **Integration test** - Do UI changes reflect in audio output?

Final integration test: Build a patch with Oscillator → Filter → Output, modulate filter cutoff with LFO, trigger envelope from keyboard. Save, close, reload, verify it sounds the same.
