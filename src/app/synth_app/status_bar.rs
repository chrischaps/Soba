//! Which message the status bar shows, and how long each stays up (#308).
//! Every message but the patch's counts times out, so one that says what an
//! action did can't hide the warnings and refusals beneath it for good.

use super::*;

/// What the left of the status bar shows, most pressing first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Shown {
    /// The prompt to move a CC, for as long as MIDI Learn waits for one.
    Learn,
    /// An action that failed, or a warning raised as the status.
    Failure,
    /// Why a cable was refused.
    Refusal,
    /// A warning: the input may hear the speakers, a driver reset.
    Notice,
    /// What the last action did.
    News,
    AudioError,
    MidiError,
    /// How many modules and cables the patch has, or how to add one.
    Counts,
}

impl SynthApp {
    /// Takes up news raised since it last looked as the status bar's status.
    pub(super) fn take_news(&mut self) {
        if let Some(text) = self.status_message.take() {
            self.status = Some(StatusLine::new(text, Severity::Info));
        }
    }

    pub(super) fn status_bar_shows(&mut self) -> Shown {
        let failed = self.status.as_ref().is_some_and(|line| line.severity >= Severity::Warning);
        if self.is_midi_learning() {
            Shown::Learn
        } else if failed {
            Shown::Failure
        } else if self.user_state.validation_message().is_some() {
            Shown::Refusal
        } else if self.notice.is_some() {
            Shown::Notice
        } else if self.status.is_some() {
            Shown::News
        } else if self.audio.error.is_some() {
            Shown::AudioError
        } else if self.midi.error.is_some() {
            Shown::MidiError
        } else {
            Shown::Counts
        }
    }

    /// Starts the clock of whatever the bar shows at `now`, and takes down
    /// what has been up long enough. Returns how long until something next
    /// has to come down.
    pub(super) fn tick_status_bar(&mut self, now: f64) -> Option<f64> {
        self.take_news();
        match self.status_bar_shows() {
            Shown::Failure | Shown::News => self.status.as_mut().map(|line| line.shown(now)),
            Shown::Notice => self.notice.as_mut().map(|line| line.shown(now)),
            _ => None,
        };
        let status = status_line::tick(&mut self.status, now);
        let notice = status_line::tick(&mut self.notice, now);
        status.into_iter().chain(notice).reduce(f64::min)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the status bar's clock from `from` to `to`, a frame at a time.
    fn run(app: &mut SynthApp, from: f64, to: f64) {
        let mut now = from;
        while now <= to {
            app.tick_status_bar(now);
            now += 0.1;
        }
    }

    #[test]
    fn opening_an_example_doesnt_hold_the_status_bar() {
        let mut app = SynthApp::new();
        app.status_message = Some("Opened example: First Sound".to_string());
        app.tick_status_bar(0.0);
        assert_eq!(app.status_bar_shows(), Shown::News);
        run(&mut app, 0.0, 6.0);
        assert!(app.status.is_none(), "news goes after a while");
        assert_ne!(app.status_bar_shows(), Shown::News);
    }

    #[test]
    fn a_notice_shows_once_the_status_message_has_gone() {
        let mut app = SynthApp::new();
        app.status_error("Save failed: the disk is full");
        app.raise_notice("The input may be hearing the speakers".to_string());
        app.tick_status_bar(0.0);
        assert_eq!(app.status_bar_shows(), Shown::Failure);
        run(&mut app, 0.0, 11.5);
        assert_eq!(app.status_bar_shows(), Shown::Failure, "an error stays up a while");

        // The notice's time counts from when it's seen, not from when it was raised
        run(&mut app, 11.5, 13.0);
        assert_eq!(app.status_bar_shows(), Shown::Notice);
        run(&mut app, 13.0, 24.0);
        assert_eq!(app.status_bar_shows(), Shown::Notice, "it gets its whole time");
        run(&mut app, 24.0, 26.0);
        assert!(app.notice.is_none());
    }

    #[test]
    fn a_warning_comes_before_news_and_an_error_before_both() {
        let mut app = SynthApp::new();
        app.raise_notice("The driver's settings changed: audio restarted".to_string());
        app.status_message = Some("Saved: patch.json".to_string());
        app.tick_status_bar(0.0);
        assert_eq!(app.status_bar_shows(), Shown::Notice);
        app.status_error("Couldn't paste: the clipboard is empty");
        assert_eq!(app.status_bar_shows(), Shown::Failure);
        // News raised after an error replaces it
        app.status_message = Some("Pasted 2 modules".to_string());
        app.take_news();
        assert_eq!(app.status_bar_shows(), Shown::Notice);
    }
}
