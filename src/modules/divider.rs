//! Clock Divider module.
//!
//! Fires once every so many clock pulses, holds a gate open for as many as
//! you like, and says how far through the division the count is. It counts
//! edges rather than timing them, so it can't drift from its clock, and
//! phrases built on it stay on the beat at any tempo.

use crate::dsp::{
    context::ProcessContext,
    gate::Edge,
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::SignalBuffer,
    ParameterDisplay, SignalType,
};

/// The largest division: eight bars of sixteenths.
pub const MAX_DIVIDE: usize = 128;

/// The module's ID, as patches name it.
pub const ID: &str = "util.divider";

/// Divides a clock by counting its edges.
///
/// # Ports
///
/// **Inputs:**
/// - **Clock** (Gate): Each rising edge counts one.
/// - **Reset** (Gate): Starts the count over: the next clock is count 0.
///
/// **Outputs:**
/// - **Trig** (Gate): The clock's own pulse, on the counts that fire.
/// - **Gate** (Gate): Opens on a count that fires and stays open for
///   Length clocks.
/// - **Count** (Control): Where the count is, as a 0 to 1 ramp.
///
/// # Parameters
///
/// - **Divide** (1-128): Fires once every this many clocks.
/// - **Offset** (0-127): Which count fires, from 0, the first clock after a
///   reset.
/// - **Length** (1-128): How many clocks Gate stays open. At Divide or more
///   it never closes, dipping for a sample to mark each new count.
pub struct ClockDivider {
    /// The current count, `None` before the first clock and after a reset.
    count: Option<usize>,
    /// Clocks left before Gate closes; 0 when it's closed.
    gate_remaining: usize,
    /// The last sample's Gate, to dip for a sample when a count fires while
    /// it's still open.
    gate_was_high: bool,
    /// Whether the clock pulse now high fired: Trig follows it while it lasts.
    trig_active: bool,
    /// The clock's period, from its last two rising edges.
    period: Option<u64>,
    /// Samples since the clock last rose, `None` until it has.
    since_clock: Option<u64>,
    clock: Edge,
    reset: Edge,
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

impl ClockDivider {
    /// Creates a Clock Divider dividing by 4.
    pub fn new() -> Self {
        Self {
            count: None,
            gate_remaining: 0,
            gate_was_high: false,
            trig_active: false,
            period: None,
            since_clock: None,
            clock: Edge::LOW,
            reset: Edge::LOW,
            ports: vec![
                PortDefinition::input_with_default("clock", "Clock", SignalType::Gate, 0.0).describe("Each rising edge counts one"),
                PortDefinition::input_with_default("reset", "Reset", SignalType::Gate, 0.0).describe("A rising edge starts the count over: the next clock is count 0"),
                PortDefinition::output("trig", "Trig", SignalType::Gate).describe("The clock's own pulse, once every Divide clocks"),
                PortDefinition::output("gate", "Gate", SignalType::Gate).describe("Opens once every Divide clocks and stays open for Length clocks"),
                PortDefinition::output("count", "Count", SignalType::Control).describe("Where the count is, as a 0 to 1 ramp"),
            ],
            parameters: vec![
                ParameterDefinition::new("divide", "Divide", 1.0, MAX_DIVIDE as f32, 4.0, ParameterDisplay::stepped(""))
                    .describe("Fires once every this many clocks"),
                ParameterDefinition::new("offset", "Offset", 0.0, (MAX_DIVIDE - 1) as f32, 0.0, ParameterDisplay::stepped(""))
                    .describe("Which count fires; 0 is the first clock after a reset"),
                ParameterDefinition::new("length", "Length", 1.0, MAX_DIVIDE as f32, 1.0, ParameterDisplay::stepped(""))
                    .describe("How many clocks Gate stays open"),
            ],
        }
    }

    const PORT_CLOCK: usize = 0;
    const PORT_RESET: usize = 1;

    const OUT_TRIG: usize = 0;
    const OUT_GATE: usize = 1;
    const OUT_COUNT: usize = 2;

    const PARAM_DIVIDE: usize = 0;
    const PARAM_OFFSET: usize = 1;
    const PARAM_LENGTH: usize = 2;

    /// Readout slots: the count (-1 before the first clock) and Gate.
    pub const READOUT_COUNT: usize = 0;
    pub const READOUT_GATE: usize = 1;

    /// An open Gate lets go when no clock has come for this many periods,
    /// so a stopped clock can't hold it open for good.
    const STALL_PERIODS: u64 = 2;

    /// Divide, Offset and Length from the knobs, as counts.
    pub fn counts(params: &[f32]) -> (usize, usize, usize) {
        let divide = (params[Self::PARAM_DIVIDE].round() as usize).clamp(1, MAX_DIVIDE);
        let offset = (params[Self::PARAM_OFFSET].round().max(0.0) as usize) % divide;
        let length = (params[Self::PARAM_LENGTH].round() as usize).clamp(1, divide);
        (divide, offset, length)
    }
}

impl Default for ClockDivider {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for ClockDivider {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "Clock Divider",
            category: ModuleCategory::Utility,
            description: "Fire once every N clocks, hold a gate for a phrase, and count where you are",
        };
        &INFO
    }

    fn ports(&self) -> &[PortDefinition] {
        &self.ports
    }

    fn parameters(&self) -> &[ParameterDefinition] {
        &self.parameters
    }

    fn prepare(&mut self, _sample_rate: f32, _max_block_size: usize) {
        self.period = None;
        self.since_clock = None;
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        let (divide, offset, length) = Self::counts(params);
        let at = |port: usize, i: usize| inputs.get(port).map_or(0.0, |buf| buf.at(i));

        for i in 0..context.block_size {
            let clock_rising = self.clock.rising(at(Self::PORT_CLOCK, i));
            let reset_rising = self.reset.rising(at(Self::PORT_RESET, i));

            // A reset ends the phrase: the gate closes, and the next clock,
            // even one on this same sample, is count 0
            if reset_rising {
                self.count = None;
                self.gate_remaining = 0;
            }

            let mut fired = false;
            if clock_rising {
                if let Some(samples) = self.since_clock {
                    self.period = Some(samples.max(1));
                }
                self.since_clock = Some(0);

                let count = self.count.map_or(0, |count| (count + 1) % divide);
                self.count = Some(count);
                self.gate_remaining = self.gate_remaining.saturating_sub(1);
                if count == offset {
                    fired = true;
                    self.gate_remaining = length;
                }
            }
            if !self.clock.is_high() {
                self.trig_active = false;
            }
            self.trig_active |= fired;

            // A stopped clock lets the gate go
            if let (Some(period), Some(since)) = (self.period, self.since_clock) {
                if since > period * Self::STALL_PERIODS {
                    self.gate_remaining = 0;
                }
            }

            // A count that fires over an open gate dips it for a sample, so
            // whatever it drives sees a new rising edge
            let gate = self.gate_remaining > 0 && !(fired && self.gate_was_high);
            self.gate_was_high = gate;

            let count = self.count.unwrap_or(0);
            outputs[Self::OUT_TRIG].samples[i] = if self.trig_active { 1.0 } else { 0.0 };
            outputs[Self::OUT_GATE].samples[i] = if gate { 1.0 } else { 0.0 };
            outputs[Self::OUT_COUNT].samples[i] = count as f32 / (divide - 1).max(1) as f32;

            if let Some(since) = self.since_clock.as_mut() {
                *since = since.saturating_add(1);
            }
        }
    }

    fn reset(&mut self) {
        self.count = None;
        self.gate_remaining = 0;
        self.gate_was_high = false;
        self.trig_active = false;
        self.period = None;
        self.since_clock = None;
        self.clock.reset();
        self.reset.reset();
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        readout.values[Self::READOUT_COUNT] = self.count.map_or(-1.0, |count| count as f32);
        readout.values[Self::READOUT_GATE] = if self.gate_was_high { 1.0 } else { 0.0 };
        Some(readout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::Clock;

    const SAMPLE_RATE: f32 = 48000.0;
    const BLOCK: usize = 256;

    /// Divide, Offset, Length.
    fn params(divide: usize, offset: usize, length: usize) -> [f32; 3] {
        [divide as f32, offset as f32, length as f32]
    }

    fn outputs() -> Vec<SignalBuffer> {
        (0..3).map(|_| SignalBuffer::control(BLOCK)).collect()
    }

    /// Runs a clock and a reset (one value per sample; a missing reset reads
    /// 0) through a fresh Clock Divider, returning Trig, Gate and Count.
    fn run(clock: &[f32], reset: &[f32], params: &[f32]) -> Vec<Vec<f32>> {
        let mut divider = ClockDivider::new();
        divider.prepare(SAMPLE_RATE, BLOCK);
        let len = clock.len();
        let ctx = ProcessContext::new(SAMPLE_RATE, BLOCK);
        let mut outs = outputs();
        let mut result = (0..3).map(|_| Vec::with_capacity(len)).collect::<Vec<_>>();
        for start in (0..len).step_by(BLOCK) {
            let buffers: Vec<SignalBuffer> = [clock, reset]
                .iter()
                .map(|input| {
                    let mut buf = SignalBuffer::control(BLOCK);
                    for (i, sample) in buf.samples.iter_mut().enumerate() {
                        *sample = input.get(start + i).copied().unwrap_or(0.0);
                    }
                    buf
                })
                .collect();
            let refs: Vec<&SignalBuffer> = buffers.iter().collect();
            divider.process(&refs, &mut outs, params, &ctx);
            for (out, buf) in result.iter_mut().zip(&outs) {
                out.extend_from_slice(&buf.samples[..BLOCK.min(len - start)]);
            }
        }
        result
    }

    /// A clock: a `width`-sample pulse every `period` samples, `count` times.
    fn pulses(period: usize, width: usize, count: usize) -> Vec<f32> {
        (0..period * count).map(|i| if i % period < width { 1.0 } else { 0.0 }).collect()
    }

    fn rises(signal: &[f32]) -> Vec<usize> {
        (0..signal.len()).filter(|&i| signal[i] > 0.5 && (i == 0 || signal[i - 1] <= 0.5)).collect()
    }

    fn falls(signal: &[f32]) -> Vec<usize> {
        (1..signal.len()).filter(|&i| signal[i] <= 0.5 && signal[i - 1] > 0.5).collect()
    }

    const TRIG: usize = ClockDivider::OUT_TRIG;
    const GATE: usize = ClockDivider::OUT_GATE;
    const COUNT: usize = ClockDivider::OUT_COUNT;

    #[test]
    fn test_divider_info() {
        let divider = ClockDivider::new();
        assert_eq!(divider.info().id, "util.divider");
        assert_eq!(divider.info().category, ModuleCategory::Utility);
        assert_eq!(divider.ports().iter().filter(|p| p.is_input()).count(), 2);
        assert_eq!(divider.ports().iter().filter(|p| p.is_output()).count(), 3);
        assert_eq!(divider.parameters().len(), 3);
    }

    #[test]
    fn test_divide_fires_on_the_first_clock_then_every_nth() {
        let clock = pulses(100, 10, 12);
        let out = run(&clock, &[], &params(4, 0, 1));
        assert_eq!(rises(&out[TRIG]), vec![0, 400, 800]);
        assert_eq!(rises(&out[GATE]), vec![0, 400, 800]);
        // Trig is the clock's own pulse; Gate lasts one clock period
        assert_eq!(falls(&out[TRIG]), vec![10, 410, 810]);
        assert_eq!(falls(&out[GATE]), vec![100, 500, 900]);
    }

    #[test]
    fn test_offset_picks_the_count_that_fires() {
        let clock = pulses(100, 10, 12);
        let out = run(&clock, &[], &params(4, 2, 1));
        assert_eq!(rises(&out[TRIG]), vec![200, 600, 1000]);
        // Offsets past Divide wrap
        let wrapped = run(&clock, &[], &params(4, 6, 1));
        assert_eq!(rises(&wrapped[TRIG]), vec![200, 600, 1000]);
    }

    #[test]
    fn test_length_holds_the_gate_for_that_many_clocks() {
        let clock = pulses(100, 10, 12);
        let out = run(&clock, &[], &params(8, 0, 3));
        assert_eq!(rises(&out[GATE]), vec![0, 800]);
        assert_eq!(falls(&out[GATE]), vec![300, 1100]);
    }

    #[test]
    fn test_full_length_gate_dips_for_a_sample_at_each_division() {
        let clock = pulses(100, 10, 9);
        let gate = &run(&clock, &[], &params(3, 0, 128))[GATE];
        assert_eq!(rises(gate), vec![0, 301, 601]);
        assert_eq!(falls(gate), vec![300, 600]);
    }

    #[test]
    fn test_reset_starts_the_count_over() {
        let clock = pulses(100, 10, 12);
        // A reset between clocks 5 and 6: clock 6 is count 0 again
        let mut reset = vec![0.0; 1200];
        reset[550..560].fill(1.0);
        let out = run(&clock, &reset, &params(4, 0, 1));
        assert_eq!(rises(&out[TRIG]), vec![0, 400, 600, 1000]);

        // A reset on the same sample as a clock makes that clock count 0
        let mut reset = vec![0.0; 1200];
        reset[500..510].fill(1.0);
        let out = run(&clock, &reset, &params(4, 0, 1));
        assert_eq!(rises(&out[TRIG]), vec![0, 400, 500, 900]);
    }

    #[test]
    fn test_reset_closes_an_open_gate() {
        let clock = pulses(100, 10, 12);
        let mut reset = vec![0.0; 1200];
        reset[250..260].fill(1.0);
        let gate = &run(&clock, &reset, &params(8, 0, 6))[GATE];
        assert_eq!(falls(gate), vec![250, 900]);
        assert_eq!(rises(gate), vec![0, 300, 1100]);
    }

    #[test]
    fn test_count_ramps_from_0_to_1() {
        let clock = pulses(100, 10, 5);
        let count = &run(&clock, &[], &params(5, 0, 1))[COUNT];
        for (n, expected) in [0.0, 0.25, 0.5, 0.75, 1.0].into_iter().enumerate() {
            assert_eq!(count[n * 100 + 50], expected, "count {n}");
        }
    }

    #[test]
    fn test_a_stopped_clock_lets_the_gate_go() {
        // Two clocks, then nothing: Gate (Length 8) closes two periods on
        let mut clock = pulses(100, 10, 2);
        clock.resize(1000, 0.0);
        let gate = &run(&clock, &[], &params(8, 0, 8))[GATE];
        assert_eq!(falls(gate), vec![301]);
    }

    #[test]
    fn test_divider_stays_exact_against_a_clock_for_ten_minutes() {
        // A 96 BPM sixteenth clock divided by 64, the Backbeat phrase: every
        // 64th clock edge, and only those, must fire, on the same sample
        let mut clock = Clock::new();
        let mut divider = ClockDivider::new();
        clock.prepare(SAMPLE_RATE, BLOCK);
        divider.prepare(SAMPLE_RATE, BLOCK);
        let clock_params = [96.0, 50.0, 4.0, 1.0, 0.0];
        let divider_params = params(64, 0, 17);
        let ctx = ProcessContext::new(SAMPLE_RATE, BLOCK);
        let unconnected = SignalBuffer::control(BLOCK);
        let mut clock_out: Vec<SignalBuffer> = (0..3).map(|_| SignalBuffer::control(BLOCK)).collect();
        let mut outs = outputs();

        let blocks = (600.0 * SAMPLE_RATE) as usize / BLOCK;
        let (mut prev_clock, mut prev_trig, mut prev_gate) = (false, false, false);
        let (mut edges, mut fires, mut gate_opens) = (0usize, 0usize, 0usize);
        for _ in 0..blocks {
            clock.process(&[], &mut clock_out, &clock_params, &ctx);
            let gate_in = &clock_out[0];
            divider.process(&[gate_in, &unconnected], &mut outs, &divider_params, &ctx);
            for i in 0..BLOCK {
                let clock_high = gate_in.samples[i] > 0.5;
                let trig = outs[TRIG].samples[i] > 0.5;
                let gate = outs[GATE].samples[i] > 0.5;
                if clock_high && !prev_clock {
                    let should_fire = edges % 64 == 0;
                    assert_eq!(trig && !prev_trig, should_fire, "clock edge {edges}");
                    assert_eq!(gate && !prev_gate, should_fire, "clock edge {edges}");
                    // The gate spans the fill bar and the downbeat after it
                    assert_eq!(gate, edges % 64 <= 16, "clock edge {edges}");
                    fires += should_fire as usize;
                    edges += 1;
                } else {
                    assert!(!trig || prev_trig, "Trig rose off a clock edge");
                    assert!(!gate || prev_gate, "Gate rose off a clock edge");
                }
                gate_opens += (gate && !prev_gate) as usize;
                (prev_clock, prev_trig, prev_gate) = (clock_high, trig, gate);
            }
        }
        // 96 BPM sixteenths: 6.4 a second
        assert_eq!(edges, 3840);
        assert_eq!(fires, 60);
        assert_eq!(gate_opens, 60);
    }

    #[test]
    fn test_readout_reports_the_count() {
        let mut divider = ClockDivider::new();
        assert_eq!(divider.readout(&[]).unwrap().values[ClockDivider::READOUT_COUNT], -1.0);
        let clock = pulses(100, 10, 3);
        let mut input = SignalBuffer::control(300);
        input.samples.copy_from_slice(&clock);
        let unconnected = SignalBuffer::control(300);
        let mut outs: Vec<SignalBuffer> = (0..3).map(|_| SignalBuffer::control(300)).collect();
        divider.process(&[&input, &unconnected], &mut outs, &params(4, 0, 1), &ProcessContext::new(SAMPLE_RATE, 300));
        assert_eq!(divider.readout(&[]).unwrap().values[ClockDivider::READOUT_COUNT], 2.0);
    }
}
