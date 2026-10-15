//! Monitor Board
//!
//! What the UI watches as a patch plays: the signal at each patched knob's
//! input, the peaks on each cabled output, and each module's meters and
//! readout (where a filter has swept to, how far through its stages an
//! envelope is). These are states rather than events. The UI wants the
//! newest of each once a frame, however many callbacks ran in between.
//!
//! So instead of queueing a reading per port per callback, which a big
//! patch overflows (and the readings that don't fit are lost, always the
//! same modules'), each compiled plan carries a board with a fixed place for
//! every reading. It crosses in a lock-free triple buffer: the audio thread
//! rewrites it after every callback, and the UI takes the newest once a
//! frame. Nothing is dropped, and the UI's work grows with the patch, not
//! with the patch times the callbacks in a frame.
//!
//! Peaks wait for the UI. Until it has taken a board, the next one carries
//! the louder of each output's and meter's readings, so a gate shorter than
//! a frame still lights its cable.

use std::sync::{Arc, Weak};

use triple_buffer::{triple_buffer, Input, Output};

use super::commands::{ChannelPeaks, EngineEvent, NodeId, PortIndex};
use super::graph_plan::GraphPlan;
use crate::dsp::{MeterLevels, Readout};

/// One reading of everything a plan monitors, in the plan's own order.
#[derive(Clone)]
struct Readings {
    /// Per input tap
    inputs: Box<[f32]>,
    /// Per output tap
    outputs: Box<[ChannelPeaks]>,
    /// Per module, in processing order
    meters: Box<[Option<MeterLevels>]>,
    /// Per module, in processing order
    readouts: Box<[Option<Readout>]>,
}

impl Readings {
    fn for_plan(plan: &GraphPlan) -> Self {
        let modules = plan.nodes.len();
        Self {
            inputs: vec![0.0; plan.input_taps.len()].into(),
            outputs: vec![ChannelPeaks::default(); plan.output_taps.len()].into(),
            meters: vec![None; modules].into(),
            readouts: vec![None; modules].into(),
        }
    }

    /// REAL-TIME SAFE: both were made for the same plan, so sizes match.
    fn copy_from(&mut self, other: &Readings) {
        self.inputs.copy_from_slice(&other.inputs);
        self.outputs.copy_from_slice(&other.outputs);
        self.meters.copy_from_slice(&other.meters);
        self.readouts.copy_from_slice(&other.readouts);
    }
}

/// The audio thread's end of a plan's board.
pub(crate) struct MonitorWriter {
    board: Input<Readings>,
    /// What the next board shows: the latest values, and the peaks gathered
    /// since the UI last took one
    held: Readings,
    /// Lets the UI tell when the plan is gone, and its board with it
    _alive: Arc<()>,
}

impl MonitorWriter {
    /// Records `plan`'s readings after a callback and hands them to the UI.
    ///
    /// REAL-TIME SAFE: copies into room made when the plan was compiled.
    pub(crate) fn publish(&mut self, plan: &mut GraphPlan) {
        // Peaks the UI has seen start over; ones it hasn't are kept if louder
        let fresh = self.board.consumed();
        let held = &mut self.held;
        for (slot, (_, _, value)) in held.inputs.iter_mut().zip(plan.input_values()) {
            *slot = value;
        }
        for (slot, (_, _, _, peaks)) in held.outputs.iter_mut().zip(plan.output_values()) {
            *slot = if fresh { peaks } else { slot.louder(&peaks) };
        }
        plan.restart_output_peaks();
        let modules = held.meters.iter_mut().zip(held.readouts.iter_mut()).zip(plan.nodes.iter_mut());
        for ((meter, readout), node) in modules {
            let Some(module) = node.module.as_mut() else { continue };
            let levels = module.take_meter_levels();
            *meter = match (*meter, levels) {
                (Some(earlier), Some(levels)) if !fresh => Some(louder_meters(&earlier, &levels)),
                (earlier, None) if !fresh => earlier,
                (_, levels) => levels,
            };
            // A module with nothing to show this callback keeps what it showed
            if let Some(latest) = module.readout(&node.params) {
                *readout = Some(latest);
            }
        }
        self.board.input_buffer_mut().copy_from(&self.held);
        self.board.publish();
    }
}

/// Each meter's larger peak of `a` and `b`.
fn louder_meters(a: &MeterLevels, b: &MeterLevels) -> MeterLevels {
    let mut peaks = b.peaks;
    for (peak, &earlier) in peaks.iter_mut().zip(&a.peaks) {
        if earlier.abs() > peak.abs() {
            *peak = earlier;
        }
    }
    MeterLevels { peaks }
}

/// The UI's end of a plan's board, and what each place on it is.
struct MonitorReader {
    board: Output<Readings>,
    writer: Weak<()>,
    inputs: Box<[(NodeId, PortIndex)]>,
    outputs: Box<[(NodeId, PortIndex)]>,
    modules: Box<[NodeId]>,
    /// Whether this sweep has new readings from it to give out
    fresh: bool,
}

impl MonitorReader {
    /// The reading in place `slot`, counting through inputs, outputs,
    /// meters and readouts in turn: `None` past the end, `Some(None)` for a
    /// module with no meters or readout.
    fn reading(&self, slot: usize) -> Option<Option<EngineEvent>> {
        let readings = self.board.output_buffer();
        let mut slot = slot;
        if let Some(&(node_id, input_index)) = self.inputs.get(slot) {
            let value = readings.inputs[slot];
            return Some(Some(EngineEvent::InputValue { node_id, input_index, value }));
        }
        slot -= self.inputs.len();
        if let Some(&(node_id, output_index)) = self.outputs.get(slot) {
            let channels = readings.outputs[slot];
            return Some(Some(EngineEvent::OutputValue { node_id, output_index, value: channels.overall(), channels }));
        }
        slot -= self.outputs.len();
        if let Some(&node_id) = self.modules.get(slot) {
            return Some(readings.meters[slot].map(|levels| EngineEvent::MeterLevels { node_id, levels }));
        }
        slot -= self.modules.len();
        let &node_id = self.modules.get(slot)?;
        Some(readings.readouts[slot].map(|readout| EngineEvent::Readout { node_id, readout }))
    }
}

/// The UI's ends of every plan's board, oldest first: the plan that's
/// playing, and any compiled since that haven't replaced it yet.
#[derive(Default)]
pub(crate) struct Monitors {
    readers: Vec<MonitorReader>,
    /// How far through a sweep [`next`](Self::next) is: which board, and
    /// which place on it
    place: Option<(usize, usize)>,
}

impl Monitors {
    /// Gives a freshly compiled plan a board, keeping the UI's end of it.
    pub(crate) fn watch(&mut self, mut plan: Box<GraphPlan>) -> Box<GraphPlan> {
        let (board, reader) = triple_buffer(&Readings::for_plan(&plan));
        let alive = Arc::new(());
        self.readers.push(MonitorReader {
            board: reader,
            writer: Arc::downgrade(&alive),
            inputs: plan.input_taps.iter().map(|tap| (tap.node_id, tap.input_index)).collect(),
            outputs: plan.output_taps.iter().map(|tap| (tap.node_id, tap.output_index)).collect(),
            modules: plan.nodes.iter().map(|node| node.node_id).collect(),
            fresh: false,
        });
        let held = Readings::for_plan(&plan);
        plan.monitor = Some(MonitorWriter { board, held, _alive: alive });
        plan
    }

    /// The next reading for the UI, as the event it stands for. Each sweep
    /// gives out every board's newest readings once, the oldest board's
    /// first so the newest plan's have the last word, then returns `None`.
    /// A board with nothing new since the last sweep gives nothing.
    pub(crate) fn next(&mut self) -> Option<EngineEvent> {
        let (mut reader, mut slot) = match self.place {
            Some(place) => place,
            None => {
                self.begin_sweep();
                (0, 0)
            }
        };
        while let Some(board) = self.readers.get(reader) {
            if board.fresh {
                while let Some(reading) = board.reading(slot) {
                    slot += 1;
                    if let Some(event) = reading {
                        self.place = Some((reader, slot));
                        return Some(event);
                    }
                }
            }
            reader += 1;
            slot = 0;
        }
        self.place = None;
        None
    }

    fn begin_sweep(&mut self) {
        // A plan that's been dropped won't write again
        self.readers.retain(|reader| reader.writer.strong_count() > 0);
        for reader in &mut self.readers {
            reader.fresh = reader.board.update();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::{SignalBuffer, SignalType};
    use crate::engine::graph_plan::{InputTap, MonitorSource, OutputTap};

    /// A plan with no modules, monitoring `outputs` gate outputs (each
    /// reported as node 1, port n) and `inputs` inputs reading the first.
    fn plan(outputs: usize, inputs: usize) -> Box<GraphPlan> {
        let mut plan = Box::new(GraphPlan::empty(4));
        for n in 0..outputs {
            plan.outputs.push(SignalBuffer::new(4, SignalType::Gate));
            plan.output_taps.push(OutputTap { node_id: 1, output_index: n, buffer: n, peaks: None });
        }
        for n in 0..inputs {
            plan.input_taps.push(InputTap { node_id: 2, input_index: n, source: MonitorSource::Output(0) });
        }
        plan
    }

    fn sweep(monitors: &mut Monitors) -> Vec<EngineEvent> {
        std::iter::from_fn(|| monitors.next()).collect()
    }

    fn output_value(event: &EngineEvent) -> f32 {
        match event {
            EngineEvent::OutputValue { value, .. } => *value,
            other => panic!("expected an output value, got {other:?}"),
        }
    }

    #[test]
    fn test_every_reading_of_a_big_patch_arrives() {
        let mut monitors = Monitors::default();
        let mut plan = monitors.watch(plan(600, 300));
        // Many callbacks between two UI frames
        for _ in 0..10 {
            plan.publish_monitors();
        }
        let events = sweep(&mut monitors);
        assert_eq!(events.iter().filter(|e| matches!(e, EngineEvent::InputValue { .. })).count(), 300);
        let mut outputs: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                EngineEvent::OutputValue { output_index, .. } => Some(*output_index),
                _ => None,
            })
            .collect();
        outputs.sort_unstable();
        assert_eq!(outputs, (0..600).collect::<Vec<_>>(), "each output once");
    }

    #[test]
    fn test_a_sweep_gives_only_what_is_new() {
        let mut monitors = Monitors::default();
        let mut plan = monitors.watch(plan(2, 0));
        assert!(sweep(&mut monitors).is_empty(), "nothing played yet");
        plan.publish_monitors();
        assert_eq!(sweep(&mut monitors).len(), 2);
        assert!(sweep(&mut monitors).is_empty(), "already given out");
        plan.publish_monitors();
        assert_eq!(sweep(&mut monitors).len(), 2);
    }

    #[test]
    fn test_a_hit_early_in_a_callback_is_heard() {
        // A callback of several blocks: a pulse in the first, silence after
        let mut monitors = Monitors::default();
        let mut plan = monitors.watch(plan(1, 0));
        let context = crate::dsp::ProcessContext::new(48000.0, 4);
        plan.outputs[0].samples.fill(1.0);
        plan.process(&context);
        plan.outputs[0].samples.fill(0.0);
        for _ in 0..3 {
            plan.process(&context);
        }
        plan.publish_monitors();
        assert_eq!(sweep(&mut monitors).iter().map(output_value).collect::<Vec<_>>(), [1.0]);

        // Read, it starts over: the next callback is silent
        plan.process(&context);
        plan.publish_monitors();
        assert_eq!(sweep(&mut monitors).iter().map(output_value).collect::<Vec<_>>(), [0.0]);
    }

    #[test]
    fn test_a_peak_waits_for_the_ui() {
        let mut monitors = Monitors::default();
        let mut plan = monitors.watch(plan(1, 0));
        // A pulse in one callback, then silence for several more
        plan.outputs[0].samples.fill(1.0);
        plan.publish_monitors();
        plan.outputs[0].samples.fill(0.0);
        for _ in 0..4 {
            plan.publish_monitors();
        }
        assert_eq!(output_value(&sweep(&mut monitors)[0]), 1.0, "the frame sees the pulse");
        // Once seen, it doesn't linger
        plan.publish_monitors();
        assert_eq!(output_value(&sweep(&mut monitors)[0]), 0.0);
    }

    #[test]
    fn test_values_are_the_latest() {
        let mut monitors = Monitors::default();
        let mut plan = monitors.watch(plan(1, 1));
        for value in [0.9, -0.2, 0.4] {
            plan.outputs[0].samples.fill(value);
            plan.publish_monitors();
        }
        let input = sweep(&mut monitors).into_iter().find_map(|e| match e {
            EngineEvent::InputValue { value, .. } => Some(value),
            _ => None,
        });
        assert_eq!(input, Some(0.4));
    }

    #[test]
    fn test_the_newest_plan_has_the_last_word() {
        let mut monitors = Monitors::default();
        let mut old = monitors.watch(plan(1, 0));
        let mut new = monitors.watch(plan(1, 0));
        old.outputs[0].samples.fill(0.3);
        new.outputs[0].samples.fill(0.7);
        old.publish_monitors();
        new.publish_monitors();
        let values: Vec<f32> = sweep(&mut monitors).iter().map(output_value).collect();
        assert_eq!(values.last(), Some(&0.7));
    }

    #[test]
    fn test_a_board_goes_with_its_plan() {
        let mut monitors = Monitors::default();
        let old = monitors.watch(plan(1, 0));
        let _new = monitors.watch(plan(1, 0));
        drop(old);
        sweep(&mut monitors);
        assert_eq!(monitors.readers.len(), 1);
    }
}
