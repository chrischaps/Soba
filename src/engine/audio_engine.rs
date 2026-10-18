//! Audio Engine
//!
//! Manages the cpal audio stream and interfaces with system audio hardware.
//! The audio callback runs in a separate thread and must be real-time safe.
//!
//! The engine runs on one of the computer's [`AudioSystem`]s: the one every
//! app shares (WASAPI on Windows), or, built with the `asio` feature, an
//! interface's ASIO driver. ASIO serves input and output from one driver on
//! one clock, in buffers of a few milliseconds, for live playing. It also
//! loads one driver at a time, which shapes how devices are listed and
//! opened here: see [`AudioEngine::set_audio_system`].

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, Device, FromSample, Host, SampleFormat, SizedSample, Stream, StreamConfig, SupportedBufferSize};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use web_time::Instant;

use super::audio_input::{input_channel_converting, input_channel_same_clock, InputFeed, InputMonitor, InputSender};
use super::audio_processor::AudioProcessor;
use super::latency::LatencyGauge;
use crate::dsp::{InputAudio, MidiEvent};

/// Errors that can occur during audio engine operation.
#[derive(Debug, Clone)]
pub enum AudioError {
    /// No audio output device was found.
    NoOutputDevice,
    /// The chosen input device is gone.
    NoInputDevice,
    /// Failed to get device configuration.
    ConfigurationFailed(String),
    /// Failed to create the audio stream.
    StreamCreationFailed(String),
    /// Failed to start/stop playback.
    StreamPlaybackFailed(String),
    /// No ASIO driver would start.
    NoAsioDriver,
    /// The named ASIO driver wouldn't start.
    DriverUnavailable(String),
    /// This build can't run on that audio system.
    SystemUnavailable(AudioSystem),
}

impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioError::NoOutputDevice => write!(f, "No audio output device found"),
            AudioError::NoInputDevice => write!(f, "That input device is gone: choose it again under Input"),
            AudioError::ConfigurationFailed(msg) => {
                write!(f, "Failed to get device configuration: {}", msg)
            }
            AudioError::StreamCreationFailed(msg) => {
                write!(f, "Failed to create audio stream: {}", msg)
            }
            AudioError::StreamPlaybackFailed(msg) => {
                write!(f, "Failed to control audio playback: {}", msg)
            }
            AudioError::NoAsioDriver => write!(
                f,
                "No ASIO driver would start. Check that the interface is plugged in, that no other app is using it, \
                 and that its ASIO driver is installed (for a Scarlett: Focusrite USB ASIO, from focusrite.com)"
            ),
            AudioError::DriverUnavailable(name) => write!(
                f,
                "{} wouldn't start: check that the interface is plugged in and that no other app is using it",
                name
            ),
            AudioError::SystemUnavailable(system) => write!(f, "This build of Soba can't use {}", system.label()),
        }
    }
}

impl std::error::Error for AudioError {}

/// One of the computer's audio systems.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AudioSystem {
    /// The operating system's own, shared by every app: WASAPI on Windows,
    /// CoreAudio on macOS, ALSA on Linux.
    #[default]
    System,
    /// An interface's ASIO driver, which talks to the hardware directly
    /// (Windows, in builds with the `asio` feature).
    Asio,
}

impl AudioSystem {
    /// Every system this build can run on, the default first.
    pub fn available() -> Vec<AudioSystem> {
        let mut systems = vec![AudioSystem::System];
        if cfg!(all(windows, feature = "asio")) {
            systems.push(AudioSystem::Asio);
        }
        systems
    }

    /// Its name, for menus.
    pub fn label(self) -> &'static str {
        match self {
            AudioSystem::System if cfg!(windows) => "Windows Audio",
            AudioSystem::System => "System audio",
            AudioSystem::Asio => "ASIO",
        }
    }

    /// A stable name, to remember the choice by.
    pub fn key(self) -> &'static str {
        match self {
            AudioSystem::System => "system",
            AudioSystem::Asio => "asio",
        }
    }

    /// The system remembered as `key`, if this build has it.
    pub fn from_key(key: &str) -> Option<Self> {
        AudioSystem::available().into_iter().find(|system| system.key() == key)
    }

    fn host(self) -> Result<Host, AudioError> {
        match self {
            AudioSystem::System => system_host(),
            #[cfg(all(windows, feature = "asio"))]
            AudioSystem::Asio => cpal::host_from_id(cpal::HostId::Asio).map_err(|_| AudioError::NoAsioDriver),
            #[cfg(not(all(windows, feature = "asio")))]
            AudioSystem::Asio => Err(AudioError::SystemUnavailable(self)),
        }
    }
}

/// The computer's own audio system.
#[cfg(not(target_arch = "wasm32"))]
fn system_host() -> Result<Host, AudioError> {
    Ok(cpal::default_host())
}

/// The browser's Web Audio, if it has it: cpal's default host would panic
/// without. A threaded build on a cross-origin isolated page renders in the
/// browser's audio thread (#375), and otherwise on the page's own.
#[cfg(target_arch = "wasm32")]
fn system_host() -> Result<Host, AudioError> {
    #[cfg(all(feature = "web-worklet", target_feature = "atomics"))]
    if let Ok(host) = cpal::host_from_id(cpal::HostId::AudioWorklet) {
        return Ok(host);
    }
    cpal::available_hosts()
        .into_iter()
        .find_map(|id| cpal::host_from_id(id).ok())
        .ok_or(AudioError::NoOutputDevice)
}

/// The buffer size for the browser, where the app renders sound on the
/// page's own thread, between drawing frames. Firefox runs late now and then
/// at the default 2048 frames (it drops a buffer every few seconds on Lush
/// Pad), and not at all at 4096; Chrome and Safari keep up at 2048, with
/// half the delay. An embed (`?patch=`, or a `?listen` player) takes 4096 everywhere: drawing a
/// big patch can take longer than a 2048-frame buffer leaves spare, and
/// index.html holds frames back only while one fits (#387). An embed is for
/// listening, so the delay matters less there. An AudioWorklet renders the
/// browser's own quantum, 128 frames, on its audio thread (#375).
#[cfg(target_arch = "wasm32")]
fn web_buffer() -> BufferSize {
    #[cfg(all(feature = "web-worklet", target_feature = "atomics"))]
    if cpal::host_from_id(cpal::HostId::AudioWorklet).is_ok() {
        return BufferSize::Default;
    }
    let window = web_sys::window();
    let firefox = window
        .as_ref()
        .and_then(|w| w.navigator().user_agent().ok())
        .is_some_and(|agent| agent.contains("Firefox/"));
    let embed = window
        .and_then(|w| w.location().search().ok())
        .and_then(|search| web_sys::UrlSearchParams::new_with_str(&search).ok())
        .is_some_and(|params| params.has("patch") || params.has("listen"));
    if firefox || embed { BufferSize::Fixed(4096) } else { BufferSize::Default }
}

#[cfg(not(target_arch = "wasm32"))]
fn web_buffer() -> BufferSize {
    BufferSize::Default
}

/// The buffer sizes, in frames, offered on systems that let Soba choose.
pub const BUFFER_CHOICES: [u32; 5] = [32, 64, 128, 256, 512];

/// How often to look for a default output to come back, while there's none.
const DEFAULT_POLL: Duration = Duration::from_secs(1);

/// An input device's stream, while it's open.
struct OpenInput {
    /// Kept alive to keep recording; dropping it closes the device.
    _stream: Stream,
    /// The device's index in [`AudioEngine::enumerate_input_devices`].
    index: usize,
    name: String,
    /// The device's own channel count (the patch hears its first two).
    channels: u16,
    /// The device's own sample rate, converted to the output's if it differs.
    sample_rate: u32,
}

/// Information about an audio output device.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    /// Human-readable device name.
    pub name: String,
    /// Whether this is the default output device.
    pub is_default: bool,
    /// Index in the device list (for selection).
    pub index: usize,
}

/// Shared state between audio callback and main thread.
/// All fields use atomics for lock-free access.
struct AudioState {
    /// Counts audio callbacks: while the device is taking audio this keeps
    /// climbing, so the UI can tell a live stream from a stalled one.
    callbacks: AtomicU64,
    /// Set by the stream's error callback (device unplugged, driver reset).
    stream_failed: AtomicBool,
    /// Why the stream stopped: one of the `STOPPED_*` reasons.
    stopped: AtomicU8,
    /// Whether the stream plays to whatever the system's default output is,
    /// rather than to a device chosen by name.
    follows_default: AtomicBool,
    /// Glitches the output device reported, which the stream recovers from.
    xruns: AtomicU64,
    /// The output device's own delay, from callback to playback.
    output_latency: LatencyGauge,
}

/// The stream is running (or hasn't failed).
const STOPPED_NOT: u8 = 0;
/// The device went away, or failed.
const STOPPED_LOST: u8 = 1;
/// The driver asked for the stream to be rebuilt: its buffer size or sample
/// rate was changed from its own control panel.
const STOPPED_RESET: u8 = 2;
/// The system's default output changed (headphones plugged in, say) under a
/// stream that follows it. The stream stays on the old device, so it's
/// rebuilt on the new one, at the settings it had.
const STOPPED_DEFAULT_CHANGED: u8 = 3;

/// Why a stream that hit `kind` stopped. Windows Audio reports the default
/// output changing as an invalidated stream, the way an ASIO driver reports
/// a reset, and the default going away with none to take its place as an
/// unavailable device.
fn stop_reason(kind: cpal::ErrorKind, follows_default: bool) -> u8 {
    match kind {
        cpal::ErrorKind::StreamInvalidated if follows_default => STOPPED_DEFAULT_CHANGED,
        cpal::ErrorKind::StreamInvalidated => STOPPED_RESET,
        _ => STOPPED_LOST,
    }
}

impl AudioState {
    /// Takes in an error from a stream's error callback: a glitch is
    /// counted, anything else ends the stream.
    fn take_error(&self, err: &cpal::Error) {
        if is_glitch(err) {
            self.xruns.fetch_add(1, Ordering::Relaxed);
            return;
        }
        eprintln!("Audio stream error: {}", err);
        let reason = stop_reason(err.kind(), self.follows_default.load(Ordering::Relaxed));
        self.stopped.store(reason, Ordering::Relaxed);
        self.stream_failed.store(true, Ordering::Relaxed);
    }

    /// Readies the state for a new stream, which plays to the system's
    /// default output if `follows_default`.
    fn reset(&self, follows_default: bool) {
        self.follows_default.store(follows_default, Ordering::Relaxed);
        self.stream_failed.store(false, Ordering::Relaxed);
        self.stopped.store(STOPPED_NOT, Ordering::Relaxed);
        self.output_latency.clear();
    }

    fn new() -> Self {
        Self {
            callbacks: AtomicU64::new(0),
            stream_failed: AtomicBool::new(false),
            stopped: AtomicU8::new(STOPPED_NOT),
            follows_default: AtomicBool::new(false),
            xruns: AtomicU64::new(0),
            output_latency: LatencyGauge::default(),
        }
    }
}

/// The main audio engine that manages cpal streams.
pub struct AudioEngine {
    system: AudioSystem,
    host: Host,
    device: Device,
    config: StreamConfig,
    /// The output's sample format: f32, or what an ASIO driver takes.
    format: SampleFormat,
    /// The buffer size asked for, in frames, on systems that let Soba
    /// choose; `None` leaves it to the driver.
    buffer_request: Option<u32>,
    /// The ASIO drivers, listed while none was loaded: ASIO loads one at a
    /// time, so while one runs the others can't be listed.
    asio_devices: Vec<Device>,
    stream: Option<Stream>,
    state: Arc<AudioState>,
    /// The graph processor driven by the stream, kept here so it survives a
    /// device change (the stream, and its callback's handle, are rebuilt).
    processor: Option<Arc<Mutex<AudioProcessor>>>,
    /// Whether a stream should be running: set when one starts, cleared only
    /// by [`stop`](Self::stop). A rebuild that fails (the device gone) leaves
    /// it set, so choosing the device again starts the stream there.
    wanted: bool,
    /// The input device's stream, while one is open.
    input: Option<OpenInput>,
    /// Whether `device` is the system's default output, whichever device
    /// that is at the time, rather than one chosen from the list.
    follows_default: bool,
    /// When [`default_returned`](Self::default_returned) last asked the
    /// system for a default output, while there was none.
    last_default_poll: Option<Instant>,
}

impl AudioEngine {
    /// Create a new AudioEngine using the default output device.
    pub fn new() -> Result<Self, AudioError> {
        Self::with_system(AudioSystem::System, None)
    }

    /// Creates an engine on `system`'s default output device (an ASIO
    /// system's first driver), asking for `buffer` frames per callback
    /// where the system lets Soba choose.
    pub fn with_system(system: AudioSystem, buffer: Option<u32>) -> Result<Self, AudioError> {
        let host = system.host()?;
        let asio_devices = list_asio_devices(&host, system);
        let device = default_output_device(&host, system, &asio_devices)?;
        let (config, format) = output_config(&device, system, buffer)?;
        Ok(Self {
            system,
            host,
            device,
            config,
            format,
            buffer_request: buffer,
            asio_devices,
            stream: None,
            state: Arc::new(AudioState::new()),
            processor: None,
            wanted: false,
            input: None,
            follows_default: system == AudioSystem::System,
            last_default_poll: None,
        })
    }

    /// The audio system the engine runs on.
    pub fn audio_system(&self) -> AudioSystem {
        self.system
    }

    /// Moves the engine, and a running patch with it, to `system`, on its
    /// default output device. The input closes: open it again from
    /// [`enumerate_input_devices`](Self::enumerate_input_devices), which
    /// lists the new system's. If `system` won't start, the engine goes
    /// back to the one it was on and returns why.
    pub fn set_audio_system(&mut self, system: AudioSystem) -> Result<(), AudioError> {
        if system == self.system {
            return Ok(());
        }
        let previous = self.system;
        let was_running = self.wanted;
        // ASIO lists its drivers by loading each in turn, which it can't do
        // while one is in use
        self.close_input();
        self.drop_stream();
        match self.move_to(system, was_running) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = self.move_to(previous, was_running);
                Err(e)
            }
        }
    }

    /// Opens `system`'s default output device, and restarts the stream
    /// there if `restart`.
    fn move_to(&mut self, system: AudioSystem, restart: bool) -> Result<(), AudioError> {
        let host = system.host()?;
        let asio_devices = list_asio_devices(&host, system);
        let device = default_output_device(&host, system, &asio_devices)?;
        let (config, format) = output_config(&device, system, self.buffer_request)?;
        self.system = system;
        self.host = host;
        self.asio_devices = asio_devices;
        self.device = device;
        self.follows_default = system == AudioSystem::System;
        self.config = config;
        self.format = format;
        if restart {
            self.restart_stream()?;
        }
        Ok(())
    }

    /// The buffer sizes, in frames, the output device runs at from
    /// [`BUFFER_CHOICES`]. Empty where the system sets its own (Windows
    /// Audio rounds any request up to its own period).
    pub fn buffer_choices(&self) -> Vec<u32> {
        if self.system != AudioSystem::Asio {
            return Vec::new();
        }
        match self.device.default_output_config().map(|config| *config.buffer_size()) {
            Ok(SupportedBufferSize::Range { min, max }) => {
                BUFFER_CHOICES.into_iter().filter(|frames| (min..=max).contains(frames)).collect()
            }
            _ => Vec::new(),
        }
    }

    /// The buffer size asked for, in frames, if one was.
    pub fn buffer_request(&self) -> Option<u32> {
        self.buffer_request
    }

    /// The output's frames per callback, once the stream has said.
    pub fn buffer_frames(&self) -> Option<u32> {
        self.stream.as_ref().and_then(|stream| stream.buffer_size().ok())
    }

    /// Asks for `frames` per callback where the system lets Soba choose
    /// (`None` leaves it to the driver), rebuilding a running stream to
    /// take it. The input closes, as for
    /// [`set_audio_system`](Self::set_audio_system). A driver that won't run
    /// at that size picks its own: see [`buffer_frames`](Self::buffer_frames).
    pub fn set_buffer_size(&mut self, frames: Option<u32>) -> Result<(), AudioError> {
        self.buffer_request = frames;
        if self.system != AudioSystem::Asio {
            return Ok(());
        }
        let was_running = self.wanted;
        // The driver keeps the size its buffers were made at until every
        // stream on it has closed
        self.close_input();
        self.drop_stream();
        let (config, format) = output_config(&self.device, self.system, frames)?;
        self.config = config;
        self.format = format;
        if was_running {
            self.restart_stream()?;
        }
        Ok(())
    }

    /// Whether the driver stopped the stream to change its own settings
    /// (its buffer size or sample rate, from its control panel), so it
    /// should be rebuilt with [`restart_after_reset`](Self::restart_after_reset).
    pub fn stream_reset(&self) -> bool {
        self.state.stopped.load(Ordering::Relaxed) == STOPPED_RESET
    }

    /// Rebuilds the stream after the driver reset it, at the driver's new
    /// settings: its own buffer size from now on, rather than the one asked
    /// for. The input closes, as for [`set_audio_system`](Self::set_audio_system).
    pub fn restart_after_reset(&mut self) -> Result<(), AudioError> {
        self.buffer_request = None;
        self.close_input();
        self.drop_stream();
        let (config, format) = output_config(&self.device, self.system, None)?;
        self.config = config;
        self.format = format;
        self.restart_stream()
    }

    /// Whether the system's default output changed under a stream that
    /// follows it, so it should be rebuilt there with
    /// [`follow_default`](Self::follow_default).
    pub fn default_changed(&self) -> bool {
        self.state.stopped.load(Ordering::Relaxed) == STOPPED_DEFAULT_CHANGED
    }

    /// Whether a stream that follows the default output has lost it, and
    /// waits for one to come back: see [`default_returned`](Self::default_returned).
    pub fn awaiting_default(&self) -> bool {
        let lost = self.state.stopped.load(Ordering::Relaxed) == STOPPED_LOST || self.stream.is_none();
        self.follows_default && self.wanted && lost
    }

    /// Whether the default output went away and a default has since come
    /// back (a device plugged in, or another made the default), so the
    /// stream should be rebuilt there with [`follow_default`](Self::follow_default).
    /// It asks the system about once a second.
    pub fn default_returned(&mut self) -> bool {
        if !self.awaiting_default() {
            self.last_default_poll = None;
            return false;
        }
        let now = Instant::now();
        if self.last_default_poll.is_some_and(|at| now - at < DEFAULT_POLL) {
            return false;
        }
        self.last_default_poll = Some(now);
        self.host.default_output_device().is_some()
    }

    /// Rebuilds the stream on the system's default output, at its own rate
    /// and layout, keeping the buffer size asked for. An open input stays
    /// open, as it's another device, but if the output's
    /// [`sample_rate`](Self::sample_rate) changed it needs opening again to
    /// convert to the new one. Returns the new device's name.
    pub fn follow_default(&mut self) -> Result<String, AudioError> {
        self.drop_stream();
        // Until a stream starts there, it's waiting for a default
        self.state.stopped.store(STOPPED_LOST, Ordering::Relaxed);
        let device = self.host.default_output_device().ok_or(AudioError::NoOutputDevice)?;
        let (config, format) = output_config(&device, self.system, self.buffer_request)?;
        self.device = device;
        self.config = config;
        self.format = format;
        self.last_default_poll = None;
        self.restart_stream()?;
        Ok(self.current_device_name())
    }

    /// Starts the processor's stream again on the current device. Without a
    /// processor there's no stream to start: one is only ever wanted after
    /// [`start_with_processor`](Self::start_with_processor).
    fn restart_stream(&mut self) -> Result<(), AudioError> {
        let Some(processor) = self.processor.clone() else { return Ok(()) };
        // The old stream is gone, so this lock is uncontended
        if let Some(mut proc) = lock_processor(&processor) {
            proc.set_sample_rate(self.config.sample_rate as f32);
        }
        self.build_processor_stream(processor)
    }

    /// The output devices to choose from, in menu order.
    fn output_device_list(&self) -> Vec<Device> {
        match self.system {
            AudioSystem::Asio => self.asio_devices.clone(),
            AudioSystem::System => self.host.output_devices().map(|devices| devices.collect()).unwrap_or_default(),
        }
    }

    /// Get information about all available output devices.
    pub fn enumerate_devices(&self) -> Vec<DeviceInfo> {
        // ASIO has no default: its first driver opens first
        let default_name = match self.system {
            AudioSystem::System => self.host.default_output_device().and_then(|d| device_name(&d)),
            AudioSystem::Asio => self.asio_devices.first().and_then(device_name),
        };
        self.output_device_list()
            .iter()
            .enumerate()
            .filter_map(|(index, device)| {
                device_name(device).map(|name| DeviceInfo {
                    is_default: Some(&name) == default_name.as_ref(),
                    name,
                    index,
                })
            })
            .collect()
    }

    /// Get the name of the currently selected device.
    pub fn current_device_name(&self) -> String {
        device_name(&self.device).unwrap_or_else(|| "Unknown".to_string())
    }

    /// The current device's index in [`enumerate_devices`](Self::enumerate_devices).
    pub fn current_device_index(&self) -> Option<usize> {
        let name = device_name(&self.device)?;
        self.enumerate_devices().into_iter().find(|info| info.name == name).map(|info| info.index)
    }

    /// Select a different output device by index.
    ///
    /// If a stream was running it is rebuilt on the new device. When the
    /// engine is driving an `AudioProcessor`, the same processor (and so the
    /// whole patch) moves to the new device, re-prepared at its sample rate.
    /// On ASIO the input closes: its driver is the output's.
    pub fn select_device(&mut self, index: usize) -> Result<(), AudioError> {
        let device = self.output_device_list().into_iter().nth(index).ok_or(AudioError::NoOutputDevice)?;
        let (config, format) = output_config(&device, self.system, self.buffer_request)?;

        let was_running = self.wanted;
        if self.system == AudioSystem::Asio {
            // The old driver must unload before the new one can load
            self.close_input();
        }
        self.drop_stream();
        self.device = device;
        self.follows_default = false;
        self.config = config;
        self.format = format;
        if was_running {
            self.restart_stream()?;
        }
        Ok(())
    }

    /// Get information about all available input devices. On ASIO, that's
    /// the output's own driver, if it has inputs: one driver serves both.
    pub fn enumerate_input_devices(&self) -> Vec<DeviceInfo> {
        if self.system == AudioSystem::Asio {
            let has_inputs = self.device.default_input_config().is_ok_and(|config| config.channels() > 0);
            return device_name(&self.device)
                .filter(|_| has_inputs)
                .map(|name| DeviceInfo { name, is_default: true, index: 0 })
                .into_iter()
                .collect();
        }
        let default_name = self.host.default_input_device().and_then(|d| device_name(&d));
        self.host
            .input_devices()
            .map(|devices| {
                devices
                    .enumerate()
                    .filter_map(|(index, device)| {
                        device_name(&device).map(|name| DeviceInfo {
                            is_default: Some(&name) == default_name.as_ref(),
                            name,
                            index,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Opens input device `index`, closing any input already open. Returns
    /// the feed to hand to the audio thread (see
    /// [`UiHandle::connect_input`](super::UiHandle::connect_input)) and a
    /// monitor for the UI.
    ///
    /// The device runs at the output's sample rate if it can. If it can't
    /// (Windows often sets a microphone to 48 kHz and speakers to 44.1 kHz),
    /// it runs at its own and the input is converted on the way in.
    pub fn open_input(&mut self, index: usize) -> Result<(InputFeed, InputMonitor), AudioError> {
        self.close_input();
        if self.system == AudioSystem::Asio {
            return self.open_asio_input(index);
        }
        let device = self
            .host
            .input_devices()
            .map_err(|e| AudioError::ConfigurationFailed(e.to_string()))?
            .nth(index)
            .ok_or(AudioError::NoInputDevice)?;
        let name = device_name(&device).unwrap_or_else(|| "Unknown".to_string());

        let rate = self.config.sample_rate;
        let supported: Vec<_> = device
            .supported_input_configs()
            .map_err(|e| AudioError::ConfigurationFailed(e.to_string()))?
            .collect();
        // The richest format the device offers at our rate, in stereo if it can
        let format_rank = |format: SampleFormat| match format {
            SampleFormat::F32 => Some(0),
            SampleFormat::I32 => Some(1),
            SampleFormat::I16 => Some(2),
            SampleFormat::U16 => Some(3),
            _ => None,
        };
        let channel_rank = |channels: u16| match channels {
            2 => 0,
            1 => 1,
            n => n,
        };
        let at_output_rate = supported
            .iter()
            .filter(|range| range.min_sample_rate() <= rate && rate <= range.max_sample_rate())
            .filter_map(|range| format_rank(range.sample_format()).map(|rank| (rank, channel_rank(range.channels()), range)))
            .min_by_key(|&(rank, channels, _)| (rank, channels))
            .map(|(_, _, range)| (*range).with_sample_rate(rate));
        // Otherwise the device's own settings, converted on the way in
        let supported_config = match at_output_rate {
            Some(config) => config,
            None => device
                .default_input_config()
                .map_err(|e| AudioError::ConfigurationFailed(e.to_string()))?,
        };
        if format_rank(supported_config.sample_format()).is_none() {
            return Err(AudioError::ConfigurationFailed(format!(
                "{} records {:?} samples, which Soba can't read",
                name,
                supported_config.sample_format()
            )));
        }

        let config = StreamConfig {
            channels: supported_config.channels(),
            sample_rate: supported_config.sample_rate(),
            buffer_size: cpal::BufferSize::Default,
        };
        let (sender, feed, monitor) = input_channel_converting(config.sample_rate, rate);
        let stream = build_input_stream(supported_config.sample_format(), &device, &config, sender, monitor.clone())?;
        stream.play().map_err(|e| AudioError::StreamPlaybackFailed(e.to_string()))?;

        self.input = Some(OpenInput {
            _stream: stream,
            index,
            name,
            channels: config.channels,
            sample_rate: config.sample_rate,
        });
        Ok((feed, monitor))
    }

    /// Opens the inputs of the output's ASIO driver (input `index` 0, the
    /// only one listed): on its clock, at its rate and buffer size, through
    /// a jitter buffer that holds one buffer.
    fn open_asio_input(&mut self, index: usize) -> Result<(InputFeed, InputMonitor), AudioError> {
        if index != 0 {
            return Err(AudioError::NoInputDevice);
        }
        let device = self.device.clone();
        let name = device_name(&device).unwrap_or_else(|| "Unknown".to_string());
        let supported = device
            .default_input_config()
            .map_err(|e| AudioError::ConfigurationFailed(e.to_string()))?;
        let config = StreamConfig {
            // The patch hears the first two
            channels: supported.channels().min(2),
            sample_rate: self.config.sample_rate,
            buffer_size: self.buffer_frames().map_or(self.config.buffer_size, BufferSize::Fixed),
        };
        // ASIO runs its streams' callbacks in the order they were opened, at
        // each buffer switch. The output's is opened again below, behind
        // this one, so each output reads the input that arrived just before
        let (sender, feed, monitor) = input_channel_same_clock(config.sample_rate, true);
        let stream = build_input_stream(supported.sample_format(), &device, &config, sender, monitor.clone())?;
        stream.play().map_err(|e| AudioError::StreamPlaybackFailed(e.to_string()))?;
        self.input = Some(OpenInput {
            _stream: stream,
            index,
            name,
            channels: config.channels,
            sample_rate: config.sample_rate,
        });
        if let (Some(processor), true) = (self.processor.clone(), self.stream.is_some()) {
            // Dropping the stream only removes its callback: the driver, and
            // its buffers, stay with the input's stream
            self.stream = None;
            self.build_processor_stream(processor)?;
        }
        Ok((feed, monitor))
    }

    /// Closes the input device, if one is open. Its feed stays with the
    /// audio thread, silent, until disconnected.
    pub fn close_input(&mut self) {
        self.input = None;
    }

    /// The open input device's index in
    /// [`enumerate_input_devices`](Self::enumerate_input_devices).
    pub fn input_index(&self) -> Option<usize> {
        self.input.as_ref().map(|input| input.index)
    }

    /// The open input device's name.
    pub fn input_name(&self) -> Option<&str> {
        self.input.as_ref().map(|input| input.name.as_str())
    }

    /// The open input device's channel count.
    pub fn input_channels(&self) -> Option<u16> {
        self.input.as_ref().map(|input| input.channels)
    }

    /// The open input device's sample rate. When it differs from the
    /// output's, the input is converted.
    pub fn input_sample_rate(&self) -> Option<u32> {
        self.input.as_ref().map(|input| input.sample_rate)
    }

    /// Get the current stream configuration.
    pub fn config(&self) -> &StreamConfig {
        &self.config
    }

    /// Get the sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.config.sample_rate
    }

    /// Get the number of output channels.
    pub fn channels(&self) -> u16 {
        self.config.channels
    }

    /// Stop the audio stream. It always stops, even on a device that has
    /// gone (whose stream refuses to pause), so this never fails.
    pub fn stop(&mut self) -> Result<(), AudioError> {
        self.wanted = false;
        self.drop_stream();
        Ok(())
    }

    /// Closes the stream, if one is open, without forgetting that one is
    /// wanted: for the device and setting changes that rebuild it.
    fn drop_stream(&mut self) {
        if let Some(stream) = self.stream.take() {
            // A lost device's stream can't pause (on Windows Audio its worker
            // has already exited), but dropping it closes it all the same
            let _ = stream.pause();
        }
    }

    /// Check if an audio stream has been started (it may since have failed:
    /// see [`stream_failed`](Self::stream_failed) and
    /// [`callback_count`](Self::callback_count)).
    pub fn is_running(&self) -> bool {
        self.stream.is_some()
    }

    /// Whether a stream should be running but isn't: rebuilding it on a
    /// device or setting change failed. Choosing the device again retries.
    pub fn stream_lost(&self) -> bool {
        self.wanted && self.stream.is_none()
    }

    /// How many times the device has asked for audio. It stops climbing when
    /// the device stops taking audio.
    pub fn callback_count(&self) -> u64 {
        self.state.callbacks.load(Ordering::Relaxed)
    }

    /// The output device's own delay, from callback to playback, or `None`
    /// if it doesn't report timestamps (or the graph stream hasn't run yet).
    pub fn output_latency(&self) -> Option<Duration> {
        self.state.output_latency.get()
    }

    /// Glitches the output device has reported since the engine started.
    pub fn output_xruns(&self) -> u64 {
        self.state.xruns.load(Ordering::Relaxed)
    }

    /// Whether the stream has reported an error since it was started.
    pub fn stream_failed(&self) -> bool {
        self.state.stream_failed.load(Ordering::Relaxed)
    }

    /// Start the audio stream with an AudioProcessor for graph-based synthesis.
    ///
    /// The AudioProcessor is moved into the audio callback where it processes
    /// the audio graph and produces output. The processor is wrapped in a Mutex
    /// to allow safe access from the audio callback.
    pub fn start_with_processor(&mut self, processor: AudioProcessor) -> Result<(), AudioError> {
        if self.stream.is_some() {
            return Ok(());
        }

        // Wrap processor in Mutex for the callback
        // Note: In practice, the Mutex is uncontested since only the audio
        // callback accesses it, so there's no actual blocking.
        let processor = Arc::new(Mutex::new(processor));
        self.processor = Some(Arc::clone(&processor));
        self.build_processor_stream(processor)
    }

    /// Stops the device stream but keeps its processor, re-prepared at
    /// `sample_rate`, for [`render_offline`](Self::render_offline) to drive
    /// by hand.
    pub fn go_offline(&mut self, sample_rate: f32) -> Result<(), AudioError> {
        self.stop()?;
        if let Some(mut processor) = self.processor.as_deref().and_then(lock_processor) {
            processor.set_sample_rate(sample_rate);
        }
        Ok(())
    }

    /// Renders one buffer through the processor with the given MIDI and
    /// audio input, after [`go_offline`](Self::go_offline). Returns false
    /// with no processor.
    pub fn render_offline(&self, output: &mut [f32], channels: usize, midi: &mut [MidiEvent], input: InputAudio<'_>) -> bool {
        match self.processor.as_deref().and_then(lock_processor) {
            Some(mut processor) => {
                processor.process_offline(output, channels, midi, input);
                true
            }
            _ => false,
        }
    }

    /// Builds and starts a stream on the current device that runs `processor`.
    fn build_processor_stream(
        &mut self,
        processor: Arc<Mutex<AudioProcessor>>,
    ) -> Result<(), AudioError> {
        self.state.reset(self.follows_default);
        let stream = match self.build_output_stream(Arc::clone(&processor)) {
            // Some drivers only run at certain sizes: let this one choose
            Err(e) if matches!(self.config.buffer_size, BufferSize::Fixed(_)) && e.kind() == cpal::ErrorKind::UnsupportedConfig => {
                self.config.buffer_size = BufferSize::Default;
                self.build_output_stream(processor)
            }
            built => built,
        }
        .map_err(|e| stream_error(e, &self.device, self.system))?;

        stream
            .play()
            .map_err(|e| AudioError::StreamPlaybackFailed(e.to_string()))?;

        self.stream = Some(stream);
        self.wanted = true;
        Ok(())
    }

    /// Builds a stream that runs `processor`, in the device's sample format.
    fn build_output_stream(&self, processor: Arc<Mutex<AudioProcessor>>) -> Result<Stream, cpal::Error> {
        match self.format {
            SampleFormat::I32 => self.build_output::<i32>(processor),
            SampleFormat::I24 => self.build_output::<cpal::I24>(processor),
            SampleFormat::I16 => self.build_output::<i16>(processor),
            SampleFormat::F64 => self.build_output::<f64>(processor),
            _ => self.build_output::<f32>(processor),
        }
    }

    /// Builds a stream that runs `processor`, writing samples of type `T`.
    fn build_output<T>(&self, processor: Arc<Mutex<AudioProcessor>>) -> Result<Stream, cpal::Error>
    where
        T: SizedSample + FromSample<f32>,
    {
        let channels = self.config.channels as usize;
        let state = Arc::clone(&self.state);
        let error_state = Arc::clone(&self.state);
        // Rendered here, then converted: room for the largest buffer the
        // device might ask for
        let mut scratch = vec![0.0f32; largest_buffer(&self.device) * channels.max(1)];
        self.device.build_output_stream(
            self.config,
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                state.callbacks.fetch_add(1, Ordering::Relaxed);
                let time = info.timestamp();
                state.output_latency.record(time.playback.checked_duration_since(time.callback));
                // Link times are when sound reaches the speakers
                #[cfg(feature = "ableton-link")]
                crate::ableton_link::set_output_latency(time.playback.checked_duration_since(time.callback));
                // REAL-TIME SAFE: the lock is only ever taken here, or by
                // the engine while no stream is running, so try_lock never
                // fails in practice. If it did, output silence rather than
                // wait.
                match processor.try_lock() {
                    Ok(mut proc) => proc.process_into(data, &mut scratch, channels),
                    Err(_) => data.fill(T::EQUILIBRIUM),
                }
            },
            move |err| error_state.take_error(&err),
            None,
        )
    }
}

/// The ASIO drivers that will start, if `system` is ASIO. Each is loaded in
/// turn to list it, so this runs while none is in use.
fn list_asio_devices(host: &Host, system: AudioSystem) -> Vec<Device> {
    match system {
        AudioSystem::Asio => host.output_devices().map(|devices| devices.collect()).unwrap_or_default(),
        AudioSystem::System => Vec::new(),
    }
}

/// The output device a system opens on: the system's default, or the first
/// ASIO driver that will start.
fn default_output_device(host: &Host, system: AudioSystem, asio_devices: &[Device]) -> Result<Device, AudioError> {
    match system {
        AudioSystem::System => host.default_output_device().ok_or(AudioError::NoOutputDevice),
        AudioSystem::Asio => asio_devices.first().cloned().ok_or(AudioError::NoAsioDriver),
    }
}

/// The stream config and sample format to run `device` at, asking for
/// `buffer` frames per callback on systems that let Soba choose.
fn output_config(device: &Device, system: AudioSystem, buffer: Option<u32>) -> Result<(StreamConfig, SampleFormat), AudioError> {
    let supported = device
        .default_output_config()
        .map_err(|e| AudioError::ConfigurationFailed(e.to_string()))?;
    let buffer_size = match (system, buffer, supported.buffer_size()) {
        (AudioSystem::Asio, Some(frames), SupportedBufferSize::Range { min, max }) => BufferSize::Fixed(frames.clamp(*min, *max)),
        _ if cfg!(target_arch = "wasm32") => web_buffer(),
        _ => BufferSize::Default,
    };
    let channels = match system {
        // An interface's first pair: Soba plays stereo
        AudioSystem::Asio => supported.channels().min(2),
        // Shared mode runs at the device's own layout
        AudioSystem::System => supported.channels(),
    };
    // Some ASIO drivers don't know their rate until they run
    let sample_rate = match supported.sample_rate() {
        0 => 48_000,
        rate => rate,
    };
    Ok((StreamConfig { channels, sample_rate, buffer_size }, supported.sample_format()))
}

/// The most frames a callback on `device` might ask for, for a buffer
/// that has to hold one.
fn largest_buffer(device: &Device) -> usize {
    match device.default_output_config().map(|config| *config.buffer_size()) {
        Ok(SupportedBufferSize::Range { max, .. }) => (max as usize).clamp(4096, 16384),
        _ => 8192,
    }
}

/// The processor's lock, from the UI's side. A threaded browser build's
/// audio thread can still be finishing a buffer for a stream just dropped
/// (its AudioContext closes a moment later), and the page's own thread may
/// not wait on a lock, which browsers forbid: it spins for that moment
/// instead.
fn lock_processor(processor: &Mutex<AudioProcessor>) -> Option<MutexGuard<'_, AudioProcessor>> {
    #[cfg(target_feature = "atomics")]
    loop {
        match processor.try_lock() {
            Ok(guard) => return Some(guard),
            Err(std::sync::TryLockError::WouldBlock) => std::hint::spin_loop(),
            Err(std::sync::TryLockError::Poisoned(_)) => return None,
        }
    }
    #[cfg(not(target_feature = "atomics"))]
    processor.lock().ok()
}

/// Explains a stream that wouldn't build.
fn stream_error(err: cpal::Error, device: &Device, system: AudioSystem) -> AudioError {
    match (system, err.kind()) {
        (AudioSystem::Asio, cpal::ErrorKind::DeviceBusy | cpal::ErrorKind::DeviceNotAvailable) => {
            AudioError::DriverUnavailable(device_name(device).unwrap_or_else(|| "The ASIO driver".to_string()))
        }
        _ => AudioError::StreamCreationFailed(err.to_string()),
    }
}

/// Whether a stream error is a passing glitch the stream carries on from
/// (it dropped or repeated a buffer, or the OS wouldn't raise the audio
/// thread's priority) rather than the end of the stream.
fn is_glitch(err: &cpal::Error) -> bool {
    matches!(err.kind(), cpal::ErrorKind::Xrun | cpal::ErrorKind::RealtimeDenied)
}

/// A device's human-readable name, or `None` if it can't say.
fn device_name(device: &Device) -> Option<String> {
    device.description().ok().map(|description| description.name().to_string())
}

/// Builds a stream that pushes `device`'s input, in samples of `format`,
/// into `sender`.
fn build_input_stream(
    format: SampleFormat,
    device: &Device,
    config: &StreamConfig,
    sender: InputSender,
    monitor: InputMonitor,
) -> Result<Stream, AudioError> {
    match format {
        SampleFormat::I32 => build_input_stream_of::<i32>(device, config, sender, monitor),
        SampleFormat::I24 => build_input_stream_of::<cpal::I24>(device, config, sender, monitor),
        SampleFormat::I16 => build_input_stream_of::<i16>(device, config, sender, monitor),
        SampleFormat::U16 => build_input_stream_of::<u16>(device, config, sender, monitor),
        SampleFormat::F64 => build_input_stream_of::<f64>(device, config, sender, monitor),
        _ => build_input_stream_of::<f32>(device, config, sender, monitor),
    }
}

/// Builds a stream that pushes `device`'s input, in samples of type `T`,
/// into `sender`.
fn build_input_stream_of<T>(
    device: &Device,
    config: &StreamConfig,
    mut sender: InputSender,
    monitor: InputMonitor,
) -> Result<Stream, AudioError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = config.channels as usize;
    device
        .build_input_stream(
            *config,
            move |data: &[T], info: &cpal::InputCallbackInfo| {
                // REAL-TIME SAFE: a copy into the ring and an atomic store
                let time = info.timestamp();
                sender.record_latency(time.callback.checked_duration_since(time.capture));
                sender.push(data, channels, |sample| sample.to_sample::<f32>());
            },
            move |err| {
                if is_glitch(&err) {
                    monitor.mark_xrun();
                } else {
                    eprintln!("Audio input error: {}", err);
                    monitor.mark_failed();
                }
            },
            None,
        )
        .map_err(|e| AudioError::StreamCreationFailed(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_error_display() {
        let err = AudioError::NoOutputDevice;
        assert_eq!(err.to_string(), "No audio output device found");

        let err = AudioError::StreamCreationFailed("test error".to_string());
        assert!(err.to_string().contains("test error"));
    }

    #[test]
    fn test_device_info() {
        let info = DeviceInfo {
            name: "Test Device".to_string(),
            is_default: true,
            index: 0,
        };
        assert_eq!(info.name, "Test Device");
        assert!(info.is_default);
        assert_eq!(info.index, 0);
    }

    /// Only a stream that follows the default output takes an invalidated
    /// stream for the default moving: one on a chosen device (or an ASIO
    /// driver) was reset by its driver (#337).
    #[test]
    fn an_invalidated_stream_is_a_moved_default_only_when_it_follows_one() {
        use cpal::ErrorKind;
        assert_eq!(stop_reason(ErrorKind::StreamInvalidated, true), STOPPED_DEFAULT_CHANGED);
        assert_eq!(stop_reason(ErrorKind::StreamInvalidated, false), STOPPED_RESET);
        assert_eq!(stop_reason(ErrorKind::DeviceNotAvailable, true), STOPPED_LOST);
        assert_eq!(stop_reason(ErrorKind::DeviceNotAvailable, false), STOPPED_LOST);
    }

    /// Starts `engine` on an empty patch: a stream that plays silence.
    fn start_silent(engine: &mut AudioEngine) {
        let (_ui, handle) = crate::engine::EngineChannels::with_defaults().split();
        let processor = AudioProcessor::new(engine.sample_rate() as f32, crate::engine::BLOCK_SIZE, handle);
        engine.start_with_processor(processor).unwrap();
    }

    /// Choosing the device again restarts the stream after a rebuild failed,
    /// and only `stop` forgets it was wanted (#312). Needs an output device:
    /// `cargo test --lib reselecting -- --ignored`
    #[test]
    #[ignore]
    fn reselecting_the_device_restarts_a_lost_stream() {
        let mut engine = AudioEngine::new().expect("an output device");
        let index = engine.current_device_index().expect("the device is listed");
        start_silent(&mut engine);
        // As a rebuild that failed leaves it: no stream, but one wanted
        engine.drop_stream();
        assert!(engine.stream_lost());
        engine.select_device(index).unwrap();
        assert!(engine.is_running() && !engine.stream_lost());
        engine.stop().unwrap();
        engine.select_device(index).unwrap();
        assert!(!engine.is_running() && !engine.stream_lost());
    }

    /// A stream on the default output moves to the default again, keeping
    /// the buffer asked for, and one on a chosen device doesn't follow it
    /// (#337). Needs an output device: `cargo test --lib follow -- --ignored`
    #[test]
    #[ignore]
    fn following_the_default_rebuilds_the_stream_there() {
        let mut engine = AudioEngine::new().expect("an output device");
        assert!(engine.follows_default);
        start_silent(&mut engine);
        // As the default going away with none to take its place leaves it
        engine.drop_stream();
        assert!(engine.awaiting_default() && engine.default_returned());
        // Asked once a second, not every frame
        assert!(!engine.default_returned());
        engine.follow_default().unwrap();
        assert!(engine.is_running() && !engine.awaiting_default() && !engine.default_changed());
        let index = engine.current_device_index().expect("the device is listed");
        engine.select_device(index).unwrap();
        engine.drop_stream();
        assert!(!engine.awaiting_default());
        engine.stop().unwrap();
    }
}
