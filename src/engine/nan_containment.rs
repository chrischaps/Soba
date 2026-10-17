//! Tests that one NaN can't silence a patch until Stop (#319).
//!
//! A filter, an EQ or a reverb that meets a NaN keeps it in its state, so
//! everything after it would be NaN forever. The plan checks each module's
//! outputs after it runs, and silences and resets one that went bad.

use super::audio_graph::AudioGraph;
use super::create_module_registry;
use super::graph_plan::GraphPlan;
use super::NodeId;
use crate::dsp::{DspModule, ModuleCategory, ModuleInfo, ParameterDefinition, PortDefinition, PortDirection, ProcessContext, SignalBuffer, SignalType};

const RATE: f32 = 48000.0;
const BLOCK: usize = 256;

/// A 220 Hz sine at half scale, to give the module under test something to chew.
struct Sine {
    phase: f32,
}

impl DspModule for Sine {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo { id: "test.sine", name: "Sine", category: ModuleCategory::Source, description: "" };
        &INFO
    }

    fn ports(&self) -> &[PortDefinition] {
        static PORTS: &[PortDefinition] = &[PortDefinition {
            id: "out",
            name: "Out",
            signal_type: SignalType::Audio,
            direction: PortDirection::Output,
            default_value: 0.0,
            description: "",
            late: false,
        }];
        PORTS
    }

    fn parameters(&self) -> &[ParameterDefinition] {
        &[]
    }

    fn prepare(&mut self, _sample_rate: f32, _max_block_size: usize) {}

    fn process(&mut self, _inputs: &[&SignalBuffer], outputs: &mut [SignalBuffer], _params: &[f32], _context: &ProcessContext) {
        for sample in outputs[0].samples.iter_mut() {
            *sample = 0.5 * (std::f32::consts::TAU * self.phase).sin();
            self.phase = (self.phase + 220.0 / RATE).fract();
        }
    }

    fn reset(&mut self) {}
}

/// A module that goes bad and stays bad, as unstable state does: once its
/// input has played for `after` blocks it outputs NaN until it's reset.
struct Latching {
    after: usize,
    blocks: usize,
    resets: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl DspModule for Latching {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo { id: "test.latching", name: "Latching", category: ModuleCategory::Filter, description: "" };
        &INFO
    }

    fn ports(&self) -> &[PortDefinition] {
        static PORTS: &[PortDefinition] = &[
            PortDefinition { id: "in", name: "In", signal_type: SignalType::Audio, direction: PortDirection::Input, default_value: 0.0, description: "", late: false },
            PortDefinition { id: "out", name: "Out", signal_type: SignalType::Audio, direction: PortDirection::Output, default_value: 0.0, description: "", late: false },
        ];
        PORTS
    }

    fn parameters(&self) -> &[ParameterDefinition] {
        &[]
    }

    fn prepare(&mut self, _sample_rate: f32, _max_block_size: usize) {}

    fn process(&mut self, inputs: &[&SignalBuffer], outputs: &mut [SignalBuffer], _params: &[f32], _context: &ProcessContext) {
        self.blocks += 1;
        let bad = self.blocks > self.after;
        for (out, &input) in outputs[0].samples.iter_mut().zip(&inputs[0].samples) {
            *out = if bad { f32::NAN } else { input };
        }
        // Goes bad halfway through a block, and only the once
        if self.blocks == self.after.saturating_add(1) {
            outputs[0].samples[..BLOCK / 2].copy_from_slice(&inputs[0].samples[..BLOCK / 2]);
        }
    }

    fn reset(&mut self) {
        self.resets.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.blocks = usize::MAX / 2; // Past going bad: it stays good now
        self.after = usize::MAX;
    }
}

/// Every output of node `node_id` after the last block.
fn outputs_of(plan: &GraphPlan, node_id: NodeId) -> Vec<f32> {
    let node = plan.nodes.iter().find(|node| node.node_id == node_id).unwrap();
    node.outputs.clone().flat_map(|buffer| plan.outputs[buffer].samples.clone()).collect()
}

fn faults(plan: &mut GraphPlan) -> Vec<NodeId> {
    let mut faults = Vec::new();
    plan.take_faults(|node_id| faults.push(node_id));
    faults
}

/// Sine (1) into the module `module_id` (2), its parameters at their defaults.
fn sine_into(module_id: &str) -> (Box<GraphPlan>, Vec<ParameterDefinition>) {
    let registry = create_module_registry();
    let module = registry.create(module_id).unwrap();
    let params = module.parameters().to_vec();
    let mut graph = AudioGraph::new(RATE, BLOCK);
    graph.add_module_instance(1, Box::new(Sine { phase: 0.0 }));
    graph.add_module_instance(2, module);
    for (index, param) in params.iter().enumerate() {
        graph.set_parameter(2, index, param.default);
    }
    assert!(graph.connect(1, 0, 2, 0));
    (graph.compile(), params)
}

#[test]
fn a_module_that_goes_bad_is_silenced_reset_and_named() {
    let resets = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut graph = AudioGraph::new(RATE, BLOCK);
    graph.add_module_instance(1, Box::new(Sine { phase: 0.0 }));
    graph.add_module_instance(2, Box::new(Latching { after: 2, blocks: 0, resets: resets.clone() }));
    // A real filter downstream, which would latch the NaN for good
    let filter = create_module_registry().create("filter.svf").unwrap();
    let defaults: Vec<f32> = filter.parameters().iter().map(|param| param.default).collect();
    graph.add_module_instance(3, filter);
    for (index, default) in defaults.into_iter().enumerate() {
        graph.set_parameter(3, index, default);
    }
    assert!(graph.connect(1, 0, 2, 0));
    assert!(graph.connect(2, 1, 3, 0));
    let mut plan = graph.compile();
    let block = ProcessContext::new(RATE, BLOCK);

    for _ in 0..2 {
        plan.process(&block);
        assert!(faults(&mut plan).is_empty());
    }
    let resets_before = resets.load(std::sync::atomic::Ordering::Relaxed);

    // The block it goes bad in: it's silenced, reset and named, and the
    // filter after it never hears a NaN
    plan.process(&block);
    assert_eq!(faults(&mut plan), [2]);
    assert_eq!(resets.load(std::sync::atomic::Ordering::Relaxed), resets_before + 1);
    assert!(outputs_of(&plan, 2).iter().all(|&s| s == 0.0));
    assert!(outputs_of(&plan, 3).iter().all(|s| s.is_finite()));

    // And the patch plays on, without a Stop
    for _ in 0..4 {
        plan.process(&block);
    }
    assert!(faults(&mut plan).is_empty());
    let heard = outputs_of(&plan, 3);
    assert!(heard.iter().all(|s| s.is_finite()));
    assert!(heard.iter().any(|&s| s.abs() > 0.01), "sound comes back through the filter");
}

/// Filter, EQ, reverb and delay: each made to go NaN by a bad knob value
/// for one block plays again the moment the knob is good, rather than
/// staying silent until Stop.
#[test]
fn filter_eq_reverb_and_delay_recover_from_a_nan() {
    let block = ProcessContext::new(RATE, BLOCK);
    for module_id in ["filter.svf", "fx.eq", "fx.reverb", "fx.delay"] {
        let (mut plan, params) = sine_into(module_id);
        let mut faulted = false;
        for (index, param) in params.iter().enumerate() {
            for _ in 0..4 {
                plan.process(&block);
            }
            plan.set_parameter(2, index, f32::NAN);
            plan.process(&block);
            faulted |= faults(&mut plan).contains(&2);
            assert!(outputs_of(&plan, 2).iter().all(|s| s.is_finite()), "{module_id}: a NaN got out past {}", param.name);
            plan.set_parameter(2, index, param.default);
        }
        assert!(faulted, "{module_id}: no NaN knob made it go bad, so this tests nothing");

        // What the bad knob left may still trip it once more: a smoother
        // gliding from NaN as the knob comes good, or a NaN a delay wrote
        // to its line, coming round a delay time later. Each is reset in
        // turn, so give it longer than the longest delay (2 s)...
        for _ in 0..(3.0 * RATE) as usize / BLOCK {
            plan.process(&block);
        }
        let _ = faults(&mut plan);
        // ...after which it plays clean
        for _ in 0..(RATE as usize / BLOCK) {
            plan.process(&block);
        }
        assert!(faults(&mut plan).is_empty(), "{module_id} is still going bad with good knobs");
        let heard = outputs_of(&plan, 2);
        assert!(heard.iter().all(|s| s.is_finite()));
        assert!(heard.iter().any(|&s| s.abs() > 1e-3), "{module_id} is silent after recovering");
    }
}
