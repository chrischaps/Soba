//! Notes a Pads module claims (#428), so a Note In on Omni lets them be.
//!
//! A pad controller sends notes, and without claims every hit would also
//! play whatever synth an Omni Note In drives. A Pads module claims the
//! notes its pads listen to, on their channels, and a Note In whose Channel
//! is Omni skips those. A Note In set to a channel hears exactly that
//! channel, claimed or not. The UI's piano ears and lit keys skip them too.
//!
//! The claims are a fixed 16 × 128 bitset, a `u128` per MIDI channel, so
//! building it, clearing it and reading it never allocate on the audio
//! thread. The computer keys' channel ([`KEYS_CHANNEL`]) is never claimed:
//! the keys always play.
//!
//! [`KEYS_CHANNEL`]: super::KEYS_CHANNEL

/// Which notes, on which MIDI channels, a Pads module listens to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoteClaims {
    /// Bit `n` of channel `c`'s word is set when note `n` on channel `c`
    /// (0-15, as on the wire) is claimed.
    channels: [u128; 16],
}

/// No claims: what a module sees outside a compiled graph.
pub static NO_CLAIMS: NoteClaims = NoteClaims::EMPTY;

impl NoteClaims {
    /// Nothing claimed.
    pub const EMPTY: NoteClaims = NoteClaims { channels: [0; 16] };

    /// Claims `note` on `channel` (0-15). `None` claims it on every channel,
    /// for a Pads module on Omni. A channel past 15 (the computer keys') is
    /// never claimed.
    #[inline]
    pub fn claim(&mut self, channel: Option<u8>, note: u8) {
        let bit = 1u128 << (note & 127);
        match channel {
            Some(channel) => {
                if let Some(word) = self.channels.get_mut(channel as usize) {
                    *word |= bit;
                }
            }
            None => {
                for word in &mut self.channels {
                    *word |= bit;
                }
            }
        }
    }

    /// Whether `note` on `channel` (0-15, or the keys') is claimed.
    #[inline]
    pub fn claims(&self, channel: u8, note: u8) -> bool {
        self.channels.get(channel as usize).is_some_and(|word| word & (1u128 << (note & 127)) != 0)
    }

    /// Lets every claim go.
    #[inline]
    pub fn clear(&mut self) {
        self.channels = [0; 16];
    }

    /// Whether nothing is claimed.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.channels.iter().all(|&word| word == 0)
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
        claims.claim(Some(9), 36);
        assert!(claims.claims(9, 36));
        assert!(!claims.claims(0, 36) && !claims.claims(9, 37));
        claims.claim(None, 127);
        assert!((0..16).all(|channel| claims.claims(channel, 127)));
        // The keys always play
        assert!(!claims.claims(KEYS_CHANNEL, 127));
        claims.claim(Some(KEYS_CHANNEL), 60);
        assert!(!claims.claims(KEYS_CHANNEL, 60));
        claims.clear();
        assert_eq!(claims, NO_CLAIMS);
    }
}
