//! Opus, for Ogg Opus sample files. symphonia 0.6 finds an Opus stream in
//! an Ogg file and splits it into packets, but has no decoder for them:
//! this is one, over the pure-Rust `opus-decoder`, so the browser build can
//! play them too.
//!
//! symphonia's Ogg reader marks where the last packet runs past the sound's
//! end, but not the encoder's pre-skip at the start (RFC 7845, 4.): this
//! drops that itself.

use opus_decoder::OpusMultistreamDecoder;
use symphonia::core::audio::{AsGenericAudioBufferRef, AudioBuffer, AudioMut, AudioSpec, GenericAudioBufferRef};
use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions, FinalizeResult};
use symphonia::core::codecs::registry::{RegisterableAudioDecoder, SupportedAudioCodec};
use symphonia::core::codecs::CodecInfo;
use symphonia::core::errors::{decode_error, unsupported_error, Result};
use symphonia::core::packet::PacketRef;

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
    params: AudioCodecParameters,
    decoder: OpusMultistreamDecoder,
    channels: usize,
    gain: f32,
    /// Whether to trim what lies before and after the sound.
    gapless: bool,
    /// How much of the pre-skip is still to come.
    skip: usize,
    /// One packet, interleaved, as it comes out of the decoder.
    interleaved: Vec<f32>,
    buffer: AudioBuffer<f32>,
}

impl OpusDecoder {
    fn try_new(params: &AudioCodecParameters, options: &AudioDecoderOptions) -> Result<Self> {
        if params.codec != CODEC_ID_OPUS {
            return unsupported_error("opus: not an Opus stream");
        }
        let Some(head) = params.extra_data.as_deref().and_then(Head::parse) else {
            return unsupported_error("opus: an identification header it can't read");
        };
        let mapping = &head.mapping[..head.channels];
        let Ok(decoder) = OpusMultistreamDecoder::new(RATE, head.channels, head.streams, head.coupled, mapping) else {
            return unsupported_error("opus: a channel mapping it can't decode");
        };
        let Some(layout) = params.channels.clone() else {
            return unsupported_error("opus: no channel layout");
        };
        Ok(OpusDecoder {
            params: params.clone(),
            decoder,
            channels: head.channels,
            gain: 10f32.powf(head.gain as f32 / 256.0 / 20.0),
            gapless: options.gapless,
            skip: head.pre_skip,
            interleaved: vec![0.0; MAX_FRAMES * head.channels],
            buffer: AudioBuffer::new(AudioSpec::new(RATE, layout), MAX_FRAMES),
        })
    }

    fn decode_packet(&mut self, packet: &PacketRef<'_>) -> Result<()> {
        let Ok(frames) = self.decoder.decode_float(packet.data, &mut self.interleaved, false) else {
            return decode_error("opus: a damaged packet");
        };
        self.buffer.clear();
        self.buffer.render_uninit(Some(frames));
        for channel in 0..self.channels {
            let samples = self.interleaved[..frames * self.channels].iter().skip(channel).step_by(self.channels);
            let Some(out) = self.buffer.plane_mut(symphonia_channel(self.channels, channel)) else {
                return decode_error("opus: a channel the layout doesn't have");
            };
            for (out, sample) in out.iter_mut().zip(samples) {
                *out = sample * self.gain;
            }
        }
        if self.gapless {
            // The pre-skip can span more than one packet. Should the reader
            // ever mark it, take its word instead
            if packet.trim_start.get() > 0 {
                self.skip = 0;
            }
            let skip = self.skip.min(frames);
            self.skip -= skip;
            self.buffer.trim(skip + packet.trim_start.get() as usize, packet.trim_end.get() as usize);
        }
        Ok(())
    }
}

impl AudioDecoder for OpusDecoder {
    fn reset(&mut self) {
        self.decoder.reset();
    }

    fn codec_info(&self) -> &CodecInfo {
        &Self::supported_codecs()[0].info
    }

    fn codec_params(&self) -> &AudioCodecParameters {
        &self.params
    }

    fn decode_ref(&mut self, packet: &PacketRef<'_>) -> Result<GenericAudioBufferRef<'_>> {
        if let Err(e) = self.decode_packet(packet) {
            self.buffer.clear();
            return Err(e);
        }
        Ok(self.buffer.as_generic_audio_buffer_ref())
    }

    fn finalize(&mut self) -> FinalizeResult {
        FinalizeResult::default()
    }

    fn last_decoded(&self) -> GenericAudioBufferRef<'_> {
        self.buffer.as_generic_audio_buffer_ref()
    }
}

impl RegisterableAudioDecoder for OpusDecoder {
    fn try_registry_new(params: &AudioCodecParameters, options: &AudioDecoderOptions) -> Result<Box<dyn AudioDecoder>> {
        Ok(Box::new(OpusDecoder::try_new(params, options)?))
    }

    fn supported_codecs() -> &'static [SupportedAudioCodec] {
        &[SupportedAudioCodec { id: CODEC_ID_OPUS, info: CodecInfo { short_name: "opus", long_name: "Opus", profiles: &[] } }]
    }
}
