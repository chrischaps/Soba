//! A take (#489): a presentation as it was played, written as a capture
//! script. Each beat's cues are kept as they fired, timed from the first
//! press, so `soba patch.json --capture take.txt` films the same run frame
//! by frame, in time with the narration recorded live.
//!
//! A beat finished early writes what was left at that moment and a `land`,
//! which is how the runner finished it live. A beat played again drops
//! what was kept since it began: the film holds still where the presenter
//! went back, and the editor cuts the retake there. Hand moves aren't
//! kept, only the beats' cues.

use super::cues::Cue;
use super::presentation::Script;

/// How long before the first press a take sets things as they stood.
pub const OPENING: f64 = -2.0;

/// A beat's start in a take.
#[derive(Debug, Clone, Copy)]
struct Began {
    beat: usize,
    /// Seconds from the first press.
    at: f64,
    /// Its first cue in `cues`.
    from: usize,
    /// Played again: where the first try began, which the film holds still
    /// from until this one
    again_from: Option<f64>,
}

/// A presentation's take, kept as it plays.
#[derive(Debug, Default)]
pub struct Take {
    /// When the first beat began, on the app's clock.
    origin: Option<f64>,
    /// Cues that set things as they stood at the first press, once known.
    opening: Option<Vec<String>>,
    /// Every cue the beats fired, in seconds from the first press.
    cues: Vec<(f64, Cue)>,
    began: Vec<Began>,
}

impl Take {
    /// Beat `beat` began at `now`, on the app's clock.
    pub fn begin(&mut self, beat: usize, now: f64) {
        let origin = *self.origin.get_or_insert(now);
        self.began.push(Began { beat, at: now - origin, from: self.cues.len(), again_from: None });
    }

    /// Beat `beat` began again at `now`: what was kept since it last began
    /// is dropped.
    pub fn again(&mut self, beat: usize, now: f64) {
        let mut again_from = None;
        if let Some(at) = self.began.iter().rposition(|b| b.beat == beat) {
            // A second retake holds from the first try, not the second
            again_from = Some(self.began[at].again_from.unwrap_or(self.began[at].at));
            self.cues.truncate(self.began[at].from);
            self.began.truncate(at);
        }
        self.begin(beat, now);
        if let Some(began) = self.began.last_mut() {
            began.again_from = again_from;
        }
    }

    /// Keeps cues a beat fired, timed from its start at `started` on the
    /// app's clock.
    pub fn keep(&mut self, started: f64, fired: Vec<(f64, Cue)>) {
        let Some(origin) = self.origin else { return };
        self.cues.extend(fired.into_iter().map(|(at, cue)| (started - origin + at, cue)));
    }

    /// Whether the first beat has begun.
    pub fn started(&self) -> bool {
        self.origin.is_some()
    }

    /// Whether the cues that set things up before the first press are in.
    pub fn opened(&self) -> bool {
        self.opening.is_some()
    }

    /// Cues, as a script writes them, that set things as they stood at the
    /// first press.
    pub fn open(&mut self, cues: Vec<String>) {
        self.opening = Some(cues);
    }

    /// How long the take has run at `now`.
    pub fn length(&self, now: f64) -> Option<f64> {
        self.origin.map(|origin| (now - origin).max(0.0))
    }

    /// The take as a capture script, ending at `now`; `None` before the
    /// first press. `patch` is the patch's file, for the line that films it.
    pub fn script(&self, script: &Script, patch: Option<&str>, now: f64) -> Option<String> {
        let stopped = self.length(now)?;
        let end = stopped.max(self.cues.last().map_or(0.0, |(at, _)| *at)) + 0.5;
        let title = script.title.as_deref().unwrap_or("a presentation");
        let mut out = format!("# A take of {title}: {} beats in {}, as presented\n", self.began.len(), clock(end));
        out += &format!("# Film it as it played:  soba {} --capture take.txt --out take\n", patch.unwrap_or("patch.json"));
        out += "# Only the beats' cues are in it: knobs turned and the view moved by hand aren't\n";
        if let Some(opening) = self.opening.as_ref().filter(|cues| !cues.is_empty()) {
            out += "\n# As things stood at the first press\n";
            for cue in opening {
                out += &format!("{OPENING:.3}  {cue}\n");
            }
        }
        for (i, began) in self.began.iter().enumerate() {
            let beat = &script.beats[began.beat];
            out += &format!("\n# Beat {} · {}, at {}\n", began.beat + 1, beat.name, clock(began.at));
            if let Some(from) = began.again_from {
                // For the editor (and take_markers.py): where the retake's cut goes
                out += &format!("# Played again: the film holds still from {}, where the first try began\n", clock(from));
            }
            for say in &beat.say {
                out += &format!("#   {say}\n");
            }
            let to = self.began.get(i + 1).map_or(self.cues.len(), |next| next.from);
            for (k, (at, cue)) in self.cues.iter().enumerate().take(to).skip(began.from) {
                let line = match cue {
                    Cue::Note { channel, note, velocity, on: true } => {
                        // Held until its end, or until the presentation stopped
                        let off = self.cues[k + 1..]
                            .iter()
                            .find(|(_, c)| matches!(c, Cue::Note { channel: ch, note: n, on: false, .. } if ch == channel && n == note))
                            .map_or(stopped.max(*at), |(off, _)| *off);
                        Some(format!("note {note} {velocity} {:.3} {}", off - at, channel + 1))
                    }
                    Cue::Note { on: false, .. } | Cue::SecondaryButton(false) => continue,
                    cue => cue.script(),
                };
                match line {
                    Some(line) => out += &format!("{at:.3}  {line}\n"),
                    None => out += &format!("# {at:.3}  (a typed \" can't be written in a script)\n"),
                }
            }
        }
        out += &format!("\n{end:.3}  end\n");
        Some(out)
    }
}

/// Seconds as m:ss.s.
fn clock(seconds: f64) -> String {
    format!("{}:{:04.1}", (seconds / 60.0).floor(), seconds % 60.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::cues::{parse_cues, Mode};
    use crate::app::presentation;

    fn cues(text: &str) -> Vec<(f64, Cue)> {
        parse_cues(text, Mode::Beat).unwrap()
    }

    #[test]
    fn a_take_is_each_beat_where_it_fired() {
        let script = presentation::parse("title Filters\nbeat Open\nsay Listen.\n0 play\nbeat Close\n0 note 60 100 0.5").unwrap();
        let mut take = Take::default();
        assert!(take.script(&script, None, 5.0).is_none(), "nothing before the first press");
        take.begin(0, 10.0);
        take.open(vec!["param filter.svf Cutoff 800 0".into()]);
        take.keep(10.0, cues("0 play"));
        take.begin(1, 14.25);
        take.keep(14.25, cues("0 note 60 100 0.5"));
        let text = take.script(&script, Some("patches/x.json"), 20.0).unwrap();
        assert!(text.contains("soba patches/x.json --capture"), "{text}");
        assert!(text.contains("-2.000  param filter.svf Cutoff 800 0"), "{text}");
        assert!(text.contains("# Beat 1 · Open, at 0:00.0\n#   Listen.\n0.000  play"), "{text}");
        assert!(text.contains("4.250  note 60 100 0.500 1\n"), "{text}");
        assert!(text.ends_with("10.500  end\n"), "{text}");
        // ...and films as it reads
        let film = parse_cues(&text, Mode::Film).unwrap();
        let times: Vec<f64> = film.iter().map(|(at, _)| *at).collect();
        assert_eq!(times, [-2.0, 0.0, 4.25, 4.75, 10.5]);
    }

    #[test]
    fn a_beat_played_again_drops_what_it_did() {
        let script = presentation::parse("beat A\n0 play\nbeat B\n0 stop\n1 note 60 100 3").unwrap();
        let mut take = Take::default();
        take.begin(0, 0.0);
        take.keep(0.0, cues("0 play"));
        take.begin(1, 2.0);
        // B's note is let go as B is played again: it lasts to where it was
        take.keep(2.0, cues("0 stop\n1 note 60 100 3").into_iter().take(2).collect());
        take.again(1, 6.0);
        take.keep(6.0, cues("0 stop"));
        let text = take.script(&script, None, 8.0).unwrap();
        let film = parse_cues(&text, Mode::Film).unwrap();
        let fired: Vec<String> = film.iter().map(|(at, cue)| format!("{at} {}", cue.script().unwrap_or_else(|| "note".into()))).collect();
        assert_eq!(fired, ["0 play", "6 stop", "8.5 end"], "{text}");
        assert_eq!(text.matches("# Beat 2").count(), 1);
        // ...and says where the editor cuts the first try
        assert!(text.contains("# Beat 2 · B, at 0:06.0\n# Played again: the film holds still from 0:02.0,"), "{text}");
        assert!(!text.contains("# Beat 1 · A, at 0:00.0\n# Played again"), "{text}");
    }

    #[test]
    fn a_second_retake_holds_from_the_first_try() {
        let script = presentation::parse("beat A\n0 play\nbeat B\n0 stop").unwrap();
        let mut take = Take::default();
        take.begin(0, 0.0);
        take.begin(1, 2.0);
        take.again(1, 5.0);
        take.again(1, 9.0);
        let text = take.script(&script, None, 10.0).unwrap();
        assert!(text.contains("# Beat 2 · B, at 0:09.0\n# Played again: the film holds still from 0:02.0,"), "{text}");
    }

    #[test]
    fn a_note_still_held_lasts_to_the_end() {
        let script = presentation::parse("beat A\n0 note 60 100 30").unwrap();
        let mut take = Take::default();
        take.begin(0, 0.0);
        take.keep(0.0, cues("0 note 60 100 30").into_iter().take(1).collect());
        let text = take.script(&script, None, 4.0).unwrap();
        assert!(text.contains("0.000  note 60 100 4.000 1"), "{text}");
    }
}
