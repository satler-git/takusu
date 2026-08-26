//! Audio playback for WAV buffers and raw PCM streams.
//!
//! Parses WAV data, validates the header and sample format, then plays the audio
//! through the default output device using cpal. Also provides streaming
//! playback for raw PCM byte streams so TTS audio can start playing before the
//! full response is received.

use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::thread;
use std::time::Duration;

use bytes::{Buf, BytesMut};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use futures_util::StreamExt;
use thiserror::Error;

use crate::tts::{TtsError, TtsStream};
use crate::wav::I16_MAX_F32;

#[derive(Debug, Error)]
pub enum PlayError {
    #[error("no output device")]
    NoOutputDevice,
    #[error("wav parse error: {0}")]
    WavParse(String),
    #[error("unsupported wav format: {0}")]
    UnsupportedFormat(String),
    #[error("cpal error: {0}")]
    Cpal(String),
    #[error("tts error: {0}")]
    Tts(#[from] TtsError),
}

impl From<cpal::Error> for PlayError {
    fn from(e: cpal::Error) -> Self {
        Self::Cpal(e.to_string())
    }
}

impl From<hound::Error> for PlayError {
    fn from(e: hound::Error) -> Self {
        Self::WavParse(e.to_string())
    }
}

fn cpal_error(err: cpal::Error) {
    eprintln!("audio playback stream error: {err}");
}

/// A clip parsed from a WAV buffer, ready for playback.
#[derive(Debug, Clone)]
pub struct AudioClip {
    samples: Vec<f32>,
    sample_rate: u32,
    channels: u16,
}

impl AudioClip {
    /// Build a clip from already-decoded samples.
    pub fn from_parts(samples: Vec<f32>, sample_rate: u32, channels: u16) -> Self {
        Self {
            samples,
            sample_rate,
            channels,
        }
    }

    /// Parse a WAV buffer and validate the format.
    ///
    /// Only 16-bit integer PCM, mono or stereo WAVs are supported.
    pub fn from_wav_bytes(bytes: &[u8]) -> Result<Self, PlayError> {
        let mut reader = hound::WavReader::new(Cursor::new(bytes))?;
        let spec = reader.spec();

        if spec.sample_format != hound::SampleFormat::Int {
            return Err(PlayError::UnsupportedFormat(format!(
                "sample_format={:?}",
                spec.sample_format
            )));
        }
        if spec.bits_per_sample != 16 {
            return Err(PlayError::UnsupportedFormat(format!(
                "bits_per_sample={}",
                spec.bits_per_sample
            )));
        }
        if spec.channels == 0 || spec.channels > 2 {
            return Err(PlayError::UnsupportedFormat(format!(
                "channels={}",
                spec.channels
            )));
        }

        let samples: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| Ok(s? as f32 / I16_MAX_F32))
            .collect::<Result<_, hound::Error>>()?;

        Ok(Self {
            samples,
            sample_rate: spec.sample_rate,
            channels: spec.channels,
        })
    }

    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }
}

/// PCM sample format used by a raw audio stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcmFormat {
    /// 16-bit signed little-endian integer samples.
    I16,
    /// 32-bit IEEE 754 little-endian float samples.
    F32,
}

/// Description of a raw PCM stream for streaming playback.
#[derive(Debug, Clone, Copy)]
pub struct StreamedAudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub pcm_format: PcmFormat,
}

/// Play a parsed audio clip on the default output device.
pub fn play(clip: &AudioClip) -> Result<(), PlayError> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or(PlayError::NoOutputDevice)?;
    let supported = device.default_output_config()?;
    let sample_format = supported.sample_format();
    let stream_config: cpal::StreamConfig = supported.into();
    let output_channels = stream_config.channels as usize;
    let output_rate = stream_config.sample_rate;

    let mut samples = clip.samples.clone();
    if clip.sample_rate != output_rate {
        samples = resample_interleaved(
            &samples,
            clip.sample_rate,
            output_rate,
            clip.channels as usize,
        );
    }
    if clip.channels as usize != output_channels {
        samples = convert_channels(&samples, clip.channels as usize, output_channels);
    }

    let buffer = Arc::new(Mutex::new(samples));
    let pos = Arc::new(AtomicUsize::new(0));
    let len = buffer.lock().unwrap().len();

    let stream = match sample_format {
        cpal::SampleFormat::F32 => {
            build_output_stream::<f32>(&device, stream_config, buffer, pos.clone(), cpal_error)?
        }
        cpal::SampleFormat::I16 => {
            build_output_stream::<i16>(&device, stream_config, buffer, pos.clone(), cpal_error)?
        }
        cpal::SampleFormat::U16 => {
            build_output_stream::<u16>(&device, stream_config, buffer, pos.clone(), cpal_error)?
        }
        _ => {
            return Err(PlayError::UnsupportedFormat(format!(
                "output sample format {sample_format:?}"
            )));
        }
    };

    stream.play()?;

    while pos.load(Ordering::Relaxed) < len {
        thread::sleep(Duration::from_millis(10));
    }
    // Give the backend a moment to drain the last frames.
    thread::sleep(Duration::from_millis(100));

    drop(stream);
    Ok(())
}

/// Return the default output device configuration.
pub fn default_output_config() -> Result<cpal::SupportedStreamConfig, PlayError> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or(PlayError::NoOutputDevice)?;
    device.default_output_config().map_err(|e| e.into())
}

/// Play a chunked raw PCM stream on the default output device.
///
/// Audio starts playing as soon as the first complete frame is received. The
/// stream may be mono or stereo and at any sample rate; it is resampled and
/// converted to the output device's configuration on the fly.
pub async fn play_stream(
    mut stream: TtsStream,
    format: StreamedAudioFormat,
    cancel: Arc<AtomicBool>,
) -> Result<(), PlayError> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or(PlayError::NoOutputDevice)?;
    let supported = device.default_output_config()?;
    let output_rate = supported.sample_rate();
    let output_channels = supported.channels();
    let sample_format = supported.sample_format();
    if format.channels == 0 || format.channels > 2 {
        return Err(PlayError::UnsupportedFormat(format!(
            "stream channels={}",
            format.channels
        )));
    }
    if output_channels == 0 || output_channels > 2 {
        return Err(PlayError::UnsupportedFormat(format!(
            "output channels={}",
            output_channels
        )));
    }

    let buffer = Arc::new(Mutex::new(VecDeque::new()));
    let stream_ended = Arc::new(AtomicBool::new(false));
    let buffer_for_thread = Arc::clone(&buffer);
    let stream_ended_for_thread = Arc::clone(&stream_ended);

    let handle = tokio::task::spawn_blocking(move || {
        let stream_config: cpal::StreamConfig = supported.into();

        let stream = match sample_format {
            cpal::SampleFormat::F32 => build_stream_output::<f32>(
                &device,
                stream_config,
                Arc::clone(&buffer_for_thread),
                cpal_error,
            )?,
            cpal::SampleFormat::I16 => build_stream_output::<i16>(
                &device,
                stream_config,
                Arc::clone(&buffer_for_thread),
                cpal_error,
            )?,
            cpal::SampleFormat::U16 => build_stream_output::<u16>(
                &device,
                stream_config,
                Arc::clone(&buffer_for_thread),
                cpal_error,
            )?,
            _ => {
                return Err(PlayError::UnsupportedFormat(format!(
                    "output sample format {sample_format:?}"
                )));
            }
        };

        stream.play()?;
        while !stream_ended_for_thread.load(Ordering::Acquire)
            || !buffer_for_thread.lock().unwrap().is_empty()
        {
            thread::sleep(Duration::from_millis(10));
        }
        // Give the backend a moment to drain the last frames.
        thread::sleep(Duration::from_millis(100));
        drop(stream);
        Ok(())
    });

    let mut resampler = Resampler::new(
        format.channels as usize,
        output_channels as usize,
        format.sample_rate,
        output_rate,
        Arc::clone(&buffer),
    );
    let mut pending = BytesMut::new();
    let mut chunk_samples = Vec::new();
    loop {
        let chunk = tokio::select! {
            chunk = stream.next() => chunk,
            _ = async {
                while !cancel.load(Ordering::Acquire) {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            } => {
                // Cancel requested: stop feeding the stream and signal the
                // cpal thread that playback has ended.
                stream_ended.store(true, Ordering::Release);
                return Ok(());
            }
        };
        let Some(chunk) = chunk else {
            break;
        };
        let chunk = chunk?;
        pending.extend_from_slice(&chunk);
        decode_pcm_chunk(&mut pending, &format, &mut chunk_samples)?;
        if !chunk_samples.is_empty() {
            resampler.feed(&chunk_samples);
        }
    }
    resampler.flush();

    stream_ended.store(true, Ordering::Release);
    handle
        .await
        .map_err(|e| PlayError::Cpal(format!("playback task failed: {e}")))?
}

fn build_output_stream<T: SizedSample + FromSample<f32> + Send + 'static>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    buffer: Arc<Mutex<Vec<f32>>>,
    pos: Arc<AtomicUsize>,
    error_fn: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream, PlayError> {
    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let mut p = pos.load(Ordering::Relaxed);
            let buf = buffer.lock().unwrap();
            for sample in data.iter_mut() {
                if p < buf.len() {
                    *sample = T::from_sample(buf[p]);
                    p += 1;
                } else {
                    *sample = T::from_sample(0.0f32);
                }
            }
            pos.store(p, Ordering::Relaxed);
        },
        error_fn,
        None,
    )?;
    Ok(stream)
}

fn build_stream_output<T: SizedSample + FromSample<f32> + Send + 'static>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    buffer: Arc<Mutex<VecDeque<f32>>>,
    error_fn: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream, PlayError> {
    let output_channels = config.channels as usize;
    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let mut buf = buffer.lock().unwrap();
            for frame in data.chunks_mut(output_channels) {
                if buf.len() >= output_channels {
                    for out in frame {
                        *out = T::from_sample(buf.pop_front().unwrap());
                    }
                } else {
                    for out in frame {
                        *out = T::from_sample(0.0f32);
                    }
                }
            }
        },
        error_fn,
        None,
    )?;
    Ok(stream)
}

/// State for streaming sample-rate conversion and channel conversion.
///
/// Keeps a short window of input frames and produces output frames on demand
/// using linear interpolation. Input and output are limited to one or two
/// channels.
struct Resampler {
    input_channels: usize,
    output_channels: usize,
    output_step: f64,
    phase: f64,
    input_frames: VecDeque<[f32; 2]>,
    output: Arc<Mutex<VecDeque<f32>>>,
}

impl Resampler {
    fn new(
        input_channels: usize,
        output_channels: usize,
        input_rate: u32,
        output_rate: u32,
        output: Arc<Mutex<VecDeque<f32>>>,
    ) -> Self {
        Self {
            input_channels,
            output_channels,
            output_step: input_rate as f64 / output_rate as f64,
            phase: 0.0,
            input_frames: VecDeque::new(),
            output,
        }
    }

    fn feed(&mut self, samples: &[f32]) {
        let mut frame = [0.0f32; 2];
        for chunk in samples.chunks(self.input_channels) {
            for (i, s) in chunk.iter().enumerate() {
                frame[i] = *s;
            }
            self.input_frames.push_back(frame);
        }
        self.produce();
    }

    fn produce(&mut self) {
        while self.input_frames.len() >= 2 {
            if self.phase > 1.0 {
                self.input_frames.pop_front();
                self.phase -= 1.0;
                continue;
            }
            let a = self.input_frames[0];
            let b = self.input_frames[1];
            let t = self.phase.clamp(0.0, 1.0) as f32;
            let mut frame = [0.0f32; 2];
            for ch in 0..self.input_channels {
                frame[ch] = a[ch] + (b[ch] - a[ch]) * t;
            }
            self.push_output(frame);
            self.phase += self.output_step;
        }
    }

    fn push_output(&self, frame: [f32; 2]) {
        let mut out = self.output.lock().unwrap();
        match (self.input_channels, self.output_channels) {
            (1, 1) => out.push_back(frame[0]),
            (1, 2) => {
                out.push_back(frame[0]);
                out.push_back(frame[0]);
            }
            (2, 1) => out.push_back((frame[0] + frame[1]) * 0.5),
            (2, 2) => {
                out.push_back(frame[0]);
                out.push_back(frame[1]);
            }
            _ => {}
        }
    }

    fn flush(&mut self) {
        if let Some(&last) = self.input_frames.back() {
            self.input_frames.push_back(last);
            self.produce();
        }
        self.input_frames.clear();
    }
}

pub fn decode_pcm_chunk(
    buf: &mut BytesMut,
    format: &StreamedAudioFormat,
    out: &mut Vec<f32>,
) -> Result<(), PlayError> {
    out.clear();
    let sample_bytes = match format.pcm_format {
        PcmFormat::I16 => 2,
        PcmFormat::F32 => 4,
    };
    let frame_bytes = sample_bytes * format.channels as usize;
    if frame_bytes == 0 {
        return Ok(());
    }
    let frames = buf.len() / frame_bytes;
    let consumed = frames * frame_bytes;
    out.reserve(frames * format.channels as usize);
    let bytes = &buf[..consumed];
    let mut offset = 0;
    for _ in 0..frames {
        for ch in 0..format.channels as usize {
            let s = decode_sample(&bytes[offset + ch * sample_bytes..], format.pcm_format)?;
            out.push(s);
        }
        offset += frame_bytes;
    }
    buf.advance(consumed);
    Ok(())
}

fn decode_sample(bytes: &[u8], format: PcmFormat) -> Result<f32, PlayError> {
    Ok(match format {
        PcmFormat::I16 => {
            if bytes.len() < 2 {
                return Err(PlayError::UnsupportedFormat("short i16 sample".into()));
            }
            i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / I16_MAX_F32
        }
        PcmFormat::F32 => {
            if bytes.len() < 4 {
                return Err(PlayError::UnsupportedFormat("short f32 sample".into()));
            }
            f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
        }
    })
}

fn resample_interleaved(input: &[f32], from_rate: u32, to_rate: u32, channels: usize) -> Vec<f32> {
    if input.is_empty() || from_rate == to_rate {
        return input.to_vec();
    }

    if channels == 1 {
        return resample_mono(input, from_rate, to_rate);
    }

    let frame_count = input.len() / channels;
    let mut per_channel: Vec<Vec<f32>> = vec![Vec::with_capacity(frame_count); channels];
    for (i, s) in input.iter().enumerate() {
        per_channel[i % channels].push(*s);
    }

    let mut resampled: Vec<Vec<f32>> = Vec::with_capacity(channels);
    for ch in per_channel {
        resampled.push(resample_mono(&ch, from_rate, to_rate));
    }

    let output_len = resampled[0].len();
    let mut output = Vec::with_capacity(output_len * channels);
    for i in 0..output_len {
        for ch in &resampled {
            output.push(ch[i]);
        }
    }
    output
}

fn resample_mono(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
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

fn convert_channels(input: &[f32], from_channels: usize, to_channels: usize) -> Vec<f32> {
    if from_channels == to_channels {
        return input.to_vec();
    }

    if from_channels == 1 && to_channels == 2 {
        let mut output = Vec::with_capacity(input.len() * 2);
        for s in input {
            output.push(*s);
            output.push(*s);
        }
        return output;
    }

    if from_channels == 2 && to_channels == 1 {
        let frame_count = input.len() / 2;
        let mut output = Vec::with_capacity(frame_count);
        for i in 0..frame_count {
            let l = input[i * 2];
            let r = input[i * 2 + 1];
            output.push((l + r) / 2.0);
        }
        return output;
    }

    input.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_wav_16bit_mono() {
        let mut buf = Vec::new();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::new(Cursor::new(&mut buf), spec).unwrap();
        for s in [0.0, 0.5, -0.5, 0.9] {
            writer.write_sample((s * 32767.0) as i16).unwrap();
        }
        writer.finalize().unwrap();

        let clip = AudioClip::from_wav_bytes(&buf).unwrap();
        assert_eq!(clip.channels(), 1);
        assert_eq!(clip.sample_rate(), 16000);
        assert_eq!(clip.samples().len(), 4);
    }

    #[test]
    fn reject_8bit_wav() {
        let mut buf = Vec::new();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 8,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::new(Cursor::new(&mut buf), spec).unwrap();
        writer.write_sample(0i8).unwrap();
        writer.finalize().unwrap();

        assert!(AudioClip::from_wav_bytes(&buf).is_err());
    }

    #[test]
    fn reject_float_wav() {
        let mut buf = Vec::new();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::new(Cursor::new(&mut buf), spec).unwrap();
        writer.write_sample(0.0f32).unwrap();
        writer.finalize().unwrap();

        assert!(AudioClip::from_wav_bytes(&buf).is_err());
    }

    #[test]
    fn decode_pcm_i16_mono_to_mono() {
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&((0.5 * 32768.0) as i16).to_le_bytes());
        buf.extend_from_slice(&((-0.5 * 32768.0) as i16).to_le_bytes());
        let format = StreamedAudioFormat {
            sample_rate: 44100,
            channels: 1,
            pcm_format: PcmFormat::I16,
        };
        let mut out = Vec::new();
        decode_pcm_chunk(&mut buf, &format, &mut out).unwrap();
        assert_eq!(out.len(), 2);
        assert!((out[0] - 0.5).abs() < 1e-5);
        assert!((out[1] + 0.5).abs() < 1e-5);
        assert!(buf.is_empty());
    }

    #[test]
    fn decode_pcm_i16_keeps_partial_bytes() {
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&[0x12, 0x34, 0x56]);
        let format = StreamedAudioFormat {
            sample_rate: 44100,
            channels: 1,
            pcm_format: PcmFormat::I16,
        };
        let mut out = Vec::new();
        decode_pcm_chunk(&mut buf, &format, &mut out).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(buf.len(), 1);
        assert_eq!(buf[0], 0x56);
    }

    #[test]
    fn resampler_converts_mono_to_stereo() {
        let output = Arc::new(Mutex::new(VecDeque::new()));
        let mut resampler = Resampler::new(1, 2, 44100, 44100, Arc::clone(&output));
        resampler.feed(&[0.25]);
        resampler.feed(&[0.75]);
        let out = output.lock().unwrap();
        assert_eq!(
            out.iter().copied().collect::<Vec<_>>(),
            vec![0.25, 0.25, 0.75, 0.75]
        );
    }

    #[test]
    fn resampler_upsamples_mono() {
        let output = Arc::new(Mutex::new(VecDeque::new()));
        let mut resampler = Resampler::new(1, 1, 22050, 44100, Arc::clone(&output));
        resampler.feed(&[0.0, 1.0]);
        let out = output.lock().unwrap();
        // 22050 -> 44100 doubles the number of mono frames (minus the one-frame lookahead).
        assert!(out.len() >= 2);
    }

    #[test]
    fn resample_mono_doubles_length() {
        let input = vec![0.0, 1.0, 0.0, -1.0];
        let out = resample_mono(&input, 16000, 32000);
        assert_eq!(out.len(), 8);
    }

    #[test]
    fn convert_channels_mono_to_stereo() {
        let input = vec![0.5, -0.5];
        let out = convert_channels(&input, 1, 2);
        assert_eq!(out, vec![0.5, 0.5, -0.5, -0.5]);
    }

    #[test]
    fn convert_channels_stereo_to_mono() {
        let input = vec![0.5, 0.5, -0.5, -0.5];
        let out = convert_channels(&input, 2, 1);
        assert_eq!(out, vec![0.5, -0.5]);
    }
}
