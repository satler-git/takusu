use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use takusu_audio::{
    CartesiaOutputFormat, CartesiaSonic, CartesiaSonicConfig, ExecutionProvider, FishAudio,
    FishAudioConfig, Hush, SHERPA_SAMPLE_RATE, SherpaOnnxModel, StreamingSpeechToText, SttError,
    SttRuntimeConfig, TextToSpeech, TtsOptions, TtsRequest, normalize_for_tts,
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
struct SttCache {
    stt: Arc<dyn StreamingSpeechToText>,
    asr_model: String,
}

#[derive(uniffi::Object)]
pub struct MobileAudio {
    hush: Mutex<Option<Hush>>,
    stt: Mutex<Option<SttCache>>,
    stt_model: Mutex<String>,
    tts: Option<Arc<dyn TextToSpeech>>,
    runtime: Mutex<Option<Arc<Runtime>>>,
    runtime_shutdown: AtomicBool,
    model_dir: PathBuf,
    language: String,
    voice_id: String,
    sample_rate: u32,
    speed: Option<f32>,
    mute: AtomicBool,
    streaming: Mutex<Option<StreamingAsrSession>>,
}

/// State for one concurrent (record + streaming ASR) session.
struct StreamingAsrSession {
    chunk_tx: mpsc::Sender<Vec<f32>>,
    partial_text: Arc<Mutex<String>>,
    final_rx: Mutex<Option<mpsc::Receiver<Result<String, TakusuError>>>>,
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

    /// Load and cache the Sherpa-ONNX streaming STT backend for the current
    /// `asr_model`. The cached `Arc` is cloned and returned.
    fn load_stt(&self) -> Result<Arc<dyn StreamingSpeechToText>, TakusuError> {
        let asr_model = self
            .stt_model
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let (model, model_dir) = self.parse_asr_model(&asr_model)?;

        let mut stt_guard = self.stt.lock().unwrap_or_else(|error| {
            let mut guard = error.into_inner();
            guard.take();
            guard
        });
        let should_reload = stt_guard
            .as_ref()
            .map(|cache| cache.asr_model != asr_model || !model_dir.exists())
            .unwrap_or(true);
        if should_reload {
            let stt_config = SttRuntimeConfig {
                backend: takusu_audio::SttBackend::Sherpa,
                model,
                model_dir: Some(model_dir),
                language: self.language.clone(),
                use_itn: true,
                num_threads: 2,
                provider: ExecutionProvider::Cpu,
                sample_rate: SHERPA_SAMPLE_RATE as i32,
            };
            let stt =
                stt_config
                    .build_streaming()
                    .map_err(|error: SttError| TakusuError::Audio {
                        detail: format!("failed to load {asr_model}: {error}"),
                    })?;
            *stt_guard = Some(SttCache { stt, asr_model });
        }
        Ok(stt_guard.as_ref().unwrap().stt.clone())
    }
}

#[uniffi::export]
impl MobileAudio {
    #[uniffi::constructor]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        model_dir: String,
        provider: String,
        api_key: String,
        model: String,
        voice_id: String,
        language: String,
        sample_rate: u32,
        speed: Option<f32>,
        mute: bool,
    ) -> Result<Self, TakusuError> {
        let root = Path::new(&model_dir).to_path_buf();
        let tts: Option<Arc<dyn TextToSpeech>> = match provider.as_str() {
            "cartesia" if !api_key.trim().is_empty() => {
                let mut tts_config = CartesiaSonicConfig::new(api_key);
                if !voice_id.trim().is_empty() {
                    tts_config.voice_id = voice_id.clone();
                }
                if !model.trim().is_empty() {
                    tts_config.model_id = model;
                }
                tts_config.language = Some(language.clone());
                tts_config.output_format = CartesiaOutputFormat::mp3(sample_rate, 128_000);
                tts_config.mute = mute;
                Some(Arc::new(CartesiaSonic::new(tts_config)))
            }
            "fish" if !api_key.trim().is_empty() => {
                let mut tts_config = FishAudioConfig::new(api_key);
                if !voice_id.trim().is_empty() {
                    tts_config.voice_id = voice_id.clone();
                }
                if !model.trim().is_empty() {
                    tts_config.model = model;
                }
                tts_config.sample_rate = sample_rate;
                tts_config.mute = mute;
                Some(Arc::new(FishAudio::new(tts_config)))
            }
            _ => None,
        };
        Ok(Self {
            hush: Mutex::new(None),
            stt: Mutex::new(None),
            stt_model: Mutex::new("sherpa-sense-voice-int8".to_string()),
            tts,
            runtime: Mutex::new(None),
            runtime_shutdown: AtomicBool::new(false),
            model_dir: root,
            language,
            voice_id,
            sample_rate,
            speed,
            mute: AtomicBool::new(mute),
            streaming: Mutex::new(None),
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

    pub fn set_asr_model(&self, asr_model: String) {
        let mut guard = self
            .stt_model
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *guard = asr_model;
    }

    pub fn transcribe_pcm(&self, samples: Vec<i16>) -> Result<String, TakusuError> {
        if samples.is_empty() {
            return Err(TakusuError::Audio {
                detail: "recording was empty".to_string(),
            });
        }
        let pcm: Vec<f32> = samples
            .into_iter()
            .map(|sample| sample as f32 / 32768.0)
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

        let asr_model = self
            .stt_model
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let stt = self.load_stt()?;

        stt.transcribe_sync(&enhanced)
            .map_err(|error| TakusuError::Audio {
                detail: format!("{asr_model} inference failed: {error}"),
            })
    }

    pub fn start_streaming_asr(&self, language: String) -> Result<(), TakusuError> {
        let runtime = self.ensure_runtime()?;
        let stt = self.load_stt()?;
        let language = if language.is_empty() {
            self.language.clone()
        } else {
            language
        };

        let asr_stream = runtime
            .block_on(stt.start_stream(&language))
            .map_err(|error| TakusuError::Audio {
                detail: format!("failed to start streaming ASR: {error}"),
            })?;

        let (chunk_tx, chunk_rx) = mpsc::channel::<Vec<f32>>();
        let (final_tx, final_rx) = mpsc::channel::<Result<String, TakusuError>>();
        let partial_text = Arc::new(Mutex::new(String::new()));

        let runtime_for_thread = Arc::clone(&runtime);
        let partial_for_thread = Arc::clone(&partial_text);
        std::thread::spawn(move || {
            let mut asr_stream = asr_stream;
            while let Ok(chunk) = chunk_rx.recv() {
                asr_stream.accept_waveform(&chunk);
                *partial_for_thread
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = asr_stream.text();
            }
            let final_result = runtime_for_thread
                .block_on(asr_stream.finish())
                .map_err(|error| TakusuError::Audio {
                    detail: format!("streaming ASR finish failed: {error}"),
                });
            let _ = final_tx.send(final_result);
        });

        *self
            .streaming
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(StreamingAsrSession {
            chunk_tx,
            partial_text,
            final_rx: Mutex::new(Some(final_rx)),
        });
        Ok(())
    }

    pub fn feed_streaming_chunk(&self, samples: Vec<i16>) -> Result<String, TakusuError> {
        if samples.is_empty() {
            return Ok(String::new());
        }

        let pcm: Vec<f32> = samples
            .into_iter()
            .map(|sample| sample as f32 / 32768.0)
            .collect();

        let guard = self
            .streaming
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let session = guard.as_ref().ok_or(TakusuError::Audio {
            detail: "streaming ASR is not started".to_string(),
        })?;
        session
            .chunk_tx
            .send(pcm)
            .map_err(|error| TakusuError::Audio {
                detail: format!("streaming ASR channel closed: {error}"),
            })?;
        Ok(session
            .partial_text
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone())
    }

    pub fn finish_streaming_asr(&self) -> Result<String, TakusuError> {
        let session = self
            .streaming
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
            .ok_or(TakusuError::Audio {
                detail: "streaming ASR is not started".to_string(),
            })?;

        // Dropping the sender closes the channel and tells the consumer thread
        // that no more chunks will arrive.
        drop(session.chunk_tx);

        let final_rx = session
            .final_rx
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
            .ok_or(TakusuError::Audio {
                detail: "streaming ASR is already finished".to_string(),
            })?;

        final_rx.recv().map_err(|_| TakusuError::Audio {
            detail: "streaming ASR consumer was dropped".to_string(),
        })?
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
            voice: if self.voice_id.trim().is_empty() {
                None
            } else {
                Some(self.voice_id.clone())
            },
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

impl MobileAudio {
    fn parse_asr_model(&self, asr_model: &str) -> Result<(SherpaOnnxModel, PathBuf), TakusuError> {
        let (model, id) = match asr_model {
            "sense-voice" | "sherpa-sense-voice-int8" => {
                (SherpaOnnxModel::SenseVoice, "sherpa-sense-voice-int8")
            }
            "parakeet-ctc-ja" | "sherpa-parakeet-ctc-ja-0.6b" => (
                SherpaOnnxModel::ParakeetJaCtc,
                "sherpa-parakeet-ctc-ja-0.6b",
            ),
            "nemotron-ja" | "sherpa-nemotron-ja-0.6b" => (
                SherpaOnnxModel::NemotronMultilingual,
                "sherpa-nemotron-ja-0.6b",
            ),
            _ => {
                return Err(TakusuError::Audio {
                    detail: format!("unsupported asr model: {asr_model}"),
                });
            }
        };
        Ok((model, self.model_dir.join(id)))
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
