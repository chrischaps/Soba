//! MIDI devices in the browser, through Web MIDI.
//!
//! The browser asks the visitor before a page may use MIDI, so nothing is
//! asked until **Connect MIDI** is clicked ([`request`]). Once allowed, the
//! `MIDIAccess` lists the inputs and outputs, and its `statechange` says
//! when one is plugged in or out: the browser's answer to the desktop's
//! scanning thread. [`MidiEngine`](super::MidiEngine) opens inputs from
//! here, and `link::web_sender` sends MIDI Out to the outputs.
//!
//! midir has a Web MIDI backend, but it asks for access as soon as it's
//! first touched rather than on a click, always asks for sysex (which
//! Chrome words as "control and reprogram your MIDI devices"), doesn't
//! hear `statechange`, and sends without a timestamp.
//!
//! Safari has no Web MIDI: [`request`] says so rather than failing quietly.
//!
//! **SysEx** (#440) is asked for apart, the first time something sends it
//! ([`request_sysex`]), so a visitor who only plays notes is never asked to
//! let Soba reprogram their gear. The access the browser hands back can
//! send it; the outputs opened before can't, so the sender opens them
//! afresh.
//!
//! Compiled only for the web build.

use std::cell::RefCell;

use eframe::egui;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{MidiAccess, MidiInput, MidiMessageEvent, MidiOptions, MidiOutput, MidiPortDeviceState};

/// Whether the page may use MIDI devices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    /// Not asked yet: Connect MIDI asks.
    Unasked,
    /// Asked; the browser is waiting on the visitor.
    Asking,
    /// Allowed: devices are listed.
    Allowed,
    /// The visitor (or the browser) said no, and why.
    Refused(String),
    /// This browser has no Web MIDI (Safari).
    Unsupported,
}

/// Whether the page may send SysEx (#440), which the browser asks about
/// apart, as letting it "control and reprogram your MIDI devices".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SysexAccess {
    /// Not asked yet: the first SysEx sent asks.
    Unasked,
    /// Asked; the browser is waiting on the visitor.
    Asking,
    Allowed,
    /// The visitor (or the browser) said no, and why.
    Refused(String),
}

/// What this page holds of Web MIDI, on its one thread.
struct State {
    access: Access,
    sysex: SysexAccess,
    midi: Option<MidiAccess>,
    /// Counts the times the devices changed, so the UI lists them again.
    changes: u32,
    /// Kept alive while `midi` hears `statechange`.
    _on_change: Option<Closure<dyn FnMut(JsValue)>>,
}

thread_local! {
    static STATE: RefCell<State> = const { RefCell::new(State { access: Access::Unasked, sysex: SysexAccess::Unasked, midi: None, changes: 0, _on_change: None }) };
}

/// Whether the page may use MIDI devices yet.
pub fn access() -> Access {
    STATE.with_borrow(|s| s.access.clone())
}

/// Whether the page may send SysEx yet.
pub fn sysex_access() -> SysexAccess {
    STATE.with_borrow(|s| s.sysex.clone())
}

/// Asks the browser to let the page send SysEx, and for MIDI devices if
/// they haven't been allowed yet: one question for both. Called from a
/// click, the first time SysEx is sent. The answer comes later; `ctx`
/// draws a frame for it.
pub fn request_sysex(ctx: &egui::Context) {
    if matches!(sysex_access(), SysexAccess::Asking | SysexAccess::Allowed) {
        return;
    }
    let Some(navigator) = web_sys::window().map(|w| w.navigator()) else { return };
    if !js_sys::Reflect::has(&navigator, &"requestMIDIAccess".into()).unwrap_or(false) {
        set_access(Access::Unsupported);
        return;
    }
    let options = MidiOptions::new();
    options.set_sysex(true);
    let Ok(promise) = navigator.request_midi_access_with_options(&options) else { return };
    STATE.with_borrow_mut(|s| s.sysex = SysexAccess::Asking);
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        match wasm_bindgen_futures::JsFuture::from(promise).await {
            Ok(midi) => {
                STATE.with_borrow_mut(|s| s.sysex = SysexAccess::Allowed);
                allowed(midi.unchecked_into(), ctx.clone());
                // The outputs opened before can't send it
                crate::link::midi::MIDI.reconnect();
            }
            Err(error) => {
                let name = js_sys::Reflect::get(&error, &"name".into()).ok().and_then(|n| n.as_string()).unwrap_or_default();
                let why = if matches!(name.as_str(), "NotAllowedError" | "SecurityError") {
                    "SysEx wasn't allowed, so notes and CCs go but SysEx doesn't. Allow it in the site settings (the icon left of the address) to send it".to_string()
                } else {
                    refusal(&error)
                };
                STATE.with_borrow_mut(|s| s.sysex = SysexAccess::Refused(why));
            }
        }
        ctx.request_repaint();
    });
}

/// How many times the devices have changed: compare with the last count
/// seen to know when to list them again.
pub fn changes() -> u32 {
    STATE.with_borrow(|s| s.changes)
}

/// Asks the browser for MIDI devices. Called from a click: Firefox asks
/// only within a moment of one. The answer comes later; `ctx` draws a
/// frame for it, and for every device plugged in or out after.
pub fn request(ctx: &egui::Context) {
    if matches!(access(), Access::Asking | Access::Allowed) {
        return;
    }
    let Some(navigator) = web_sys::window().map(|w| w.navigator()) else { return };
    let supported = js_sys::Reflect::has(&navigator, &"requestMIDIAccess".into()).unwrap_or(false);
    let asked = supported.then(|| navigator.request_midi_access().ok()).flatten();
    let Some(promise) = asked else {
        set_access(Access::Unsupported);
        return;
    };
    set_access(Access::Asking);
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        match wasm_bindgen_futures::JsFuture::from(promise).await {
            Ok(midi) => allowed(midi.unchecked_into(), ctx.clone()),
            Err(error) => set_access(Access::Refused(refusal(&error))),
        }
        ctx.request_repaint();
    });
}

/// Keeps `midi`, and hears it say devices came and went.
fn allowed(midi: MidiAccess, ctx: egui::Context) {
    let on_change = Closure::<dyn FnMut(JsValue)>::new(move |_event: JsValue| {
        STATE.with_borrow_mut(|s| s.changes = s.changes.wrapping_add(1));
        crate::link::midi::MIDI.rescan();
        ctx.request_repaint();
    });
    midi.set_onstatechange(Some(on_change.as_ref().unchecked_ref()));
    STATE.with_borrow_mut(|s| {
        // One asked for again, for SysEx: the last mustn't call a closure dropped below
        if let Some(old) = s.midi.take() {
            old.set_onstatechange(None);
        }
        s.access = Access::Allowed;
        s.midi = Some(midi);
        s.changes = s.changes.wrapping_add(1);
        s._on_change = Some(on_change);
    });
    crate::link::midi::MIDI.rescan();
}

fn set_access(access: Access) {
    STATE.with_borrow_mut(|s| s.access = access);
}

/// Why the browser said no, in a few words.
fn refusal(error: &JsValue) -> String {
    let name = js_sys::Reflect::get(error, &"name".into()).ok().and_then(|n| n.as_string()).unwrap_or_default();
    match name.as_str() {
        "NotAllowedError" | "SecurityError" => "MIDI wasn't allowed. Allow it in the site settings (the icon left of the address), then Connect MIDI again".to_string(),
        _ => {
            let message = js_sys::Reflect::get(error, &"message".into()).ok().and_then(|m| m.as_string());
            format!("MIDI couldn't start: {}", message.unwrap_or(name))
        }
    }
}

/// The values of a `MIDIInputMap` or `MIDIOutputMap` that are plugged in.
fn ports<T: JsCast>(map: JsValue) -> Vec<T> {
    let mut found = Vec::new();
    map.unchecked_into::<js_sys::Map>().for_each(&mut |port, _| {
        let state = js_sys::Reflect::get(&port, &"state".into()).ok().and_then(|s| MidiPortDeviceState::from_js_value(&s));
        if state != Some(MidiPortDeviceState::Disconnected) {
            found.push(port.unchecked_into());
        }
    });
    found
}

fn inputs() -> Vec<MidiInput> {
    STATE.with_borrow(|s| s.midi.as_ref().map(|m| ports(m.inputs().into())).unwrap_or_default())
}

fn outputs() -> Vec<MidiOutput> {
    STATE.with_borrow(|s| s.midi.as_ref().map(|m| ports(m.outputs().into())).unwrap_or_default())
}

fn input_name(input: &MidiInput) -> String {
    input.name().unwrap_or_else(|| input.id())
}

fn output_name(output: &MidiOutput) -> String {
    output.name().unwrap_or_else(|| output.id())
}

/// The inputs plugged in, by name.
pub fn input_names() -> Vec<String> {
    inputs().iter().map(input_name).collect()
}

/// The outputs plugged in, by name.
pub fn output_names() -> Vec<String> {
    outputs().iter().map(output_name).collect()
}

/// The output called `name`, if it's plugged in.
pub fn output(name: &str) -> Option<MidiOutput> {
    outputs().into_iter().find(|o| output_name(o) == name)
}

/// An input being listened to. Dropping or closing it stops listening.
pub struct InputConnection {
    input: MidiInput,
    _on_message: Closure<dyn FnMut(MidiMessageEvent)>,
}

impl InputConnection {
    pub fn close(self) {}
}

impl Drop for InputConnection {
    fn drop(&mut self) {
        self.input.set_onmidimessage(None);
        let _ = self.input.close();
    }
}

/// Listens to the input called `name`, handing each message's bytes to
/// `on_message`. `None` if it isn't plugged in.
pub fn open_input(name: &str, mut on_message: impl FnMut(&[u8]) + 'static) -> Option<InputConnection> {
    let input = inputs().into_iter().find(|i| input_name(i) == name)?;
    let on_message = Closure::<dyn FnMut(MidiMessageEvent)>::new(move |event: MidiMessageEvent| {
        if let Ok(data) = event.data() {
            on_message(&data);
        }
    });
    // Listening opens the port; a message handler is all Web MIDI needs
    input.set_onmidimessage(Some(on_message.as_ref().unchecked_ref()));
    Some(InputConnection { input, _on_message: on_message })
}

/// The page's clock, in milliseconds: the one `MIDIOutput.send` is stamped on.
pub fn now_ms() -> f64 {
    web_sys::window().and_then(|w| w.performance()).map_or(0.0, |p| p.now())
}
