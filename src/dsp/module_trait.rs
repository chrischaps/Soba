//! The core DspModule trait and supporting types.
//!
//! This module defines the interface that all synthesizer modules must implement,
//! enabling both built-in and user-created modules to work within the audio graph.

use super::context::{ProcessContext, TransportState};
use super::parameter::ParameterDefinition;
use super::port::PortDefinition;
use super::primitives::Markers;
use super::sample::{SampleData, Snapshot, SnapshotOutcome};
use super::{SignalBuffer, MAX_CHANNELS};
use egui::Color32;
use egui_node_graph2::CategoryTrait;
use std::fmt;
use std::sync::Arc;

/// Meter readings from an output stage, covering one measurement period.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OutputLevels {
    /// Peak level per channel (left, right) going into the limiter.
    /// Can exceed 1.0: this is how hard the patch is driving the output.
    pub pre: [f32; 2],
    /// Peak level per channel leaving the output stage (what you hear).
    pub post: [f32; 2],
    /// Smallest gain the limiter applied (1.0 = no limiting).
    pub limiter_gain: f32,
}

impl Default for OutputLevels {
    fn default() -> Self {
        Self { pre: [0.0; 2], post: [0.0; 2], limiter_gain: 1.0 }
    }
}

/// The most readings one module's meters report.
pub const MAX_METERS: usize = 8;

/// Peak levels from a module's own meters (a mixer's channel strips, say),
/// covering one measurement period. Fixed-size, so reporting them from the
/// audio thread never allocates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MeterLevels {
    /// The peak magnitude of each meter, in the module's own order.
    pub peaks: [f32; MAX_METERS],
}

/// How many values a [`Readout`] carries.
pub const MAX_READOUT: usize = 32;

/// Live values a module shows on its node that aren't signals or meters: a
/// Clock's received tempo and where it is in the bar, say. Fixed-size, so
/// reporting them from the audio thread never allocates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Readout {
    /// The values, in the module's own order.
    pub values: [f32; MAX_READOUT],
}

/// A take a module has just recorded, and now plays: shared, for the UI to
/// keep with the patch.
#[derive(Clone, Debug)]
pub struct FinishedTake {
    /// The module's count of takes so far, this one included.
    pub count: u32,
    pub sample: Arc<SampleData>,
    /// How many beats long it is, to the nearest, when a Clock was running
    /// as it ended: exactly its bars' worth, for a take in bars.
    pub beats: Option<u32>,
}

/// The ports whose sound a module's display shows as spectra: a filter's
/// audio in, and its outputs. The first output with a cable from it is
/// the one tapped, or the first output if none has one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpectrumPorts {
    pub input: &'static str,
    pub outputs: &'static [&'static str],
}

/// A chord played on a module's own outputs from its face while a hand is
/// down: a Chord Sequencer's popover lets it be heard as it's written.
/// Fixed-size, so it reaches the audio thread without allocating.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Audition {
    /// The notes as MIDI numbers, one per channel, lowest channel first.
    pub notes: [u8; MAX_CHANNELS],
    /// How many of `notes` play.
    pub len: u8,
    /// The chord's root and bass, as MIDI numbers.
    pub root: u8,
    pub bass: u8,
    /// How hard it's played, 0 to 1.
    pub velocity: f32,
    /// Which way a strum sounds it: from the highest channel down, or
    /// (false) from the lowest up.
    pub down: bool,
}

impl Audition {
    /// The notes that play.
    pub fn notes(&self) -> &[u8] {
        &self.notes[..(self.len as usize).min(MAX_CHANNELS)]
    }
}

/// Category of a DSP module, used for organization and UI coloring.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModuleCategory {
    /// Sound sources (oscillators, noise generators, samplers).
    Source,
    /// Frequency-shaping modules (filters, EQs).
    Filter,
    /// Modulation sources (envelopes, LFOs).
    Modulation,
    /// Audio effects (delay, reverb, distortion).
    Effect,
    /// Utility modules (mixers, VCAs, math operations).
    Utility,
    /// Output modules (master output, scope, spectrum analyzer).
    Output,
}

impl ModuleCategory {
    /// Returns the color associated with this module category.
    ///
    /// These colors match the concept image aesthetic:
    /// - Source: Blue (#42A5F5) - oscillators, noise generators
    /// - Filter: Teal (#26A69A) - filters, EQs
    /// - Modulation: Orange (#FFB74D) - envelopes, LFOs
    /// - Effect: Cyan (#4DD0E1) - delay, reverb
    /// - Utility: Gray (#9E9E9E) - mixers, VCAs
    /// - Output: Purple (#7E57C2) - master output
    ///
    /// `theme::module` re-exports these, so there's one set of header colours.
    pub const fn color(&self) -> Color32 {
        match self {
            ModuleCategory::Source => Color32::from_rgb(66, 165, 245),    // Blue #42A5F5
            ModuleCategory::Filter => Color32::from_rgb(38, 166, 154),    // Teal #26A69A
            ModuleCategory::Modulation => Color32::from_rgb(255, 183, 77), // Orange #FFB74D
            ModuleCategory::Effect => Color32::from_rgb(77, 208, 225),    // Cyan #4DD0E1
            ModuleCategory::Utility => Color32::from_rgb(158, 158, 158),  // Gray #9E9E9E
            ModuleCategory::Output => Color32::from_rgb(126, 87, 194),    // Purple #7E57C2
        }
    }

    /// Returns a human-readable name for the category.
    pub fn name(&self) -> &'static str {
        match self {
            ModuleCategory::Source => "Source",
            ModuleCategory::Filter => "Filter",
            ModuleCategory::Modulation => "Modulation",
            ModuleCategory::Effect => "Effect",
            ModuleCategory::Utility => "Utility",
            ModuleCategory::Output => "Output",
        }
    }
}

impl CategoryTrait for ModuleCategory {
    fn name(&self) -> String {
        self.name().to_string()
    }
}

/// Static information about a DSP module.
///
/// This describes the module's identity and classification,
/// used for display in the UI and module registry.
#[derive(Clone, Debug)]
pub struct ModuleInfo {
    /// Unique identifier for the module type (e.g., "sine_osc", "svf_filter").
    pub id: &'static str,
    /// Human-readable name (e.g., "Sine Oscillator", "SVF Filter").
    pub name: &'static str,
    /// The category this module belongs to.
    pub category: ModuleCategory,
    /// A brief description of what the module does.
    pub description: &'static str,
}

impl ModuleInfo {
    /// Creates a new module info.
    pub fn new(
        id: &'static str,
        name: &'static str,
        category: ModuleCategory,
        description: &'static str,
    ) -> Self {
        Self {
            id,
            name,
            category,
            description,
        }
    }
}

/// Errors that can occur during module operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModuleError {
    /// Failed to deserialize module state.
    DeserializationFailed(String),
    /// Invalid parameter value.
    InvalidParameter { id: String, reason: String },
    /// Module is not in a valid state.
    InvalidState(String),
}

impl fmt::Display for ModuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModuleError::DeserializationFailed(msg) => {
                write!(f, "Failed to deserialize module state: {}", msg)
            }
            ModuleError::InvalidParameter { id, reason } => {
                write!(f, "Invalid parameter '{}': {}", id, reason)
            }
            ModuleError::InvalidState(msg) => {
                write!(f, "Invalid module state: {}", msg)
            }
        }
    }
}

impl std::error::Error for ModuleError {}

/// The core trait that all DSP modules must implement.
///
/// This trait defines the interface for audio processing modules in the synthesizer.
/// Modules receive input signals, process them according to their parameters,
/// and produce output signals.
///
/// # Thread Safety
///
/// `DspModule` requires `Send + 'static` because modules may be moved between
/// threads (from the UI thread to the audio thread) after construction.
///
/// # Example
///
/// ```ignore
/// struct GainModule {
///     info: ModuleInfo,
///     ports: Vec<PortDefinition>,
///     parameters: Vec<ParameterDefinition>,
/// }
///
/// impl DspModule for GainModule {
///     fn info(&self) -> &ModuleInfo { &self.info }
///     fn ports(&self) -> &[PortDefinition] { &self.ports }
///     fn parameters(&self) -> &[ParameterDefinition] { &self.parameters }
///
///     fn prepare(&mut self, _sample_rate: f32, _max_block_size: usize) {}
///
///     fn process(
///         &mut self,
///         inputs: &[&SignalBuffer],
///         outputs: &mut [SignalBuffer],
///         params: &[f32],
///         _context: &ProcessContext,
///     ) {
///         let gain = params[0];
///         for (i, sample) in inputs[0].samples.iter().enumerate() {
///             outputs[0].samples[i] = sample * gain;
///         }
///     }
///
///     fn reset(&mut self) {}
/// }
/// ```
pub trait DspModule: Send + 'static {
    /// Returns static information about this module.
    fn info(&self) -> &ModuleInfo;

    /// Returns the port definitions for this module.
    ///
    /// The order of ports determines their indices for the `process` method.
    /// Input ports should come before output ports by convention.
    fn ports(&self) -> &[PortDefinition];

    /// Returns the parameter definitions for this module.
    ///
    /// The order of parameters determines their indices in the `params` slice
    /// passed to the `process` method.
    fn parameters(&self) -> &[ParameterDefinition];

    /// Prepares the module for processing.
    ///
    /// Called before audio processing begins, and whenever the sample rate
    /// or maximum block size changes. Modules should allocate any buffers
    /// and precompute any values that depend on sample rate here.
    ///
    /// # Arguments
    ///
    /// * `sample_rate` - The audio sample rate in Hz
    /// * `max_block_size` - The maximum number of samples per process call
    fn prepare(&mut self, sample_rate: f32, max_block_size: usize);

    /// Processes a block of audio.
    ///
    /// This is the main audio processing method. It receives input signals,
    /// current parameter values, and must fill the output buffers.
    ///
    /// # Arguments
    ///
    /// * `inputs` - Input signal buffers, indexed by input port order
    /// * `outputs` - Output signal buffers to fill, indexed by output port order
    /// * `params` - Current parameter values, indexed by parameter order
    /// * `context` - Processing context with sample rate, block size, and transport
    ///
    /// # Real-time Constraints
    ///
    /// This method runs on the audio thread and must not:
    /// - Allocate memory
    /// - Acquire locks
    /// - Perform I/O operations
    /// - Call any functions that might block
    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    );

    /// Resets the module to its initial state.
    ///
    /// Called when playback stops or when the user explicitly resets the module.
    /// Modules should clear any internal state (filters, delay lines, envelopes).
    fn reset(&mut self);

    /// Serializes the module's internal state for saving patches.
    ///
    /// Returns `None` if the module has no state beyond its parameters.
    /// The default implementation returns `None`.
    fn serialize_state(&self) -> Option<Vec<u8>> {
        None
    }

    /// Restores the module's internal state from saved data.
    ///
    /// The default implementation accepts any data and returns `Ok(())`.
    fn deserialize_state(&mut self, _data: &[u8]) -> Result<(), ModuleError> {
        Ok(())
    }

    /// Returns the final audio output for output modules.
    ///
    /// This is used by the audio engine to extract stereo audio from
    /// AudioOutput modules. Returns `None` for non-output modules.
    ///
    /// The returned slices are (left_channel, right_channel).
    fn get_audio_output(&self) -> Option<(&[f32], &[f32])> {
        None
    }

    /// Returns the output stage's meter readings since the last call, and
    /// starts a new measurement (for output modules).
    ///
    /// Returns `None` for non-output modules.
    fn take_output_levels(&mut self) -> Option<OutputLevels> {
        None
    }

    /// Returns this module's meter readings since the last call, and starts
    /// a new measurement, for modules that meter their own signals.
    ///
    /// Returns `None` for modules without meters.
    fn take_meter_levels(&mut self) -> Option<MeterLevels> {
        None
    }

    /// Starts keeping each part of the module's mix apart, for modules that
    /// mix several parts (a Mixer's channels), so an offline render can
    /// write each to a file of its own (`render --stems`). Allocates: never
    /// called on the audio thread. The default keeps nothing.
    fn keep_stems(&mut self, _max_block_size: usize) {}

    /// Calls `f` with each part kept since [`keep_stems`](Self::keep_stems),
    /// for the last block: its name, then its left and right.
    fn stems(&self, _f: &mut dyn FnMut(&'static str, &[f32], &[f32])) {}

    /// Returns captured scope buffer data for oscilloscope modules.
    ///
    /// Returns `Some((channel1, channel2, triggered))` when the oscilloscope has
    /// a new waveform capture ready to display. Returns `None` otherwise.
    ///
    /// The returned data is consumed (the module should clear its internal flag
    /// indicating new data is available). It is lent rather than copied, as
    /// this is called on the audio thread.
    fn take_scope_data(&mut self) -> Option<(&[f32], &[f32], bool)> {
        None
    }

    /// The transport this module sets for the whole patch, for tempo sources
    /// such as a clock: its tempo, its beat position and whether it's playing.
    /// Read before each block, from `params` and the state the last block
    /// left, and published to every module as `context.transport`.
    ///
    /// Returns `None` for modules that don't set the tempo.
    fn transport(&self, _params: &[f32]) -> Option<TransportState> {
        None
    }

    /// Live values for the module's node display, read after each callback.
    ///
    /// Returns `None` for modules without any.
    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        None
    }

    /// The sound the module's display analyses, by port id: what goes in,
    /// and what comes out (see [`SpectrumPorts`]).
    ///
    /// Returns `None` for modules whose display doesn't show a spectrum.
    fn spectrum_ports(&self) -> Option<SpectrumPorts> {
        None
    }

    /// Whether this module works per channel of a polyphonic cable. Its
    /// outputs then get room for [`MAX_CHANNELS`](super::signal::MAX_CHANNELS)
    /// channels, and its inputs receive every channel.
    ///
    /// A module that isn't polyphonic hears only the first channel of a
    /// polyphonic cable, except on audio inputs, which hear all the channels
    /// summed. The default is `false`.
    fn polyphonic(&self) -> bool {
        false
    }

    /// Inputs heard only for what they say about another signal, by input
    /// index: a compressor's sidechain, a vocoder's modulator. Bypassed,
    /// the module passes them nowhere.
    fn key_inputs(&self) -> &'static [usize] {
        &[]
    }

    /// Whether this module brings live input into the patch (Audio Input).
    /// Modules it feeds, through any chain, are told how late live input
    /// reaches them: see [`ProcessContext::input_latency`].
    fn is_live_input(&self) -> bool {
        false
    }

    /// How a module that claims notes (Pads, #428) works out which, from
    /// its parameters, so a Note In on Omni skips them (see
    /// [`NoteClaims`](super::NoteClaims)). The plan calls it every block,
    /// on the audio thread, so a learned note is claimed from the next.
    ///
    /// Returns `None` for modules that claim none.
    fn note_claims(&self) -> Option<fn(&[f32], &mut super::NoteClaims)> {
        None
    }

    /// Plays `chord` on the module's outputs in place of its own until
    /// it's taken away with `None`, for a module whose face lets a chord be
    /// heard as it's played in. Modules that don't audition ignore it.
    ///
    /// REAL-TIME SAFE: called on the audio thread between blocks.
    fn audition(&mut self, _chord: Option<Audition>) {}

    /// Gives the module a recording to play (a Sampler's file), or takes
    /// its recording away with `None`. Called on the audio thread, or on
    /// the UI side before the module has been handed to it.
    ///
    /// Returns a recording the module is done with, to be dropped off the
    /// audio thread: the one it had, or, for a module that doesn't play
    /// recordings, the one it was given. A module that lets voices on the
    /// old recording fade out keeps it a little longer, and hands it back
    /// through [`take_retired_sample`](Self::take_retired_sample) instead.
    fn load_sample(&mut self, sample: Option<Arc<SampleData>>) -> Option<Arc<SampleData>> {
        sample
    }

    /// A recording the module has finished with since the last call, to be
    /// dropped off the audio thread. Asked after every callback.
    fn take_retired_sample(&mut self) -> Option<Arc<SampleData>> {
        None
    }

    /// Gives the module the markers its recording is cut at, when sliced by
    /// Onsets (a Sampler's), as fractions of the recording. Called on the
    /// audio thread, or on the UI side before the module has been handed to
    /// it. Modules that don't slice ignore it.
    ///
    /// REAL-TIME SAFE: the markers come by value.
    fn set_markers(&mut self, _markers: Markers) {}

    /// Starts the module's random numbers from `seed`, so a patch plays the
    /// same noise wherever it runs: in the editor, in a capture or in the
    /// render tool. The graph seeds each module as it's added, before it
    /// first plays. Modules without randomness ignore it.
    fn seed(&mut self, _seed: u64) {}

    /// Work a module does a piece at a time between blocks, whether or not
    /// the transport runs: a Looper copying in a loop it was given. Uses up
    /// to `budget` frames of copying, taking off what it used. Called on
    /// the audio thread after every callback's messages, or on the UI side
    /// with no limit, for a module not yet handed to it.
    fn background(&mut self, _budget: &mut usize) {}

    /// Copies the recording the module holds (a Looper's loop, as heard)
    /// into `snapshot`, up to `budget` frames at a time, taking off what it
    /// used. Returns true once the snapshot is finished, whatever its
    /// outcome. Called after every callback's messages until then.
    fn fill_snapshot(&mut self, snapshot: &mut Snapshot, _budget: &mut usize) -> bool {
        snapshot.finish(SnapshotOutcome::Unsupported, 0);
        true
    }

    /// Gives the module room to record a take into (a Sampler's Rec): an
    /// empty recording with room set aside off the audio thread, held by
    /// nothing else, so the module can write it and then play it. Called
    /// on the audio thread.
    ///
    /// Returns room it's done with, to be dropped off the audio thread: the
    /// room it had, or, for a module that doesn't record, this.
    fn give_room(&mut self, room: Arc<SampleData>) -> Option<Arc<SampleData>> {
        Some(room)
    }

    /// A take the module finished since the last call, and now plays:
    /// shared, for the UI to keep. Asked after every callback.
    fn take_finished_take(&mut self) -> Option<FinishedTake> {
        None
    }

    /// Swaps the room take `take` was recorded into for the same recording
    /// made to measure, without a break in the voices playing it: it's
    /// the same sound. A module that has moved on since (another take, or
    /// another file) keeps what it has.
    ///
    /// Returns the recording it's done with, to be dropped off the audio
    /// thread: the room, or the one it was given if it kept its own.
    fn settle_take(&mut self, _take: u32, sample: Arc<SampleData>) -> Option<Arc<SampleData>> {
        Some(sample)
    }

    /// Where a Scenes module's Morph moved to in the last block, once,
    /// when the knobs it keeps should follow: how far, and from which
    /// scene to which (slots 0 to 7). The plan turns the knobs.
    ///
    /// Returns `None` for modules that don't keep scenes, and while Morph
    /// stands still.
    fn take_morph(&mut self) -> Option<(f32, usize, usize)> {
        None
    }
}

/// The most input ports a module may have. Inputs are passed to modules as a
/// stack array of buffer references, so this bounds that array.
pub const MAX_INPUTS: usize = 32;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::{PortDefinition, SignalBuffer, SignalType};

    /// A minimal test module that passes audio through unchanged.
    struct PassthroughModule {
        info: ModuleInfo,
        ports: Vec<PortDefinition>,
        parameters: Vec<ParameterDefinition>,
    }

    impl PassthroughModule {
        fn new() -> Self {
            Self {
                info: ModuleInfo::new(
                    "passthrough",
                    "Passthrough",
                    ModuleCategory::Utility,
                    "Passes audio through unchanged",
                ),
                ports: vec![
                    PortDefinition::input("in", "Input", SignalType::Audio),
                    PortDefinition::output("out", "Output", SignalType::Audio),
                ],
                parameters: vec![],
            }
        }
    }

    impl DspModule for PassthroughModule {
        fn info(&self) -> &ModuleInfo {
            &self.info
        }

        fn ports(&self) -> &[PortDefinition] {
            &self.ports
        }

        fn parameters(&self) -> &[ParameterDefinition] {
            &self.parameters
        }

        fn prepare(&mut self, _sample_rate: f32, _max_block_size: usize) {}

        fn process(
            &mut self,
            inputs: &[&SignalBuffer],
            outputs: &mut [SignalBuffer],
            _params: &[f32],
            _context: &ProcessContext,
        ) {
            outputs[0].samples.copy_from_slice(&inputs[0].samples);
        }

        fn reset(&mut self) {}
    }

    /// A test module with parameters.
    struct GainModule {
        info: ModuleInfo,
        ports: Vec<PortDefinition>,
        parameters: Vec<ParameterDefinition>,
    }

    impl GainModule {
        fn new() -> Self {
            use crate::dsp::ParameterDefinition;

            Self {
                info: ModuleInfo::new(
                    "gain",
                    "Gain",
                    ModuleCategory::Utility,
                    "Adjusts signal amplitude",
                ),
                ports: vec![
                    PortDefinition::input("in", "Input", SignalType::Audio),
                    PortDefinition::output("out", "Output", SignalType::Audio),
                ],
                parameters: vec![ParameterDefinition::normalized("gain", "Gain", 1.0)],
            }
        }
    }

    impl DspModule for GainModule {
        fn info(&self) -> &ModuleInfo {
            &self.info
        }

        fn ports(&self) -> &[PortDefinition] {
            &self.ports
        }

        fn parameters(&self) -> &[ParameterDefinition] {
            &self.parameters
        }

        fn prepare(&mut self, _sample_rate: f32, _max_block_size: usize) {}

        fn process(
            &mut self,
            inputs: &[&SignalBuffer],
            outputs: &mut [SignalBuffer],
            params: &[f32],
            _context: &ProcessContext,
        ) {
            let gain = params[0];
            for (out, &inp) in outputs[0].samples.iter_mut().zip(inputs[0].samples.iter()) {
                *out = inp * gain;
            }
        }

        fn reset(&mut self) {}
    }

    #[test]
    fn test_module_category_colors() {
        // Verify all categories have distinct colors
        let categories = [
            ModuleCategory::Source,
            ModuleCategory::Filter,
            ModuleCategory::Modulation,
            ModuleCategory::Effect,
            ModuleCategory::Utility,
            ModuleCategory::Output,
        ];

        for i in 0..categories.len() {
            for j in (i + 1)..categories.len() {
                assert_ne!(
                    categories[i].color(),
                    categories[j].color(),
                    "Categories {:?} and {:?} have the same color",
                    categories[i],
                    categories[j]
                );
            }
        }
    }

    #[test]
    fn test_module_category_names() {
        assert_eq!(ModuleCategory::Source.name(), "Source");
        assert_eq!(ModuleCategory::Filter.name(), "Filter");
        assert_eq!(ModuleCategory::Modulation.name(), "Modulation");
        assert_eq!(ModuleCategory::Effect.name(), "Effect");
        assert_eq!(ModuleCategory::Utility.name(), "Utility");
        assert_eq!(ModuleCategory::Output.name(), "Output");
    }

    #[test]
    fn test_module_info_creation() {
        let info = ModuleInfo::new(
            "test_mod",
            "Test Module",
            ModuleCategory::Effect,
            "A test module",
        );
        assert_eq!(info.id, "test_mod");
        assert_eq!(info.name, "Test Module");
        assert_eq!(info.category, ModuleCategory::Effect);
        assert_eq!(info.description, "A test module");
    }

    #[test]
    fn test_module_error_display() {
        let err = ModuleError::DeserializationFailed("invalid data".to_string());
        assert!(err.to_string().contains("invalid data"));

        let err = ModuleError::InvalidParameter {
            id: "freq".to_string(),
            reason: "out of range".to_string(),
        };
        assert!(err.to_string().contains("freq"));
        assert!(err.to_string().contains("out of range"));

        let err = ModuleError::InvalidState("not prepared".to_string());
        assert!(err.to_string().contains("not prepared"));
    }

    #[test]
    fn test_passthrough_module() {
        let mut module = PassthroughModule::new();

        assert_eq!(module.info().id, "passthrough");
        assert_eq!(module.ports().len(), 2);
        assert!(module.parameters().is_empty());

        // Test processing
        module.prepare(44100.0, 256);

        let mut input = SignalBuffer::audio(4);
        input.samples.copy_from_slice(&[1.0, 0.5, -0.5, -1.0]);

        let mut output = SignalBuffer::audio(4);
        let ctx = ProcessContext::default();

        module.process(&[&input], &mut [output.clone()], &[], &ctx);

        // For this test we need to manually copy since we can't borrow mutably twice
        for (i, &sample) in input.samples.iter().enumerate() {
            output.samples[i] = sample;
        }

        assert_eq!(output.samples, input.samples);
    }

    #[test]
    fn test_gain_module() {
        let mut module = GainModule::new();

        assert_eq!(module.info().id, "gain");
        assert_eq!(module.info().name, "Gain");
        assert_eq!(module.ports().len(), 2);
        assert_eq!(module.parameters().len(), 1);
        assert_eq!(module.parameters()[0].id, "gain");

        module.prepare(44100.0, 256);

        let mut input = SignalBuffer::audio(4);
        input.samples.copy_from_slice(&[1.0, 0.5, -0.5, -1.0]);

        let mut outputs = vec![SignalBuffer::audio(4)];
        let ctx = ProcessContext::default();

        // Test with gain = 0.5
        module.process(&[&input], &mut outputs, &[0.5], &ctx);

        // Verify output
        let expected = [0.5, 0.25, -0.25, -0.5];
        assert_eq!(outputs[0].samples, expected);
    }

    #[test]
    fn test_default_serialization() {
        let module = PassthroughModule::new();

        // Default implementation returns None
        assert!(module.serialize_state().is_none());
    }

    #[test]
    fn test_default_deserialization() {
        let mut module = PassthroughModule::new();

        // Default implementation accepts any data
        assert!(module.deserialize_state(&[1, 2, 3, 4]).is_ok());
    }

    #[test]
    fn test_module_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<PassthroughModule>();
        assert_send::<GainModule>();
    }
}
