//! Opus, for Ogg Opus sample files. symphonia 0.5 finds an Opus stream in
//! an Ogg file and splits it into packets, but has no decoder for them:
//! this is one, over the pure-Rust `opus-decoder`, so the browser build can
//! play them too.
//!
//! symphonia trims neither end of an Opus stream, so this does. It drops
//! the encoder's pre-skip from the start, and stops where the last page's
//! granule position (symphonia's `n_frames`) says the sound ends, less the
//! pre-skip (RFC 7845, 4.).

use opus_decoder::OpusMultistreamDecoder;
use symphonia::core::audio::{AsAudioBufferRef, AudioBuffer, AudioBufferRef, Signal, SignalSpec};
use symphonia::core::codecs::{
    CodecDescriptor, CodecParameters, Decoder, DecoderOptions, FinalizeResult, CODEC_TYPE_OPUS,
};
use symphonia::core::errors::{decode_error, unsupported_error, Result};
use symphonia::core::formats::Packet;

/// Opus always decodes at 48 kHz, whatever rate was fed to the encoder.
const RATE: u32 = 48_000;

/// The longest a packet can be: 120 ms.
const MAX_FRAMES: usize = RATE as usize * 120 / 1000;

/// What the Ogg Opus identification header (RFC 7845, 5.1) says.
struct Head {
    channels: usize,
    /// How many frames the encoder laid before the sound, to drop.
    pre_skip: usize,
    /// A gain to apply to everything decoded, in Q7.8 dB.
    gain: i16,
    streams: usize,
    coupled: usize,
    /// Which decoded channel each output channel takes.
    mapping: Vec<u8>,
}

impl Head {
    fn parse(head: &[u8]) -> Option<Head> {
        if head.len() < 19 || &head[..8] != b"OpusHead" {
            return None;
        }
        let channels = head[9] as usize;
        let pre_skip = u16::from_le_bytes([head[10], head[11]]) as usize;
        let gain = i16::from_le_bytes([head[16], head[17]]);
        match head[18] {
            // One stream, mono or stereo
            0 if channels <= 2 => Some(Head { channels, pre_skip, gain, streams: 1, coupled: channels - 1, mapping: vec![0, 1] }),
            0 => None,
            _ => {
                let mapping = head.get(21..21 + channels)?.to_vec();
                Some(Head { channels, pre_skip, gain, streams: head[19] as usize, coupled: head[20] as usize, mapping })
            }
        }
    }
}

/// Where Vorbis channel order (which Opus uses) puts each channel in
/// symphonia's, so the first two are always front left and right.
fn symphonia_channel(channels: usize, channel: usize) -> usize {
    let order: &[usize] = match channels {
        3 => &[0, 2, 1],
        5 => &[0, 2, 1, 3, 4],
        6 => &[0, 2, 1, 4, 5, 3],
        7 => &[0, 2, 1, 5, 6, 4, 3],
        8 => &[0, 2, 1, 6, 7, 4, 5, 3],
        _ => return channel,
    };
    order[channel]
}

pub struct OpusDecoder {
    params: CodecParameters,
    decoder: OpusMultistreamDecoder,
    channels: usize,
    /// How much of the pre-skip is still to come.
    skip: usize,
    /// How many frames of sound are left to give, if the file says.
    left: Option<usize>,
    gain: f32,
    /// One packet, interleaved, as it comes out of the decoder.
    interleaved: Vec<f32>,
    buffer: AudioBuffer<f32>,
}

impl Decoder for OpusDecoder {
    fn try_new(params: &CodecParameters, _: &DecoderOptions) -> Result<Self> {
        if params.codec != CODEC_TYPE_OPUS {
            return unsupported_error("opus: not an Opus stream");
        }
        let Some(head) = params.extra_data.as_deref().and_then(Head::parse) else {
            return unsupported_error("opus: an identification header it can't read");
        };
        let mapping = &head.mapping[..head.channels];
        let Ok(decoder) = OpusMultistreamDecoder::new(RATE, head.channels, head.streams, head.coupled, mapping) else {
            return unsupported_error("opus: a channel mapping it can't decode");
        };
        let Some(layout) = params.channels else {
            return unsupported_error("opus: no channel layout");
        };
        Ok(OpusDecoder {
            params: params.clone(),
            decoder,
            channels: head.channels,
            skip: head.pre_skip,
            left: params.n_frames.map(|n| (n as usize).saturating_sub(head.pre_skip)),
            gain: 10f32.powf(head.gain as f32 / 256.0 / 20.0),
            interleaved: vec![0.0; MAX_FRAMES * head.channels],
            buffer: AudioBuffer::new(MAX_FRAMES as u64, SignalSpec::new(RATE, layout)),
        })
    }

    fn supported_codecs() -> &'static [CodecDescriptor] {
        &[CodecDescriptor {
            codec: CODEC_TYPE_OPUS,
            short_name: "opus",
            long_name: "Opus",
            inst_func: |params, options| Ok(Box::new(OpusDecoder::try_new(params, options)?)),
        }]
    }

    fn reset(&mut self) {
        self.decoder.reset();
    }

    fn codec_params(&self) -> &CodecParameters {
        &self.params
    }

    fn decode(&mut self, packet: &Packet) -> Result<AudioBufferRef<'_>> {
        let Ok(frames) = self.decoder.decode_float(&packet.data, &mut self.interleaved, false) else {
            return decode_error("opus: a damaged packet");
        };
        self.buffer.clear();
        self.buffer.render_reserved(Some(frames));
        for channel in 0..self.channels {
            let samples = self.interleaved[..frames * self.channels].iter().skip(channel).step_by(self.channels);
            let out = self.buffer.chan_mut(symphonia_channel(self.channels, channel));
            for (out, sample) in out.iter_mut().zip(samples) {
                *out = sample * self.gain;
            }
        }
        // The pre-skip can span more than one packet
        let skip = self.skip.min(frames);
        self.skip -= skip;
        self.buffer.shift(skip);
        if let Some(left) = &mut self.left {
            self.buffer.truncate(*left);
            *left -= self.buffer.frames();
        }
        Ok(self.buffer.as_audio_buffer_ref())
    }

    fn finalize(&mut self) -> FinalizeResult {
        FinalizeResult::default()
    }

    fn last_decoded(&self) -> AudioBufferRef<'_> {
        self.buffer.as_audio_buffer_ref()
    }
}

