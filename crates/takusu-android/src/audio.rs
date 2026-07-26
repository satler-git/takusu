use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{
    Mutex, MutexGuard,
    atomic::{AtomicBool, Ordering},
};

use takusu_audio::{
    CartesiaOutputFormat, CartesiaSonic, CartesiaSonicConfig, Hush, SherpaOnnxAsr,
    SherpaOnnxAsrConfig, SherpaOnnxModel, SpeechToText, SttError, TextToSpeech, TtsOptions,
    TtsRequest, normalize_for_tts,
};
use tokio::runtime::{Builder, Runtime};

use crate::TakusuError;

/// State of the tokio runtime. All transitions happen under the same mutex,
/// so `shutdown` and `ensure_runtime` cannot race.
#[derive(Default)]
enum RuntimeState {
    #[default]
    Uninitialized,
    Running(Runtime),
    ShutDown,
}

/// Guard that keeps the runtime mutex locked while exposing the underlying
/// `Runtime`. The `Deref` implementation assumes the state is `Running`;
/// it panics only if that invariant is violated.
struct RuntimeGuard<'a> {
    guard: MutexGuard<'a, RuntimeState>,
}

impl<'a> RuntimeGuard<'a> {
    fn new(guard: MutexGuard<'a, RuntimeState>) -> Self {
        Self { guard }
    }
}

impl<'a> Deref for RuntimeGuard<'a> {
    type Target = Runtime;
    fn deref(&self) -> &Self::Target {
        match &*self.guard {
            RuntimeState::Running(runtime) => runtime,
            _ => unreachable!("RuntimeGuard created only from Running state"),
        }
    }
}

/// Android audio bridge. Recording is performed by Kotlin AudioRecord; model
/// inference and provider TTS stay in Rust so desktop and Android share the
/// same audio backend behavior.
///
/// STT models (Hush + Sherpa) are loaded lazily on the first call to
/// `transcribe_pcm` so that users who only want Android system TTS do not need
/// to download STT weights up front. The tokio runtime is also created lazily,
/// both to avoid forcing model/TTS-only users to pay for it and to avoid
/// `process`/`signal` driver registration that can fail on some Android builds.
#[derive(uniffi::Object)]
pub struct MobileAudio {
    hush: Mutex<Option<Hush>>,
    stt: Mutex<Option<SherpaOnnxAsr>>,
    tts: Option<CartesiaSonic>,
    runtime: Mutex<RuntimeState>,
    model_dir: PathBuf,
    language: String,
    voice_id: String,
    sample_rate: u32,
    speed: Option<f32>,
    mute: AtomicBool,
}

impl MobileAudio {
    /// Return the tokio runtime, initializing it on first use.
    ///
    /// Only `io` and `time` drivers are enabled; `signal`/`process` are not
    /// needed for audio inference/networking and can trip up Android hosts.
    /// After `shutdown` this returns an error, and the state is permanently
    /// `ShutDown` under the same mutex.
    fn ensure_runtime(&self) -> Result<RuntimeGuard<'_>, TakusuError> {
        let mut guard = self.runtime.lock().map_err(|error| TakusuError::Audio {
            detail: format!("runtime lock poisoned: {error}"),
        })?;
        if matches!(&*guard, RuntimeState::ShutDown) {
            return Err(TakusuError::Audio {
                detail: "audio runtime has been shut down".to_string(),
            });
        }
        if matches!(&*guard, RuntimeState::Uninitialized) {
            let runtime = Builder::new_multi_thread()
                .enable_io()
                .enable_time()
                .build()
                .map_err(|error| TakusuError::Audio {
                    detail: format!("failed to create audio runtime: {error}"),
                })?;
            *guard = RuntimeState::Running(runtime);
        }
        Ok(RuntimeGuard::new(guard))
    }
}

#[uniffi::export]
impl MobileAudio {
    #[uniffi::constructor]
    pub fn new(
        model_dir: String,
        api_key: String,
        voice_id: String,
        language: String,
        sample_rate: u32,
        speed: Option<f32>,
        mute: bool,
    ) -> Result<Self, TakusuError> {
        let root = Path::new(&model_dir).to_path_buf();
        let tts = if api_key.trim().is_empty() {
            None
        } else {
            let mut tts_config = CartesiaSonicConfig::new(api_key);
            tts_config.voice_id = voice_id.clone();
            tts_config.language = Some(language.clone());
            tts_config.output_format = CartesiaOutputFormat::mp3(sample_rate, 128_000);
            tts_config.mute = mute;
            Some(CartesiaSonic::new(tts_config))
        };
        Ok(Self {
            hush: Mutex::new(None),
            stt: Mutex::new(None),
            tts,
            runtime: Mutex::new(RuntimeState::Uninitialized),
            model_dir: root,
            language,
            voice_id,
            sample_rate,
            speed,
            mute: AtomicBool::new(mute),
        })
    }

    pub fn shutdown(&self) -> Result<(), TakusuError> {
        let mut guard = self.runtime.lock().map_err(|error| TakusuError::Audio {
            detail: format!("runtime lock poisoned: {error}"),
        })?;
        if let RuntimeState::Running(runtime) = std::mem::take(&mut *guard) {
            runtime.shutdown_background();
        }
        *guard = RuntimeState::ShutDown;
        Ok(())
    }

    pub fn transcribe_pcm(&self, samples: Vec<i16>) -> Result<String, TakusuError> {
        if samples.is_empty() {
            return Err(TakusuError::Audio {
                detail: "recording was empty".to_string(),
            });
        }
        let pcm: Vec<f32> = samples
            .into_iter()
            .map(|sample| sample as f32 / i16::MAX as f32)
            .collect();

        let mut hush_guard = self.hush.lock().map_err(|error| TakusuError::Audio {
            detail: format!("Hush lock poisoned: {error}"),
        })?;
        if hush_guard.is_none() {
            let hush = Hush::from_model_dir(self.model_dir.join("hush")).map_err(|error| {
                TakusuError::Audio {
                    detail: format!("failed to load Hush: {error}"),
                }
            })?;
            *hush_guard = Some(hush);
        }
        let hush = hush_guard.as_mut().unwrap();
        let enhanced = hush.enhance(&pcm).map_err(|error| TakusuError::Audio {
            detail: format!("Hush inference failed: {error}"),
        })?;
        drop(hush_guard);

        let mut stt_guard = self.stt.lock().map_err(|error| TakusuError::Audio {
            detail: format!("SenseVoice lock poisoned: {error}"),
        })?;
        if stt_guard.is_none() {
            let stt = SherpaOnnxAsr::from_config(&SherpaOnnxAsrConfig {
                model: SherpaOnnxModel::SenseVoice,
                model_dir: self.model_dir.join("sherpa-sense-voice-int8"),
                sample_rate: 16_000,
                language: Some(self.language.clone()),
                use_itn: true,
                ..Default::default()
            })
            .map_err(|error: SttError| TakusuError::Audio {
                detail: format!("failed to load SenseVoice: {error}"),
            })?;
            *stt_guard = Some(stt);
        }
        let stt = stt_guard.as_ref().unwrap();
        let runtime = self.ensure_runtime()?;
        runtime
            .block_on(stt.transcribe(&enhanced))
            .map_err(|error| TakusuError::Audio {
                detail: format!("SenseVoice inference failed: {error}"),
            })
    }

    pub fn synthesize(&self, text: String) -> Result<Vec<u8>, TakusuError> {
        if text.trim().is_empty() {
            return Err(TakusuError::Audio {
                detail: "TTS text was empty".to_string(),
            });
        }
        if self.mute.load(Ordering::Relaxed) {
            // Return an empty buffer when muted. Callers are expected to
            // check the mute flag before playing; the empty buffer is not a
            // valid audio file on its own.
            return Ok(Vec::new());
        }
        let tts = self.tts.as_ref().ok_or(TakusuError::Audio {
            detail: "TTS backend is not configured".to_string(),
        })?;
        // Lindera dictionary initialization can block on first use, so run
        // the normalization off the runtime worker threads.
        let runtime = self.ensure_runtime()?;
        let language = self.language.clone();
        let text = runtime
            .block_on(tokio::task::spawn_blocking(move || {
                normalize_for_tts(&text, &language).into_owned()
            }))
            .map_err(|error| TakusuError::Audio {
                detail: format!("TTS normalization failed: {error}"),
            })?;
        let request = TtsRequest {
            text,
            voice: Some(self.voice_id.clone()),
            reference_audio_path: None,
            options: TtsOptions {
                response_format: Some("mp3".to_string()),
                speed: self.speed,
            },
        };
        runtime
            .block_on(tts.synthesize(&request))
            .map_err(|error| TakusuError::Audio {
                detail: format!("TTS failed at {} Hz: {error}", self.sample_rate),
            })
    }

    pub fn set_muted(&self, muted: bool) {
        self.mute.store(muted, Ordering::Relaxed);
    }

    pub fn is_muted(&self) -> bool {
        self.mute.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_without_api_key_succeeds_without_creating_runtime() {
        let model_dir = std::env::temp_dir()
            .join("takusu_android_audio_new_test")
            .to_string_lossy()
            .to_string();
        let audio = MobileAudio::new(
            model_dir,
            String::new(),
            String::new(),
            "ja".to_string(),
            44100,
            Some(1.0),
            false,
        );
        assert!(
            audio.is_ok(),
            "MobileAudio::new should not fail when the API key is empty: {:?}",
            audio.err()
        );
        let audio = audio.unwrap();
        assert!(
            matches!(&*audio.runtime.lock().unwrap(), RuntimeState::Uninitialized),
            "runtime should not be created when the API key is empty"
        );
    }
}
