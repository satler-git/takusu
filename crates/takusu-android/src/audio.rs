use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use takusu_audio::{
    CartesiaOutputFormat, CartesiaSonic, CartesiaSonicConfig, Hush, SherpaOnnxAsr,
    SherpaOnnxAsrConfig, SherpaOnnxModel, SpeechToText, SttError, TextToSpeech, TtsOptions,
    TtsRequest, normalize_for_tts,
};
use tokio::runtime::{Builder, Runtime};

use crate::TakusuError;

/// Android audio bridge. Recording is performed by Kotlin AudioRecord; model
/// inference and provider TTS stay in Rust so desktop and Android share the
/// same audio backend behavior.
///
/// STT models (Hush + Sherpa) are loaded lazily on the first call to
/// `transcribe_pcm` so that users who only want Android system TTS do not need
/// to download STT weights up front. The tokio runtime is also created lazily,
/// both to avoid forcing model/TTS-only users to pay for it and to avoid
/// `process`/`signal` driver registration that can fail on some Android builds.
///
/// The runtime is stored as `Option<Arc<Runtime>>` so that a task can clone the
/// `Arc`, release the mutex, and call `block_on` without the mutex being held.
/// This prevents a panic in an audio task from poisoning the runtime lock and
/// permanently disabling TTS/STT.
#[derive(uniffi::Object)]
pub struct MobileAudio {
    hush: Mutex<Option<Hush>>,
    stt: Mutex<Option<Arc<SherpaOnnxAsr>>>,
    tts: Option<CartesiaSonic>,
    runtime: Mutex<Option<Arc<Runtime>>>,
    runtime_shutdown: AtomicBool,
    model_dir: PathBuf,
    language: String,
    voice_id: String,
    sample_rate: u32,
    speed: Option<f32>,
    mute: AtomicBool,
}

impl MobileAudio {
    /// Return a handle to the tokio runtime, initializing it on first use.
    ///
    /// Only `io` and `time` drivers are enabled; `signal`/`process` are not
    /// needed for audio inference/networking and can trip up Android hosts.
    /// The returned `Arc<Runtime>` is released before `block_on` is called by
    /// the public methods, so the runtime mutex is never held across a blocking
    /// audio operation.
    ///
    /// A concurrent `shutdown` may set `runtime_shutdown` after we clone the
    /// `Arc`; the clone keeps the runtime alive for the current task, and
    /// `shutdown` will not call `shutdown_background` while any clone is alive.
    fn ensure_runtime(&self) -> Result<Arc<Runtime>, TakusuError> {
        if self.runtime_shutdown.load(Ordering::Acquire) {
            return Err(TakusuError::Audio {
                detail: "audio runtime has been shut down".to_string(),
            });
        }
        let mut guard = self
            .runtime
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if self.runtime_shutdown.load(Ordering::Acquire) {
            return Err(TakusuError::Audio {
                detail: "audio runtime has been shut down".to_string(),
            });
        }
        if guard.is_none() {
            let runtime = Builder::new_multi_thread()
                .enable_io()
                .enable_time()
                .build()
                .map_err(|error| TakusuError::Audio {
                    detail: format!("failed to create audio runtime: {error}"),
                })?;
            *guard = Some(Arc::new(runtime));
        }
        Ok(guard.as_ref().unwrap().clone())
    }

    /// Normalize `text` for TTS using the runtime's blocking thread pool.
    ///
    /// Uses `runtime.spawn_blocking` (the method on the `Runtime` instance)
    /// rather than the `tokio::task::spawn_blocking` free function so it works
    /// even when called from outside a tokio runtime context.
    fn normalize_text(
        &self,
        runtime: Arc<Runtime>,
        text: String,
        language: &str,
    ) -> Result<String, TakusuError> {
        let language = language.to_string();
        // `spawn_blocking` returns a `JoinHandle` immediately without needing
        // a current runtime context; `block_on` then awaits it on the calling
        // thread.
        let handle =
            runtime.spawn_blocking(move || normalize_for_tts(&text, &language).into_owned());
        runtime
            .block_on(handle)
            .map_err(|error| TakusuError::Audio {
                detail: format!("TTS normalization failed: {error}"),
            })
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
            runtime: Mutex::new(None),
            runtime_shutdown: AtomicBool::new(false),
            model_dir: root,
            language,
            voice_id,
            sample_rate,
            speed,
            mute: AtomicBool::new(mute),
        })
    }

    pub fn shutdown(&self) -> Result<(), TakusuError> {
        let mut guard = self
            .runtime
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.runtime_shutdown.store(true, Ordering::Release);
        if let Some(arc) = guard.take() {
            // Any task that already cloned the `Arc` can finish. If no task is
            // active we can shut the runtime down immediately; otherwise it will
            // be dropped once the last clone is released.
            drop(guard);
            if let Ok(runtime) = Arc::try_unwrap(arc) {
                runtime.shutdown_background();
            }
        }
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

        // Hush state is mutated by enhance(), so a panic while the guard is
        // held could leave the model in an inconsistent state. If the mutex is
        // poisoned, discard the cached model so the next call reloads it.
        let mut hush_guard = self.hush.lock().unwrap_or_else(|error| {
            let mut guard = error.into_inner();
            guard.take();
            guard
        });
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

        // Load the STT model once, then release the mutex before the runtime
        // block_on call so a panic in transcribe() cannot poison the stt lock.
        let stt = {
            let mut stt_guard = self.stt.lock().unwrap_or_else(|error| error.into_inner());
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
                *stt_guard = Some(Arc::new(stt));
            }
            stt_guard.as_ref().unwrap().clone()
        };
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
        //
        // `runtime.spawn_blocking` (the method on the `Runtime` instance) is
        // used instead of the `tokio::task::spawn_blocking` free function.
        // The free function looks up the *current* runtime via
        // `Handle::current()`, which panics with "there is no reactor running"
        // when `synthesize` is called from outside a tokio runtime context —
        // e.g. from Kotlin's `Dispatchers.IO` on Android (issue #1175).
        let runtime = self.ensure_runtime()?;
        let language = self.language.clone();
        let text = self.normalize_text(runtime.clone(), text, &language)?;
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
            audio.runtime.lock().unwrap().is_none()
                && !audio.runtime_shutdown.load(Ordering::Relaxed),
            "runtime should not be created when the API key is empty"
        );
    }

    #[test]
    fn ensure_runtime_after_shutdown_returns_error() {
        let model_dir = std::env::temp_dir()
            .join("takusu_ensure_runtime_after_shutdown_test")
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
        )
        .unwrap();

        let _runtime = audio.ensure_runtime().unwrap();
        audio.shutdown().unwrap();
        let err = audio.ensure_runtime().unwrap_err();
        assert!(
            matches!(err, TakusuError::Audio { ref detail } if detail == "audio runtime has been shut down"),
            "ensure_runtime should fail after shutdown: {err:?}"
        );
    }

    #[test]
    fn runtime_lock_poison_is_recovered() {
        let model_dir = std::env::temp_dir()
            .join("takusu_runtime_lock_poison_test")
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
        )
        .unwrap();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = audio.runtime.lock().unwrap();
            panic!("intentional runtime lock poison");
        }));
        assert!(
            result.is_err(),
            "the test must intentionally poison the runtime lock"
        );

        // ensure_runtime should recover from the poisoned lock and create a
        // usable runtime.
        let runtime = audio.ensure_runtime().unwrap();
        let value = runtime.block_on(async { 42 });
        assert_eq!(value, 42);
        audio.shutdown().unwrap();
    }

    #[test]
    fn shutdown_during_block_on_races_safely() {
        use std::time::Duration;

        let model_dir = std::env::temp_dir()
            .join("takusu_shutdown_during_block_on_test")
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
        )
        .unwrap();

        let runtime = audio.ensure_runtime().unwrap();
        std::thread::scope(|s| {
            let worker_runtime = Arc::clone(&runtime);
            s.spawn(move || {
                worker_runtime.block_on(async {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                });
            });
            // This should not deadlock or panic even though the worker is still
            // inside block_on. The Arc clone keeps the runtime alive for the
            // worker; once the clone is dropped the runtime is shut down.
            audio.shutdown().unwrap();
        });

        let err = audio.ensure_runtime().unwrap_err();
        assert!(
            matches!(err, TakusuError::Audio { ref detail } if detail == "audio runtime has been shut down"),
            "ensure_runtime should fail after shutdown: {err:?}"
        );
    }

    #[test]
    fn normalize_text_works_outside_runtime_context() {
        // Reproduces issue #1175: synthesize is called from Kotlin's
        // Dispatchers.IO on Android, which is a plain thread with no tokio
        // runtime. The previous implementation used the
        // `tokio::task::spawn_blocking` free function, which looks up the
        // *current* runtime via `Handle::current()` and panics with
        // "there is no reactor running" when no runtime is active.
        let model_dir = std::env::temp_dir()
            .join("takusu_normalize_text_no_runtime_test")
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
        )
        .unwrap();
        let runtime = audio.ensure_runtime().unwrap();

        // Call from a plain thread with no tokio runtime context, mimicking
        // the Android call path.
        let handle = std::thread::spawn(move || {
            audio.normalize_text(runtime, "7月8日(火)の予定".to_string(), "ja")
        });
        let result = handle.join().unwrap_or_else(|panic| {
            panic!("normalize_text panicked outside a runtime context: {panic:?}")
        });
        let normalized = result.expect("normalize_text should succeed");
        assert!(
            normalized.contains("7月8日火曜日"),
            "expected weekday normalization, got: {normalized}"
        );
    }
}
