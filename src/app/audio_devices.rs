//! The UI's side of the sound devices (#343): the engine, the outputs and
//! inputs to choose from and the ones chosen, the open input's health, and
//! whether the output is still taking audio.
//!
//! Choosing a device touches more than this: a take ends, the patch's Audio
//! Inputs are fed or let go, the status bar warns. `SynthApp` does those
//! around the calls here. What's kept here are the devices and the small
//! rules about them (when a stream counts as stalled, how long a dropout
//! stays amber, when to warn of feedback), which can be tested without one.

use std::time::Duration;

use web_time::Instant;

use crate::engine::{AudioEngine, AudioError, DeviceInfo, InputMonitor};
use super::input_device;

/// How long the audio callback can go quiet before the output is shown as
/// stalled. Devices ask for audio every few milliseconds.
pub const AUDIO_STALL: Duration = Duration::from_millis(750);

/// How long the input's status stays amber after a dropout.
pub const INPUT_GLITCH_HOLD: Duration = Duration::from_secs(3);

/// A count, and when it last moved.
#[derive(Clone, Copy, Debug)]
struct Watch {
    count: u64,
    moved: Instant,
}

impl Watch {
    fn new(now: Instant) -> Self {
        Self { count: 0, moved: now }
    }

    /// Notes the count as it is `now`; how long it's been since it moved.
    fn see(&mut self, count: u64, now: Instant) -> Duration {
        if count != self.count {
            *self = Self { count, moved: now };
        }
        now - self.moved
    }
}

/// The sound devices, as the UI holds them.
pub struct AudioDevices {
    /// The engine, or why there isn't one.
    pub engine: Result<AudioEngine, AudioError>,
    /// What last went wrong with the output, for the status bar.
    pub error: Option<String>,
    /// The outputs, for the Output menu.
    pub outputs: Vec<DeviceInfo>,
    /// The output chosen, by its index among `outputs`.
    pub selected_output: usize,
    /// The inputs, for the Input menu.
    pub inputs: Vec<DeviceInfo>,
    /// The open input's counters and health, while one is open.
    pub input: Option<InputMonitor>,
    /// The input's dropouts so far (underruns, overflows and the device's
    /// own), and when that last grew.
    input_glitches: Watch,
    /// The browser build's late buffers so far, and when that last grew.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    output_dropouts: Watch,
    /// Whether this session has warned about the input hearing the speakers.
    feedback_warned: bool,
    /// The audio callback count, and when it last moved: a count that
    /// stops moving means the device has stopped taking audio.
    heartbeat: Watch,
}

impl AudioDevices {
    /// The devices of `engine`, the system's default output chosen. Inputs
    /// are listed, not opened: nothing opens the microphone unasked.
    pub fn new(engine: Result<AudioEngine, AudioError>) -> Self {
        let error = engine.as_ref().err().map(ToString::to_string);
        let (outputs, inputs) = match &engine {
            Ok(engine) => (engine.enumerate_devices(), engine.enumerate_input_devices()),
            Err(_) => (Vec::new(), Vec::new()),
        };
        let selected_output = outputs.iter().position(|d| d.is_default).unwrap_or(0);
        let now = Instant::now();
        Self {
            engine,
            error,
            outputs,
            selected_output,
            inputs,
            input: None,
            input_glitches: Watch::new(now),
            output_dropouts: Watch::new(now),
            feedback_warned: false,
            heartbeat: Watch::new(now),
        }
    }

    /// Lists the outputs again.
    pub fn refresh_outputs(&mut self) {
        if let Ok(engine) = &self.engine {
            self.outputs = engine.enumerate_devices();
        }
    }

    /// Lists the inputs again.
    pub fn refresh_inputs(&mut self) {
        if let Ok(engine) = &self.engine {
            self.inputs = engine.enumerate_input_devices();
        }
    }

    /// The output chosen, if it's listed.
    pub fn output(&self) -> Option<&DeviceInfo> {
        self.outputs.get(self.selected_output)
    }

    /// The input to open when an open one moves to another system: the
    /// system's default, or its first.
    pub fn default_input(&self) -> Option<usize> {
        self.inputs.iter().find(|d| d.is_default).or(self.inputs.first()).map(|d| d.index)
    }

    /// An input opened, with its counters.
    pub fn input_opened(&mut self, monitor: InputMonitor) {
        self.input_glitches = Watch::new(Instant::now());
        self.input = Some(monitor);
    }

    /// Lists the devices again after the output changed system, device or
    /// buffer, pointing the Output menu at the one running. Returns whether
    /// the engine closed the input, which the caller lets go of in turn.
    pub fn after_output_change(&mut self) -> bool {
        let Ok(engine) = &self.engine else { return false };
        self.outputs = engine.enumerate_devices();
        self.selected_output = engine.current_device_index().unwrap_or(0);
        self.inputs = engine.enumerate_input_devices();
        if !engine.stream_failed() {
            self.error = None;
        }
        let closed = engine.input_index().is_none();
        if closed {
            self.input = None;
        }
        closed
    }

    /// The first time an input is open while the output is speakers, the
    /// warning that it may feed back; after that, and for headphones, none.
    pub fn feedback_warning(&mut self) -> Option<String> {
        let output = self.output()?.name.clone();
        let warning = feedback_warning(&mut self.feedback_warned, self.input.is_some(), &output)?;
        Some(warning)
    }

    /// Has the input dropped out lately? `glitches` is its count so far.
    pub fn input_glitched(&mut self, glitches: u64, now: Instant) -> bool {
        let since = self.input_glitches.see(glitches, now);
        glitches > 0 && since < INPUT_GLITCH_HOLD
    }

    /// Has the output dropped a buffer lately? `late` is its count so far
    /// (the browser build's, which the page counts).
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub fn output_dropped(&mut self, late: u64, now: Instant) -> bool {
        let since = self.output_dropouts.see(late, now);
        late > 0 && since < INPUT_GLITCH_HOLD
    }

    /// Has the output stopped asking for audio? `count` is its callbacks so far.
    pub fn stalled(&mut self, count: u64, now: Instant) -> bool {
        self.heartbeat.see(count, now) > AUDIO_STALL
    }
}

/// The feedback warning for an input open (`input_open`) into `output`, the
/// first time: `warned` remembers it was given. Headphones don't feed back.
fn feedback_warning(warned: &mut bool, input_open: bool, output: &str) -> Option<String> {
    if *warned || !input_open || input_device::looks_like_headphones(output) {
        return None;
    }
    *warned = true;
    Some(input_device::feedback_warning(output))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_count_that_stops_moving_is_a_stall() {
        let start = Instant::now();
        let mut watch = Watch::new(start);
        assert_eq!(watch.see(5, start), Duration::ZERO);
        let later = start + AUDIO_STALL * 2;
        assert!(watch.see(5, later) > AUDIO_STALL);
        // Moving again starts the clock over
        assert_eq!(watch.see(6, later), Duration::ZERO);
    }

    #[test]
    fn a_dropout_stays_amber_for_a_while() {
        let start = Instant::now();
        let mut devices = AudioDevices::new(Err(AudioError::NoOutputDevice));
        assert!(!devices.input_glitched(0, start));
        assert!(devices.input_glitched(3, start));
        assert!(devices.input_glitched(3, start + INPUT_GLITCH_HOLD / 2));
        assert!(!devices.input_glitched(3, start + INPUT_GLITCH_HOLD * 2));
        assert!(devices.input_glitched(4, start + INPUT_GLITCH_HOLD * 2));
    }

    #[test]
    fn a_late_buffer_stays_amber_as_long_as_an_input_dropout() {
        let start = Instant::now();
        let mut devices = AudioDevices::new(Err(AudioError::NoOutputDevice));
        assert!(!devices.output_dropped(0, start));
        assert!(devices.output_dropped(1, start));
        assert!(!devices.output_dropped(1, start + INPUT_GLITCH_HOLD * 2));
        assert!(devices.output_dropped(2, start + INPUT_GLITCH_HOLD * 2));
        // The input's own dropouts are watched apart
        assert!(!devices.input_glitched(0, start + INPUT_GLITCH_HOLD * 2));
    }

    #[test]
    fn feedback_is_warned_of_once_and_not_for_headphones() {
        let mut warned = false;
        assert!(feedback_warning(&mut warned, false, "Speakers").is_none());
        assert!(feedback_warning(&mut warned, true, "Headphones (USB Audio)").is_none());
        assert!(!warned);
        assert!(feedback_warning(&mut warned, true, "Speakers").is_some());
        assert!(feedback_warning(&mut warned, true, "Speakers").is_none());
    }

    #[test]
    fn no_engine_is_no_devices_and_says_why() {
        let devices = AudioDevices::new(Err(AudioError::NoOutputDevice));
        assert!(devices.outputs.is_empty() && devices.inputs.is_empty());
        assert!(devices.error.is_some());
        assert!(devices.output().is_none());
    }
}
