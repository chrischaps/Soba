//! The app as a page's listener (`?listen`, see [`crate::app::listen`]):
//! nothing drawn, no session kept, just patches opened, played and metered
//! as the page asks.

use eframe::egui;
use wasm_bindgen::JsValue;

use super::SynthApp;
use crate::app::listen::{self, page, Audition, Message, FADE_SECONDS, LEVEL_HZ, STOP_FADE_SECONDS};
use crate::engine::{MidiEvent, UI_VELOCITY};
use crate::persistence::patch_link;

/// How long after Play to ask whether the browser let the sound start:
/// a resume the page's click allows takes a moment to come through.
const BLOCKED_AFTER_SECONDS: f64 = 0.5;

/// The listener's state between frames.
#[derive(Default)]
pub(super) struct Listener {
    /// The page to answer: the one that last sent a message.
    origin: Option<String>,
    /// The channels the open patch's Note Ins hear the audition on.
    channels: Vec<u8>,
    /// The phrase played into them while the patch plays.
    audition: Option<Audition>,
    /// A patch link waiting for the sound to fade out, and when it will have.
    swap: Option<(String, f64)>,
    /// A stop waiting for the sound to fade out: when it will have, and the
    /// patch link to open then, if a swap was under way.
    stopping: Option<(f64, Option<String>)>,
    /// When the level was last sent.
    level_sent: f64,
    /// When to ask whether the browser held the sound back, after Play.
    ask_blocked: Option<f64>,
}

impl SynthApp {
    /// Makes this copy of the app a page's listener: nothing is drawn or
    /// kept, and patches come from the page's messages.
    pub fn start_listening(&mut self, ctx: &egui::Context) {
        self.embedded = true;
        self.listener = Some(Listener::default());
        page::wake_on_message(ctx);
    }

    /// A listener's frame: the engine kept up to date and the page
    /// answered, with nothing drawn.
    pub(super) fn draw_listening(&mut self, ctx: &egui::Context) {
        self.before_drawing(ctx);
        self.tend_listener(ctx);
        self.follow_arrangers();
        self.user_state.poly_note_ins = crate::graph::note_in_face::poly_note_ins(&self.graph_state.graph);
        self.glide_scenes(ctx);
        self.sync_parameters();
        self.process_midi_events(ctx.input(|i| i.time));
        self.sync_engine(ctx);
    }

    /// Hears the page's messages, swaps a patch once it has faded, plays
    /// the audition and sends the level.
    fn tend_listener(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        for (origin, kind, payload) in page::take() {
            if !listen::trusted(&origin) {
                continue;
            }
            if let Some(listener) = self.listener.as_mut() {
                listener.origin = Some(origin);
            }
            match listen::parse(kind.as_deref(), payload.as_deref()) {
                Ok(message) => self.hear_page(message, now),
                Err(why) => self.answer_page("error", Some(("why", why.into()))),
            }
        }

        let Some(listener) = self.listener.as_mut() else { return };
        match listener.swap.take() {
            Some((link, at)) if now >= at => self.open_listened(&link, true, now),
            Some((link, at)) => {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(at - now));
                listener.swap = Some((link, at));
            }
            None => {}
        }
        let Some(listener) = self.listener.as_mut() else { return };
        match listener.stopping.take() {
            Some((at, link)) if now >= at => self.halt_listening(link, now),
            Some((at, link)) => {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(at - now));
                listener.stopping = Some((at, link));
            }
            None => {}
        }

        if !self.is_playing {
            return;
        }
        let due = self.listener.as_mut().and_then(|l| l.audition.as_mut()).map(|a| a.due(now)).unwrap_or_default();
        self.play_audition_notes(due);

        let Some(listener) = self.listener.as_mut() else { return };
        let period = 1.0 / LEVEL_HZ;
        if now - listener.level_sent >= period {
            // On the beat, so frames that don't land on it still send LEVEL_HZ a second
            listener.level_sent = if now - listener.level_sent < 2.0 * period { listener.level_sent + period } else { now };
            let db = self.user_state.output_meter.post_db();
            let level = 10f64.powf(db[0].max(db[1]) as f64 / 20.0);
            self.answer_page("level", Some(("rms", level.into())));
        }
        let Some(listener) = self.listener.as_mut() else { return };
        if listener.ask_blocked.is_some_and(|at| now >= at) {
            listener.ask_blocked = None;
            if crate::app::web::audio_blocked() {
                self.answer_page("blocked", None);
            }
        }
    }

    /// Does what the page asked.
    fn hear_page(&mut self, message: Message, now: f64) {
        let Some(listener) = self.listener.as_mut() else { return };
        match message {
            // Playing, the sound fades out first, and the new patch comes in playing
            // (fading out to stop, it swaps once that fade is done instead)
            Message::Load(link) if self.is_playing => {
                let fading = listener.swap.take().map(|(_, at)| at).or(listener.stopping.take().map(|(at, _)| at));
                if fading.is_none() {
                    page::fade(0.0, FADE_SECONDS, 0.0);
                }
                listener.swap = Some((link, fading.unwrap_or(now + FADE_SECONDS)));
            }
            Message::Load(link) => self.open_listened(&link, false, now),
            // A swap under way comes in playing anyway
            Message::Play if listener.swap.is_some() => {}
            // Fading out to stop, the sound comes back
            Message::Play if listener.stopping.is_some() => {
                listener.stopping = None;
                page::fade(1.0, FADE_SECONDS, 0.0);
            }
            Message::Play => {
                page::resume();
                listener.audition = Audition::start(listener.channels.clone(), now);
                listener.ask_blocked = Some(now + BLOCKED_AFTER_SECONDS);
                self.set_playing(true);
            }
            // Playing, the sound fades out first (a swap under way already is)
            Message::Stop if self.is_playing => {
                if listener.stopping.is_none() {
                    listener.stopping = Some(match listener.swap.take() {
                        Some((link, at)) => (at, Some(link)),
                        None => {
                            page::fade(0.0, STOP_FADE_SECONDS, 0.0);
                            (now + STOP_FADE_SECONDS, None)
                        }
                    });
                }
            }
            Message::Stop => {
                let link = listener.swap.take().map(|(link, _)| link);
                self.halt_listening(link, now);
            }
        }
    }

    /// Stops playing, and opens `link` stopped if a swap was under way.
    fn halt_listening(&mut self, link: Option<String>, now: f64) {
        self.stop_listening_audition();
        self.set_playing(false);
        page::fade(1.0, 0.0, 0.0);
        if let Some(link) = link {
            self.open_listened(&link, false, now);
        }
    }

    /// Opens the patch `link` holds, playing it from `now` if `play`, or
    /// tells the page why it can't (and the patch that was open carries on).
    fn open_listened(&mut self, link: &str, play: bool, now: f64) {
        self.stop_listening_audition();
        let opened = patch_link::patch_from_fragment(link).and_then(|patch| {
            let channels = crate::engine::audition_channels(&patch);
            self.open_linked(patch, None)?;
            Ok(channels)
        });
        match opened {
            Ok(channels) => {
                let Some(listener) = self.listener.as_mut() else { return };
                listener.channels = channels;
                if play {
                    // In again once the old patch's sound already on its way has passed
                    let queued = self.audio.engine.as_ref().ok().and_then(|e| e.output_latency()).map_or(0.2, |d| d.as_secs_f64());
                    page::fade(1.0, FADE_SECONDS, queued);
                    self.hear_page(Message::Play, now);
                }
            }
            Err(why) => {
                page::fade(1.0, FADE_SECONDS, 0.0);
                self.answer_page("error", Some(("why", why.into())));
            }
        }
    }

    /// Lets go of the audition's notes, and of the audition.
    fn stop_listening_audition(&mut self) {
        let released = self.listener.as_mut().and_then(|l| l.audition.take()).map(|mut a| a.release()).unwrap_or_default();
        self.play_audition_notes(released);
    }

    /// Plays (channel, note, on) notes as the computer keys play theirs.
    fn play_audition_notes(&mut self, notes: Vec<(u8, u8, bool)>) {
        for (channel, note, on) in notes {
            let event = if on {
                MidiEvent::NoteOn { channel, note, velocity: UI_VELOCITY }
            } else {
                MidiEvent::NoteOff { channel, note, velocity: 0 }
            };
            self.midi.send_ui(event);
        }
    }

    /// Sends the page `{type: kind}`, with `field` if given.
    fn answer_page(&self, kind: &str, field: Option<(&str, JsValue)>) {
        if let Some(origin) = self.listener.as_ref().and_then(|l| l.origin.as_deref()) {
            page::post(origin, kind, field);
        }
    }
}
