//! WAV file I/O for ASR pipelines.
//!
//! `read_wav` decodes a WAV file into mono f32 samples resampled to 16 kHz,
//! matching the format expected by the Sherpa-ONNX ASR backends. `write_wav`
//! encodes f32 samples as 16-bit integer PCM. The 16 kHz target is baked in
//! because every consumer in this crate (recording, transcription, denoising)
//! operates at 16 kHz; a more general API would force each caller to repeat the
//! mono + resample pipeline that previously lived in `takusu-audio-cli`.

use std::path::Path;

use thiserror::Error;

/// Sample rate required by Sherpa-ONNX and used by all ASR backends in this
/// crate. Every recording, transcription, and denoising path operates at this
/// rate.
pub const SHERPA_SAMPLE_RATE: u32 = 16000;

/// Divisor for normalizing `i16` PCM samples to the `[-1.0, 1.0]` f32 range.
/// `i16::MAX + 1` as a float.
pub const I16_MAX_F32: f32 = 32768.0;

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("wav: {0}")]
    Wav(String),
    #[error("unsupported wav bit depth: {0}")]
    UnsupportedBitDepth(u16),
}

impl From<hound::Error> for AudioError {
    fn from(e: hound::Error) -> Self {
        Self::Wav(e.to_string())
    }
}

/// Read a WAV file and return mono f32 samples resampled to 16 kHz.
///
/// Multi-channel files are downmixed by averaging channels. Integer samples
/// are normalized by `2^(bits-1)`. hound sign-extends 8/24-bit samples into
/// the next wider integer type, so the `2^(bits-1)` divisor is correct for
/// every supported bit depth. The output is always mono 16 kHz regardless of
/// the input format.
pub fn read_wav(path: &Path) -> Result<Vec<f32>, AudioError> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let bits = spec.bits_per_sample;
            if bits == 0 || bits > 32 {
                return Err(AudioError::UnsupportedBitDepth(bits));
            }
            // hound decodes 8/16-bit integer WAVs as i16 and 24/32-bit as
            // i32, so decoding with the wrong type produces garbage or
            // errors. Compute the normalization divisor via u64 to avoid
            // u32 overflow for >16-bit.
            if bits <= 16 {
                let max_val = (1u32 << (bits - 1)) as f32;
                reader
                    .samples::<i16>()
                    .map(|s| Ok(s? as f32 / max_val))
                    .collect::<Result<_, hound::Error>>()?
            } else {
                let max_val = (1u64 << (bits - 1)) as f32;
                reader
                    .samples::<i32>()
                    .map(|s| Ok(s? as f32 / max_val))
                    .collect::<Result<_, hound::Error>>()?
            }
        }
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<_, hound::Error>>()?,
    };

    let samples = if spec.channels > 1 {
        mix_to_mono(&samples, spec.channels as usize)
    } else {
        samples
    };

    if spec.sample_rate != SHERPA_SAMPLE_RATE {
        Ok(resample(&samples, spec.sample_rate, SHERPA_SAMPLE_RATE))
    } else {
        Ok(samples)
    }
}

/// Write f32 samples as a 16-bit mono PCM WAV file.
///
/// If any sample exceeds full scale (`|s| > 1.0`), the whole buffer is
/// normalized by `32767.0 / max` so relative amplitude is preserved. The
/// final `clamp` is only a safety net for floating-point edge cases; under
/// the normalization above it is a no-op for `|s| <= 1.0` inputs.
pub fn write_wav(path: &Path, samples: &[f32], sample_rate: u32) -> Result<(), AudioError> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = hound::WavWriter::create(path, spec)?;

    let max = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    let scale = if max > 1.0 { 32767.0 / max } else { 32767.0 };

    for &s in samples {
        let clamped = (s * scale).clamp(-I16_MAX_F32, 32767.0);
        writer.write_sample(clamped as i16)?;
    }

    writer.finalize()?;
    Ok(())
}

/// Downmix interleaved multi-channel samples to mono by averaging channels.
///
/// Trailing samples that do not form a complete frame are silently dropped
/// (matching the previous `chunks_exact` behavior). Panics in debug builds
/// if `channels == 0`; callers should guard with `channels > 1` first.
pub fn mix_to_mono(input: &[f32], channels: usize) -> Vec<f32> {
    debug_assert!(channels > 0, "channels must be non-zero");
    if channels <= 1 {
        return input.to_vec();
    }
    let len = input.len() / channels;
    let mut mono = Vec::with_capacity(len);
    for i in 0..len {
        let mut sum = 0.0f32;
        for c in 0..channels {
            sum += input[i * channels + c];
        }
        mono.push(sum / channels as f32);
    }
    mono
}

/// Linear-interpolation resampling between integer sample rates.
///
/// Uses naive linear interpolation with **no anti-aliasing filter**. This is
/// adequate for the ASR pipelines in this crate (which only downmix/upsample
/// between common device rates) but will alias on strong downsample. For
/// higher-quality resampling, use a dedicated resampler.
pub fn resample(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if input.is_empty() || from_rate == to_rate {
        return input.to_vec();
    }
    let ratio = to_rate as f64 / from_rate as f64;
    let output_len = ((input.len() as f64) * ratio).ceil() as usize;
    let mut output = Vec::with_capacity(output_len);
    for i in 0..output_len {
        let src = i as f64 / ratio;
        let idx = src.floor() as usize;
        let frac = src - idx as f64;
        let s0 = input.get(idx).copied().unwrap_or(0.0);
        let s1 = input.get(idx + 1).copied().unwrap_or(s0);
        output.push((s0 as f64 + (s1 as f64 - s0 as f64) * frac) as f32);
    }
    output
}

/// Scale `input` so its RMS matches `target_rms`. Returns the input unchanged
/// if it is empty or silent.
pub fn normalize(input: &[f32], target_rms: f32) -> Vec<f32> {
    if input.is_empty() {
        return input.to_vec();
    }
    let rms = {
        let sum_sq: f32 = input.iter().map(|&x| x * x).sum();
        (sum_sq / input.len() as f32).sqrt()
    };
    if rms < 1e-10 {
        return input.to_vec();
    }
    let scale = target_rms / rms;
    input.iter().map(|&x| x * scale).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_wav_int(path: &std::path::Path, bits: u16, samples: &[f32]) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: SHERPA_SAMPLE_RATE,
            bits_per_sample: bits,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        let max_val = (1u64 << (bits - 1)) as f32;
        for &s in samples {
            let scaled = (s * max_val) as i32;
            if bits <= 16 {
                writer.write_sample(scaled as i16).unwrap();
            } else {
                writer.write_sample(scaled).unwrap();
            }
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn read_wav_16bit_normalizes_correctly() {
        let dir = std::env::temp_dir();
        let path = dir.join("takusu-wav-16.wav");
        // Avoid full-scale 1.0 which overflows i16 on write.
        write_wav_int(&path, 16, &[0.0, 0.5, -0.5, 0.9]);
        let out = read_wav(&path).unwrap();
        assert_eq!(out.len(), 4);
        assert!((out[0]).abs() < 1e-4);
        assert!((out[1] - 0.5).abs() < 1e-3);
        assert!((out[2] + 0.5).abs() < 1e-3);
        assert!((out[3] - 0.9).abs() < 1e-3);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn read_wav_32bit_normalizes_correctly() {
        let dir = std::env::temp_dir();
        let path = dir.join("takusu-wav-32.wav");
        write_wav_int(&path, 32, &[0.0, 0.25, -0.25, 0.9]);
        let out = read_wav(&path).unwrap();
        assert_eq!(out.len(), 4);
        assert!((out[0]).abs() < 1e-5);
        assert!((out[1] - 0.25).abs() < 1e-4);
        assert!((out[2] + 0.25).abs() < 1e-4);
        assert!((out[3] - 0.9).abs() < 1e-4);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn read_wav_8bit_normalizes_correctly() {
        let dir = std::env::temp_dir();
        let path = dir.join("takusu-wav-8.wav");
        // hound sign-extends (not left-shifts) 8-bit samples into i16, so the
        // 2^(bits-1)=128 divisor is correct. This test documents that.
        write_wav_int(&path, 8, &[0.0, 0.5, -0.5, 0.9]);
        let out = read_wav(&path).unwrap();
        assert_eq!(out.len(), 4);
        assert!((out[0]).abs() < 1e-2);
        assert!((out[1] - 0.5).abs() < 2e-2);
        assert!((out[2] + 0.5).abs() < 2e-2);
        assert!((out[3] - 0.9).abs() < 2e-2);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn read_wav_24bit_normalizes_correctly() {
        let dir = std::env::temp_dir();
        let path = dir.join("takusu-wav-24.wav");
        // hound sign-extends 24-bit samples into i32, so 2^(bits-1) is correct.
        write_wav_int(&path, 24, &[0.0, 0.25, -0.25, 0.9]);
        let out = read_wav(&path).unwrap();
        assert_eq!(out.len(), 4);
        assert!((out[0]).abs() < 1e-5);
        assert!((out[1] - 0.25).abs() < 1e-4);
        assert!((out[2] + 0.25).abs() < 1e-4);
        assert!((out[3] - 0.9).abs() < 1e-4);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn write_wav_roundtrip_preserves_samples() {
        let dir = std::env::temp_dir();
        let path = dir.join("takusu-wav-write.wav");
        let samples = vec![0.0, 0.25, -0.25, 0.9];
        write_wav(&path, &samples, SHERPA_SAMPLE_RATE).unwrap();
        let out = read_wav(&path).unwrap();
        assert_eq!(out.len(), samples.len());
        for (a, b) in samples.iter().zip(out.iter()) {
            assert!((a - b).abs() < 1e-3, "{a} vs {b}");
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn write_wav_clamps_overscale_without_panicking() {
        let dir = std::env::temp_dir();
        let path = dir.join("takusu-wav-overscale.wav");
        // Samples beyond [-1, 1] must not panic; the whole buffer is
        // normalized to fit so relative amplitude is preserved.
        write_wav(&path, &[2.0, -2.0, 1.0], SHERPA_SAMPLE_RATE).unwrap();
        let out = read_wav(&path).unwrap();
        assert_eq!(out.len(), 3);
        assert!((out[0] - 1.0).abs() < 1e-3);
        assert!((out[1] + 1.0).abs() < 1e-3);
        assert!((out[2] - 0.5).abs() < 1e-3);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn mix_to_mono_averages_channels() {
        let stereo = vec![0.0, 1.0, 0.5, 0.5, -1.0, 1.0];
        let mono = mix_to_mono(&stereo, 2);
        assert_eq!(mono, vec![0.5, 0.5, 0.0]);
    }

    #[test]
    fn mix_to_mono_mono_passthrough() {
        let mono_in = vec![0.1, 0.2, 0.3];
        assert_eq!(mix_to_mono(&mono_in, 1), mono_in);
    }

    #[test]
    fn resample_doubles_length_on_upsample() {
        let input = vec![0.0, 1.0, 0.0, -1.0];
        let out = resample(&input, 8000, 16000);
        assert_eq!(out.len(), 8);
    }

    #[test]
    fn resample_halves_length_on_downsample() {
        let input = vec![0.0_f32; 1600];
        let out = resample(&input, 16000, 8000);
        assert_eq!(out.len(), 800);
    }

    #[test]
    fn resample_noop_when_rates_equal() {
        let input = vec![0.1, 0.2, 0.3];
        assert_eq!(resample(&input, 16000, 16000), input);
    }

    #[test]
    fn normalize_scales_to_target_rms() {
        let input = vec![0.5, -0.5, 0.5, -0.5];
        let out = normalize(&input, 0.1);
        let rms = (out.iter().map(|&x| x * x).sum::<f32>() / out.len() as f32).sqrt();
        assert!((rms - 0.1).abs() < 1e-6);
    }

    #[test]
    fn normalize_silent_input_unchanged() {
        let input = vec![0.0, 0.0, 0.0];
        assert_eq!(normalize(&input, 0.1), input);
    }

    #[test]
    fn normalize_empty_input_unchanged() {
        let input: Vec<f32> = vec![];
        assert_eq!(normalize(&input, 0.1), input);
    }
}
