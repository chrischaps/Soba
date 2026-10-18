//! Turning a knob without dragging it: the arrow keys while it has focus
//! (a click or drag gives it focus, and a click elsewhere takes it away),
//! and Alt+wheel while the pointer is over it.

use eframe::egui::{self, Event, EventFilter, Id, Key, LayerId, Rect, Response, Ui};

use super::knob::KnobConfig;

/// How far one notch turns a knob: a share of its travel, or for a stepped
/// knob a number of whole steps.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Turn {
    pub share: f32,
    pub steps: f32,
}

impl Turn {
    /// An arrow key, or a notch of the wheel: 1% of the way round, or a
    /// tenth of that held `fine`. A stepped knob moves a step either way.
    pub fn notch(config: &KnobConfig, fine: bool) -> Self {
        let share = if fine { NOTCH * config.fine_multiplier } else { NOTCH };
        Self { share, steps: 1.0 }
    }

    /// Page Up or Page Down: a tenth of the way round, or as near to that as
    /// whole steps go.
    pub fn page(config: &KnobConfig) -> Self {
        let range = config.range.end() - config.range.start();
        Self { share: PAGE, steps: (range * PAGE).round().max(1.0) }
    }

    fn times(self, by: f32) -> Self {
        Self { share: self.share * by, steps: self.steps * by }
    }

    fn add(&mut self, other: Self) {
        self.share += other.share;
        self.steps += other.steps;
    }
}

const NOTCH: f32 = 0.01;
const PAGE: f32 = 0.1;

/// `value` turned by `turn`, round the knob's travel so a logarithmic knob
/// moves as evenly as it drags.
pub fn turned(config: &KnobConfig, value: f32, turn: Turn) -> f32 {
    let (min, max) = (*config.range.start(), *config.range.end());
    if config.stepped {
        (value.round() + turn.steps).clamp(min, max)
    } else {
        config.value_at_travel((config.travel(value) + turn.share).clamp(0.0, 1.0))
    }
}

fn focus_id() -> Id {
    Id::unique("knob-focus")
}

/// Whether a text field has the keys. A knob with focus takes only the keys
/// [`keys`] reads, so notes, shortcuts and Tab's palette carry on around it.
pub fn typing(ctx: &egui::Context) -> bool {
    let focused = ctx.memory(|m| m.focused());
    focused.is_some() && focused != ctx.data(|d| d.get_temp::<Id>(focus_id()))
}

/// What the keys pressed this frame ask of a knob with focus: arrows nudge
/// it (Shift finer), Page Up and Down move it further, Home resets it. The
/// keys it takes are its own, and neither the arrows nor Tab move focus on
/// from it: Tab opens the palette, as it does with nothing in focus.
pub fn keys(ui: &Ui, response: &Response, config: &KnobConfig, value: &mut f32) {
    if response.clicked() || response.drag_started() {
        response.request_focus();
    } else if response.has_focus() && response.clicked_elsewhere() {
        response.surrender_focus();
    }
    if !response.has_focus() {
        return;
    }
    ui.data_mut(|d| d.insert_temp(focus_id(), response.id));
    let filter = EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, ..Default::default() };
    ui.memory_mut(|m| m.set_focus_lock_filter(response.id, filter));
    let mut turn = Turn::default();
    let mut reset = false;
    ui.input_mut(|i| {
        i.events.retain(|event| {
            let Event::Key { key, pressed: true, modifiers, .. } = event else { return true };
            if modifiers.ctrl || modifiers.alt || modifiers.command {
                return true;
            }
            match key {
                Key::ArrowUp | Key::ArrowRight => turn.add(Turn::notch(config, modifiers.shift)),
                Key::ArrowDown | Key::ArrowLeft => turn.add(Turn::notch(config, modifiers.shift).times(-1.0)),
                Key::PageUp => turn.add(Turn::page(config)),
                Key::PageDown => turn.add(Turn::page(config).times(-1.0)),
                Key::Home => reset = true,
                _ => return true,
            }
            false
        })
    });
    if reset {
        *value = config.default;
    }
    if turn != Turn::default() {
        *value = turned(config, *value, turn);
    }
}

/// The knob under the pointer last frame, which Alt+wheel turns.
#[derive(Clone, Copy, Debug)]
struct Hovered {
    id: Id,
    layer: LayerId,
    rect: Rect,
}

fn hovered_id() -> Id {
    Id::unique("knob-wheel-hovered")
}

/// Notches of the wheel taken for a knob, as (plain, fine), not yet turned.
fn wheel_id(knob: Id) -> Id {
    knob.with("wheel")
}

/// Turns the knob by the wheel notches [`claim_wheel`] took for it, and
/// marks it as the one under the pointer for next frame's.
pub fn wheel(ui: &Ui, response: &Response, config: &KnobConfig, value: &mut f32) {
    if response.hovered() && ui.is_enabled() {
        let rect = response.rect.intersect(ui.clip_rect());
        ui.data_mut(|d| d.insert_temp(hovered_id(), Hovered { id: response.id, layer: ui.layer_id(), rect }));
    }
    let Some((plain, fine)) = ui.data_mut(|d| d.remove_temp::<(f32, f32)>(wheel_id(response.id))) else { return };
    let mut turn = Turn::notch(config, false).times(plain);
    turn.add(Turn::notch(config, true).times(fine));
    if config.stepped {
        // A trackpad turns by fractions of a notch: whole steps go, the rest waits
        let whole = turn.steps.trunc();
        if whole.abs() < 1.0 {
            ui.data_mut(|d| d.insert_temp(wheel_id(response.id), (plain, fine)));
            return;
        }
        let left = turn.steps - whole;
        turn.steps = whole;
        ui.data_mut(|d| d.insert_temp(wheel_id(response.id), (left, 0.0)));
    }
    *value = turned(config, *value, turn);
}

/// Takes Alt+wheel over a knob out of the frame's input before the canvas
/// zooms with it, and keeps the notches for the knob: up turns it up, and
/// Shift as well turns it finely. It goes by the knob hovered last frame.
/// Call it from the app's raw input hook.
pub fn claim_wheel(ctx: &egui::Context, raw: &mut egui::RawInput) {
    let Some(knob) = ctx.data_mut(|d| {
        let knob = d.get_temp::<Hovered>(hovered_id());
        d.remove::<Hovered>(hovered_id());
        knob
    }) else {
        return;
    };
    let line = ctx.options(|options| options.input_options.line_scroll_speed);
    let mut pointer = ctx.input(|input| input.pointer.latest_pos());
    raw.events.retain(|event| match event {
        Event::PointerMoved(at) => {
            pointer = Some(*at);
            true
        }
        Event::MouseWheel { unit, delta, modifiers, .. } if modifiers.alt => {
            let over = pointer.is_some_and(|at| knob.rect.contains(at) && ctx.layer_id_at(at) == Some(knob.layer));
            if !over {
                return true;
            }
            // Shift can turn a vertical wheel sideways: either way counts
            let notches = match unit {
                egui::MouseWheelUnit::Line => delta.x + delta.y,
                egui::MouseWheelUnit::Point => (delta.x + delta.y) / line,
                egui::MouseWheelUnit::Page => (delta.x + delta.y) * 10.0,
            };
            ctx.data_mut(|d| {
                let (plain, fine) = d.get_temp_mut_or_default::<(f32, f32)>(wheel_id(knob.id));
                if modifiers.shift {
                    *fine += notches;
                } else {
                    *plain += notches;
                }
            });
            false
        }
        _ => true,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notch_is_a_hundredth_of_the_way_round() {
        let config = KnobConfig::percent(0.5);
        let up = turned(&config, 0.5, Turn::notch(&config, false));
        assert!((up - 0.51).abs() < 1e-6);
        let fine = turned(&config, 0.5, Turn::notch(&config, true).times(-1.0));
        assert!((fine - 0.499).abs() < 1e-6);
        let page = turned(&config, 0.5, Turn::page(&config));
        assert!((page - 0.6).abs() < 1e-6);
        assert_eq!(turned(&config, 0.995, Turn::page(&config)), 1.0);
    }

    #[test]
    fn a_log_knob_turns_round_its_travel() {
        let config = KnobConfig::frequency(20.0, 20000.0, 1000.0);
        let up = turned(&config, 1000.0, Turn::notch(&config, false));
        assert!((config.travel(up) - config.travel(1000.0) - 0.01).abs() < 1e-4);
        // A notch at the top of the range moves further in Hz than at the bottom
        let low = turned(&config, 100.0, Turn::notch(&config, false)) - 100.0;
        let high = turned(&config, 10000.0, Turn::notch(&config, false)) - 10000.0;
        assert!(high > low * 10.0);
    }

    #[test]
    fn a_stepped_knob_moves_whole_steps() {
        let config = KnobConfig { range: 1.0..=64.0, default: 8.0, stepped: true, ..Default::default() };
        assert_eq!(turned(&config, 8.0, Turn::notch(&config, false)), 9.0);
        assert_eq!(turned(&config, 8.0, Turn::notch(&config, true).times(-1.0)), 7.0);
        assert_eq!(turned(&config, 8.0, Turn::page(&config)), 14.0);
        assert_eq!(turned(&config, 1.0, Turn::page(&config).times(-1.0)), 1.0);
        // A small range still pages by a step
        let small = KnobConfig { range: 0.0..=3.0, stepped: true, ..Default::default() };
        assert_eq!(turned(&small, 1.0, Turn::page(&small)), 2.0);
    }
}
