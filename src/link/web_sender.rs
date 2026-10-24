//! MIDI Out in the browser: the desktop's [`MidiSender`], run by a timer
//! instead of a thread, sending to Web MIDI outputs.
//!
//! The page has one thread, and a frame being drawn holds the timer up, so
//! the sender hands each message over [`AHEAD`] of when its sample is
//! heard, stamped with that moment on the page's clock. The browser keeps
//! to the stamp, so notes and clock ticks land in time however late the
//! timer ran.
//!
//! The timer is a `setTimeout` set again each time for when the sender
//! next has work. A hidden tab's frames stop, but a tab playing sound keeps
//! its timers, so MIDI keeps going while Soba is in the background.
//!
//! Compiled only for the web build.

use std::cell::RefCell;
use std::time::Duration;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_time::Instant;

use super::midi::MIDI;
use super::midi_sender::{Connection, MidiSender, Outputs};
use super::midi_thru::THRU;
use crate::engine::web_midi;

/// How far ahead of its moment a message is sent. Longer than a slow
/// frame holds the timer up; short enough that letting go of notes isn't
/// held up long behind the ones already sent.
const AHEAD: Duration = Duration::from_millis(100);

/// The outputs Web MIDI lists: none until Connect MIDI is allowed.
struct WebOutputs;

impl Outputs for WebOutputs {
    fn scan(&mut self) -> Vec<String> {
        web_midi::output_names()
    }

    fn open(&mut self, name: &str) -> Result<Box<dyn Connection>, String> {
        let output = web_midi::output(name).ok_or_else(|| "not plugged in".to_string())?;
        Ok(Box::new(WebConnection(output)))
    }
}

struct WebConnection(web_sys::MidiOutput);

impl Connection for WebConnection {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.0.send(&js_sys::Uint8Array::from(bytes)).map_err(|e| format!("{e:?}"))
    }

    fn send_at(&mut self, bytes: &[u8], at: Instant) -> Result<(), String> {
        self.0.send_with_timestamp(&js_sys::Uint8Array::from(bytes), page_time(at)).map_err(|e| format!("{e:?}"))
    }
}

/// `at` on the page's clock, in milliseconds. The two clocks are tied
/// once: reading both afresh for each message would give two due at the
/// same moment different stamps, as the page's clock is rounded to 0.1 ms,
/// and could swap a clock tick and the note beside it.
fn page_time(at: Instant) -> f64 {
    let (instant, ms) = EPOCH.with(|epoch| *epoch.get_or_init(|| (Instant::now(), web_midi::now_ms())));
    let offset = if at >= instant { at.duration_since(instant).as_secs_f64() } else { -instant.duration_since(at).as_secs_f64() };
    ms + offset * 1000.0
}

thread_local! {
    static EPOCH: std::cell::OnceCell<(Instant, f64)> = const { std::cell::OnceCell::new() };
    static SENDER: RefCell<Option<MidiSender>> = const { RefCell::new(None) };
    static TIMER: RefCell<Option<Closure<dyn FnMut()>>> = const { RefCell::new(None) };
}

/// Starts sending MIDI Out, once however often it's called. From now on
/// each MIDI Out gets a ring, so one made before Connect MIDI plays as
/// soon as its device is chosen.
pub fn start() {
    if SENDER.with_borrow(|s| s.is_some()) {
        return;
    }
    SENDER.set(Some(MidiSender::new(&MIDI, Box::new(WebOutputs)).ahead(AHEAD).with_thru(&THRU)));
    TIMER.set(Some(Closure::new(tick)));
    tick();
}

/// Sends what's due, and sets the timer for the next time there's work.
fn tick() {
    let now = Instant::now();
    let next = SENDER.with_borrow_mut(|s| s.as_mut().map(|sender| sender.tick(now)));
    let Some(next) = next else { return };
    let wait = next.saturating_duration_since(Instant::now()).as_millis() as i32;
    TIMER.with_borrow(|timer| {
        if let (Some(window), Some(timer)) = (web_sys::window(), timer) {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(timer.as_ref().unchecked_ref(), wait);
        }
    });
}

/// Sends what a MIDI In just passed down a thru route (#446), from the
/// Web MIDI callback that heard it: there's no thread to wake. The page
/// has one thread, so the sender is never mid-tick when it's called.
pub fn forward_now() {
    SENDER.with(|sender| {
        if let Ok(mut sender) = sender.try_borrow_mut() {
            if let Some(sender) = sender.as_mut() {
                sender.forward(Instant::now());
            }
        }
    });
}

/// Sends what the UI just sent (SysEx, #440) now, rather than when the
/// timer next runs. The page has one thread, so the sender is never
/// mid-tick when it's called; the timer keeps its own time.
pub fn send_now() {
    SENDER.with(|sender| {
        if let Ok(mut sender) = sender.try_borrow_mut() {
            if let Some(sender) = sender.as_mut() {
                sender.tick(Instant::now());
            }
        }
    });
}
