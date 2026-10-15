//! Link Out module: plays Seoul, a music visualizer, from the patch.
//!
//! Whatever is patched into its eight CV lanes and four gates goes to Seoul
//! over OSC, with the patch's Clock, and Seoul's presets read them as
//! `cv1` … `cv8` and `gate1` … `gate4`. Cables decide what the picture
//! follows: an LFO that sweeps a filter can sweep Seoul's zoom too, and the
//! kick's trigger can flash it.
//!
//! The module only stores each lane's newest value on the [`LinkBus`] once a
//! block. The sending is done by a thread of its own (`link::sender`), on
//! the desktop only: in the browser a Link Out runs but sends nothing.

use crate::dsp::{
    context::ProcessContext,
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::{connected_input, SignalBuffer},
    SignalType,
};
use crate::link::{ClockState, LinkBus, BUS, CV_LANES, GATE_LANES, GATE_THRESHOLD, PORT_LABELS};

/// Sends its lanes, gates and the patch's Clock to Seoul.
///
/// # Ports
///
/// **Inputs:**
/// - **CV 1** … **CV 8** (Control): sent as `/soba/cv/1` … `/soba/cv/8`,
///   as they are (0 to 1, or -1 to 1). An empty lane sends 0.
/// - **Gate 1** … **Gate 4** (Gate): sent as `/soba/gate/1` … `/soba/gate/4`.
///   Every rising edge reaches Seoul, however short the trigger.
///
/// # Parameters
///
/// - **Port**: the port on this machine Seoul listens on (9137 unless its
///   `seoul.toml` says otherwise).
pub struct LinkOut {
    bus: &'static LinkBus,
    /// Each gate's level at the end of the last block.
    gate_high: [bool; GATE_LANES],
    /// Rising edges so far, per gate, for the face to flash on.
    gate_edges: [u32; GATE_LANES],
    cv: [f32; CV_LANES],
    /// Which lanes have cables, as bits: CV in the low eight, gates above.
    patched: u32,
    clock: Option<ClockState>,
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

impl LinkOut {
    /// A Link Out sending through the app's bus.
    pub fn new() -> Self {
        Self::on_bus(&BUS)
    }

    /// A Link Out storing into `bus`: a test's own, say.
    pub fn on_bus(bus: &'static LinkBus) -> Self {
        const CV_IDS: [&str; CV_LANES] = ["cv1", "cv2", "cv3", "cv4", "cv5", "cv6", "cv7", "cv8"];
        const CV_NAMES: [&str; CV_LANES] = ["CV 1", "CV 2", "CV 3", "CV 4", "CV 5", "CV 6", "CV 7", "CV 8"];
        const GATE_IDS: [&str; GATE_LANES] = ["gate1", "gate2", "gate3", "gate4"];
        const GATE_NAMES: [&str; GATE_LANES] = ["Gate 1", "Gate 2", "Gate 3", "Gate 4"];
        let mut ports: Vec<PortDefinition> = CV_IDS
            .iter()
            .zip(CV_NAMES)
            .map(|(&id, name)| {
                PortDefinition::input_with_default(id, name, SignalType::Control, 0.0)
                    .describe("Sent to Seoul as it is, for a preset to read as cv1 to cv8. An empty lane sends 0")
            })
            .collect();
        ports.extend(GATE_IDS.iter().zip(GATE_NAMES).map(|(&id, name)| {
            PortDefinition::input_with_default(id, name, SignalType::Gate, 0.0)
                .describe("Sent to Seoul as gate1 to gate4. Every rising edge gets there, however short the trigger")
        }));
        Self {
            bus,
            gate_high: [false; GATE_LANES],
            gate_edges: [0; GATE_LANES],
            cv: [0.0; CV_LANES],
            patched: 0,
            clock: None,
            ports,
            parameters: vec![ParameterDefinition::choice("port", "Port", PORT_LABELS, 0)
                .describe("The port Seoul listens on: 9137 unless its seoul.toml says otherwise")],
        }
    }

    pub const PARAM_PORT: usize = 0;

    /// The first gate's input port.
    const PORT_GATE: usize = CV_LANES;

    // Readout slots

    /// CV 1 to CV 8 as sent, from this slot on.
    pub const READOUT_CV: usize = 0;
    /// Gate 1 to Gate 4, 1 while high, from this slot on.
    pub const READOUT_GATE: usize = 8;
    /// Rising edges so far per gate, wrapping at [`WRAP`](Self::WRAP), from
    /// this slot on.
    pub const READOUT_EDGES: usize = 12;
    /// Which lanes have cables, as bits: CV in the low eight, gates above.
    pub const READOUT_PATCHED: usize = 16;
    /// 1 when the patch has a Clock, 0 otherwise.
    pub const READOUT_HAS_CLOCK: usize = 17;
    /// 1 while the Clock plays.
    pub const READOUT_PLAYING: usize = 18;
    pub const READOUT_BPM: usize = 19;
    /// Where in the bar the Clock is, in beats from 0.
    pub const READOUT_BEAT_IN_BAR: usize = 20;
    /// The Clock's bar, counting from 0.
    pub const READOUT_BAR: usize = 21;
    pub const READOUT_BEATS_PER_BAR: usize = 22;

    /// Edge counts in the readout wrap here, well inside an f32's whole numbers.
    pub const WRAP: u32 = 4096;
}

impl Default for LinkOut {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for LinkOut {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: "output.link",
            name: "Link Out",
            category: ModuleCategory::Output,
            description: "Play Seoul's visuals from the patch: eight CV lanes, four gates and the Clock, sent over OSC",
        };
        &INFO
    }

    fn ports(&self) -> &[PortDefinition] {
        &self.ports
    }

    fn parameters(&self) -> &[ParameterDefinition] {
        &self.parameters
    }

    fn prepare(&mut self, _sample_rate: f32, _max_block_size: usize) {}

    fn process(&mut self, inputs: &[&SignalBuffer], _outputs: &mut [SignalBuffer], params: &[f32], context: &ProcessContext) {
        let port = params.get(Self::PARAM_PORT).map_or(0, |&p| p.round().max(0.0) as usize);
        let slot = self.bus.slot(port);
        let frames = context.block_size;
        self.patched = 0;

        for lane in 0..CV_LANES {
            let Some(input) = connected_input(inputs, lane) else {
                self.cv[lane] = 0.0;
                continue;
            };
            let samples = &input.samples[..frames.min(input.samples.len())];
            let value = samples.last().copied().unwrap_or(0.0);
            self.cv[lane] = value;
            self.patched |= 1 << lane;
            slot.set_cv(lane, value);
        }

        for lane in 0..GATE_LANES {
            let Some(input) = connected_input(inputs, Self::PORT_GATE + lane) else {
                self.gate_high[lane] = false;
                continue;
            };
            let mut high = self.gate_high[lane];
            let mut rises = 0;
            for &sample in &input.samples[..frames.min(input.samples.len())] {
                let now = sample > GATE_THRESHOLD;
                if now && !high {
                    rises += 1;
                }
                high = now;
            }
            self.gate_high[lane] = high;
            self.gate_edges[lane] = (self.gate_edges[lane] + rises) % Self::WRAP;
            self.patched |= 1 << (CV_LANES + lane);
            slot.set_gate(lane, high, rises);
        }

        // The Clock where this block ends, which is the newest
        let transport = &context.transport;
        self.clock = transport.beat_at(frames, context.sample_rate).map(|beat| ClockState {
            playing: transport.playing,
            bpm: transport.tempo_bpm.unwrap_or(120.0),
            beat,
            beats_per_bar: transport.time_sig_numerator,
        });
        slot.set_clock(self.clock);
        slot.pulse();
    }

    fn reset(&mut self) {
        self.gate_high = [false; GATE_LANES];
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        let values = &mut readout.values;
        values[Self::READOUT_CV..Self::READOUT_CV + CV_LANES].copy_from_slice(&self.cv);
        for lane in 0..GATE_LANES {
            values[Self::READOUT_GATE + lane] = if self.gate_high[lane] { 1.0 } else { 0.0 };
            values[Self::READOUT_EDGES + lane] = self.gate_edges[lane] as f32;
        }
        values[Self::READOUT_PATCHED] = self.patched as f32;
        if let Some(clock) = self.clock {
            let beats_per_bar = clock.beats_per_bar.max(1) as f64;
            values[Self::READOUT_HAS_CLOCK] = 1.0;
            values[Self::READOUT_PLAYING] = if clock.playing { 1.0 } else { 0.0 };
            values[Self::READOUT_BPM] = clock.bpm;
            values[Self::READOUT_BEAT_IN_BAR] = clock.beat.rem_euclid(beats_per_bar) as f32;
            values[Self::READOUT_BAR] = (clock.beat / beats_per_bar).floor() as f32;
            values[Self::READOUT_BEATS_PER_BAR] = beats_per_bar as f32;
        }
        Some(readout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::context::TransportState;

    const BLOCK: usize = 64;

    fn bus() -> &'static LinkBus {
        Box::leak(Box::new(LinkBus::new()))
    }

    fn buffers(module: &LinkOut) -> Vec<SignalBuffer> {
        module.ports().iter().map(|port| SignalBuffer::unconnected(BLOCK, port.signal_type)).collect()
    }

    fn connect(buffer: &mut SignalBuffer, samples: impl Fn(usize) -> f32) {
        *buffer = SignalBuffer::new(BLOCK, buffer.signal_type);
        for (i, sample) in buffer.samples.iter_mut().enumerate() {
            *sample = samples(i);
        }
    }

    fn run(module: &mut LinkOut, inputs: &[SignalBuffer], params: &[f32], transport: TransportState) {
        let refs: Vec<&SignalBuffer> = inputs.iter().collect();
        let mut context = ProcessContext::new(48000.0, BLOCK);
        context.transport = transport;
        module.process(&refs, &mut [], params, &context);
    }

    #[test]
    fn ports_and_info() {
        let module = LinkOut::on_bus(bus());
        assert_eq!(module.info().id, "output.link");
        assert_eq!(module.ports().len(), CV_LANES + GATE_LANES);
        assert!(module.ports().iter().all(|p| p.is_input()));
        assert_eq!(module.ports()[0].id, "cv1");
        assert_eq!(module.ports()[CV_LANES].signal_type, SignalType::Gate);
        assert_eq!(module.parameters()[LinkOut::PARAM_PORT].id, "port");
    }

    #[test]
    fn patched_lanes_reach_the_bus_and_empty_ones_dont() {
        let bus = bus();
        let mut module = LinkOut::on_bus(bus);
        let mut inputs = buffers(&module);
        connect(&mut inputs[1], |i| i as f32 / BLOCK as f32);
        run(&mut module, &inputs, &[0.0], TransportState::new());
        let reading = bus.slot(0).read();
        assert_eq!(reading.pulse, 1);
        assert_eq!(reading.cv[1], (BLOCK - 1) as f32 / BLOCK as f32, "the newest sample");
        assert_eq!(reading.cv_written[1], 1);
        assert_eq!(reading.cv_written[0], 0, "an empty lane isn't written");
        assert_eq!(reading.gate_written, [0; GATE_LANES]);
        assert!(reading.clock.is_none(), "no Clock in the patch");
        let readout = module.readout(&[0.0]).unwrap();
        assert_eq!(readout.values[LinkOut::READOUT_PATCHED], 2.0);
    }

    #[test]
    fn the_port_picks_the_slot() {
        let bus = bus();
        let mut module = LinkOut::on_bus(bus);
        let inputs = buffers(&module);
        run(&mut module, &inputs, &[2.0], TransportState::new());
        assert_eq!(bus.slot(0).read().pulse, 0);
        assert_eq!(bus.slot(2).read().pulse, 1);
    }

    #[test]
    fn every_rising_edge_is_counted() {
        let bus = bus();
        let mut module = LinkOut::on_bus(bus);
        let mut inputs = buffers(&module);
        // Two one-sample triggers, then high to the end
        connect(&mut inputs[CV_LANES + 2], |i| if i == 3 || i == 10 || i >= 40 { 1.0 } else { 0.0 });
        run(&mut module, &inputs, &[0.0], TransportState::new());
        let reading = bus.slot(0).read();
        assert_eq!(reading.gate_edges[2], 3);
        assert!(reading.gate[2]);
        // Still high across the block boundary: no new edge
        connect(&mut inputs[CV_LANES + 2], |_| 1.0);
        run(&mut module, &inputs, &[0.0], TransportState::new());
        assert_eq!(bus.slot(0).read().gate_edges[2], 3);
        assert_eq!(module.readout(&[0.0]).unwrap().values[LinkOut::READOUT_EDGES + 2], 3.0);
    }

    #[test]
    fn the_clock_is_where_the_block_ends() {
        let bus = bus();
        let mut module = LinkOut::on_bus(bus);
        let inputs = buffers(&module);
        let transport = TransportState { beat_position: Some(6.0), ..TransportState::playing_at(120.0) };
        run(&mut module, &inputs, &[0.0], transport);
        let clock = bus.slot(0).read().clock.unwrap();
        assert!(clock.playing);
        assert_eq!(clock.bpm, 120.0);
        // 64 samples at 48 kHz and two beats a second
        assert!((clock.beat - (6.0 + 64.0 / 24000.0)).abs() < 1e-9);
        let readout = module.readout(&[0.0]).unwrap();
        assert_eq!(readout.values[LinkOut::READOUT_BAR], 1.0);
        assert!((readout.values[LinkOut::READOUT_BEAT_IN_BAR] - 2.0).abs() < 0.01);
    }

    #[test]
    fn link_out_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<LinkOut>();
    }
}
