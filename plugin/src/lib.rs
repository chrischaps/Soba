//! Soba as a CLAP instrument, and a VST3 one in builds with the `vst3`
//! feature (GPLv3, as nih-plug's VST3 wrapper is).
//!
//! The DAW plays the patch: its buffers, MIDI and transport come in, and the
//! node graph is the plugin's editor. Inside, it's the app as it is, made
//! with [`SynthApp::hosted`]: the app builds the patch and ships it to an
//! [`AudioProcessor`], which this plugin's audio thread drives with the
//! host's buffers instead of a sound card's.
//!
//! - **Audio:** [`Soba::process`] runs the processor over each buffer, with
//!   the buffer's MIDI placed on its samples and the host's transport for
//!   Clocks whose Source is Host. Nothing there allocates or locks.
//! - **The app** lives in [`Shared`], behind a lock that only the editor,
//!   the runner and the host's state calls take. While the editor is
//!   closed, the runner keeps the app's frames going without a window, so
//!   what the host changes (a project loaded, a macro automated) still
//!   reaches the sound.
//! - **State:** the patch is the plugin's state (`state::PatchState`), kept
//!   in the DAW's project. Samples are kept by path, as patch files keep them.
//! - **Macros:** eight host parameters, which knobs follow once mapped to
//!   them (right-click a knob, then *Map to Macro*).

use nih_plug::midi::MidiResult;
use nih_plug::prelude::*;
use parking_lot::Mutex;
use rtrb::RingBuffer;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use soba::app::SynthApp;
use soba::dsp::HostTransport;
use soba::graph::MACRO_COUNT;

mod editor;
pub mod player;
mod runner;
mod state;

use player::Player;

/// Room for MIDI on its way to the app's monitor and MIDI Learn.
const UI_MIDI_CAPACITY: usize = 1024;

/// What the plugin's threads share: the app, made once the host first says
/// what rate it plays at, and whether its editor is open.
pub struct Shared {
    app: Mutex<Option<SynthApp>>,
    /// The state the host restored before the app was made, to open then.
    pending_state: Mutex<Option<String>>,
    /// Whether the editor is drawing the app's frames (the runner rests).
    editor_open: AtomicBool,
}

/// Readies an egui context for the app: its fonts, the theme's styles and
/// colours, and no quit key. The app styles the first context it draws in
/// by itself; the plugin draws in several (one per opening of the editor,
/// and the runner's), so each is readied as it's made.
pub fn prepare_context(ctx: &egui::Context) {
    soba::app::theme::install_fonts(ctx);
    soba::app::theme::apply_theme(ctx);
    // egui closes the window on Ctrl+Q (since 0.34): in a DAW that's the host's to quit
    ctx.options_mut(|o| o.quit_shortcuts.clear());
}

impl Shared {
    fn new() -> Self {
        Self { app: Mutex::new(None), pending_state: Mutex::new(None), editor_open: AtomicBool::new(false) }
    }

    /// Runs `f` on the app, if it's been made.
    fn with_app<R>(&self, f: impl FnOnce(&mut SynthApp) -> R) -> Option<R> {
        self.app.lock().as_mut().map(f)
    }
}

pub struct Soba {
    params: Arc<SobaParams>,
    shared: Arc<Shared>,
    /// The audio thread's half: plays what the app builds.
    player: Option<Player>,
    /// Runs the app's frames while the editor is closed. Declared last, so
    /// it stops after the rest of the plugin is let go.
    runner: Option<runner::Runner>,
}

#[derive(Params)]
pub struct SobaParams {
    /// The patch, kept in the DAW's project.
    #[persist = "patch"]
    patch: state::PatchState,

    #[id = "macro1"]
    m1: FloatParam,
    #[id = "macro2"]
    m2: FloatParam,
    #[id = "macro3"]
    m3: FloatParam,
    #[id = "macro4"]
    m4: FloatParam,
    #[id = "macro5"]
    m5: FloatParam,
    #[id = "macro6"]
    m6: FloatParam,
    #[id = "macro7"]
    m7: FloatParam,
    #[id = "macro8"]
    m8: FloatParam,
}

impl SobaParams {
    fn new(shared: &Arc<Shared>) -> Self {
        let m = |n: u8| FloatParam::new(format!("Macro {n}"), 0.0, FloatRange::Linear { min: 0.0, max: 1.0 });
        Self {
            patch: state::PatchState::new(shared),
            m1: m(1),
            m2: m(2),
            m3: m(3),
            m4: m(4),
            m5: m(5),
            m6: m(6),
            m7: m(7),
            m8: m(8),
        }
    }

    /// The macros' values, 0-1 each.
    fn macros(&self) -> [f32; MACRO_COUNT as usize] {
        [&self.m1, &self.m2, &self.m3, &self.m4, &self.m5, &self.m6, &self.m7, &self.m8].map(|p| p.value())
    }
}

impl Default for Soba {
    fn default() -> Self {
        let shared = Arc::new(Shared::new());
        Self {
            params: Arc::new(SobaParams::new(&shared)),
            shared,
            player: None,
            runner: None,
        }
    }
}

impl Plugin for Soba {
    const NAME: &'static str = "Soba";
    const VENDOR: &'static str = "chaps.dev";
    const URL: &'static str = "https://docs.chaps.dev/soba";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: None,
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];

    // Notes and CCs: CCs reach MIDI Learn and the modules that read them
    const MIDI_INPUT: MidiConfig = MidiConfig::MidiCCs;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        Some(Box::new(editor::SobaEditor::new(self.shared.clone(), self.params.clone())))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, buffer_config: &BufferConfig, _context: &mut impl InitContext<Self>) -> bool {
        let (sample_rate, max_frames) = (buffer_config.sample_rate, buffer_config.max_buffer_size as usize);
        match self.player.as_mut() {
            // Again, perhaps at another rate: the patch carries on
            Some(player) => player.reset(sample_rate, max_frames),
            None => {
                let (ui_midi, heard) = RingBuffer::new(UI_MIDI_CAPACITY);
                let (mut app, processor) = SynthApp::hosted(sample_rate, heard);
                if let Some(saved) = self.shared.pending_state.lock().take() {
                    if let Err(e) = app.restore_hosted_state(&saved) {
                        nih_log!("Soba couldn't open the project's patch: {e}");
                    }
                }
                *self.shared.app.lock() = Some(app);
                self.player = Some(Player::new(processor, ui_midi, sample_rate, max_frames));
                self.runner = Some(runner::start(&self.shared, &self.params));
            }
        }
        true
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let frames = buffer.samples();
        let Some(player) = self.player.as_mut() else {
            for channel in buffer.as_slice() {
                channel.fill(0.0);
            }
            return ProcessStatus::Normal;
        };
        while let Some(event) = context.next_event() {
            let timing = event.timing();
            if let Some(MidiResult::Basic(bytes)) = event.as_midi() {
                player.queue(timing, &bytes);
            }
        }
        // The DAW's transport, for Clocks that follow it
        let transport = context.transport();
        let host = transport.tempo.map(|tempo_bpm| HostTransport {
            playing: transport.playing,
            tempo_bpm,
            beat: transport.pos_beats().unwrap_or(0.0),
        });
        player.play(buffer.as_slice(), frames, host);
        ProcessStatus::KeepAlive
    }
}

impl ClapPlugin for Soba {
    const CLAP_ID: &'static str = "dev.chaps.soba";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("A node-based modular synthesizer");
    const CLAP_MANUAL_URL: Option<&'static str> = Some("https://docs.chaps.dev/soba");
    const CLAP_SUPPORT_URL: Option<&'static str> = Some("https://github.com/chrischaps/Soba/issues");
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::Instrument, ClapFeature::Synthesizer, ClapFeature::Stereo];
}

nih_export_clap!(Soba);

#[cfg(feature = "vst3")]
impl Vst3Plugin for Soba {
    const VST3_CLASS_ID: [u8; 16] = *b"SobaChapsDevSynt";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Instrument, Vst3SubCategory::Synth, Vst3SubCategory::Stereo];
}

#[cfg(feature = "vst3")]
nih_export_vst3!(Soba);
