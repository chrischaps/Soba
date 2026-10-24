//! Reading a value typed into a knob's menu, in the units its readout uses:
//! "1.2 kHz", "250 ms", "-6 dB", "L 40", "C#3", "50%".

use super::knob::ParamFormat;

impl ParamFormat {
    /// The value `text` names, read the way [`ParamFormat::format`] writes
    /// it. A bare number is taken in the units the readout shows: a percent
    /// knob reads "50" as 50%, and a time knob reading "250 ms" now reads
    /// "300" as 300 ms (`current` says which). Nothing for text it can't read.
    pub fn parse(&self, text: &str, current: f32) -> Option<f32> {
        let text = text.trim().to_lowercase();
        let (number, unit) = split_number(&text);
        match self {
            ParamFormat::Raw { .. } | ParamFormat::RawWithUnit { .. } => number,
            ParamFormat::Percent | ParamFormat::BipolarPercent => number.map(|n| n / 100.0),
            ParamFormat::Frequency => {
                let n = number?;
                Some(if unit.starts_with('k') { n * 1000.0 } else { n })
            }
            ParamFormat::Time => parse_seconds(number, unit, current),
            ParamFormat::Milliseconds => parse_seconds(number, unit, current / 1000.0).map(|s| s * 1000.0),
            ParamFormat::Decibels => match (number, unit) {
                (None, "-∞" | "-inf" | "-∞ db" | "-inf db") => Some(f32::NEG_INFINITY),
                (number, _) => number,
            },
            ParamFormat::Semitones => number,
            ParamFormat::Pan => match (number, unit) {
                (None, "c") => Some(0.0),
                (Some(n), "") => Some(n / 100.0),
                (None, side) => {
                    let sign = match side.chars().next()? {
                        'l' => -1.0,
                        'r' => 1.0,
                        _ => return None,
                    };
                    Some(sign * side[1..].trim().parse::<f32>().ok()?.abs() / 100.0)
                }
                _ => None,
            },
            ParamFormat::Note => number.or_else(|| note_from_name(&text).map(f32::from)),
            ParamFormat::OrOff { off, .. } => match (number, unit) {
                (None, "off") => Some(*off),
                (number, _) => number,
            },
            ParamFormat::Slices => match (number, unit) {
                (None, "off") => Some(0.0),
                (None, "onsets") => Some(crate::dsp::primitives::ONSETS),
                (number, _) => number,
            },
        }
    }
}

/// The number `text` starts with, if it does, and the rest of it, trimmed:
/// "1.2 khz" is (1.2, "khz").
fn split_number(text: &str) -> (Option<f32>, &str) {
    let end = text
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || c == '.' || (i == 0 && (c == '-' || c == '+'))))
        .map_or(text.len(), |(i, _)| i);
    match text[..end].parse::<f32>() {
        Ok(number) => (Some(number), text[end..].trim()),
        Err(_) => (None, text),
    }
}

/// Seconds from a time typed "250 ms", "1.5 s" or "off". A bare number is
/// in the unit a value of `current` seconds reads in.
fn parse_seconds(number: Option<f32>, unit: &str, current: f32) -> Option<f32> {
    match (number, unit) {
        (None, "off") => Some(0.0),
        (Some(n), "ms") => Some(n / 1000.0),
        (Some(n), "s" | "sec") => Some(n),
        (Some(n), "") if current >= 1.0 => Some(n),
        (Some(n), "") => Some(n / 1000.0),
        _ => None,
    }
}

/// The MIDI note a name like "c4", "f#2" or "bb-1" names, with middle C
/// as C4 (60), as `note_to_name` writes them.
fn note_from_name(name: &str) -> Option<u8> {
    let mut chars = name.chars();
    let pitch = match chars.next()? {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => return None,
    };
    let rest = chars.as_str();
    let (shift, octave) = match rest.chars().next() {
        Some('#') => (1, &rest[1..]),
        Some('b') => (-1, &rest[1..]),
        _ => (0, rest),
    };
    let octave: i32 = octave.trim().parse().ok()?;
    u8::try_from((octave + 1) * 12 + pitch + shift).ok().filter(|note| *note <= 127)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_back_what_it_writes() {
        let cases = [
            (ParamFormat::Percent, 0.35),
            (ParamFormat::BipolarPercent, -0.2),
            (ParamFormat::Frequency, 1250.0),
            (ParamFormat::Frequency, 440.0),
            (ParamFormat::Time, 0.25),
            (ParamFormat::Time, 1.5),
            (ParamFormat::Time, 0.0),
            (ParamFormat::Milliseconds, 120.0),
            (ParamFormat::Decibels, -6.5),
            (ParamFormat::Semitones, -12.0),
            (ParamFormat::Semitones, 7.0),
            (ParamFormat::Pan, -0.4),
            (ParamFormat::Pan, 0.0),
            (ParamFormat::Pan, 0.25),
            (ParamFormat::Note, 61.0),
            (ParamFormat::Slices, 8.0),
            (ParamFormat::Raw { decimals: 2 }, 0.75),
            (ParamFormat::RawWithUnit { decimals: 0, unit: "BPM" }, 120.0),
        ];
        for (format, value) in cases {
            let text = format.format(value);
            let read = format.parse(&text, value).unwrap_or_else(|| panic!("{format:?} couldn't read {text:?}"));
            assert!((read - value).abs() < 1e-4, "{format:?}: {text:?} read as {read}, not {value}");
        }
    }

    #[test]
    fn a_bare_number_is_in_the_readouts_units() {
        assert_eq!(ParamFormat::Percent.parse("50", 0.1), Some(0.5));
        assert_eq!(ParamFormat::Frequency.parse("2k", 440.0), Some(2000.0));
        assert_eq!(ParamFormat::Pan.parse("-30", 0.0), Some(-0.3));
        // A time showing milliseconds takes milliseconds, one showing seconds seconds
        assert_eq!(ParamFormat::Time.parse("300", 0.25), Some(0.3));
        assert_eq!(ParamFormat::Time.parse("2", 1.5), Some(2.0));
        assert_eq!(ParamFormat::Time.parse("2 s", 0.25), Some(2.0));
        assert_eq!(ParamFormat::Milliseconds.parse("40", 120.0), Some(40.0));
    }

    #[test]
    fn names_and_words() {
        assert_eq!(ParamFormat::Note.parse("C4", 0.0), Some(60.0));
        assert_eq!(ParamFormat::Note.parse("bb2", 0.0), Some(46.0));
        assert_eq!(ParamFormat::Note.parse("C-1", 0.0), Some(0.0));
        assert_eq!(ParamFormat::Note.parse("H4", 0.0), None);
        assert_eq!(ParamFormat::Pan.parse("C", 0.3), Some(0.0));
        assert_eq!(ParamFormat::Pan.parse("l40", 0.0), Some(-0.4));
        assert_eq!(ParamFormat::Time.parse("Off", 0.5), Some(0.0));
        assert_eq!(ParamFormat::Decibels.parse("-inf", 0.0), Some(f32::NEG_INFINITY));
        assert_eq!(ParamFormat::Slices.parse("Onsets", 0.0), Some(crate::dsp::primitives::ONSETS));
        assert_eq!(ParamFormat::Percent.parse("lots", 0.5), None);
        assert_eq!(ParamFormat::Percent.parse("", 0.5), None);
    }
}
