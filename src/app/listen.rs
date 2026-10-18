//! `?listen`: one hidden copy of the web build that a page hands patches to,
//! to play and meter them (#391). The Patch Garden's cards play through it
//! as they're hovered.
//!
//! The page sends it `window` messages, which it takes only from
//! soba.chaps.dev and from `trunk serve` on this machine:
//!
//! - `{type: "load", payload: "1.<data>"}` opens a patch link's patch,
//!   stopped. Sent while one plays, the sound fades out, the patch is
//!   swapped and the new one plays, fading in.
//! - `{type: "play"}` and `{type: "stop"}`.
//!
//! It answers the page that sent them:
//!
//! - `{type: "level", rms}` about [`LEVEL_HZ`] times a second while playing:
//!   the output's level from 0 to 1, as its meter reads it.
//! - `{type: "blocked"}` when the browser holds the sound back until the
//!   frame is clicked (its autoplay rule).
//! - `{type: "error", why}` for a patch that won't open.
//!
//! A patch played with Note Ins hears [`AUDITION`] over and over while it
//! plays, through the notes the computer keys play.

use crate::engine::AUDITION;

/// How many times a second the level is sent while playing.
pub const LEVEL_HZ: f64 = 20.0;

/// How long the sound fades out over before a patch is swapped, and back in after.
pub const FADE_SECONDS: f64 = 0.15;

/// What the page asked for.
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    /// Open the patch a link holds: its fragment, `1.<data>`.
    Load(String),
    Play,
    Stop,
}

/// Reads a message: its `type`, and its `payload` for a load.
pub fn parse(kind: Option<&str>, payload: Option<&str>) -> Result<Message, String> {
    match kind {
        Some("load") => match payload.map(str::trim) {
            Some(link) if !link.is_empty() => {
                // A whole fragment (`p=1.…`, `#p=1.…`) opens as well as its payload
                let link = link.trim_start_matches('#');
                Ok(Message::Load(if link.starts_with("p=") { link.to_string() } else { format!("p={link}") }))
            }
            _ => Err("a load needs a payload: the patch link's 1.<data>".to_string()),
        },
        Some("play") => Ok(Message::Play),
        Some("stop") => Ok(Message::Stop),
        Some(other) => Err(format!("no message called {other:?}")),
        None => Err("a message needs a type".to_string()),
    }
}

/// Whether a page at `origin` may play patches here: soba.chaps.dev, or
/// `trunk serve` (any port) on this machine.
pub fn trusted(origin: &str) -> bool {
    if origin == "https://soba.chaps.dev" {
        return true;
    }
    let Some(host) = origin.strip_prefix("http://") else { return false };
    let name = host.rsplit_once(':').map_or(host, |(name, port)| if port.bytes().all(|b| b.is_ascii_digit()) { name } else { host });
    name == "localhost" || name == "127.0.0.1"
}

/// How long one pass of [`AUDITION`] lasts, its rest included, before it
/// starts again.
pub const AUDITION_LOOP_SECONDS: f64 = 5.0;

/// [`AUDITION`] played over and over on the clock the frames keep, into
/// the channels its patch's Note Ins hear (`engine::audition_channels`).
pub struct Audition {
    /// The channels each note goes to.
    channels: Vec<u8>,
    /// The phrase as notes starting and ending, (seconds in, note, on), in order.
    events: Vec<(f64, u8, bool)>,
    /// When the first pass started, on the frames' clock.
    started: f64,
    /// How many of the events have been played, over all the passes.
    played: u64,
    /// The notes on now, and the channel each is on.
    held: Vec<(u8, u8)>,
}

impl Audition {
    /// The phrase into `channels`, starting `now`. None when there are none
    /// to play into.
    pub fn start(channels: Vec<u8>, now: f64) -> Option<Self> {
        if channels.is_empty() {
            return None;
        }
        let mut events: Vec<(f64, u8, bool)> = AUDITION
            .iter()
            .flat_map(|&(note, at, length)| [(at as f64, note, true), ((at + length) as f64, note, false)])
            .collect();
        // A note ending as another starts lets go first
        events.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.2.cmp(&b.2)));
        Some(Self { channels, events, started: now, played: 0, held: Vec::new() })
    }

    /// The notes due by `now`, as (channel, note, on), in order. After a
    /// long wait (a hidden tab's frames stop) it lets go of what it held
    /// and picks up where the phrase is now, rather than playing every note
    /// it missed at once.
    pub fn due(&mut self, now: f64) -> Vec<(u8, u8, bool)> {
        let mut due = Vec::new();
        if now - self.when(self.played) > AUDITION_LOOP_SECONDS {
            due.append(&mut self.release());
            let pass = ((now - self.started) / AUDITION_LOOP_SECONDS).floor() as u64;
            self.played = pass * self.events.len() as u64;
            while self.when(self.played) < now {
                self.played += 1;
            }
        }
        while self.when(self.played) <= now {
            let (_, note, on) = self.events[(self.played % self.events.len() as u64) as usize];
            for &channel in &self.channels {
                if on {
                    self.held.push((channel, note));
                } else if let Some(at) = self.held.iter().position(|&held| held == (channel, note)) {
                    self.held.remove(at);
                } else {
                    // Let go already, after a wait
                    continue;
                }
                due.push((channel, note, on));
            }
            self.played += 1;
        }
        due
    }

    /// Lets go of every note on, as (channel, note, false).
    pub fn release(&mut self) -> Vec<(u8, u8, bool)> {
        self.held.drain(..).map(|(channel, note)| (channel, note, false)).collect()
    }

    /// When event number `index` (over all the passes) is due.
    fn when(&self, index: u64) -> f64 {
        let count = self.events.len() as u64;
        self.started + (index / count) as f64 * AUDITION_LOOP_SECONDS + self.events[(index % count) as usize].0
    }
}

/// The page's side: the messages `index.html` queued, and the answers.
#[cfg(target_arch = "wasm32")]
pub mod page {
    use eframe::egui;
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;

    fn window() -> Option<web_sys::Window> {
        web_sys::window()
    }

    /// Has a message arriving wake the app, which collects it with [`take`].
    pub fn wake_on_message(ctx: &egui::Context) {
        let Some(window) = window() else { return };
        let ctx = ctx.clone();
        let wake = Closure::<dyn Fn()>::new(move || ctx.request_repaint());
        let _ = js_sys::Reflect::set(&window, &"sobaListenWake".into(), wake.as_ref());
        wake.forget();
    }

    /// The messages that came since the last call, as (origin, type, payload).
    pub fn take() -> Vec<(String, Option<String>, Option<String>)> {
        let Some(window) = window() else { return Vec::new() };
        let Ok(queue) = js_sys::Reflect::get(&window, &"sobaListenQueue".into()) else { return Vec::new() };
        let Ok(queue) = queue.dyn_into::<js_sys::Array>() else { return Vec::new() };
        let taken = queue.splice(0, queue.length(), &JsValue::UNDEFINED);
        let text = |value: &JsValue, key: &str| js_sys::Reflect::get(value, &key.into()).ok().and_then(|v| v.as_string());
        taken
            .iter()
            .map(|message| {
                let data = js_sys::Reflect::get(&message, &"data".into()).unwrap_or(JsValue::UNDEFINED);
                (text(&message, "origin").unwrap_or_default(), text(&data, "type"), text(&data, "payload"))
            })
            .collect()
    }

    /// Sends `{type, key: value}` (or just `{type}`) to the page at `origin`.
    pub fn post(origin: &str, kind: &str, field: Option<(&str, JsValue)>) {
        let Some(window) = window() else { return };
        let Ok(Some(parent)) = window.parent() else { return };
        let message = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&message, &"type".into(), &kind.into());
        if let Some((key, value)) = field {
            let _ = js_sys::Reflect::set(&message, &key.into(), &value);
        }
        let _ = parent.post_message(&message, origin);
    }

    /// Ramps the sound's level to `to` over `seconds`, starting `after`
    /// seconds from now, on the audio's own clock (`index.html`'s `sobaFade`).
    pub fn fade(to: f64, seconds: f64, after: f64) {
        call("sobaFade", &[to.into(), seconds.into(), after.into()]);
    }

    /// Asks the browser to let the sound start (`sobaResume`), which it
    /// does when the page around has been clicked and lets this frame play.
    pub fn resume() {
        call("sobaResume", &[]);
    }

    fn call(name: &str, args: &[JsValue]) {
        let Some(window) = window() else { return };
        let Some(f) = js_sys::Reflect::get(&window, &name.into()).ok().and_then(|f| f.dyn_into::<js_sys::Function>().ok()) else {
            return;
        };
        let _ = f.apply(&window, &args.iter().collect::<js_sys::Array>());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_read_as_the_page_sends_them() {
        assert_eq!(parse(Some("play"), None), Ok(Message::Play));
        assert_eq!(parse(Some("stop"), Some("ignored")), Ok(Message::Stop));
        assert_eq!(parse(Some("load"), Some("1.abc")), Ok(Message::Load("p=1.abc".into())));
        // A whole fragment does as well as its payload
        assert_eq!(parse(Some("load"), Some("#p=1.abc")), Ok(Message::Load("p=1.abc".into())));
        assert_eq!(parse(Some("load"), Some(" p=1.abc\n")), Ok(Message::Load("p=1.abc".into())));
        assert!(parse(Some("load"), None).unwrap_err().contains("payload"));
        assert!(parse(Some("load"), Some("  ")).unwrap_err().contains("payload"));
        assert!(parse(Some("dance"), None).unwrap_err().contains("dance"));
        assert!(parse(None, Some("1.abc")).unwrap_err().contains("type"));
    }

    #[test]
    fn a_loaded_payload_opens_as_its_patch_link_does() {
        let patch = crate::persistence::examples::first_sound().patch().unwrap();
        let fragment = crate::persistence::patch_link::fragment(&patch).unwrap();
        let payload = fragment.strip_prefix("p=").unwrap();
        let Ok(Message::Load(link)) = parse(Some("load"), Some(payload)) else { panic!() };
        assert_eq!(crate::persistence::patch_link::patch_from_fragment(&link).unwrap(), patch);
    }

    #[test]
    fn only_soba_and_this_machine_are_heard() {
        for origin in ["https://soba.chaps.dev", "http://127.0.0.1:8080", "http://localhost:8080", "http://localhost"] {
            assert!(trusted(origin), "{origin}");
        }
        for origin in [
            "http://soba.chaps.dev",
            "https://soba.chaps.dev.evil.example",
            "https://evil.example",
            "http://localhost.evil.example",
            "http://localhost:80@evil.example",
            "https://localhost:8080",
            "null",
            "",
        ] {
            assert!(!trusted(origin), "{origin}");
        }
    }

    /// Every event due from `from` to `to`, in steps of `step` seconds as frames would ask.
    fn play(audition: &mut Audition, from: f64, to: f64, step: f64) -> Vec<(f64, u8, u8, bool)> {
        let mut heard = Vec::new();
        let mut now = from;
        while now <= to {
            heard.extend(audition.due(now).into_iter().map(|(c, n, on)| (now, c, n, on)));
            now += step;
        }
        heard
    }

    #[test]
    fn a_patch_without_note_ins_has_no_audition() {
        assert!(Audition::start(Vec::new(), 0.0).is_none());
    }

    #[test]
    fn the_phrase_plays_on_time_and_loops() {
        let mut audition = Audition::start(vec![16], 10.0).unwrap();
        let heard = play(&mut audition, 10.0, 10.0 + 2.0 * AUDITION_LOOP_SECONDS, 0.01);
        let ons: Vec<(f64, u8)> = heard.iter().filter(|e| e.3).map(|e| (e.0, e.2)).collect();
        // Each note within a frame of its time, in both passes
        for pass in 0..2 {
            for &(note, at, _) in AUDITION {
                let due = 10.0 + pass as f64 * AUDITION_LOOP_SECONDS + at as f64;
                assert!(ons.iter().any(|&(t, n)| n == note && t >= due && t - due < 0.011), "{note} at {due}");
            }
        }
        // Every pass lets go of all it played before the next begins
        let first_pass: Vec<_> = heard.iter().filter(|e| e.0 < 10.0 + AUDITION_LOOP_SECONDS).collect();
        assert_eq!(first_pass.iter().filter(|e| e.3).count(), AUDITION.len());
        assert_eq!(first_pass.iter().filter(|e| !e.3).count(), AUDITION.len());
        assert!(heard.iter().all(|e| e.1 == 16));
    }

    #[test]
    fn each_note_goes_to_every_channel() {
        let mut audition = Audition::start(vec![16, 0], 0.0).unwrap();
        assert_eq!(audition.due(0.0), vec![(16, 60, true), (0, 60, true)]);
        assert_eq!(audition.release(), vec![(16, 60, false), (0, 60, false)]);
        assert!(audition.release().is_empty());
    }

    #[test]
    fn a_long_wait_lets_go_and_picks_up_the_pass_under_way() {
        let mut audition = Audition::start(vec![16], 0.0).unwrap();
        // The chord at 2.5 is held
        let _ = audition.due(3.0);
        assert_eq!(audition.held.len(), 3);
        // The frames stop for a minute, then start again 2.6 s into a pass
        let now = 60.0 + 2.6;
        let due = audition.due(now);
        assert_eq!(due, vec![(16, 60, false), (16, 64, false), (16, 67, false)], "only the chord let go");
        assert!(audition.held.is_empty());
        // ...and the next pass starts on time
        let next = audition.due(65.0);
        assert_eq!(next, vec![(16, 60, true)]);
    }
}
