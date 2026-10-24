//! MIDI Engine
//!
//! Handles MIDI input from hardware controllers and virtual MIDI ports.
//! Uses midir for cross-platform MIDI access and rtrb for lock-free
//! communication with the audio thread.
//!
//! Every incoming message is stamped with the moment it arrived and sent two
//! ways: to the audio thread, which places it at the matching sample (see
//! [`MidiScheduler`](super::MidiScheduler)), and to the UI, for display,
//! MIDI Learn and CC mappings.
//!
//! In the browser the devices come from Web MIDI ([`super::web_midi`]) once
//! Connect MIDI is allowed, and arrive in the same queues.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use web_time::Instant;

use midir::{MidiInput, MidiInputPort};
#[cfg(not(target_arch = "wasm32"))]
use midir::MidiInputConnection;
use rtrb::{Consumer, Producer, RingBuffer};

/// Default buffer size for MIDI events.
pub const DEFAULT_MIDI_BUFFER_SIZE: usize = 512;

/// Information about a MIDI input device.
#[derive(Debug, Clone)]
pub struct MidiDeviceInfo {
    /// Human-readable device name.
    pub name: String,
    /// Internal port index.
    pub index: usize,
}

/// MIDI event types received from hardware.
#[derive(Debug, Clone, Copy)]
pub enum MidiEvent {
    /// Note On event.
    NoteOn {
        /// MIDI channel (0-15).
        channel: u8,
        /// Note number (0-127).
        note: u8,
        /// Velocity (0-127).
        velocity: u8,
    },
    /// Note Off event.
    NoteOff {
        /// MIDI channel (0-15).
        channel: u8,
        /// Note number (0-127).
        note: u8,
        /// Velocity (0-127, often ignored).
        velocity: u8,
    },
    /// Control Change (CC) event.
    ControlChange {
        /// MIDI channel (0-15).
        channel: u8,
        /// Controller number (0-127).
        controller: u8,
        /// Controller value (0-127).
        value: u8,
    },
    /// Pitch Bend event.
    PitchBend {
        /// MIDI channel (0-15).
        channel: u8,
        /// Pitch bend value (-8192 to 8191, center = 0).
        value: i16,
    },
    /// Channel Aftertouch (pressure).
    ChannelPressure {
        /// MIDI channel (0-15).
        channel: u8,
        /// Pressure value (0-127).
        pressure: u8,
    },
    /// Polyphonic Aftertouch (per-note pressure).
    PolyPressure {
        /// MIDI channel (0-15).
        channel: u8,
        /// Note number (0-127).
        note: u8,
        /// Pressure value (0-127).
        pressure: u8,
    },
    /// Program Change.
    ProgramChange {
        /// MIDI channel (0-15).
        channel: u8,
        /// Program number (0-127).
        program: u8,
    },
    /// Timing Clock (0xF8), 24 per beat from a clock master.
    Clock,
    /// Start (0xFA).
    Start,
    /// Continue (0xFB).
    Continue,
    /// Stop (0xFC).
    Stop,
    /// Song Position Pointer (0xF2), in sixteenth notes from the top.
    SongPosition {
        /// Position (0-16383).
        sixteenths: u16,
    },
}

impl MidiEvent {
    /// Parse a MIDI event from raw bytes.
    /// Returns None for unsupported or malformed messages.
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.is_empty() {
            return None;
        }

        let status = data[0];
        if status >= 0xF0 {
            return Self::from_system_bytes(data);
        }
        let channel = status & 0x0F;
        let msg_type = status & 0xF0;

        match msg_type {
            0x90 => {
                // Note On (velocity 0 = Note Off)
                if data.len() >= 3 {
                    let note = data[1] & 0x7F;
                    let velocity = data[2] & 0x7F;
                    if velocity == 0 {
                        Some(MidiEvent::NoteOff {
                            channel,
                            note,
                            velocity: 0,
                        })
                    } else {
                        Some(MidiEvent::NoteOn {
                            channel,
                            note,
                            velocity,
                        })
                    }
                } else {
                    None
                }
            }
            0x80 => {
                // Note Off
                if data.len() >= 3 {
                    Some(MidiEvent::NoteOff {
                        channel,
                        note: data[1] & 0x7F,
                        velocity: data[2] & 0x7F,
                    })
                } else {
                    None
                }
            }
            0xB0 => {
                // Control Change
                if data.len() >= 3 {
                    Some(MidiEvent::ControlChange {
                        channel,
                        controller: data[1] & 0x7F,
                        value: data[2] & 0x7F,
                    })
                } else {
                    None
                }
            }
            0xE0 => {
                // Pitch Bend
                if data.len() >= 3 {
                    let lsb = data[1] as i16;
                    let msb = data[2] as i16;
                    // Pitch bend is 14-bit, centered at 8192
                    let value = ((msb << 7) | lsb) - 8192;
                    Some(MidiEvent::PitchBend { channel, value })
                } else {
                    None
                }
            }
            0xD0 => {
                // Channel Aftertouch
                if data.len() >= 2 {
                    Some(MidiEvent::ChannelPressure {
                        channel,
                        pressure: data[1] & 0x7F,
                    })
                } else {
                    None
                }
            }
            0xA0 => {
                // Poly Aftertouch
                if data.len() >= 3 {
                    Some(MidiEvent::PolyPressure {
                        channel,
                        note: data[1] & 0x7F,
                        pressure: data[2] & 0x7F,
                    })
                } else {
                    None
                }
            }
            0xC0 => {
                // Program Change
                if data.len() >= 2 {
                    Some(MidiEvent::ProgramChange {
                        channel,
                        program: data[1] & 0x7F,
                    })
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Parses the system messages modules use: the real-time messages of a
    /// MIDI clock, and Song Position.
    fn from_system_bytes(data: &[u8]) -> Option<Self> {
        match data[0] {
            0xF8 => Some(MidiEvent::Clock),
            0xFA => Some(MidiEvent::Start),
            0xFB => Some(MidiEvent::Continue),
            0xFC => Some(MidiEvent::Stop),
            0xF2 if data.len() >= 3 => {
                let sixteenths = (data[1] as u16 & 0x7F) | ((data[2] as u16 & 0x7F) << 7);
                Some(MidiEvent::SongPosition { sixteenths })
            }
            _ => None,
        }
    }

    /// The audio-thread form of this event, placed at `sample_offset`.
    /// Every message a module could use has one, so it's always `Some`.
    pub fn to_dsp(&self, sample_offset: u32) -> Option<crate::dsp::MidiEvent> {
        use crate::dsp::MidiMessage;
        let (channel, message) = match *self {
            MidiEvent::NoteOn { channel, note, velocity } => {
                (channel, MidiMessage::NoteOn { note, velocity })
            }
            MidiEvent::NoteOff { channel, note, velocity } => {
                (channel, MidiMessage::NoteOff { note, velocity })
            }
            MidiEvent::ControlChange { channel, controller, value } => {
                (channel, MidiMessage::ControlChange { controller, value })
            }
            MidiEvent::PitchBend { channel, value } => (channel, MidiMessage::PitchBend { value }),
            MidiEvent::ChannelPressure { channel, pressure } => {
                (channel, MidiMessage::Aftertouch { pressure })
            }
            MidiEvent::ProgramChange { channel, program } => {
                (channel, MidiMessage::ProgramChange { program })
            }
            MidiEvent::PolyPressure { channel, note, pressure } => {
                (channel, MidiMessage::PolyPressure { note, pressure })
            }
            MidiEvent::Clock => (0, MidiMessage::Clock),
            MidiEvent::Start => (0, MidiMessage::Start),
            MidiEvent::Continue => (0, MidiMessage::Continue),
            MidiEvent::Stop => (0, MidiMessage::Stop),
            MidiEvent::SongPosition { sixteenths } => (0, MidiMessage::SongPosition { sixteenths }),
        };
        Some(crate::dsp::MidiEvent::new(sample_offset, channel, message))
    }

    /// Get the MIDI channel for this event. System messages have none, and
    /// report channel 0.
    pub fn channel(&self) -> u8 {
        match self {
            MidiEvent::NoteOn { channel, .. } => *channel,
            MidiEvent::NoteOff { channel, .. } => *channel,
            MidiEvent::ControlChange { channel, .. } => *channel,
            MidiEvent::PitchBend { channel, .. } => *channel,
            MidiEvent::ChannelPressure { channel, .. } => *channel,
            MidiEvent::PolyPressure { channel, .. } => *channel,
            MidiEvent::ProgramChange { channel, .. } => *channel,
            MidiEvent::Clock
            | MidiEvent::Start
            | MidiEvent::Continue
            | MidiEvent::Stop
            | MidiEvent::SongPosition { .. } => 0,
        }
    }
}

/// MIDI event with timestamp for sample-accurate playback.
#[derive(Debug, Clone, Copy)]
pub struct TimestampedMidiEvent {
    /// The MIDI event.
    pub event: MidiEvent,
    /// When the event arrived, on the same clock the audio callback reads.
    pub received: Instant,
}

impl TimestampedMidiEvent {
    /// Stamps `event` as arriving now.
    pub fn now(event: MidiEvent) -> Self {
        Self { event, received: Instant::now() }
    }
}

/// The receiving ends of the MIDI event queues.
pub struct MidiReceivers {
    /// For the audio thread, to hand to the
    /// [`AudioProcessor`](super::AudioProcessor).
    pub audio: Consumer<TimestampedMidiEvent>,
    /// For the UI: monitor display, piano, MIDI Learn and CC mappings.
    pub ui: Consumer<TimestampedMidiEvent>,
}

/// The sending ends, shared with the midir callback.
struct MidiSenders {
    audio: Producer<TimestampedMidiEvent>,
    ui: Producer<TimestampedMidiEvent>,
    /// Wakes the UI for what it's sent, which it reads only when it draws
    /// and it doesn't draw while nothing moves (#307)
    waker: Option<eframe::egui::Context>,
}

impl MidiSenders {
    /// Sends an event both ways. Lossy: a full queue drops it rather than
    /// hold up MIDI input.
    ///
    /// Clock ticks go to the audio thread only. They come 24 to the beat,
    /// and the UI has no use for them; it would only scroll them through
    /// the MIDI Monitor.
    fn send(&mut self, event: TimestampedMidiEvent) {
        let _ = self.audio.push(event);
        if !matches!(event.event, MidiEvent::Clock) {
            let _ = self.ui.push(event);
            if let Some(waker) = &self.waker {
                waker.request_repaint();
            }
        }
    }
}

/// CC 123, All Notes Off.
pub const ALL_NOTES_OFF: u8 = 123;

/// Error type for MIDI operations.
#[derive(Debug)]
pub enum MidiError {
    /// Failed to initialize MIDI subsystem.
    InitError(String),
    /// Failed to connect to device.
    ConnectionError(String),
    /// Device not found.
    DeviceNotFound,
    /// No MIDI devices available.
    NoDevices,
}

impl std::fmt::Display for MidiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MidiError::InitError(s) => write!(f, "MIDI init error: {}", s),
            MidiError::ConnectionError(s) => write!(f, "MIDI connection error: {}", s),
            MidiError::DeviceNotFound => write!(f, "MIDI device not found"),
            MidiError::NoDevices => write!(f, "No MIDI devices available"),
        }
    }
}

impl std::error::Error for MidiError {}

/// A device being listened to: midir's on the desktop, Web MIDI's in the browser.
#[cfg(not(target_arch = "wasm32"))]
type InputConnection = MidiInputConnection<()>;
#[cfg(target_arch = "wasm32")]
type InputConnection = super::web_midi::InputConnection;

/// MIDI engine state shared between threads.
struct MidiState {
    /// Currently available ports (refreshed periodically). The browser
    /// opens its inputs by name and keeps none.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    ports: Vec<MidiInputPort>,
    /// Port names for UI display.
    port_names: Vec<String>,
}

/// MIDI engine for receiving MIDI input.
pub struct MidiEngine {
    /// Cached device list.
    devices: Vec<MidiDeviceInfo>,
    /// Currently selected device index (None = no device).
    selected_device: Option<usize>,
    /// Active MIDI connection.
    connection: Option<InputConnection>,
    /// Producers for sending events to the audio thread and the UI. Shared
    /// with the midir callback; each connection clones the Arc, and closing
    /// the connection drops that clone, so the producers outlive any one
    /// device. Only MIDI input and UI threads lock this, never audio.
    senders: Arc<Mutex<MidiSenders>>,
    /// Shared state for device enumeration.
    state: Arc<Mutex<MidiState>>,
    /// Flag to signal device scan thread to stop.
    scan_running: Arc<AtomicBool>,
    /// Handle for the device scan thread.
    scan_thread: Option<thread::JoinHandle<()>>,
}

impl MidiEngine {
    /// Create a new MIDI engine.
    ///
    /// Returns the engine and the receiving ends of its event queues.
    pub fn new() -> Result<(Self, MidiReceivers), MidiError> {
        // One queue to the audio thread, one to the UI
        let (audio_producer, audio_consumer) = RingBuffer::new(DEFAULT_MIDI_BUFFER_SIZE);
        let (ui_producer, ui_consumer) = RingBuffer::new(DEFAULT_MIDI_BUFFER_SIZE);

        // Get initial port list. Web MIDI needs asking for: in the browser
        // there are no devices until Connect MIDI (see `enumerate_devices`),
        // and the computer keyboard plays through the queues (see `send`)
        let (ports, port_names): (Vec<MidiInputPort>, Vec<String>) = if cfg!(target_arch = "wasm32") {
            (Vec::new(), Vec::new())
        } else {
            let midi_in = MidiInput::new("Soba")
                .map_err(|e| MidiError::InitError(e.to_string()))?;
            let ports: Vec<MidiInputPort> = midi_in.ports().into_iter().collect();
            let port_names = ports
                .iter()
                .map(|p| midi_in.port_name(p).unwrap_or_else(|_| "Unknown".to_string()))
                .collect();
            (ports, port_names)
        };

        let devices: Vec<MidiDeviceInfo> = port_names
            .iter()
            .enumerate()
            .map(|(i, name)| MidiDeviceInfo {
                name: name.clone(),
                index: i,
            })
            .collect();

        let state = Arc::new(Mutex::new(MidiState { ports, port_names }));

        // Start background thread for device scanning (hot-plug detection)
        let scan_running = Arc::new(AtomicBool::new(true));
        let scan_thread = spawn_scanner(Arc::clone(&state), Arc::clone(&scan_running));

        let engine = Self {
            devices,
            selected_device: None,
            connection: None,
            senders: Arc::new(Mutex::new(MidiSenders { audio: audio_producer, ui: ui_producer, waker: None })),
            state,
            scan_running,
            scan_thread,
        };

        Ok((engine, MidiReceivers { audio: audio_consumer, ui: ui_consumer }))
    }

    /// Enumerate available MIDI input devices.
    /// This returns a fresh list reflecting any hot-plugged devices.
    pub fn enumerate_devices(&mut self) -> Vec<MidiDeviceInfo> {
        // The browser's list is Web MIDI's, kept up to date by its statechange
        #[cfg(target_arch = "wasm32")]
        if let Ok(mut state) = self.state.lock() {
            state.port_names = super::web_midi::input_names();
        }
        if let Ok(state) = self.state.lock() {
            self.devices = state
                .port_names
                .iter()
                .enumerate()
                .map(|(i, name)| MidiDeviceInfo {
                    name: name.clone(),
                    index: i,
                })
                .collect();
        }
        self.devices.clone()
    }

    /// Get the currently cached device list without rescanning.
    pub fn devices(&self) -> &[MidiDeviceInfo] {
        &self.devices
    }

    /// Get the currently selected device index.
    pub fn selected_device(&self) -> Option<usize> {
        self.selected_device
    }

    /// Connect to a MIDI device by index.
    pub fn connect(&mut self, device_index: usize) -> Result<(), MidiError> {
        // Disconnect existing connection
        self.disconnect();

        let name = self
            .devices
            .get(device_index)
            .map(|d| d.name.clone())
            .ok_or(MidiError::DeviceNotFound)?;
        let senders = Arc::clone(&self.senders);
        let on_message = move |data: &[u8]| {
            // Stamped on arrival rather than with the device's timestamp,
            // whose clock starts at connection and differs per platform;
            // the audio callback reads this same clock
            if let Some(event) = MidiEvent::from_bytes(data) {
                let stamped = TimestampedMidiEvent::now(event);
                if let Ok(mut senders) = senders.lock() {
                    senders.send(stamped);
                }
            }
        };
        let connection = self.open(&name, on_message)?;

        self.connection = Some(connection);
        self.selected_device = Some(device_index);

        eprintln!("MIDI connected to device {}: {}", device_index, name);

        Ok(())
    }

    /// Listens to the input called `name`.
    #[cfg(not(target_arch = "wasm32"))]
    fn open(&self, name: &str, mut on_message: impl FnMut(&[u8]) + Send + 'static) -> Result<InputConnection, MidiError> {
        // Resolve the port by name: the scan thread may have reordered the
        // port list since the UI's device list was cached.
        let port = {
            let state = self.state.lock().map_err(|_| {
                MidiError::ConnectionError("Failed to lock state".to_string())
            })?;

            let pos = state
                .port_names
                .iter()
                .position(|n| n == name)
                .ok_or(MidiError::DeviceNotFound)?;
            state.ports[pos].clone()
        };

        // Create a new MIDI input for this connection
        let midi_in = MidiInput::new("Soba Input")
            .map_err(|e| MidiError::InitError(e.to_string()))?;

        midi_in
            .connect(&port, "Soba Input", move |_timestamp_us, data, _| on_message(data), ())
            .map_err(|e| {
                // On Windows, a port already opened by another app (a DAW,
                // Arturia MIDI Control Center, a browser) refuses a second open.
                MidiError::ConnectionError(format!(
                    "{}: {} (is another app using it?)",
                    name, e
                ))
            })
    }

    /// Listens to the input called `name`, through Web MIDI.
    #[cfg(target_arch = "wasm32")]
    fn open(&self, name: &str, on_message: impl FnMut(&[u8]) + 'static) -> Result<InputConnection, MidiError> {
        super::web_midi::open_input(name, on_message).ok_or(MidiError::DeviceNotFound)
    }

    /// Disconnect from the current MIDI device.
    ///
    /// Notes still held on it will never get their Note Off, so the audio
    /// thread is sent All Notes Off on every channel.
    pub fn disconnect(&mut self) {
        if let Some(connection) = self.connection.take() {
            // Close the connection - this drops it
            connection.close();
            self.selected_device = None;
            if let Ok(mut senders) = self.senders.lock() {
                for channel in 0..16 {
                    let all_off = MidiEvent::ControlChange { channel, controller: ALL_NOTES_OFF, value: 0 };
                    let _ = senders.audio.push(TimestampedMidiEvent::now(all_off));
                }
            }
            eprintln!("MIDI disconnected");
        }
    }

    /// Sends an event made here rather than by a device, such as a note
    /// from the computer keyboard, as if it came in from MIDI. Lossy, like
    /// device input.
    pub fn send(&self, event: MidiEvent) {
        if let Ok(mut senders) = self.senders.lock() {
            senders.send(TimestampedMidiEvent::now(event));
        }
    }

    /// Has `ctx` drawn a frame for each event the UI is sent from now on.
    pub fn set_waker(&self, ctx: &eframe::egui::Context) {
        if let Ok(mut senders) = self.senders.lock() {
            senders.waker = Some(ctx.clone());
        }
    }

    /// Check if currently connected to a device.
    pub fn is_connected(&self) -> bool {
        self.connection.is_some()
    }
}

/// Starts the thread that rescans the MIDI ports every couple of seconds,
/// so a device plugged in later shows up (hot-plug).
#[cfg(not(target_arch = "wasm32"))]
fn spawn_scanner(state: Arc<Mutex<MidiState>>, running: Arc<AtomicBool>) -> Option<thread::JoinHandle<()>> {
    let scanner = thread::spawn(move || {
        while running.load(Ordering::Relaxed) {
            // Sleep between scans
            thread::sleep(std::time::Duration::from_secs(2));

            if !running.load(Ordering::Relaxed) {
                break;
            }

            // Rescan MIDI ports
            if let Ok(midi_in) = MidiInput::new("Soba Scanner") {
                let new_ports: Vec<MidiInputPort> = midi_in.ports().into_iter().collect();
                let new_names: Vec<String> = new_ports
                    .iter()
                    .map(|p| midi_in.port_name(p).unwrap_or_else(|_| "Unknown".to_string()))
                    .collect();

                if let Ok(mut state) = state.lock() {
                    state.ports = new_ports;
                    state.port_names = new_names;
                }
            }
        }
    });
    Some(scanner)
}

/// The browser has no threads to scan from.
#[cfg(target_arch = "wasm32")]
fn spawn_scanner(_state: Arc<Mutex<MidiState>>, _running: Arc<AtomicBool>) -> Option<thread::JoinHandle<()>> {
    None
}

impl Drop for MidiEngine {
    fn drop(&mut self) {
        // Stop the scan thread
        self.scan_running.store(false, Ordering::Relaxed);

        // Disconnect if connected
        self.disconnect();

        // Wait for scan thread to finish
        if let Some(thread) = self.scan_thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_midi_event_from_bytes_note_on() {
        let data = [0x90, 60, 100]; // Note On, channel 0, middle C, velocity 100
        let event = MidiEvent::from_bytes(&data);
        assert!(event.is_some());
        if let Some(MidiEvent::NoteOn {
            channel,
            note,
            velocity,
        }) = event
        {
            assert_eq!(channel, 0);
            assert_eq!(note, 60);
            assert_eq!(velocity, 100);
        } else {
            panic!("Expected NoteOn event");
        }
    }

    #[test]
    fn test_midi_event_from_bytes_note_off() {
        let data = [0x80, 60, 64]; // Note Off, channel 0, middle C
        let event = MidiEvent::from_bytes(&data);
        assert!(event.is_some());
        if let Some(MidiEvent::NoteOff { channel, note, .. }) = event {
            assert_eq!(channel, 0);
            assert_eq!(note, 60);
        } else {
            panic!("Expected NoteOff event");
        }
    }

    #[test]
    fn test_midi_event_from_bytes_note_on_zero_velocity() {
        // Note On with velocity 0 should be treated as Note Off
        let data = [0x90, 60, 0];
        let event = MidiEvent::from_bytes(&data);
        assert!(event.is_some());
        assert!(matches!(event, Some(MidiEvent::NoteOff { .. })));
    }

    #[test]
    fn test_midi_event_from_bytes_control_change() {
        let data = [0xB0, 1, 64]; // CC, channel 0, mod wheel, value 64
        let event = MidiEvent::from_bytes(&data);
        assert!(event.is_some());
        if let Some(MidiEvent::ControlChange {
            channel,
            controller,
            value,
        }) = event
        {
            assert_eq!(channel, 0);
            assert_eq!(controller, 1);
            assert_eq!(value, 64);
        } else {
            panic!("Expected ControlChange event");
        }
    }

    #[test]
    fn test_midi_event_from_bytes_pitch_bend() {
        // Pitch bend centered (8192 = 0x2000)
        let data = [0xE0, 0x00, 0x40]; // LSB=0, MSB=64 -> 64*128 = 8192 -> value = 0
        let event = MidiEvent::from_bytes(&data);
        assert!(event.is_some());
        if let Some(MidiEvent::PitchBend { channel, value }) = event {
            assert_eq!(channel, 0);
            assert_eq!(value, 0);
        } else {
            panic!("Expected PitchBend event");
        }
    }

    #[test]
    fn test_midi_event_from_bytes_channel() {
        // Test channel extraction
        let data = [0x95, 60, 100]; // Note On, channel 5
        let event = MidiEvent::from_bytes(&data).unwrap();
        assert_eq!(event.channel(), 5);
    }

    #[test]
    fn test_midi_event_from_bytes_empty() {
        let data: [u8; 0] = [];
        assert!(MidiEvent::from_bytes(&data).is_none());
    }

    #[test]
    fn test_midi_event_from_bytes_incomplete() {
        let data = [0x90, 60]; // Missing velocity byte
        assert!(MidiEvent::from_bytes(&data).is_none());
    }

    #[test]
    fn test_midi_event_from_bytes_program_change() {
        let data = [0xC0, 42]; // Program change, channel 0, program 42
        let event = MidiEvent::from_bytes(&data);
        assert!(event.is_some());
        if let Some(MidiEvent::ProgramChange { channel, program }) = event {
            assert_eq!(channel, 0);
            assert_eq!(program, 42);
        } else {
            panic!("Expected ProgramChange event");
        }
    }

    #[test]
    fn test_midi_event_from_bytes_channel_pressure() {
        let data = [0xD0, 100]; // Channel pressure, channel 0, pressure 100
        let event = MidiEvent::from_bytes(&data);
        assert!(event.is_some());
        if let Some(MidiEvent::ChannelPressure { channel, pressure }) = event {
            assert_eq!(channel, 0);
            assert_eq!(pressure, 100);
        } else {
            panic!("Expected ChannelPressure event");
        }
    }

    #[test]
    fn test_to_dsp_keeps_channel_and_message() {
        use crate::dsp::MidiMessage;
        let bend = MidiEvent::PitchBend { channel: 3, value: -4096 };
        let dsp = bend.to_dsp(17).unwrap();
        assert_eq!(dsp.sample_offset, 17);
        assert_eq!(dsp.channel, 3);
        assert_eq!(dsp.message, MidiMessage::PitchBend { value: -4096 });

        let pressure = MidiEvent::ChannelPressure { channel: 0, pressure: 90 };
        assert_eq!(pressure.to_dsp(0).unwrap().message, MidiMessage::Aftertouch { pressure: 90 });

        // Pads hear how hard each pad is pressed (#428)
        let poly = MidiEvent::PolyPressure { channel: 9, note: 36, pressure: 90 };
        let dsp = poly.to_dsp(0).unwrap();
        assert_eq!((dsp.channel, dsp.message), (9, MidiMessage::PolyPressure { note: 36, pressure: 90 }));
    }

    #[test]
    fn test_pitch_bend_extremes() {
        let down = MidiEvent::from_bytes(&[0xE0, 0x00, 0x00]).unwrap();
        let up = MidiEvent::from_bytes(&[0xE0, 0x7F, 0x7F]).unwrap();
        assert!(matches!(down, MidiEvent::PitchBend { value: -8192, .. }));
        assert!(matches!(up, MidiEvent::PitchBend { value: 8191, .. }));
    }

    #[test]
    fn test_midi_event_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<MidiEvent>();
        assert_send::<TimestampedMidiEvent>();
    }

    #[test]
    fn test_midi_event_is_copy() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<MidiEvent>();
        assert_copy::<TimestampedMidiEvent>();
    }
}
