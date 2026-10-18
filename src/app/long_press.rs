//! A long press: a finger's right-click.
//!
//! egui turns a finger held still into a secondary click, but only on the
//! widget under it, and the canvas's add menu listens to the pointer rather
//! than to a widget. So the canvas watches for one itself.

use std::time::Duration;

use egui::Pos2;

/// How long a finger rests before the press counts, in seconds. Sooner than
/// egui's 0.8 s, after which a resting finger starts a drag (on empty
/// canvas, a pan).
const HOLD: f64 = 0.5;

/// How far a finger may wander, in points, and still be resting. A click
/// allows as much.
const WANDER: f32 = 6.0;

/// Watches the finger that's down for a long press.
#[derive(Default)]
pub struct LongPress {
    /// When the press under watch started, and how it's going.
    press: Option<(f64, State)>,
    /// The finger behind a long press lifted this frame.
    lifted: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    Resting,
    /// Moved, or joined by a second finger: a pan or a pinch, not a press.
    Moving,
    /// Counted. The finger is still down.
    Pressed,
}

impl LongPress {
    /// Follows the finger, once a frame. Returns where it is on the frame
    /// it has rested long enough.
    pub fn update(&mut self, ctx: &egui::Context) -> Option<Pos2> {
        let (touching, pinching, down, origin, start, pos, now) = ctx.input(|i| {
            (
                i.any_touches(),
                i.multi_touch().is_some(),
                i.pointer.primary_down(),
                i.pointer.press_origin(),
                i.pointer.press_start_time(),
                i.pointer.latest_pos(),
                i.time,
            )
        });
        let (true, true, Some(origin), Some(start), Some(pos)) = (touching, down, origin, start, pos) else {
            self.lifted = self.holding();
            self.press = None;
            return None;
        };
        self.lifted = false;
        let before = match self.press {
            Some((started, state)) if started == start => state,
            _ => State::Resting,
        };
        let state = match before {
            State::Resting if pinching || pos.distance(origin) > WANDER => State::Moving,
            State::Resting if now - start >= HOLD => State::Pressed,
            State::Resting => {
                // No input arrives while a finger rests, so ask to look again
                ctx.request_repaint_after(Duration::from_secs_f64(HOLD - (now - start)));
                State::Resting
            }
            other => other,
        };
        self.press = Some((start, state));
        (state == State::Pressed && before != State::Pressed).then_some(pos)
    }

    /// Whether the finger behind a long press is still down, so it doesn't
    /// drag anything when it moves on.
    pub fn holding(&self) -> bool {
        matches!(self.press, Some((_, State::Pressed)))
    }

    /// Whether the finger behind a long press lifted this frame. Lifted
    /// before egui's 0.8 s it's also a click, which isn't one.
    pub fn lifted(&self) -> bool {
        self.lifted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, PointerButton, RawInput, TouchDeviceId, TouchId, TouchPhase};

    /// A finger (or with `touch` false, a mouse) and what it's done.
    struct Finger {
        ctx: egui::Context,
        watch: LongPress,
        touch: bool,
    }

    impl Finger {
        fn new(touch: bool) -> Self {
            Self { ctx: egui::Context::default(), watch: LongPress::default(), touch }
        }

        /// One frame at `time`, with these events. Returns what the watch saw.
        fn frame(&mut self, time: f64, events: Vec<Event>) -> Option<Pos2> {
            let input = RawInput { time: Some(time), events, ..Default::default() };
            let mut seen = None;
            self.ctx.run_ui(input, |ui| seen = self.watch.update(ui.ctx())).textures_delta.clear();
            seen
        }

        fn events(&self, phase: TouchPhase, pos: Pos2) -> Vec<Event> {
            let mut events = Vec::new();
            if self.touch {
                events.push(Event::Touch { device_id: TouchDeviceId(0), id: TouchId(0), phase, pos, force: None });
            }
            events.push(Event::PointerMoved(pos));
            let button = |pressed| Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            match phase {
                TouchPhase::Start => events.push(button(true)),
                TouchPhase::End => events.push(button(false)),
                _ => {}
            }
            events
        }

        fn down(&mut self, time: f64, pos: Pos2) -> Option<Pos2> {
            let events = self.events(TouchPhase::Start, pos);
            self.frame(time, events)
        }

        fn drag(&mut self, time: f64, pos: Pos2) -> Option<Pos2> {
            let events = self.events(TouchPhase::Move, pos);
            self.frame(time, events)
        }

        fn up(&mut self, time: f64, pos: Pos2) -> Option<Pos2> {
            let events = self.events(TouchPhase::End, pos);
            self.frame(time, events)
        }
    }

    #[test]
    fn a_resting_finger_presses_once() {
        let at = Pos2::new(100.0, 100.0);
        let mut finger = Finger::new(true);
        assert_eq!(finger.down(1.0, at), None);
        assert_eq!(finger.frame(1.3, vec![]), None);
        assert_eq!(finger.frame(1.6, vec![]), Some(at));
        assert!(finger.watch.holding());
        assert_eq!(finger.frame(1.9, vec![]), None, "once a press");
        // Its release isn't a tap
        assert_eq!(finger.up(2.0, at), None);
        assert!(finger.watch.lifted());
        assert!(!finger.watch.holding());
        finger.frame(2.1, vec![]);
        assert!(!finger.watch.lifted());
    }

    #[test]
    fn a_wandering_finger_pans_instead() {
        let mut finger = Finger::new(true);
        finger.down(1.0, Pos2::new(100.0, 100.0));
        finger.drag(1.2, Pos2::new(120.0, 100.0));
        // Back where it started, but it's a pan now
        finger.drag(1.4, Pos2::new(100.0, 100.0));
        assert_eq!(finger.frame(1.8, vec![]), None);
        assert!(!finger.watch.holding());
    }

    #[test]
    fn a_tremble_is_still_resting() {
        let mut finger = Finger::new(true);
        finger.down(1.0, Pos2::new(100.0, 100.0));
        finger.drag(1.2, Pos2::new(103.0, 102.0));
        assert_eq!(finger.frame(1.6, vec![]), Some(Pos2::new(103.0, 102.0)));
    }

    #[test]
    fn the_next_press_is_watched_afresh() {
        let at = Pos2::new(100.0, 100.0);
        let mut finger = Finger::new(true);
        finger.down(1.0, at);
        finger.drag(1.1, Pos2::new(150.0, 100.0));
        finger.up(1.2, Pos2::new(150.0, 100.0));
        assert!(!finger.watch.lifted(), "a pan's release is a release");
        finger.down(2.0, at);
        assert_eq!(finger.frame(2.6, vec![]), Some(at));
    }

    #[test]
    fn a_held_mouse_button_is_not_a_press() {
        let mut mouse = Finger::new(false);
        mouse.down(1.0, Pos2::new(100.0, 100.0));
        assert_eq!(mouse.frame(2.0, vec![]), None);
    }
}
