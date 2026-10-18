//! Polyphony for modules written for one voice.
//!
//! [`Poly`] runs one copy of a module per channel of a polyphonic cable.
//! Each copy keeps its own state (oscillator phase, filter memory, envelope
//! stage), so a four-note chord through a polyphonic filter is four filters,
//! each following its own note.
//!
//! The module itself stays written for one voice. Voice 1 runs on the real
//! buffers, so with mono cables nothing changes and nothing extra is done.
//! The other voices see each input's channel for them, and write into their
//! output channel, which is swapped in as their own one-channel buffer: a
//! pointer swap, not a copy, so the audio thread never allocates.

use super::context::{ProcessContext, TransportState};
use super::module_trait::{Audition, DspModule, MeterLevels, ModuleInfo, OutputLevels, Readout, SpectrumPorts, MAX_INPUTS};
use super::parameter::ParameterDefinition;
use super::port::PortDefinition;
use super::signal::{SignalBuffer, MAX_CHANNELS};
use super::ModuleError;

/// Filler for unused entries of a voice's input array.
static EMPTY: SignalBuffer = SignalBuffer::EMPTY;

/// A module run once per channel: one voice per channel of its widest input.
///
/// Inputs carrying one channel are shared by every voice, so a mono LFO into
/// a polyphonic filter's cutoff moves every voice's cutoff together. Knobs
/// are shared too. Voices that join (when a cable gains channels) start from
/// a reset, not from wherever they were left.
pub struct Poly<M> {
    /// One module per channel, [`MAX_CHANNELS`] of them.
    voices: Vec<M>,
    /// How many voices ran in the last block.
    active: usize,
    /// Stand-ins swapped with a voice's output channels while it runs, one
    /// per output port.
    scratch: Vec<SignalBuffer>,
}

impl<M: DspModule> Poly<M> {
    /// Wraps a module, building one copy per channel with `make`.
    pub fn new(make: impl Fn() -> M) -> Self {
        let voices: Vec<M> = (0..MAX_CHANNELS).map(|_| make()).collect();
        let outputs = voices[0].ports().iter().filter(|port| port.is_output()).count();
        Self {
            voices,
            active: 1,
            scratch: vec![SignalBuffer::EMPTY; outputs],
        }
    }

    /// The module running voice `channel` (from 0).
    pub fn voice(&self, channel: usize) -> &M {
        &self.voices[channel]
    }
}

impl<M: DspModule + Default> Default for Poly<M> {
    fn default() -> Self {
        Self::new(M::default)
    }
}

impl<M: DspModule> DspModule for Poly<M> {
    fn info(&self) -> &ModuleInfo {
        self.voices[0].info()
    }

    fn ports(&self) -> &[PortDefinition] {
        self.voices[0].ports()
    }

    fn parameters(&self) -> &[ParameterDefinition] {
        self.voices[0].parameters()
    }

    fn prepare(&mut self, sample_rate: f32, max_block_size: usize) {
        for voice in &mut self.voices {
            voice.prepare(sample_rate, max_block_size);
        }
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        // One voice per channel of the widest input, as far as the outputs
        // have room for
        let wanted = inputs.iter().map(|input| input.channels()).max().unwrap_or(1);
        let room = outputs.iter().map(|output| output.max_channels()).min().unwrap_or(MAX_CHANNELS);
        let channels = wanted.min(room).min(self.voices.len());
        for output in outputs.iter_mut() {
            output.set_channels(channels);
        }

        // Voices joining start fresh, not from wherever they were left
        if channels > self.active {
            for voice in &mut self.voices[self.active..channels] {
                voice.reset();
            }
        }
        self.active = channels;

        let Self { voices, scratch, .. } = self;
        let Some((first, rest)) = voices.split_first_mut() else {
            return;
        };
        first.process(inputs, outputs, params, context);

        let scratch = &mut scratch[..outputs.len()];
        for (index, voice) in rest[..channels - 1].iter_mut().enumerate() {
            let channel = index + 1;
            let mut voice_inputs = [&EMPTY; MAX_INPUTS];
            for (slot, input) in voice_inputs.iter_mut().zip(inputs) {
                *slot = input.voice(channel);
            }

            // Lend this voice its output channels as its own buffers
            for (output, own) in outputs.iter_mut().zip(scratch.iter_mut()) {
                std::mem::swap(own, output.poly_buffer_mut(channel));
            }
            voice.process(&voice_inputs[..inputs.len().min(MAX_INPUTS)], scratch, params, context);
            for (output, own) in outputs.iter_mut().zip(scratch.iter_mut()) {
                std::mem::swap(own, output.poly_buffer_mut(channel));
            }
        }
    }

    fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.reset();
        }
    }

    /// Each voice gets its own seed, so no two voices of one module, nor
    /// voices of two modules, play the same noise.
    fn seed(&mut self, seed: u64) {
        for (channel, voice) in (0..).zip(&mut self.voices) {
            voice.seed(seed.wrapping_mul(MAX_CHANNELS as u64).wrapping_add(channel));
        }
    }

    fn serialize_state(&self) -> Option<Vec<u8>> {
        self.voices[0].serialize_state()
    }

    fn deserialize_state(&mut self, data: &[u8]) -> Result<(), ModuleError> {
        for voice in &mut self.voices {
            voice.deserialize_state(data)?;
        }
        Ok(())
    }

    fn get_audio_output(&self) -> Option<(&[f32], &[f32])> {
        self.voices[0].get_audio_output()
    }

    fn take_output_levels(&mut self) -> Option<OutputLevels> {
        self.voices[0].take_output_levels()
    }

    fn take_meter_levels(&mut self) -> Option<MeterLevels> {
        self.voices[0].take_meter_levels()
    }

    fn take_scope_data(&mut self) -> Option<(&[f32], &[f32], bool)> {
        self.voices[0].take_scope_data()
    }

    fn transport(&self, params: &[f32]) -> Option<TransportState> {
        self.voices[0].transport(params)
    }

    fn readout(&self, params: &[f32]) -> Option<Readout> {
        self.voices[0].readout(params)
    }

    fn spectrum_ports(&self) -> Option<SpectrumPorts> {
        self.voices[0].spectrum_ports()
    }

    fn polyphonic(&self) -> bool {
        true
    }

    fn key_inputs(&self) -> &'static [usize] {
        self.voices[0].key_inputs()
    }

    fn is_live_input(&self) -> bool {
        self.voices[0].is_live_input()
    }

    /// Voice 1 plays the chord: its outputs are the module's own.
    fn audition(&mut self, chord: Option<Audition>) {
        self.voices[0].audition(chord);
    }

    // The recording hand-off (load_sample, give_room, set_markers and the
    // rest) keeps the defaults: one recording shared out among voices that
    // each retire it on their own time is a design of its own, and no
    // polyphonic module plays one yet.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::{ModuleCategory, PortDefinition, SignalType};

    /// Adds its input to a running total each sample and outputs the total,
    /// so each voice's output shows both its own input and its own state.
    #[derive(Default)]
    struct Accumulator {
        total: f32,
    }

    impl DspModule for Accumulator {
        fn info(&self) -> &ModuleInfo {
            static INFO: ModuleInfo = ModuleInfo {
                id: "test.accumulator",
                name: "Accumulator",
                category: ModuleCategory::Utility,
                description: "Running total of its input",
            };
            &INFO
        }

        fn ports(&self) -> &[PortDefinition] {
            static PORTS: &[PortDefinition] = &[
                PortDefinition {
                    id: "in",
                    name: "In",
                    signal_type: SignalType::Control,
                    direction: crate::dsp::PortDirection::Input,
                    default_value: 0.0,
                    description: "",
                    late: false,
                },
                PortDefinition {
                    id: "offset",
                    name: "Offset",
                    signal_type: SignalType::Control,
                    direction: crate::dsp::PortDirection::Input,
                    default_value: 0.0,
                    description: "",
                    late: false,
                },
                PortDefinition {
                    id: "out",
                    name: "Out",
                    signal_type: SignalType::Control,
                    direction: crate::dsp::PortDirection::Output,
                    default_value: 0.0,
                    description: "",
                    late: false,
                },
            ];
            PORTS
        }

        fn parameters(&self) -> &[ParameterDefinition] {
            &[]
        }

        fn prepare(&mut self, _: f32, _: usize) {}

        fn process(&mut self, inputs: &[&SignalBuffer], outputs: &mut [SignalBuffer], _: &[f32], _: &ProcessContext) {
            for i in 0..outputs[0].len() {
                self.total += inputs[0].samples[i];
                outputs[0].samples[i] = self.total + inputs[1].samples[i];
            }
        }

        fn reset(&mut self) {
            self.total = 0.0;
        }
    }

    const BLOCK: usize = 4;

    /// A polyphonic input carrying `values`, one constant per channel.
    fn poly_input(values: &[f32]) -> SignalBuffer {
        let mut buffer = SignalBuffer::polyphonic(BLOCK, SignalType::Control);
        buffer.set_channels(values.len());
        for (channel, &value) in values.iter().enumerate() {
            buffer.channel_mut(channel).fill(value);
        }
        buffer
    }

    fn run(module: &mut Poly<Accumulator>, inputs: &[&SignalBuffer]) -> SignalBuffer {
        let mut outputs = [SignalBuffer::polyphonic(BLOCK, SignalType::Control)];
        module.process(inputs, &mut outputs, &[], &ProcessContext::new(48000.0, BLOCK));
        let [out] = outputs;
        out
    }

    #[test]
    fn test_one_voice_per_channel_each_with_its_own_state() {
        let mut module = Poly::<Accumulator>::default();
        let input = poly_input(&[1.0, 2.0, 3.0]);
        let offset = SignalBuffer::control(BLOCK);

        let out = run(&mut module, &[&input, &offset]);
        assert_eq!(out.channels(), 3);
        assert_eq!(out.voice(0).samples, [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(out.voice(1).samples, [2.0, 4.0, 6.0, 8.0]);
        assert_eq!(out.voice(2).samples, [3.0, 6.0, 9.0, 12.0]);

        // The totals carry on per voice into the next block
        let out = run(&mut module, &[&input, &offset]);
        assert_eq!(out.voice(2).samples[0], 15.0);
    }

    #[test]
    fn test_mono_input_is_shared_by_every_voice() {
        let mut module = Poly::<Accumulator>::default();
        let input = poly_input(&[1.0, 1.0]);
        let mut offset = SignalBuffer::control(BLOCK);
        offset.fill(10.0);

        let out = run(&mut module, &[&input, &offset]);
        assert_eq!(out.voice(0).samples[0], 11.0);
        assert_eq!(out.voice(1).samples[0], 11.0);
    }

    #[test]
    fn test_narrower_poly_input_is_silent_past_its_channels() {
        let mut module = Poly::<Accumulator>::default();
        let input = poly_input(&[1.0, 1.0, 1.0, 1.0]);
        let offset = poly_input(&[5.0, 6.0]);

        let out = run(&mut module, &[&input, &offset]);
        assert_eq!(out.channels(), 4);
        let firsts: Vec<f32> = (0..4).map(|c| out.voice(c).samples[0]).collect();
        assert_eq!(firsts, [6.0, 7.0, 1.0, 1.0]);
    }

    #[test]
    fn test_mono_cables_run_one_voice() {
        let mut module = Poly::<Accumulator>::default();
        let mut input = SignalBuffer::control(BLOCK);
        input.fill(1.0);
        let offset = SignalBuffer::control(BLOCK);

        let out = run(&mut module, &[&input, &offset]);
        assert_eq!(out.channels(), 1);
        assert_eq!(out.samples, [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(module.voice(1).total, 0.0, "the other voices rest");
    }

    #[test]
    fn test_outputs_without_room_run_one_voice() {
        let mut module = Poly::<Accumulator>::default();
        let input = poly_input(&[1.0, 2.0]);
        let offset = SignalBuffer::control(BLOCK);
        let mut outputs = [SignalBuffer::control(BLOCK)];
        module.process(&[&input, &offset], &mut outputs, &[], &ProcessContext::new(48000.0, BLOCK));
        assert_eq!(outputs[0].channels(), 1);
        assert_eq!(outputs[0].samples[3], 4.0);
    }

    #[test]
    fn test_returning_voices_start_fresh() {
        let mut module = Poly::<Accumulator>::default();
        let offset = SignalBuffer::control(BLOCK);
        run(&mut module, &[&poly_input(&[1.0, 1.0]), &offset]);
        run(&mut module, &[&poly_input(&[1.0]), &offset]);

        // Voice 2 comes back: its total starts again from zero
        let out = run(&mut module, &[&poly_input(&[1.0, 1.0]), &offset]);
        assert_eq!(out.voice(1).samples, [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(out.voice(0).samples[0], 9.0, "voice 1 never stopped");
    }

    /// An Accumulator that describes itself beyond the defaults: a key
    /// input, live input, and a chord it remembers being played.
    #[derive(Default)]
    struct Described {
        inner: Accumulator,
        chord: Option<Audition>,
    }

    impl DspModule for Described {
        fn info(&self) -> &ModuleInfo {
            self.inner.info()
        }

        fn ports(&self) -> &[PortDefinition] {
            self.inner.ports()
        }

        fn parameters(&self) -> &[ParameterDefinition] {
            &[]
        }

        fn prepare(&mut self, _: f32, _: usize) {}

        fn process(&mut self, inputs: &[&SignalBuffer], outputs: &mut [SignalBuffer], params: &[f32], context: &ProcessContext) {
            self.inner.process(inputs, outputs, params, context);
        }

        fn reset(&mut self) {}

        fn key_inputs(&self) -> &'static [usize] {
            &[1]
        }

        fn is_live_input(&self) -> bool {
            true
        }

        fn audition(&mut self, chord: Option<Audition>) {
            self.chord = chord;
        }
    }

    #[test]
    fn test_wrapper_forwards_what_the_module_says_of_itself() {
        let mut module = Poly::<Described>::default();
        let bare = Described::default();
        assert_eq!(module.key_inputs(), bare.key_inputs());
        assert_eq!(module.is_live_input(), bare.is_live_input());

        let chord = Audition { len: 1, ..Audition::default() };
        module.audition(Some(chord));
        assert_eq!(module.voice(0).chord, Some(chord));
        assert_eq!(module.voice(1).chord, None, "voice 1 alone plays it");
    }

    #[test]
    fn test_wrapper_presents_the_module() {
        let module = Poly::<Accumulator>::default();
        assert_eq!(module.info().id, "test.accumulator");
        assert_eq!(module.ports().len(), 3);
        assert!(module.polyphonic());
        assert!(!Accumulator::default().polyphonic());
    }
}
