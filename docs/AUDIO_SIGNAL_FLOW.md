# Audio Signal Flow Documentation

This document describes how audio signals flow through the Soba system, from UI interaction to speaker output. Understanding this flow is essential for debugging audio issues.

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                         UI THREAD                                    │
│  ┌─────────────┐    ┌──────────────┐    ┌─────────────────────┐    │
│  │ egui_node_  │───▶│  SynthApp    │───▶│  UiHandle           │    │
│  │ graph2      │    │              │    │  └─ AudioGraph      │    │
│  └─────────────┘    └──────────────┘    │     compile() ──┐   │    │
│                                         └─────────────────┼───┘    │
└───────────────────────────────────────────────────────────┼────────┘
                          GraphPlan, SetParameter, SetPlaying│ ▲ retired
                                         (lock-free rtrb)   │ │ plans
┌───────────────────────────────────────────────────────────┼─┼──────┐
│                       AUDIO THREAD                        ▼ │      │
│  ┌─────────────┐    ┌──────────────┐    ┌─────────────────────┐    │
│  │ cpal        │◀───│ AudioProc-   │◀───│    GraphPlan        │    │
│  │ callback    │    │ essor        │    │    (running)        │    │
│  └──────┬──────┘    └──────────────┘    └─────────────────────┘    │
└─────────┼───────────────────────────────────────────────────────────┘
          │
          ▼
    ┌───────────┐
    │  Speakers │
    └───────────┘
```

## Thread Model

### UI Thread
- Runs the egui event loop
- Handles user interactions (adding nodes, making connections, adjusting parameters)
- Owns the patch (`AudioGraph`): creates modules, sorts the graph, allocates buffers
- Compiles graph changes into a `GraphPlan` and sends it to the audio thread
- Drops plans (and removed modules) the audio thread hands back
- **Never blocks on audio thread**

### Audio Thread
- Runs in cpal's audio callback
- Processes audio in real-time with strict timing requirements
- Swaps in new plans and applies parameter changes from the UI thread
- **Must never allocate memory or block**. This is enforced by `tests/realtime_alloc.rs`

## Signal Flow Step-by-Step

### 1. User Interaction → Commands

When the user interacts with the UI:

```
User Action              →  EngineCommand
─────────────────────────────────────────
Add node                 →  AddModule { node_id, module_id }
Delete node              →  RemoveModule { node_id }
Connect ports            →  Connect { from_node, from_port, to_node, to_port }
Disconnect ports         →  Disconnect { node_id, port, is_input }
Adjust parameter         →  SetParameter { node_id, param_index, value }
Click Play               →  SetPlaying(true)
Click Stop               →  SetPlaying(false)
```

**Key file:** `src/app/synth_app.rs`
- `sync_parameters()` - Sends parameter changes
- Node response handlers - Send add/remove/connect commands
- End of `update()` - `ui_handle.flush()` ships the frame's graph edits

### 2. The Graph Lives on the UI Side

`UiHandle` owns the `AudioGraph`, the patch as modules, cables, parameter values and monitors. Commands are sorted as they arrive:

```
EngineCommand                      What happens
───────────────────────────────────────────────────────────────────
AddModule / RemoveModule /     →   Edit the AudioGraph (UI thread).
Connect / Disconnect /             The module is created and prepare()d
Monitor* / ClearGraph              here, never on the audio thread.
SetParameter                   →   Recorded in the AudioGraph AND queued
                                   straight to the audio thread.
SetPlaying                     →   Queued straight to the audio thread.
```

Once per UI frame, `SynthApp::update()` calls `UiHandle::flush()`. If the graph changed, it is compiled into one `GraphPlan` and queued. So a whole patch load becomes a single plan, not hundreds of messages.

**Key files:** `src/engine/channels.rs`, `src/engine/audio_graph.rs`

### 3. Compiling a GraphPlan

`AudioGraph::compile()` resolves everything the audio thread would otherwise have to look up:

```
GraphPlan
├── nodes        (in topological order)
│   └── PlanNode { node_id, module, params, inputs, outputs }
│       ├── inputs:  [Output(3), Default(0), Output(1)]   ← one per input port
│       └── outputs: 4..6                                 ← range into `outputs`
├── outputs      every output port's SignalBuffer, grouped by node in order
├── defaults     stand-ins for unpatched inputs, filled with the port default
└── input_taps / output_taps   monitor points for knob and LED animation
```

Because buffers are laid out in processing order, every input a node reads lives *before* its own outputs. The engine can split the buffer list in two (`split_at_mut`) and hand the module shared input references and mutable outputs with no copying.

Modules created since the last plan move into the new plan. Modules that are **already running** are left out (`module: None`); they are carried over when the plan is installed.

### 4. Channels (UI ↔ Audio)

Four lock-free `rtrb` ring buffers:

| Queue | Direction | Carries |
|-------|-----------|---------|
| messages | UI → audio | `AudioMessage::{InstallPlan, SetParameter, SetPlaying}` |
| retired plans | audio → UI | the plan each install replaced, to be dropped on the UI thread |
| events | audio → UI | `EngineEvent` output metering, CPU load, status |
| scope frames | audio → UI | `ScopeFrame`: oscilloscope captures in fixed arrays |

Monitor values, module meters and readouts are states, not events, so they don't queue. Each plan carries a monitor board (`src/engine/monitor_board.rs`), a lock-free triple buffer with a fixed place for every reading. The audio thread rewrites it after each callback, and the UI reads the newest once a frame. Peaks (cabled outputs, meters) keep the louder reading until the UI has taken it. A big patch can't overflow it, and the UI's work doesn't grow with the callbacks in a frame. `UiHandle::recv_event` hands its readings out as `EngineEvent`s after the queued ones.

Plans and parameter changes share one queue, so they apply in the order they were sent. A parameter change can overtake its node's plan, because the plan is only sent at the end of the frame. That's harmless: the audio thread ignores the change for an unknown node, and the plan already carries the value.

The UI keeps at most `MAX_PLANS_IN_FLIGHT` plans out, so the audio thread always has room to retire one.

### 5. Audio Callback

cpal calls our audio callback ~100 times per second (at 48kHz with 480 sample blocks):

```rust
// src/engine/audio_engine.rs - build_processor_stream()
move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
    match processor.try_lock() {
        Ok(mut proc) => proc.process(data, channels),
        Err(_) => data.fill(0.0),
    }
}
```

The `data` buffer is what cpal will send to the speakers.

### 6. AudioProcessor::process()

```rust
// src/engine/audio_processor.rs
pub fn process(&mut self, output: &mut [f32], channels: usize) {
    // 1. Apply messages: install plans, set parameters, play/stop
    self.process_messages();

    // 2. Run the plan over the device buffer in chunks of at most
    //    max_block_size frames (cpal may hand us 441, 1024, ...)
    for chunk in output.chunks_mut(chunk_len) {
        self.plan.process(&ProcessContext::new(self.sample_rate, frames));
        self.write_output(chunk, channels, frames);
    }

    // 3. Publish the monitor board; report scope captures, levels, CPU load
}
```

Installing a plan is a handful of pointer moves:

```rust
AudioMessage::InstallPlan(mut plan) => {
    plan.take_over(&mut self.plan);          // running modules move across
    let retired = std::mem::replace(&mut self.plan, plan);
    self.engine_handle.retire_plan(retired); // dropped on the UI thread
}
```

Running modules keep their state, such as oscillator phase, envelope stage and delay tails, so editing a playing patch doesn't click.

### 7. GraphPlan::process()

```rust
// src/engine/graph_plan.rs
for node in nodes.iter_mut() {
    let (upstream, rest) = outputs.split_at_mut(node.outputs.start);
    let own = &mut rest[..node.outputs.len()];
    // clear own outputs, then point each input at its buffer
    let mut inputs = [&EMPTY_BUFFER; MAX_INPUTS];
    for (slot, source) in inputs.iter_mut().zip(&node.inputs) {
        *slot = match *source {
            InputSource::Output(i) => &upstream[i],
            InputSource::Default(i) => &defaults[i],
        };
    }
    module.process(&inputs[..node.inputs.len()], own, &node.params, context);
}
```

No allocation, no hashing, no searching: only slice indexing. Unpatched inputs get a buffer holding the port's default value with `is_connected() == false`, so modules can tell "silent" from "unplugged" via `connected_input()`.

### 8. Output Extraction

The first module in processing order whose `get_audio_output()` returns `Some` (the AudioOutput module) provides the final stereo signal, which `write_output()` interleaves into the cpal buffer.

## Buffer Management

Every buffer is allocated at compile time with capacity for `max_block_size` samples. For shorter blocks, `GraphPlan` shortens buffers within that capacity, which never reallocates. Default-input buffers are refilled with their default value when they grow back.

### SignalBuffer

Holds audio/control/gate samples:

```rust
pub struct SignalBuffer {
    pub samples: Vec<f32>,       // The actual sample data
    pub signal_type: SignalType, // Audio, Control, Gate, or MIDI
    connected: bool,             // false for an unpatched input's stand-in
}
```

**Key file:** `src/dsp/signal.rs`

## Parameter Flow

Parameters flow from UI sliders to module processing:

```
UI Slider (SynthValueType)
    │
    ▼ actual_value() ← IMPORTANT: Returns Hz, not normalized!
SetParameter { value: 440.0 }
    │
    ├──▶ AudioGraph::set_parameter()      (UI-side copy, carried by the next plan)
    │
    ▼ (via ring buffer)
GraphPlan::set_parameter()                (running plan, updated in place)
    │
    ▼
module.process(..., params: &[f32], ...)
    │
    ▼
let base_freq = params[0]; // 440.0 Hz
```

**Critical:** Parameters are stored and sent as actual values (Hz, seconds, etc.), not normalized 0-1 values. The `SynthValueType::actual_value()` method handles this conversion.

## Port Index Mapping

Ports are indexed differently in different contexts:

### In egui_node_graph2
- All ports have sequential indices
- Example: SineOscillator ports [0: freq_cv, 1: fm, 2: out]

### In DspModule
- Input ports and output ports are separate
- `process()` receives one input buffer per input port, and one output buffer per output port
- The compiler maps a connection's `from_port` (a port index) to that node's output buffer

### Example: SineOscillator
```
Port Definition:
  [0] freq_cv  (Input, Control)
  [1] fm       (Input, Control)
  [2] out      (Output, Audio)

Input indices:  [0: freq_cv, 1: fm]
Output indices: [0: out]

When connecting osc.out (port 2) → output.mono (port 2):
  - Connection stores: from_port=2, to_port=2
  - Port 2 is the oscillator's first output, so it reads output buffer 0
```

## Common Debugging Points

### No Sound - Checklist

1. **Is playing?** Check `AudioProcessor::is_playing`
2. **Was the edit flushed?** Graph edits reach the audio thread only on `UiHandle::flush()`
3. **Modules in the plan?** Check `processor.plan().len()` / `plan.processing_order()`
4. **Connections made?** Check `ui_handle.graph().connections()`
5. **Parameters correct?** Verify frequency is in Hz, not normalized (0-1)
6. **Output module exists?** Look for module with `get_audio_output()` returning `Some`

### Reproduce Offline

Never add `eprintln!` to the audio callback: printing allocates and locks. Reproduce the problem in the offline renderer instead. It runs the same compile-and-swap path with no audio device:

```bash
cargo run --release --bin render -- patch.json out.wav --seconds 5
```

or in a test with `OfflineRenderer::from_patch()` and the helpers in `dsp::analysis`, where you can inspect any buffer at leisure.

`tests/realtime_alloc.rs` guards the "no allocation" rule. It fails if the audio callback allocates or frees memory while playing a patch that uses every module.

## Files Reference

| File | Purpose |
|------|---------|
| `src/engine/audio_engine.rs` | cpal setup, audio callback |
| `src/engine/audio_processor.rs` | Audio-thread loop: messages, plan swaps, output |
| `src/engine/audio_graph.rs` | Patch model, topological sort, plan compiler (UI side) |
| `src/engine/graph_plan.rs` | Compiled graph and its real-time processing |
| `src/engine/channels.rs` | Lock-free queues; `UiHandle::flush()` |
| `src/engine/commands.rs` | Commands, audio messages, events, scope frames |
| `src/engine/offline.rs` | Offline renderer for tests and the `render` tool |
| `src/dsp/module_trait.rs` | DspModule trait definition |
| `src/dsp/signal.rs` | SignalBuffer, SignalType |
| `src/modules/oscillator.rs` | SineOscillator implementation |
| `src/modules/output.rs` | AudioOutput implementation |
| `src/app/synth_app.rs` | UI, parameter sync, command sending |
| `src/graph/value_types.rs` | Parameter value types (actual_value!) |
