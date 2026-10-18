//! Note lengths at the tempo: the divisions a module's dropdown offers.
//!
//! Each module keeps its own table of [`Division`]s, in the order its
//! dropdown lists them (patches save the index, so new ones go on the end),
//! but builds it from the lengths here, so a label always comes with its
//! length and "1/8T" means the same thing everywhere.

/// A dropdown entry: its label, and its length in beats (quarter notes),
/// or `None` for Off.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Division {
    pub label: &'static str,
    pub beats: Option<f64>,
}

impl Division {
    const fn new(label: &'static str, beats: f64) -> Self {
        Self { label, beats: Some(beats) }
    }
}

/// Not locked to the tempo.
pub const OFF: Division = Division { label: "Off", beats: None };

// Bars are four beats. LFO counts its long cycles in bars; Clock and
// Chopper call the same four beats a whole note, "1".
pub const FOUR_BARS: Division = Division::new("4 bars", 16.0);
pub const TWO_BARS: Division = Division::new("2 bars", 8.0);
pub const BAR: Division = Division::new("1 bar", 4.0);
pub const WHOLE: Division = Division::new("1", 4.0);

pub const HALF: Division = Division::new("1/2", 2.0);
pub const HALF_DOTTED: Division = Division::new("1/2D", 3.0);
pub const HALF_TRIPLET: Division = Division::new("1/2T", 4.0 / 3.0);
pub const QUARTER: Division = Division::new("1/4", 1.0);
pub const QUARTER_DOTTED: Division = Division::new("1/4D", 1.5);
pub const QUARTER_TRIPLET: Division = Division::new("1/4T", 2.0 / 3.0);
pub const EIGHTH: Division = Division::new("1/8", 0.5);
pub const EIGHTH_DOTTED: Division = Division::new("1/8D", 0.75);
pub const EIGHTH_TRIPLET: Division = Division::new("1/8T", 1.0 / 3.0);
pub const SIXTEENTH: Division = Division::new("1/16", 0.25);
pub const SIXTEENTH_DOTTED: Division = Division::new("1/16D", 0.375);
pub const SIXTEENTH_TRIPLET: Division = Division::new("1/16T", 1.0 / 6.0);
pub const THIRTY_SECOND: Division = Division::new("1/32", 0.125);

/// A table's labels, for its dropdown.
pub const fn labels<const N: usize>(table: &[Division; N]) -> [&'static str; N] {
    let mut labels = [""; N];
    let mut i = 0;
    while i < N {
        labels[i] = table[i].label;
        i += 1;
    }
    labels
}

/// The length in beats of the division at `index`, or `None` for Off or
/// past the end.
pub fn beats(table: &[Division], index: usize) -> Option<f64> {
    table.get(index).and_then(|division| division.beats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_follow_the_table() {
        assert_eq!(labels(&[OFF, QUARTER, EIGHTH_TRIPLET]), ["Off", "1/4", "1/8T"]);
    }

    #[test]
    fn beats_by_index() {
        let table = [OFF, QUARTER_DOTTED];
        assert_eq!(beats(&table, 0), None);
        assert_eq!(beats(&table, 1), Some(1.5));
        assert_eq!(beats(&table, 2), None);
    }
}
