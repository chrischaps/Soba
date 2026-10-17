//! Data types for the node graph.
//!
//! Wraps SignalType to implement egui_node_graph2's DataTypeTrait.
//!
//! # Connection Compatibility
//!
//! Equality is exact: Audio equals Audio and nothing else. Which outputs may
//! feed which inputs is directional (Gate may feed Control, but not the other
//! way round), so it lives in `DataTypeTrait::can_connect`, which the editor
//! asks as (output, input) to light up, snap to and connect ports. It follows
//! the same rules as `validate_connection`, so a cable the editor offers is
//! never one the app then refuses.

use std::borrow::Cow;
use eframe::egui::Color32;
use egui_node_graph2::DataTypeTrait;

use crate::dsp::SignalType;
use super::SynthGraphState;

/// Wrapper around SignalType for the node graph library.
///
/// This implements DataTypeTrait to define how signal types are displayed
/// and how connections are validated in the graph editor.
///
/// Equality is exact. Whether one type may feed another is `can_connect`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SynthDataType(pub SignalType);

impl SynthDataType {
    /// Create a new SynthDataType from a SignalType.
    pub fn new(signal_type: SignalType) -> Self {
        Self(signal_type)
    }

    /// Get the underlying SignalType.
    pub fn signal_type(&self) -> SignalType {
        self.0
    }
}

impl From<SignalType> for SynthDataType {
    fn from(signal_type: SignalType) -> Self {
        Self(signal_type)
    }
}

impl From<SynthDataType> for SignalType {
    fn from(data_type: SynthDataType) -> Self {
        data_type.0
    }
}

impl DataTypeTrait<SynthGraphState> for SynthDataType {
    /// Returns the color for this data type in the graph editor.
    ///
    /// Uses the colors defined in SignalType::color().
    fn data_type_color(&self, _user_state: &mut SynthGraphState) -> Color32 {
        self.0.color()
    }

    /// Whether an output of this type may feed an input of type `to`: the
    /// rules of `validate_connection`, so the editor only offers cables the
    /// app will accept.
    fn can_connect(&self, to: &Self) -> bool {
        self.0.can_connect_to(to.0)
    }

    /// Returns the human-readable name for this data type.
    fn name(&self) -> Cow<'_, str> {
        Cow::Borrowed(self.0.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::validate_connection;

    #[test]
    fn test_synth_data_type_creation() {
        let audio = SynthDataType::new(SignalType::Audio);
        assert_eq!(audio.signal_type(), SignalType::Audio);

        let control = SynthDataType(SignalType::Control);
        assert_eq!(control.0, SignalType::Control);
    }

    #[test]
    fn test_from_signal_type() {
        let data_type: SynthDataType = SignalType::Gate.into();
        assert_eq!(data_type.signal_type(), SignalType::Gate);
    }

    #[test]
    fn test_into_signal_type() {
        let data_type = SynthDataType::new(SignalType::Midi);
        let signal_type: SignalType = data_type.into();
        assert_eq!(signal_type, SignalType::Midi);
    }

    #[test]
    fn test_data_type_name() {
        assert_eq!(SynthDataType::new(SignalType::Audio).name(), "Audio");
        assert_eq!(SynthDataType::new(SignalType::Control).name(), "Control");
        assert_eq!(SynthDataType::new(SignalType::Gate).name(), "Gate");
        assert_eq!(SynthDataType::new(SignalType::Midi).name(), "MIDI");
    }

    #[test]
    fn test_data_type_colors_are_unique() {
        let mut state = SynthGraphState::default();

        let colors = [
            SynthDataType::new(SignalType::Audio).data_type_color(&mut state),
            SynthDataType::new(SignalType::Control).data_type_color(&mut state),
            SynthDataType::new(SignalType::Gate).data_type_color(&mut state),
            SynthDataType::new(SignalType::Midi).data_type_color(&mut state),
        ];

        for i in 0..colors.len() {
            for j in (i + 1)..colors.len() {
                assert_ne!(colors[i], colors[j], "Data type colors should be unique");
            }
        }
    }

    // Equality is exact; connecting is directional (#314)

    const ALL: [SignalType; 5] = [
        SignalType::Audio,
        SignalType::Control,
        SignalType::Gate,
        SignalType::Midi,
        SignalType::Bus,
    ];

    #[test]
    fn test_equality_is_exact() {
        for a in ALL {
            for b in ALL {
                assert_eq!(
                    SynthDataType::new(a) == SynthDataType::new(b),
                    a == b,
                    "{a:?} vs {b:?}"
                );
            }
        }
    }

    #[test]
    fn test_can_connect_follows_validation() {
        // The editor offers exactly the cables validate_connection accepts
        for from in ALL {
            for to in ALL {
                assert_eq!(
                    SynthDataType::new(from).can_connect(&SynthDataType::new(to)),
                    validate_connection(from, to).is_valid(),
                    "{from:?} -> {to:?}"
                );
            }
        }
    }

    #[test]
    fn test_can_connect_is_directional() {
        let gate = SynthDataType::new(SignalType::Gate);
        let control = SynthDataType::new(SignalType::Control);

        // A Gate may drive a Control input, but a Control output can't
        // snap onto a Gate input
        assert!(gate.can_connect(&control));
        assert!(!control.can_connect(&gate));
    }

    #[test]
    fn test_midi_and_bus_isolated() {
        for isolated in [SignalType::Midi, SignalType::Bus] {
            for other in ALL.into_iter().filter(|&t| t != isolated) {
                let a = SynthDataType::new(isolated);
                let b = SynthDataType::new(other);
                assert!(!a.can_connect(&b), "{isolated:?} -> {other:?}");
                assert!(!b.can_connect(&a), "{other:?} -> {isolated:?}");
            }
        }
    }
}
