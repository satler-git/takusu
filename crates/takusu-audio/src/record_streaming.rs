//! Streaming microphone recorder for chunked ASR.
//!
//! `StreamingRecorder` captures audio in real time and emits fixed-size chunks
//! of 16 kHz mono f32 samples through a `tokio` channel. Callers feed the
//! chunks directly into an [`AsrStream`](crate::stt::AsrStream).

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{JoinHandle, spawn};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use crate::record::{RecordConfig, RecorderError};
use crate::wav::{I16_MAX_F32, SHERPA_SAMPLE_RATE, mix_to_mono, normalize, resample};

const CHUNK_MS: u64 = 160;

/// Handle to an in-progress streaming recording.
pub struct StreamingRecorder {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<Result<(), RecorderError>>>,
}

impl Drop for StreamingRecorder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl StreamingRecorder {
    /// Start a new streaming recorder.
    ///
    /// Returns the recorder handle and a receiver for PCM chunks. Each chunk is
    /// 16 kHz mono f32 normalized to the same target RMS as [`record`](crate::record::record).
    pub fn start(
        config: RecordConfig,
    ) -> Result<(Self, UnboundedReceiver<Vec<f32>>), RecorderError> {
        let (tx, rx) = unbounded_channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_c = Arc::clone(&stop);

        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or(RecorderError::NoInputDevice)?;
        let device_config = device.default_input_config()?;
        let device_sample_rate = device_config.sample_rate();
        let channels = device_config.channels() as usize;
        let sample_format = device_config.sample_format();
        let stream_config: cpal::StreamConfig = device_config.into();

        let target_rate = config.target_sample_rate.unwrap_or(SHERPA_SAMPLE_RATE);
        let chunk_size = (target_rate as u64 * CHUNK_MS / 1000) as usize;

        let handle = spawn(move || {
            let samples: Arc<std::sync::Mutex<Vec<f32>>> =
                Arc::new(std::sync::Mutex::new(Vec::new()));
            let samples_c = Arc::clone(&samples);
            let stopped_c = Arc::clone(&stop_c);
            let stream_error: Arc<std::sync::Mutex<Option<RecorderError>>> =
                Arc::new(std::sync::Mutex::new(None));
            let stream_error_c = Arc::clone(&stream_error);
            let stopped_err = Arc::clone(&stop_c);

            let error_fn = move |err: cpal::Error| {
                eprintln!("audio stream error: {err}");
                *stream_error_c.lock().unwrap() = Some(RecorderError::Cpal(err.to_string()));
                stopped_err.store(true, Ordering::Relaxed);
            };

            let stream = match sample_format {
                cpal::SampleFormat::F32 => device.build_input_stream(
                    stream_config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        if stopped_c.load(Ordering::Relaxed) {
                            return;
                        }
                        match samples_c.try_lock() {
                            Ok(mut buf) => buf.extend_from_slice(data),
                            Err(std::sync::TryLockError::WouldBlock) => {
                                tracing::warn!(
                                    "audio buffer busy; dropping {} f32 samples",
                                    data.len()
                                );
                            }
                            Err(std::sync::TryLockError::Poisoned(e)) => {
                                e.into_inner().extend_from_slice(data);
                            }
                        }
                    },
                    error_fn,
                    None,
                )?,
                cpal::SampleFormat::I16 => {
                    let samples_c = Arc::clone(&samples);
                    let stopped_c = Arc::clone(&stop_c);
                    device.build_input_stream(
                        stream_config,
                        move |data: &[i16], _: &cpal::InputCallbackInfo| {
                            if stopped_c.load(Ordering::Relaxed) {
                                return;
                            }
                            match samples_c.try_lock() {
                                Ok(mut buf) => {
                                    for &s in data {
                                        buf.push(s as f32 / I16_MAX_F32);
                                    }
                                }
                                Err(std::sync::TryLockError::WouldBlock) => {
                                    tracing::warn!(
                                        "audio buffer busy; dropping {} i16 samples",
                                        data.len()
                                    );
                                }
                                Err(std::sync::TryLockError::Poisoned(e)) => {
                                    let mut buf = e.into_inner();
                                    for &s in data {
                                        buf.push(s as f32 / I16_MAX_F32);
                                    }
                                }
                            }
                        },
                        error_fn,
                        None,
                    )?
                }
                _ => return Err(RecorderError::UnsupportedFormat),
            };

            stream.play()?;

            let start = Instant::now();
            let mut next_send = Instant::now() + Duration::from_millis(CHUNK_MS);

            loop {
                std::thread::sleep(Duration::from_millis(10));

                let should_stop =
                    stop_c.load(Ordering::Relaxed) || start.elapsed() >= config.max_duration;

                if Instant::now() >= next_send || should_stop {
                    let raw = {
                        let mut buf = samples.lock().unwrap_or_else(|e| e.into_inner());
                        buf.drain(..).collect::<Vec<_>>()
                    };

                    if !raw.is_empty() {
                        let mut chunk = raw;
                        if channels > 1 {
                            chunk = mix_to_mono(&chunk, channels);
                        }
                        if device_sample_rate != target_rate {
                            chunk = resample(&chunk, device_sample_rate, target_rate);
                        }
                        if config.normalize_audio {
                            chunk = normalize(&chunk, 0.1);
                        }

                        // Send in chunk_size slices so the ASR sees a regular cadence.
                        for window in chunk.chunks(chunk_size) {
                            if tx.send(window.to_vec()).is_err() {
                                return Ok(());
                            }
                        }
                    }

                    next_send = Instant::now() + Duration::from_millis(CHUNK_MS);
                }

                if should_stop {
                    break;
                }
            }

            drop(stream);
            if let Some(e) = stream_error.lock().unwrap().take() {
                return Err(e);
            }
            Ok(())
        });

        Ok((
            Self {
                stop,
                handle: Some(handle),
            },
            rx,
        ))
    }

    /// Request the recording to stop at the next poll.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Wait for the recording thread to finish.
    pub fn join(mut self) -> Result<(), RecorderError> {
        self.handle
            .take()
            .expect("handle present")
            .join()
            .map_err(|_| RecorderError::Cpal("recording thread panicked".to_string()))?
    }
}
