use std::io::BufRead;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::wav::{I16_MAX_F32, SHERPA_SAMPLE_RATE, mix_to_mono, normalize, resample};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RecorderError {
    #[error("no input device")]
    NoInputDevice,
    #[error("cpal: {0}")]
    Cpal(String),
    #[error("unsupported sample format")]
    UnsupportedFormat,
}

impl From<cpal::Error> for RecorderError {
    fn from(e: cpal::Error) -> Self {
        Self::Cpal(e.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct RecordConfig {
    pub max_duration: Duration,
    /// Target sample rate for the recorded output. Defaults to
    /// [`SHERPA_SAMPLE_RATE`]. Set to `None` to leave the device's native
    /// sample rate unchanged. Note that downstream consumers (WAV writers,
    /// Sherpa-ONNX transcription) assume 16 kHz, so `None` is only safe when
    /// the caller handles resampling itself; use `Some(SHERPA_SAMPLE_RATE)`
    /// in normal pipelines.
    pub target_sample_rate: Option<u32>,
}

impl Default for RecordConfig {
    fn default() -> Self {
        Self {
            max_duration: Duration::from_secs(300),
            target_sample_rate: Some(SHERPA_SAMPLE_RATE),
        }
    }
}

pub fn record(config: &RecordConfig) -> Result<Vec<f32>, RecorderError> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or(RecorderError::NoInputDevice)?;
    let device_config = device.default_input_config()?;
    let device_sample_rate = device_config.sample_rate();
    let channels = device_config.channels() as usize;
    let sample_format = device_config.sample_format();
    let stream_config: cpal::StreamConfig = device_config.into();

    let samples: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let stopped = Arc::new(AtomicBool::new(false));

    let error_fn = |err: cpal::Error| {
        eprintln!("audio stream error: {err}");
    };

    let stream = match sample_format {
        cpal::SampleFormat::F32 => {
            let samples_c = samples.clone();
            let stopped_c = stopped.clone();

            device.build_input_stream(
                stream_config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if stopped_c.load(Ordering::Relaxed) {
                        return;
                    }
                    if let Ok(mut buf) = samples_c.try_lock() {
                        buf.extend_from_slice(data);
                    }
                },
                error_fn,
                None,
            )?
        }
        cpal::SampleFormat::I16 => {
            let samples_c = samples.clone();
            let stopped_c = stopped.clone();

            device.build_input_stream(
                stream_config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    if stopped_c.load(Ordering::Relaxed) {
                        return;
                    }
                    if let Ok(mut buf) = samples_c.try_lock() {
                        for &s in data {
                            buf.push(s as f32 / I16_MAX_F32);
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

    let stopped_t = stopped.clone();
    let _waiter = std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut line = String::new();
        let _ = stdin.lock().read_line(&mut line);
        stopped_t.store(true, Ordering::Relaxed);
    });

    eprintln!("Recording... Press Enter to stop.");

    let start = std::time::Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(100));

        if stopped.load(Ordering::Relaxed) {
            break;
        }
        if start.elapsed() >= config.max_duration {
            stopped.store(true, Ordering::Relaxed);
            break;
        }
    }

    drop(stream);

    let mut raw = samples.lock().unwrap().clone();

    if channels > 1 {
        raw = mix_to_mono(&raw, channels);
    }

    if let Some(target) = config.target_sample_rate
        && device_sample_rate != target
    {
        raw = resample(&raw, device_sample_rate, target);
    }

    raw = normalize(&raw, 0.1);

    Ok(raw)
}
