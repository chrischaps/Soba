//! Each shipped profile's page in the manual (`docs/src/hardware/`),
//! written from the profile itself so the two can't drift apart. A test
//! checks every page matches; `UPDATE_SNAPSHOTS=1` rewrites them.

use std::fmt::Write;

use super::{Address, ButtonMode, Kind, Naming, Profile, Step, Voices};

/// How many parameters an Instrument shows by default.
const FACE: usize = 12;
/// The most named values a page lists in full.
const LISTED: usize = 12;

/// The page's file name: the profile's own file's, as Markdown.
pub fn file_name(json_name: &str) -> String {
    json_name.replace(".json", ".md")
}

/// The manual's page for `profile`.
pub fn page(profile: &Profile) -> String {
    let mut out = String::new();
    let w = &mut out;
    let _ = writeln!(w, "# {}\n", profile.name);
    let kind = match profile.kind {
        Kind::Synth => "synth",
        Kind::Kit => "kit",
    };
    let voices = match profile.voices {
        Some(Voices::Mono) => ", mono".to_string(),
        Some(Voices::Poly(n)) => format!(", {n} voices"),
        Some(Voices::Paraphonic(n)) => format!(", paraphonic, {n} voices"),
        None => String::new(),
    };
    let channels = match profile.channels.as_slice() {
        [one] => format!("channel {one}"),
        many => format!("channels {}–{}", many[0], many[many.len() - 1]),
    };
    let _ = writeln!(w, "*{} · {kind}{voices} · {channels}*\n", profile.maker);
    if !profile.about.is_empty() {
        let _ = writeln!(w, "{}\n", profile.about);
    }
    let _ = writeln!(w, "> **From the maker's chart:** {}\n", profile.source);
    if !profile.notes.is_empty() {
        let _ = writeln!(w, "## Before you play\n\n{}\n", profile.notes);
    }

    if !profile.parameters.is_empty() {
        let _ = writeln!(w, "## Parameters\n");
        let own = if profile.parameters.iter().any(|p| p.channel.is_some()) { ", except those with a channel of their own" } else { "" };
        let per_part = if profile.cc_per_part { format!(". Each part takes them on its own channel{own}") } else { String::new() };
        let _ = writeln!(w, "The first {FACE} are an Instrument's knobs when it's first made{per_part}.\n");
        let _ = writeln!(w, "| | Parameter | Message | Group | Values |");
        let _ = writeln!(w, "|---|---|---|---|---|");
        for (i, p) in profile.parameters.iter().enumerate() {
            let face = if i < FACE { "●" } else { "" };
            let mut values = if p.stepped() {
                match &p.values[..] {
                    // A long run reads as its ends: "Algo 1, Algo 2 … Algo 32"
                    [first, second, .., last] if p.values.len() > LISTED => format!("{}, {} … {}", first.label(), second.label(), last.label()),
                    steps => steps.iter().map(Step::label).collect::<Vec<_>>().join(", "),
                }
            } else if p.selects {
                "one of several, as a number".to_string()
            } else if p.range != (0, p.address.max()) {
                format!("{}–{}", p.range.0, p.range.1)
            } else {
                String::new()
            };
            if p.relative {
                values = [values, format!("an offset: {} is no change", p.centre())].into_iter().filter(|v| !v.is_empty()).collect::<Vec<_>>().join("; ");
            }
            if let Some(default) = p.default.filter(|&d| !(p.relative && d == p.centre())) {
                values = [values, format!("starts at {default}")].into_iter().filter(|v| !v.is_empty()).collect::<Vec<_>>().join("; ");
            }
            if !crate::app::mutate::moves(p) {
                values = [values, "Mutate leaves it".to_string()].into_iter().filter(|v| !v.is_empty()).collect::<Vec<_>>().join("; ");
            } else if p.picks() {
                values = [values, "Mutate moves it only with Ctrl+Alt+M".to_string()].into_iter().filter(|v| !v.is_empty()).collect::<Vec<_>>().join("; ");
            }
            let mut message = match p.address {
                Address::Cc14 { .. } => format!("{} (14-bit)", p.address.text()),
                address => address.text(),
            };
            if let Some(channel) = p.channel {
                let _ = write!(message, " (ch {channel})");
            }
            let _ = writeln!(w, "| {face} | {} | {message} | {} | {values} |", p.name, p.group);
        }
        let _ = writeln!(w);
    }

    if !profile.parts.is_empty() {
        let _ = writeln!(w, "## Parts\n");
        if profile.kind == Kind::Kit {
            let tracks = if profile.cc_per_part { ", and its knobs turn whichever track you choose, or each a track of its own" } else { "" };
            let _ = writeln!(w, "A [Kit](../modules/midi/kit.md)'s eight lanes play the first eight when it's first made{tracks}. A part without a note is struck with middle C (60).\n");
        }
        let _ = writeln!(w, "| Part | Note | Channel |");
        let _ = writeln!(w, "|---|---|---|");
        for part in &profile.parts {
            let or_dash = |v: Option<u8>| v.map_or("–".to_string(), |v| v.to_string());
            let _ = writeln!(w, "| {} | {} | {} |", part.name, or_dash(part.note), or_dash(part.channel));
        }
        let _ = writeln!(w);
    }

    if !profile.buttons.is_empty() {
        let _ = writeln!(w, "## Buttons\n");
        let _ = writeln!(w, "A gate presses a momentary button while it's open, and flips a toggle each time it opens.\n");
        let _ = writeln!(w, "| Button | CC | Kind | On / off |");
        let _ = writeln!(w, "|---|---|---|---|");
        for b in &profile.buttons {
            let mode = match b.mode {
                ButtonMode::Momentary => "momentary",
                ButtonMode::Toggle => "toggle",
            };
            let channel = b.channel.map(|c| format!(" (ch {c})")).unwrap_or_default();
            let _ = writeln!(w, "| {} | {}{channel} | {mode} | {} / {} |", b.name, b.cc, b.on, b.off);
        }
        let _ = writeln!(w);
    }

    if let Some(programs) = &profile.programs {
        let _ = writeln!(w, "## Programs\n");
        let first = programs.label(0);
        let last = programs.label(programs.count() - 1);
        let what = programs.what.to_lowercase();
        let style = match programs.naming {
            Naming::Numbered { .. } => "",
            Naming::Lettered { .. } => ", a letter a bank",
        };
        let channel = programs.channel.map(|c| format!(" on channel {c}")).unwrap_or_default();
        let _ = writeln!(w, "A program change{channel} chooses a {what}, shown {first} to {last}{style}.");
        if programs.bank_by_channel {
            let (from, _) = programs.message(0, profile.channel());
            let (to, _) = programs.message(programs.count() - 1, profile.channel());
            let _ = writeln!(w, "Each bank is on its own channel: {} on channel {from} to {} on channel {to}.", &first[..1], &last[..1]);
        }
        if !programs.banks.is_empty() {
            let banks = programs.banks.iter().map(|b| format!("{} (MSB {}{})", b.name, b.msb, b.lsb.map(|l| format!(", LSB {l}")).unwrap_or_default()));
            let _ = writeln!(w, "Bank Select reaches {}.", banks.collect::<Vec<_>>().join(", "));
        }
        let _ = writeln!(w);
    }

    let _ = writeln!(w, "## Clock\n");
    let yes = |b: bool| if b { "yes" } else { "no" };
    let clock = profile.clock;
    let _ = writeln!(w, "Sends MIDI clock: {} · follows it: {} · starts and stops with it: {}", yes(clock.sends), yes(clock.receives), yes(clock.transport));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::BUNDLED;
    use std::path::Path;

    #[test]
    fn every_shipped_profile_has_its_page_in_the_manual() {
        let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/src/hardware");
        let update = std::env::var_os("UPDATE_SNAPSHOTS").is_some();
        let index = std::fs::read_to_string(folder.join("instruments.md")).unwrap_or_default();
        for bundled in &BUNDLED {
            let profile = Profile::from_json(bundled.json).unwrap();
            let name = file_name(bundled.file_name);
            let path = folder.join(&name);
            let want = page(&profile);
            if update {
                std::fs::write(&path, &want).unwrap();
            } else {
                let have = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
                assert_eq!(have, want, "{name} is out of date: UPDATE_SNAPSHOTS=1 cargo test --lib hardware::pages");
            }
            assert!(index.contains(&format!("(./{name})")), "the Instrument Profiles page links {name}");
        }
    }
}
