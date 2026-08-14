//! Sample audio decode/resample/encode. Replaces the C# original's dependency on BASS (a
//! proprietary Windows audio library) with pure-Rust decoding via `symphonia`, plus a
//! hand-written linear-interpolation resampler and the MOD/XM-specific PCM encoders (mirrors
//! `AudioEncUtil.cs`/`BassWrapper.cs`).
//!
//! Byte-exact parity with BASS's proprietary resampler is not a goal (and not achievable without
//! reimplementing it) -- output is expected to sound equivalent, not be bit-identical.

use std::io::Read;
use std::path::Path;

use regex::RegexBuilder;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::common::VolumeScalingMode;
use crate::error::{Error, Result};
use crate::mod_format::{
    resolve_sample_rate, EncodedSample, ModSampleSource, ModSettings, ProTrackerCompatibility,
};
use crate::model::SampleData;
use crate::xm_format::{EncodedXmSample, XmSampleSource, XmSettings};

/// Decoded PCM, interleaved, normalized to `[-1.0, 1.0]` f32 samples.
pub struct DecodedAudio {
    pub sample_rate: u32,
    pub channels: u32,
    /// The source's bit depth, when known. Some lossy/transform-coded formats (MP3, Vorbis) have
    /// no fixed native bit depth; when unknown, callers should assume 16 (a reasonable real-world
    /// default -- the original fell back to 8 here, which was BASS's own quirk rather than a
    /// meaningful choice, since decoded samples carry full-precision audio regardless of the
    /// source's nominal bit depth; this field only affects loop-point rescaling bookkeeping).
    pub bits_per_sample: Option<u32>,
    /// Interleaved samples, `channels` per frame.
    pub interleaved: Vec<f32>,
}

impl DecodedAudio {
    pub fn frame_count(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.interleaved.len() / self.channels as usize
        }
    }
}

/// Locates the embedded sample audio file for `instrument_index`/`sample_index` inside an
/// `.xrns` zip and decodes it. Returns `Ok(None)` if no matching sample file exists (a genuinely
/// empty/missing sample -- silence, not an error).
pub fn decode_embedded_sample(
    xrns_path: &Path,
    instrument_index: usize,
    sample_index: usize,
) -> Result<Option<DecodedAudio>> {
    let file = std::fs::File::open(xrns_path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    let pattern = format!(
        r"SampleData/Instrument{:02}.*/Sample{:02}.*\.(wav|aiff?|ogg|flac|mp3|aac)$",
        instrument_index, sample_index
    );
    let re = RegexBuilder::new(&pattern)
        .case_insensitive(true)
        .build()
        .map_err(|e| Error::Conversion(format!("invalid internal sample-lookup pattern: {e}")))?;

    let mut matched_name = None;
    for name in archive.file_names() {
        if re.is_match(name) {
            matched_name = Some(name.to_string());
            break;
        }
    }

    let Some(name) = matched_name else {
        return Ok(None);
    };

    let extension = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if extension == "aac" {
        return Err(Error::Conversion(format!(
            "sample '{name}': AAC extension not supported"
        )));
    }

    let mut bytes = Vec::new();
    archive
        .by_name(&name)
        .map_err(|e| Error::Conversion(format!("failed to read sample '{name}': {e}")))?
        .read_to_end(&mut bytes)?;

    decode_bytes(bytes, &extension).map(Some)
}

fn decode_bytes(bytes: Vec<u8>, extension: &str) -> Result<DecodedAudio> {
    let cursor = std::io::Cursor::new(bytes);
    let mss = MediaSourceStream::new(Box::new(cursor), Default::default());

    let mut hint = Hint::new();
    let normalized_ext = if extension == "aif" {
        "aiff"
    } else {
        extension
    };
    hint.with_extension(normalized_ext);

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| Error::Conversion(format!("failed to probe sample audio: {e}")))?;

    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or_else(|| Error::Conversion("sample audio has no decodable track".to_string()))?
        .clone();

    let bits_per_sample = track.codec_params.bits_per_sample;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| Error::Conversion(format!("failed to create sample audio decoder: {e}")))?;

    let mut interleaved: Vec<f32> = Vec::new();
    let mut sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
    let mut channels = track
        .codec_params
        .channels
        .map(|c| c.count() as u32)
        .unwrap_or(1);

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(symphonia::core::errors::Error::ResetRequired) => break,
            Err(e) => {
                return Err(Error::Conversion(format!(
                    "error reading sample audio: {e}"
                )))
            }
        };
        if packet.track_id() != track.id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                sample_rate = spec.rate;
                channels = spec.channels.count() as u32;

                let mut sample_buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                sample_buf.copy_interleaved_ref(decoded);
                interleaved.extend_from_slice(sample_buf.samples());
            }
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(e) => {
                return Err(Error::Conversion(format!(
                    "error decoding sample audio: {e}"
                )))
            }
        }
    }

    if channels == 0 {
        channels = 1;
    }

    Ok(DecodedAudio {
        sample_rate,
        channels,
        bits_per_sample,
        interleaved,
    })
}

/// Downmixes interleaved multi-channel PCM to mono by averaging channels.
pub fn downmix_to_mono(interleaved: &[f32], channels: u32) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    let channels = channels as usize;
    interleaved
        .chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// Simple linear-interpolation resampler. Not byte-exact with BASS's proprietary windowed-sinc
/// resampler (no independent Rust implementation could be, short of reimplementing BASS itself);
/// standard, adequate quality for tracker sample playback.
pub fn linear_resample(mono: &[f32], src_rate: u32, dst_rate: u32) -> Vec<f32> {
    if mono.is_empty() || src_rate == 0 || dst_rate == 0 || src_rate == dst_rate {
        return mono.to_vec();
    }

    let ratio = src_rate as f64 / dst_rate as f64;
    let dst_len = ((mono.len() as f64) / ratio).round().max(1.0) as usize;
    let mut out = Vec::with_capacity(dst_len);

    for i in 0..dst_len {
        let src_pos = i as f64 * ratio;
        let idx = src_pos.floor() as usize;
        let frac = (src_pos - idx as f64) as f32;

        let a = mono[idx.min(mono.len() - 1)];
        let b = mono[(idx + 1).min(mono.len() - 1)];
        out.push(a + (b - a) * frac);
    }

    out
}

/// Scales every sample by `gain` (used for `VolumeScalingMode::Sample`, mirroring the original's
/// BASS-mixer-envelope volume bake-in).
pub fn apply_gain(samples: &mut [f32], gain: f32) {
    if (gain - 1.0).abs() < f32::EPSILON {
        return;
    }
    for s in samples.iter_mut() {
        *s *= gain;
    }
}

/// Converts normalized `[-1.0, 1.0]` f32 samples to unsigned 8-bit PCM (0-255, 128 = silence).
pub fn to_unsigned_8bit(samples: &[f32]) -> Vec<u8> {
    samples
        .iter()
        .map(|&s| (((s.clamp(-1.0, 1.0) * 127.0) + 128.0).round() as i32).clamp(0, 255) as u8)
        .collect()
}

/// Converts normalized `[-1.0, 1.0]` f32 samples to signed 16-bit PCM.
pub fn to_signed_16bit(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0).round() as i16)
        .collect()
}

/// MOD sample encoding: sign-flips unsigned 8-bit PCM to signed, optionally prefixes up to 2
/// silence bytes for Amiga ProTracker hardware compatibility (real ProTracker-played samples with
/// no loop should begin with two bytes of silence), and pads to an even length (mirrors
/// `BassWrapper.GetModEncodedSample`'s MOD path). Not delta-encoded -- that's XM-only.
pub fn encode_mod_sample(pcm_u8: &[u8], pt_compatibility: ProTrackerCompatibility) -> Vec<u8> {
    let mut out = Vec::with_capacity(pcm_u8.len() + 2);

    if pt_compatibility != ProTrackerCompatibility::None {
        for &b in pcm_u8.iter().take(2) {
            if b != 128 {
                out.push(0);
            }
        }
    }

    for &b in pcm_u8 {
        out.push(b.wrapping_sub(128));
    }

    if out.len() % 2 != 0 {
        out.push(0);
    }

    out
}

/// XM 8-bit delta encoding (mirrors `AudioEncUtil.EncodeDelta8BitMonoSample`). Predictor starts
/// at 128 (unsigned silence), matching the source PCM's own bias.
pub fn encode_xm_delta_8bit_mono(pcm_u8: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pcm_u8.len());
    let mut prev: i32 = 128;
    for &b in pcm_u8 {
        out.push((b as i32 - prev) as u8);
        prev = b as i32;
    }
    out
}

/// XM 8-bit stereo delta encoding: de-interleaves by byte-index parity, delta-encodes each
/// channel independently (predictor 128 each), then concatenates all-of-left then all-of-right
/// (mirrors `AudioEncUtil.EncodeDelta8BitStereoSample`).
pub fn encode_xm_delta_8bit_stereo(pcm_u8_interleaved: &[u8]) -> Vec<u8> {
    let mut left = Vec::with_capacity(pcm_u8_interleaved.len() / 2);
    let mut right = Vec::with_capacity(pcm_u8_interleaved.len() / 2);
    let (mut prev_l, mut prev_r): (i32, i32) = (128, 128);

    for (i, &b) in pcm_u8_interleaved.iter().enumerate() {
        if i % 2 == 0 {
            left.push((b as i32 - prev_l) as u8);
            prev_l = b as i32;
        } else {
            right.push((b as i32 - prev_r) as u8);
            prev_r = b as i32;
        }
    }

    left.extend(right);
    left
}

/// XM 16-bit delta encoding (mirrors `AudioEncUtil.EncodeDelta16BitMonoSample`). Predictor starts
/// at 0 (16-bit samples are already signed).
pub fn encode_xm_delta_16bit_mono(pcm_i16: &[i16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pcm_i16.len() * 2);
    let mut prev: i32 = 0;
    for &s in pcm_i16 {
        out.extend_from_slice(&((s as i32 - prev) as i16).to_le_bytes());
        prev = s as i32;
    }
    out
}

/// XM 16-bit stereo delta encoding: de-interleaves by sample-index parity (correct for 16-bit
/// interleaved stereo, unlike byte-level parity), delta-encodes each channel independently
/// (predictor 0 each), then concatenates all-of-left then all-of-right (mirrors
/// `AudioEncUtil.EncodeDelta16BitStereoSample`).
pub fn encode_xm_delta_16bit_stereo(pcm_i16_interleaved: &[i16]) -> Vec<u8> {
    let mut left = Vec::with_capacity(pcm_i16_interleaved.len());
    let mut right = Vec::with_capacity(pcm_i16_interleaved.len());
    let (mut prev_l, mut prev_r): (i32, i32) = (0, 0);

    for (i, &s) in pcm_i16_interleaved.iter().enumerate() {
        if i % 2 == 0 {
            left.extend_from_slice(&((s as i32 - prev_l) as i16).to_le_bytes());
            prev_l = s as i32;
        } else {
            right.extend_from_slice(&((s as i32 - prev_r) as i16).to_le_bytes());
            prev_r = s as i32;
        }
    }

    left.extend(right);
    left
}

/// Real [`ModSampleSource`] implementation backed by this module's decode/resample/encode
/// pipeline. Reads sample audio straight out of the given `.xrns` file each time it's called.
pub struct XrnsModSampleSource<'a> {
    pub xrns_path: &'a Path,
}

impl ModSampleSource for XrnsModSampleSource<'_> {
    fn encode_sample(
        &self,
        instrument_index: usize,
        sample: &SampleData,
        settings: &ModSettings,
    ) -> Result<Option<EncodedSample>> {
        let Some(decoded) = decode_embedded_sample(self.xrns_path, instrument_index, 0)? else {
            return Ok(None);
        };

        let original_bits_per_sample = decoded.bits_per_sample.unwrap_or(16);
        let original_length_bytes =
            decoded.interleaved.len() as i64 * (original_bits_per_sample as i64 / 8);

        let target_rate = resolve_sample_rate(
            &sample.sample_freq,
            decoded.sample_rate,
            settings.ntsc_mode,
            settings.pro_tracker_compatibility,
        )?;

        let mono = downmix_to_mono(&decoded.interleaved, decoded.channels);
        let mut resampled = linear_resample(&mono, decoded.sample_rate, target_rate);

        if settings.volume_scaling_mode == VolumeScalingMode::Sample {
            apply_gain(&mut resampled, sample.volume);
        }

        let pcm_u8 = to_unsigned_8bit(&resampled);
        let encoded_pcm = encode_mod_sample(&pcm_u8, settings.pro_tracker_compatibility);

        Ok(Some(EncodedSample {
            encoded_pcm,
            sample_rate: target_rate,
            original_length_bytes,
            original_channels: decoded.channels,
            original_bits_per_sample,
        }))
    }
}

/// Real [`XmSampleSource`] implementation. Unlike MOD, keeps the source's native sample rate and
/// channel count (no forced mono/resample) -- only bit depth is clamped to 8 or 16.
pub struct XrnsXmSampleSource<'a> {
    pub xrns_path: &'a Path,
}

impl XmSampleSource for XrnsXmSampleSource<'_> {
    fn encode_sample(
        &self,
        instrument_index: usize,
        sample_index: usize,
        sample: &SampleData,
        settings: &XmSettings,
    ) -> Result<Option<EncodedXmSample>> {
        let Some(decoded) = decode_embedded_sample(self.xrns_path, instrument_index, sample_index)?
        else {
            return Ok(None);
        };

        let origres = decoded.bits_per_sample.unwrap_or(16);
        let bits_per_sample = if origres > 8 { 16 } else { 8 };

        let mut pcm = decoded.interleaved;
        if settings.volume_scaling_mode == VolumeScalingMode::Sample {
            apply_gain(&mut pcm, sample.volume);
        }

        // Tracker sample files are essentially always mono or stereo; anything beyond 2 channels
        // is reduced to the first 2 (matches the original, which never handled >2 channels either
        // -- AudioEncUtil only has mono/stereo encode paths).
        let channels = decoded.channels.min(2);
        let stereo_pcm: Vec<f32> = if channels == 2 && decoded.channels > 2 {
            pcm.chunks(decoded.channels as usize)
                .flat_map(|frame| [frame[0], frame[1]])
                .collect()
        } else {
            pcm
        };

        let encoded_pcm = match (bits_per_sample, channels) {
            (8, 1) => encode_xm_delta_8bit_mono(&to_unsigned_8bit(&stereo_pcm)),
            (8, _) => encode_xm_delta_8bit_stereo(&to_unsigned_8bit(&stereo_pcm)),
            (16, 1) => encode_xm_delta_16bit_mono(&to_signed_16bit(&stereo_pcm)),
            (16, _) => encode_xm_delta_16bit_stereo(&to_signed_16bit(&stereo_pcm)),
            _ => unreachable!("bits_per_sample is always clamped to 8 or 16"),
        };

        Ok(Some(EncodedXmSample {
            encoded_pcm,
            sample_rate: decoded.sample_rate,
            channels,
            bits_per_sample,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmix_averages_stereo_channels() {
        // L=1.0, R=-1.0 -> average 0.0.
        let mono = downmix_to_mono(&[1.0, -1.0, 0.5, 0.5], 2);
        assert_eq!(mono, vec![0.0, 0.5]);
    }

    #[test]
    fn linear_resample_upsamples_to_expected_length() {
        let src = vec![0.0, 1.0, 0.0, -1.0];
        let out = linear_resample(&src, 8000, 16000);
        assert_eq!(out.len(), 8);
    }

    #[test]
    fn linear_resample_is_noop_for_equal_rates() {
        let src = vec![0.1, 0.2, 0.3];
        assert_eq!(linear_resample(&src, 44100, 44100), src);
    }

    #[test]
    fn to_unsigned_8bit_maps_silence_and_extremes() {
        assert_eq!(to_unsigned_8bit(&[0.0]), vec![128]);
        assert_eq!(to_unsigned_8bit(&[1.0]), vec![255]);
        assert_eq!(to_unsigned_8bit(&[-1.0]), vec![1]);
    }

    #[test]
    fn encode_mod_sample_sign_flips_and_pads_to_even_length() {
        // 128 (unsigned silence) -> 0 (signed silence); odd-length input gets one padding byte.
        let encoded = encode_mod_sample(&[128, 255, 0], ProTrackerCompatibility::None);
        assert_eq!(encoded, vec![0, 127, 128, 0]);
    }

    #[test]
    fn encode_mod_sample_prefixes_silence_for_pt_compat() {
        // First two source bytes are non-silence (not 128) -> two 0x00 prefix bytes inserted,
        // then the full sign-flipped payload follows.
        let encoded = encode_mod_sample(&[200, 50], ProTrackerCompatibility::B3Max);
        assert_eq!(
            encoded,
            vec![0, 0, 200u8.wrapping_sub(128), 50u8.wrapping_sub(128)]
        );
    }

    #[test]
    fn xm_delta_8bit_mono_encodes_running_difference_from_128() {
        // 128 (silence) -> 0, then 130 -> 130-128=2, then 120 -> 120-130=-10 (0xF6).
        let encoded = encode_xm_delta_8bit_mono(&[128, 130, 120]);
        assert_eq!(encoded, vec![0, 2, (120i32 - 130i32) as u8]);
    }

    #[test]
    fn xm_delta_8bit_stereo_deinterleaves_by_byte_parity() {
        // L,R,L,R = 130,140,120,150 -> left=[130,120], right=[140,150], each delta from 128.
        let encoded = encode_xm_delta_8bit_stereo(&[130, 140, 120, 150]);
        let left = vec![(130i32 - 128) as u8, (120i32 - 130) as u8];
        let right = vec![(140i32 - 128) as u8, (150i32 - 140) as u8];
        let mut expected = left;
        expected.extend(right);
        assert_eq!(encoded, expected);
    }

    #[test]
    fn xm_delta_16bit_mono_encodes_running_difference_from_zero() {
        let encoded = encode_xm_delta_16bit_mono(&[100, 50]);
        let mut expected = 100i16.to_le_bytes().to_vec();
        expected.extend_from_slice(&(-50i16).to_le_bytes());
        assert_eq!(encoded, expected);
    }

    #[test]
    fn xm_delta_16bit_stereo_deinterleaves_by_sample_parity() {
        // L,R,L,R = 100,200,150,250 -> left=[100,150], right=[200,250], each delta from 0.
        let encoded = encode_xm_delta_16bit_stereo(&[100, 200, 150, 250]);
        let mut expected = 100i16.to_le_bytes().to_vec();
        expected.extend_from_slice(&50i16.to_le_bytes()); // 150-100
        expected.extend_from_slice(&200i16.to_le_bytes());
        expected.extend_from_slice(&50i16.to_le_bytes()); // 250-200
        assert_eq!(encoded, expected);
    }
}
