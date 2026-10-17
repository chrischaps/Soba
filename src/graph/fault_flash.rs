//! Showing which module made a NaN.
//!
//! When a module's output goes NaN or infinite, the engine silences it for
//! the block and resets it (see [`crate::dsp::finite`]), then says which
//! module it was. Its title bar flushes red and fades back over
//! [`FLASH_SECONDS`], so a patch that hiccups, or keeps resetting, points at
//! the module responsible rather than going quietly silent.

use std::collections::HashMap;

use egui::Color32;
use web_time::Instant;

use crate::app::theme;
use crate::engine::NodeId as EngineNodeId;

/// How long a fault's red takes to fade from a title bar.
pub const FLASH_SECONDS: f32 = 1.5;

/// When each module last made a NaN, by engine node ID.
#[derive(Default)]
pub struct ModuleFaults {
    last: HashMap<EngineNodeId, Instant>,
}

impl ModuleFaults {
    /// Notes that `node` just faulted. True if it's news: the first fault
    /// since its title bar last faded, worth a word in the status bar. A
    /// module that faults block after block stays lit without repeating it.
    pub fn note(&mut self, node: EngineNodeId) -> bool {
        let now = Instant::now();
        let fresh = self.last.get(&node).is_none_or(|&at| now.duration_since(at).as_secs_f32() >= FLASH_SECONDS);
        self.last.insert(node, now);
        fresh
    }

    /// How lit `node`'s title bar is: 1 just after a fault, fading to 0.
    pub fn glow(&self, node: EngineNodeId) -> f32 {
        self.last.get(&node).map_or(0.0, |at| (1.0 - at.elapsed().as_secs_f32() / FLASH_SECONDS).max(0.0))
    }
}

/// What the status bar says when a module first faults.
pub fn status(name: Option<&str>) -> String {
    format!("{} made an invalid sample (NaN) and was reset", name.unwrap_or("A module"))
}

/// A title bar's `fill`, flushed red by `glow` (0 to 1).
pub fn tint(fill: Color32, glow: f32) -> Color32 {
    if glow <= 0.0 {
        return fill;
    }
    // Eased, so it lingers at red then lets go
    let t = glow * glow * (3.0 - 2.0 * glow);
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    let red = theme::accent::ERROR;
    Color32::from_rgb(mix(fill.r(), red.r()), mix(fill.g(), red.g()), mix(fill.b(), red.b()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fault_is_news_once_until_it_fades() {
        let mut faults = ModuleFaults::default();
        assert_eq!(faults.glow(3), 0.0);
        assert!(faults.note(3));
        assert!(!faults.note(3), "a module faulting again while lit isn't news");
        assert!(faults.note(4), "another module's fault is");
        assert!(faults.glow(3) > 0.9);
    }

    #[test]
    fn tint_runs_from_the_fill_to_red() {
        let fill = Color32::from_rgb(40, 90, 160);
        assert_eq!(tint(fill, 0.0), fill);
        assert_eq!(tint(fill, 1.0), theme::accent::ERROR);
    }
}
