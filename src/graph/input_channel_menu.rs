//! The Channel dropdown of a module that listens to the input device
//! (Audio Input, and anything else built with
//! [`input_channels::parameter`]): while a device is open, it offers only
//! the pairs and channels the device has, so a stereo interface shows 1+2,
//! 1 and 2, and a Tascam Model 12 shows its twelve (#450).
//!
//! The choice already made is always offered, even past the device's
//! channels (a patch made on a bigger interface), and says on hover that the
//! device hasn't it. With no device open, every choice is offered.

use crate::dsp::input_channels::{self, InputChannels};

use super::SynthGraphState;

/// The open input device's channel count, or `None` with none open.
pub fn device_channels(user_state: &SynthGraphState) -> Option<usize> {
    user_state.audio_input_name.as_ref()?;
    user_state.audio_input_channels
}

/// Whether a dropdown of `options`, set to `chosen`, lists choice `index`
/// for a device of `device` channels.
pub fn offers(options: &[String], index: usize, chosen: usize, device: Option<usize>) -> bool {
    match device {
        Some(channels) if index != chosen && input_channels::is_choice(options) => InputChannels::from_index(index).fits(channels),
        _ => true,
    }
}

/// What to say on hover when `chosen` names channels the device hasn't.
pub fn missing(options: &[String], chosen: usize, device: Option<usize>) -> Option<String> {
    let channels = device?;
    let choice = InputChannels::from_index(chosen);
    (input_channels::is_choice(options) && !choice.fits(channels)).then(|| {
        let has = if channels == 1 { "one channel".to_string() } else { format!("{channels} channels") };
        format!("The input device has {has}, so {} is silent", choice.label())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> Vec<String> {
        input_channels::LABELS.iter().map(|s| s.to_string()).collect()
    }

    fn offered(device: Option<usize>, chosen: usize) -> Vec<&'static str> {
        let options = options();
        (0..options.len()).filter(|&i| offers(&options, i, chosen, device)).map(|i| input_channels::LABELS[i]).collect()
    }

    #[test]
    fn a_stereo_device_offers_its_pair_and_each_channel() {
        assert_eq!(offered(Some(2), 0), ["1+2", "1", "2"]);
        assert_eq!(offered(Some(1), 0), ["1+2", "1", "2"], "a mono device is heard as two");
        assert_eq!(offered(Some(12), 0).len(), 18);
        assert_eq!(offered(None, 0).len(), input_channels::LABELS.len(), "no device, every choice");
    }

    #[test]
    fn the_choice_made_stays_listed_and_says_why_it_is_silent() {
        let seven = InputChannels::Mono(6).index();
        assert_eq!(offered(Some(2), seven), ["1+2", "1", "2", "7"]);
        assert_eq!(missing(&options(), seven, Some(2)).as_deref(), Some("The input device has 2 channels, so 7 is silent"));
        assert_eq!(missing(&options(), seven, Some(12)), None);
        assert_eq!(missing(&options(), seven, None), None);
    }

    #[test]
    fn other_dropdowns_are_left_alone() {
        let modes: Vec<String> = ["Off", "On", "Auto"].iter().map(|s| s.to_string()).collect();
        assert!((0..3).all(|i| offers(&modes, i, 0, Some(2))));
        assert_eq!(missing(&modes, 2, Some(1)), None);
    }
}
