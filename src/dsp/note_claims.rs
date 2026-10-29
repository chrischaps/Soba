//! Notes a Pads module claims (#428), so a Note In on Omni lets them be.
//!
//! A pad controller sends notes, and without claims every hit would also
//! play whatever synth an Omni Note In drives. A Pads module claims the
//! notes its pads listen to, on their channels, and a Note In whose Channel
//! is Omni skips those. A Note In set to a channel hears exactly that
//! channel, claimed or not. The UI's piano ears and lit keys skip them too.
//!
//! A claim is the Pads' Device's too (#454): a Pads set to the MiniLab
//! claims the MiniLab's note 36 on channel 10, and the Fantom's note 36 on
//! channel 10 still plays. A claim covers just what the Pads hears: the
//! notes of its device (of every device, on Any) and of Soba's own
//! ([`SOBA_PORT`]), which every module hears.
//!
//! The claims are a fixed bitset, a `u128` per MIDI channel per input, so
//! building it, clearing it and reading it never allocate on the audio
//! thread. The computer keys' channel ([`KEYS_CHANNEL`]) is never claimed:
//! the keys always play.
//!
//! [`KEYS_CHANNEL`]: super::KEYS_CHANNEL

use super::signal::{ListenTo, MIDI_INPUTS, SOBA_PORT};

/// The channels' words of one input's claims.
type Channels = [u128; 16];

/// Which notes, on which MIDI channels, from which inputs, a Pads module
/// listens to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoteClaims {
    /// For each input slot, then Soba's own: bit `n` of channel `c`'s word
    /// is set when note `n` on channel `c` (0-15, as on the wire) is claimed.
    ports: [Channels; MIDI_INPUTS + 1],
}

/// How a module works out the notes it claims, from its parameters and
/// the device it listens to (see [`DspModule::note_claims`]).
///
/// [`DspModule::note_claims`]: super::DspModule::note_claims
pub type ClaimNotes = fn(&[f32], ListenTo, &mut NoteClaims);

/// No claims: what a module sees outside a compiled graph.
pub static NO_CLAIMS: NoteClaims = NoteClaims::EMPTY;

/// Where in [`NoteClaims::ports`] the claims of `port` are.
#[inline]
fn slot(port: u8) -> Option<usize> {
    match port {
        SOBA_PORT => Some(MIDI_INPUTS),
        port if (port as usize) < MIDI_INPUTS => Some(port as usize),
        _ => None,
    }
}

impl NoteClaims {
    /// Nothing claimed.
    pub const EMPTY: NoteClaims = NoteClaims { ports: [[0; 16]; MIDI_INPUTS + 1] };

    /// Claims `note` on `channel` (0-15) for a Pads listening to `listen`:
    /// from its device and from Soba's own. `None` claims it on every
    /// channel, for a Pads module on Omni. A channel past 15 (the computer
    /// keys') is never claimed.
    #[inline]
    pub fn claim(&mut self, listen: ListenTo, channel: Option<u8>, note: u8) {
        let bit = 1u128 << (note & 127);
        let claim_on = |channels: &mut Channels| match channel {
            Some(channel) => {
                if let Some(word) = channels.get_mut(channel as usize) {
                    *word |= bit;
                }
            }
            None => {
                for word in channels.iter_mut() {
                    *word |= bit;
                }
            }
        };
        match listen {
            ListenTo::Any => self.ports.iter_mut().for_each(claim_on),
            ListenTo::AllBut(skipped) => {
                for (at, channels) in self.ports.iter_mut().enumerate() {
                    if at >= MIDI_INPUTS || skipped & 1 << at == 0 {
                        claim_on(channels);
                    }
                }
            }
            ListenTo::Port(port) => {
                if let Some(at) = slot(port).filter(|&at| at < MIDI_INPUTS) {
                    claim_on(&mut self.ports[at]);
                }
                claim_on(&mut self.ports[MIDI_INPUTS]);
            }
            ListenTo::Unplugged => claim_on(&mut self.ports[MIDI_INPUTS]),
        }
    }

    /// Whether `note` on `channel` (0-15, or the keys'), heard from input
    /// `port` (or [`SOBA_PORT`]), is claimed.
    #[inline]
    pub fn claims(&self, port: u8, channel: u8, note: u8) -> bool {
        slot(port)
            .and_then(|at| self.ports[at].get(channel as usize))
            .is_some_and(|word| word & (1u128 << (note & 127)) != 0)
    }

    /// Lets every claim go.
    #[inline]
    pub fn clear(&mut self) {
        *self = Self::EMPTY;
    }

    /// Adds `other`'s claims on one MIDI channel (0-15) to these: what a
    /// Pads patched from a MIDI In on that channel can hear (#479).
    #[inline]
    pub fn merge_channel(&mut self, other: &NoteClaims, channel: u8) {
        let channel = channel as usize;
        for (mine, theirs) in self.ports.iter_mut().zip(&other.ports) {
            if let (Some(mine), Some(theirs)) = (mine.get_mut(channel), theirs.get(channel)) {
                *mine |= theirs;
            }
        }
    }

    /// Whether nothing is claimed.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.ports.iter().flatten().all(|&word| word == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::KEYS_CHANNEL;

    #[test]
    fn a_claim_is_on_its_channel_and_omni_is_on_all() {
        let mut claims = NoteClaims::EMPTY;
        assert!(claims.is_empty());
        claims.claim(ListenTo::Any, Some(9), 36);
        assert!(claims.claims(0, 9, 36) && claims.claims(7, 9, 36) && claims.claims(SOBA_PORT, 9, 36));
        assert!(!claims.claims(0, 0, 36) && !claims.claims(0, 9, 37));
        claims.claim(ListenTo::Any, None, 127);
        assert!((0..16).all(|channel| claims.claims(3, channel, 127)));
        // The keys always play
        assert!(!claims.claims(SOBA_PORT, KEYS_CHANNEL, 127));
        claims.claim(ListenTo::Any, Some(KEYS_CHANNEL), 60);
        assert!(!claims.claims(SOBA_PORT, KEYS_CHANNEL, 60));
        claims.clear();
        assert_eq!(claims, NO_CLAIMS);
    }

    #[test]
    fn a_claim_is_its_devices_and_soba_s_own() {
        let mut claims = NoteClaims::EMPTY;
        // A Pads set to the device in slot 2
        claims.claim(ListenTo::Port(2), Some(9), 36);
        assert!(claims.claims(2, 9, 36));
        assert!(!claims.claims(0, 9, 36) && !claims.claims(3, 9, 36), "another device's 36 plays");
        assert!(claims.claims(SOBA_PORT, 9, 36), "a render's notes reach the Pads, so they're claimed");
        // Set to a device that isn't open, it hears only Soba's own
        claims.clear();
        claims.claim(ListenTo::Unplugged, None, 40);
        assert!(claims.claims(SOBA_PORT, 5, 40));
        assert!((0..MIDI_INPUTS as u8).all(|port| !claims.claims(port, 5, 40)));
        // A slot past the inputs claims nothing of its own, and is heard as none
        claims.clear();
        claims.claim(ListenTo::Port(42), Some(0), 1);
        assert!(claims.claims(SOBA_PORT, 0, 1) && !claims.claims(42, 0, 1));
    }

    #[test]
    fn any_device_leaves_a_thru_only_input_s_notes_be() {
        let mut claims = NoteClaims::EMPTY;
        // A Pads on any device while the input in slot 1 is Thru only (#463)
        claims.claim(ListenTo::AllBut(0b10), Some(9), 36);
        assert!(claims.claims(0, 9, 36) && claims.claims(7, 9, 36) && claims.claims(SOBA_PORT, 9, 36));
        assert!(!claims.claims(1, 9, 36), "it never hears them, so they play on");
    }

    #[test]
    fn a_pads_on_one_channel_of_a_midi_in_claims_that_channel_alone() {
        // On Omni, from a MIDI In on channel 10 of the device in slot 2 (#479)
        let mut own = NoteClaims::EMPTY;
        own.claim(ListenTo::Port(2), None, 36);
        let mut claims = NoteClaims::EMPTY;
        claims.merge_channel(&own, 9);
        assert!(claims.claims(2, 9, 36) && claims.claims(SOBA_PORT, 9, 36));
        assert!(!claims.claims(2, 0, 36), "channel 1's 36 plays on");
        assert!(!claims.claims(0, 9, 36), "and another device's");
        claims.merge_channel(&own, 99);
        assert!(!claims.claims(2, 15, 36), "a channel past 16 adds nothing");
    }
}
