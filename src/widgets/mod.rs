//! Widgets module
//!
//! Custom UI controls for the synthesizer interface.
//! Includes knobs, faders, LED indicators, meters, the scope, the piano,
//! and the noise display. Most module faces live in `graph::*_display`.

pub mod knob;
pub mod knob_input;
mod param_parse;
pub mod fader;
pub mod led;
pub mod cpu_meter;
pub mod oscilloscope_display;
pub mod adsr_display;
pub mod piano;
pub mod level_meter;
pub mod noise_display;

// Re-export commonly used items
pub use knob::{knob, mini_knob, KnobConfig, KnobStyle, ParamFormat};
pub use knob_input::{claim_wheel as claim_knob_wheel, typing};
pub use fader::{fader, horizontal_fader, FaderConfig};
pub use led::{led, LedConfig};
pub use cpu_meter::{cpu_meter, CpuMeterConfig, cpu_load_color};
pub use oscilloscope_display::{oscilloscope_display, OscilloscopeConfig, TriggerMode};
pub use adsr_display::{AdsrParams, AdsrSegment, generate_adsr_curve, get_adsr_segment_boundaries};
pub use piano::{piano, piano_keys, PianoConfig, PianoData};
pub use level_meter::{column_meter, level_meter, LevelMeter, LevelMeterConfig, ModuleMeters, PeakBallistics};
pub use noise_display::{noise_display, NoiseDisplayConfig};
