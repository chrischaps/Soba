//! The message on the left of the status bar: what the last action did, or
//! why it couldn't. It stays up for a while and then goes, so the warnings
//! and hints beneath it get their turn (#308). An error stays longer than
//! news, and is drawn in red rather than green.

use eframe::egui::{self, RichText};

use super::theme;

/// How serious a message is, least first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// What an action did: "Saved", "Pasted 3 modules".
    Info,
    /// Something to know about that didn't stop anything.
    Warning,
    /// An action that failed: "Save failed", "Couldn't paste".
    Error,
}

impl Severity {
    /// How long a message of this severity stays up once it has been seen.
    pub fn seconds(self) -> f64 {
        match self {
            Severity::Info => 5.0,
            Severity::Warning | Severity::Error => 12.0,
        }
    }
}

/// A message in the status bar, and when it was first shown.
#[derive(Clone, Debug)]
pub struct StatusLine {
    pub text: String,
    pub severity: Severity,
    /// When it was first drawn, in egui's clock; NaN until then.
    shown_at: f64,
}

impl StatusLine {
    pub fn new(text: impl Into<String>, severity: Severity) -> Self {
        Self { text: text.into(), severity, shown_at: f64::NAN }
    }

    /// Starts its clock, if it hasn't started: it's on screen at `now`.
    pub fn shown(&mut self, now: f64) {
        if self.shown_at.is_nan() {
            self.shown_at = now;
        }
    }

    /// How many seconds it has left at `now`, none once it has run out. One
    /// that hasn't been on screen yet has all of its time left.
    pub fn remaining(&self, now: f64) -> Option<f64> {
        let seen = if self.shown_at.is_nan() { 0.0 } else { now - self.shown_at };
        let left = self.severity.seconds() - seen;
        (left > 0.0).then_some(left)
    }
}

/// Keeps `line` for another frame, or drops it once its time is up. News
/// starts its clock when it's raised, as it's stale soon after; a warning or
/// an error waits until it has been seen. Returns how long until the next
/// repaint is needed to take it down.
pub fn tick(line: &mut Option<StatusLine>, now: f64) -> Option<f64> {
    if let Some(line) = line.as_mut().filter(|line| line.severity == Severity::Info) {
        line.shown(now);
    }
    let left = line.as_ref().and_then(|line| line.remaining(now));
    if left.is_none() {
        *line = None;
    }
    left
}

/// Draws `line` in the status bar. Returns whether it was clicked away.
pub fn draw(ui: &mut egui::Ui, line: &StatusLine) -> bool {
    let text = match line.severity {
        Severity::Info => RichText::new(&line.text).color(theme::accent::SUCCESS),
        Severity::Warning => RichText::new(format!("⚠ {}", line.text)).color(theme::accent::WARNING),
        Severity::Error => RichText::new(format!("⚠ {}", line.text)).color(theme::accent::ERROR),
    };
    ui.add(egui::Label::new(text.small()).sense(egui::Sense::click()))
        .on_hover_text("Click to dismiss")
        .clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_goes_after_its_time_counted_from_when_it_was_first_shown() {
        let mut line = Some(StatusLine::new("Opened example: First Sound", Severity::Info));
        // Raised long before the first frame: the clock starts when it's seen
        assert_eq!(tick(&mut line, 100.0), Some(5.0));
        assert!(tick(&mut line, 104.0).is_some());
        assert!(line.is_some());
        assert_eq!(tick(&mut line, 105.5), None);
        assert!(line.is_none(), "it doesn't stay up forever");
    }

    #[test]
    fn an_error_waits_to_be_seen() {
        let mut error = Some(StatusLine::new("Can't use ASIO", Severity::Error));
        // Hidden behind something else for a long while
        assert!(tick(&mut error, 0.0).is_some());
        assert!(tick(&mut error, 60.0).is_some());
        error.as_mut().unwrap().shown(60.0);
        assert!(tick(&mut error, 71.0).is_some());
        assert!(tick(&mut error, 73.0).is_none());
    }

    #[test]
    fn an_error_stays_up_longer_than_news() {
        let mut news = Some(StatusLine::new("Saved", Severity::Info));
        let mut error = Some(StatusLine::new("Save failed", Severity::Error));
        tick(&mut news, 0.0);
        error.as_mut().unwrap().shown(0.0);
        assert!(tick(&mut news, 8.0).is_none());
        assert!(tick(&mut error, 8.0).is_some());
        assert!(Severity::Error > Severity::Warning && Severity::Warning > Severity::Info);
    }
}
