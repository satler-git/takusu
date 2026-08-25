use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const I16_MAX_F32: f32 = 32768.0;

/// Maximum raw chunks buffered between the recording feeder and the pipeline.
/// 160 ms per chunk, so this is ~5 s of audio; beyond it the newest chunk is
/// dropped rather than letting memory grow without bound.
const MAX_BUFFERED_CHUNKS: usize = 32;

use tokio::runtime::{Builder, Runtime};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use takusu_audio::kws::DEFAULT_KWS_MODEL_ID;
use takusu_audio::{
    AmbientConfig, AmbientError, AmbientPipeline, AmbientResult, ExecutionProvider, KwsConfig,
    ModelCache, ModelError, SHERPA_SAMPLE_RATE, SherpaOnnxModel, StreamingSpeechToText, SttBackend,
    SttError, SttRuntimeConfig, VadEndpointConfig, WakeWordBackend,
    default_endpoint_with_config_and_cache_dir,
};

use crate::TakusuError;

/// Try to lock a mutex without blocking; if the owning thread panicked,
/// recover the inner value so Drop can still clean up.
fn try_lock_or_recover<T>(mutex: &Mutex<T>) -> Option<std::sync::MutexGuard<'_, T>> {
    match mutex.try_lock() {
        Ok(guard) => Some(guard),
        Err(std::sync::TryLockError::Poisoned(guard)) => Some(guard.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => None,
    }
}

/// Parse a user-provided wake-word backend name, defaulting to Sherpa KWS.
fn parse_wake_word_backend(backend: Option<&str>) -> WakeWordBackend {
    match backend.map(str::trim) {
        Some("asr_text_match") | Some("asr") => WakeWordBackend::AsrTextMatch,
        _ => WakeWordBackend::SherpaKws,
    }
}

/// Android-facing callbacks for the ambient listening pipeline.
#[uniffi::export(callback_interface)]
pub trait AmbientCallback: Send + Sync {
    fn on_listening(&self);
    fn on_transcribing(&self);
    fn on_wake_word(&self, text: String);
    fn on_result(&self, text: String, samples: Vec<f32>);
    fn on_error(&self, error: String);
    fn on_stopped(&self);
}

/// Android bridge for the continuous ambient wake-word + streaming ASR pipeline.
#[derive(uniffi::Object)]
pub struct MobileAmbient {
    model_dir: PathBuf,
    asr_model: String,
    language: String,
    wake_word: String,
    wake_word_backend: WakeWordBackend,
    pre_speech_buffer_ms: u64,
    use_speaker_verification: bool,
    callback: Arc<dyn AmbientCallback>,
    runtime: Mutex<Option<Arc<Runtime>>>,
    feed_tx: Mutex<Option<mpsc::UnboundedSender<Vec<f32>>>>,
    stop_tx: Mutex<Option<Arc<watch::Sender<bool>>>>,
    running: Arc<AtomicBool>,
    download_cancel: Arc<AtomicBool>,
    pipeline_join: Mutex<Option<JoinHandle<()>>>,
    /// Guards start/stop/teardown to prevent interleaving lifecycle operations.
    lifecycle: Mutex<()>,
}

#[uniffi::export]
impl MobileAmbient {
    /// Create a new ambient listener.
    #[uniffi::constructor]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        model_dir: String,
        asr_model: String,
        language: String,
        wake_word: Option<String>,
        wake_word_backend: Option<String>,
        pre_speech_buffer_ms: Option<u64>,
        use_speaker_verification: bool,
        callback: Box<dyn AmbientCallback>,
    ) -> Result<Self, TakusuError> {
        let asr_model = if asr_model.trim().is_empty() {
            "sherpa-sense-voice-int8".to_string()
        } else {
            asr_model
        };
        let language = if language.trim().is_empty() {
            "ja".to_string()
        } else {
            language
        };
        let wake_word = wake_word.unwrap_or_else(|| "たくす".to_string());
        let wake_word_backend = parse_wake_word_backend(wake_word_backend.as_deref());
        let pre_speech_buffer_ms = pre_speech_buffer_ms.unwrap_or(800);
        let callback = Arc::from(callback);

        Ok(Self {
            model_dir: Path::new(&model_dir).to_path_buf(),
            asr_model,
            language,
            wake_word,
            wake_word_backend,
            pre_speech_buffer_ms,
            use_speaker_verification,
            callback,
            runtime: Mutex::new(None),
            feed_tx: Mutex::new(None),
            stop_tx: Mutex::new(None),
            running: Arc::new(AtomicBool::new(false)),
            download_cancel: Arc::new(AtomicBool::new(false)),
            pipeline_join: Mutex::new(None),
            lifecycle: Mutex::new(()),
        })
    }

    /// Start the ambient pipeline on a tokio worker thread.
    ///
    /// Blocks until the ASR model and VAD endpoint have been loaded and the
    /// pipeline is ready to receive PCM chunks, so callers do not start
    /// recording before the pipeline can consume audio.
    pub fn start(&self) -> Result<(), TakusuError> {
        let _guard = self
            .lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());

        if self
            .running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(TakusuError::Audio {
                detail: "ambient pipeline is already running".to_string(),
            });
        }

        // Do not start a new pipeline until the previous one has fully exited,
        // otherwise two runtimes could run concurrently.
        if !self
            .pipeline_join
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .is_none_or(|join| join.is_finished())
        {
            self.running.store(false, Ordering::Release);
            return Err(TakusuError::Audio {
                detail: "previous ambient task is still stopping".to_string(),
            });
        }

        self.download_cancel.store(false, Ordering::Release);

        let runtime = match self.ensure_runtime() {
            Ok(runtime) => runtime,
            Err(error) => {
                self.running.store(false, Ordering::Release);
                return Err(error);
            }
        };

        let (feed_tx, feed_rx) = mpsc::unbounded_channel::<Vec<f32>>();
        let (stop_tx_base, _stop_rx) = watch::channel(false);
        let stop_tx = Arc::new(stop_tx_base);
        let (ready_tx, ready_rx) = oneshot::channel::<Result<(), String>>();

        *self
            .feed_tx
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(feed_tx);
        *self
            .stop_tx
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(Arc::clone(&stop_tx));

        let running_for_task = Arc::clone(&self.running);
        let download_cancel = Arc::clone(&self.download_cancel);
        let model_dir = self.model_dir.clone();
        let asr_model = self.asr_model.clone();
        let language = self.language.clone();
        let wake_word = self.wake_word.clone();
        let wake_word_backend = self.wake_word_backend;
        let pre_speech_buffer_ms = self.pre_speech_buffer_ms;
        let use_speaker_verification = self.use_speaker_verification;
        let callback = Arc::clone(&self.callback);

        let join_handle = runtime.spawn(async move {
            ambient_task(
                model_dir,
                asr_model,
                language,
                wake_word,
                wake_word_backend,
                pre_speech_buffer_ms,
                use_speaker_verification,
                callback,
                feed_rx,
                stop_tx,
                running_for_task,
                download_cancel,
                ready_tx,
            )
            .await;
        });

        *self
            .pipeline_join
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(join_handle);

        // Release the lifecycle lock while waiting so stop() can interrupt us.
        drop(_guard);

        // Model downloads can take several minutes on a slow mobile
        // connection, so use a long timeout. Missing models are the most
        // common first-run failure; once cached, start becomes fast.
        let mut result = runtime.block_on(async {
            match tokio::time::timeout(Duration::from_secs(600), ready_rx).await {
                Ok(Ok(Ok(()))) => Ok(()),
                Ok(Ok(Err(error))) => Err(TakusuError::Audio { detail: error }),
                Ok(Err(_)) => Err(TakusuError::Audio {
                    detail: "ambient pipeline was cancelled before becoming ready".to_string(),
                }),
                Err(_) => Err(TakusuError::Audio {
                    detail: "ambient pipeline did not become ready within 600 seconds".to_string(),
                }),
            }
        });

        // Re-acquire the lifecycle lock before checking state and cleaning up.
        let _guard = self
            .lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());

        if !self.running.load(Ordering::Acquire) && result.is_ok() {
            // stop() was called while we were waiting for the pipeline to become
            // ready; the task is already exiting, so report a controlled stop.
            result = Err(TakusuError::Audio {
                detail: "ambient pipeline was stopped before becoming ready".to_string(),
            });
        }

        if result.is_err() {
            // Cancel any in-progress model download and tell the spawned task
            // to stop. The task will finish once the blocking download sees the
            // cancellation flag or its next read timeout fires.
            self.download_cancel.store(true, Ordering::Release);
            if let Some(tx) = self
                .stop_tx
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
            {
                let _ = tx.send_replace(true);
            }
            self.running.store(false, Ordering::Release);
            *self
                .feed_tx
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = None;
            *self
                .stop_tx
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = None;
        }

        result
    }

    /// Stop the ambient pipeline and notify the callback.
    pub fn stop(&self) -> Result<(), TakusuError> {
        let _guard = self
            .lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());

        if !self.running.load(Ordering::Acquire) {
            return Ok(());
        }

        self.running.store(false, Ordering::Release);
        self.download_cancel.store(true, Ordering::Release);

        if let Some(tx) = self
            .stop_tx
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            let _ = tx.send_replace(true);
        }

        *self
            .feed_tx
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;

        Ok(())
    }

    /// Feed a chunk of 16 kHz mono i16 PCM into the pipeline.
    pub fn feed_pcm_chunk(&self, samples: Vec<i16>) -> Result<(), TakusuError> {
        if samples.is_empty() {
            return Err(TakusuError::Audio {
                detail: "PCM chunk was empty".to_string(),
            });
        }

        if !self.running.load(Ordering::Acquire) {
            return Err(TakusuError::Audio {
                detail: "ambient pipeline is not running".to_string(),
            });
        }

        let pcm: Vec<f32> = samples
            .into_iter()
            .map(|sample| sample as f32 / I16_MAX_F32)
            .collect();

        let guard = self
            .feed_tx
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let tx = guard.as_ref().ok_or(TakusuError::Audio {
            detail: "ambient pipeline is not running".to_string(),
        })?;

        tx.send(pcm).map_err(|_| TakusuError::Audio {
            detail: "ambient feed channel is closed".to_string(),
        })
    }

    /// Wait for the ambient task to finish, returning an error if it does not
    /// stop within `timeout_ms`.
    pub fn join(&self, timeout_ms: u64) -> Result<(), TakusuError> {
        let _guard = self
            .lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());

        let Some(join_handle) = self
            .pipeline_join
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        else {
            return Ok(());
        };

        let timeout = Duration::from_millis(timeout_ms);
        let runtime = self.ensure_runtime()?;

        runtime.block_on(async {
            match tokio::time::timeout(timeout, join_handle).await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(error)) => Err(TakusuError::Audio {
                    detail: format!("ambient task panicked: {error}"),
                }),
                Err(_) => Err(TakusuError::Audio {
                    detail: "ambient task did not stop in time".to_string(),
                }),
            }
        })
    }
}

impl Drop for MobileAmbient {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        self.download_cancel.store(true, Ordering::Release);

        // Use try_lock so a concurrent start/stop cannot make Drop block the
        // Android main thread. If the lock is held elsewhere, the cancellation
        // flags above still tell the task to exit.
        if let Some(mut stop_guard) = try_lock_or_recover(&self.stop_tx)
            && let Some(tx) = stop_guard.take()
        {
            let _ = tx.send_replace(true);
        }

        if let Some(mut feed_guard) = try_lock_or_recover(&self.feed_tx) {
            *feed_guard = None;
        }

        if let Some(mut join_guard) = try_lock_or_recover(&self.pipeline_join) {
            join_guard.take();
        }

        // Take the runtime and shut it down in the background. This does not
        // block, so it is safe to call from the Android main thread. The
        // spawned task will stop as soon as its blocking download observes the
        // cancellation flag.
        if let Some(mut runtime_guard) = try_lock_or_recover(&self.runtime)
            && let Some(runtime) = runtime_guard.take()
            && let Ok(runtime) = Arc::try_unwrap(runtime)
        {
            runtime.shutdown_background();
        }
    }
}

impl MobileAmbient {
    /// Return a handle to the tokio runtime, creating it on first use.
    fn ensure_runtime(&self) -> Result<Arc<Runtime>, TakusuError> {
        let mut guard = self
            .runtime
            .lock()
            .unwrap_or_else(|error| error.into_inner());

        if guard.is_none() {
            let runtime = Builder::new_multi_thread()
                .enable_io()
                .enable_time()
                .build()
                .map_err(|error| TakusuError::Audio {
                    detail: format!("failed to create ambient runtime: {error}"),
                })?;
            *guard = Some(Arc::new(runtime));
        }

        Ok(guard.as_ref().unwrap().clone())
    }
}

/// Resolve the user-facing ASR model name to the canonical model directory id
/// used both by `ModelCache` and by `build_asr`.
fn asr_model_id(asr_model: &str) -> Result<&'static str, TakusuError> {
    match asr_model.trim() {
        "sense-voice" | "sherpa-sense-voice-int8" => Ok("sherpa-sense-voice-int8"),
        "parakeet-ctc-ja" | "sherpa-parakeet-ctc-ja-0.6b" => Ok("sherpa-parakeet-ctc-ja-0.6b"),
        "nemotron-ja" | "sherpa-nemotron-ja-0.6b" => Ok("sherpa-nemotron-ja-0.6b"),
        _ => Err(TakusuError::Audio {
            detail: format!("unsupported asr model: {asr_model}"),
        }),
    }
}

/// Map the user-facing ASR model name to a concrete Sherpa-ONNX model and path.
fn parse_asr_model(
    asr_model: &str,
    model_dir: &Path,
) -> Result<(SherpaOnnxModel, PathBuf), TakusuError> {
    let id = asr_model_id(asr_model)?;
    let model = match id {
        "sherpa-sense-voice-int8" => SherpaOnnxModel::SenseVoice,
        "sherpa-parakeet-ctc-ja-0.6b" => SherpaOnnxModel::ParakeetJaCtc,
        "sherpa-nemotron-ja-0.6b" => SherpaOnnxModel::NemotronMultilingual,
        _ => unreachable!(),
    };
    Ok((model, model_dir.join(id)))
}

fn build_asr(
    model_dir: &Path,
    asr_model: &str,
    language: &str,
) -> Result<Arc<dyn StreamingSpeechToText>, TakusuError> {
    let (model, model_path) = parse_asr_model(asr_model, model_dir)?;
    let config = SttRuntimeConfig {
        backend: SttBackend::Sherpa,
        model,
        model_dir: Some(model_path),
        language: language.to_string(),
        use_itn: true,
        num_threads: 2,
        provider: ExecutionProvider::Cpu,
        sample_rate: SHERPA_SAMPLE_RATE as i32,
    };
    config
        .build_streaming()
        .map_err(|error: SttError| TakusuError::Audio {
            detail: format!("failed to load {asr_model}: {error}"),
        })
}

#[allow(clippy::too_many_arguments)]
async fn ambient_task(
    model_dir: PathBuf,
    asr_model: String,
    language: String,
    wake_word: String,
    wake_word_backend: WakeWordBackend,
    pre_speech_buffer_ms: u64,
    use_speaker_verification: bool,
    callback: Arc<dyn AmbientCallback>,
    mut feed_rx: mpsc::UnboundedReceiver<Vec<f32>>,
    stop_tx: Arc<watch::Sender<bool>>,
    running: Arc<AtomicBool>,
    download_cancel: Arc<AtomicBool>,
    ready_tx: oneshot::Sender<Result<(), String>>,
) {
    // First-run model download: ensure the requested ASR model and the shared
    // Silero VAD model are present before loading them. This makes the ambient
    // service self-contained and avoids the confusing load-failure loop when
    // starting before `TakusuAudioModule` has had a chance to cache anything.
    let asr_id = match asr_model_id(&asr_model) {
        Ok(id) => id,
        Err(error) => {
            let _ = ready_tx.send(Err(format!("{error}")));
            callback.on_error(format!("{error}"));
            running.store(false, Ordering::Release);
            callback.on_stopped();
            return;
        }
    };

    let cache = ModelCache::new(model_dir.clone());
    let cache_for_download = cache.clone();
    let asr_id_for_download = asr_id.to_string();
    let asr_cancel = Arc::clone(&download_cancel);
    let vad_cancel = Arc::clone(&download_cancel);
    let kws_cancel = Arc::clone(&download_cancel);
    // The WenetSpeech spotter is a separate model bundle from the ASR. Ensure
    // it too so the sherpa-kws backend can load without a further round-trip.
    let need_kws_model = wake_word_backend == WakeWordBackend::SherpaKws;
    let download_result: Result<Result<PathBuf, ModelError>, tokio::task::JoinError> =
        tokio::task::spawn_blocking(move || {
            cache_for_download.ensure_with_cancel(&asr_id_for_download, Some(asr_cancel))?;
            cache_for_download.ensure_with_cancel("silero-vad", Some(vad_cancel))?;
            if need_kws_model {
                return cache_for_download
                    .ensure_with_cancel(DEFAULT_KWS_MODEL_ID, Some(kws_cancel));
            }
            Ok(PathBuf::new())
        })
        .await;

    let kws_dir = match download_result {
        Ok(Ok(dir)) => dir,
        Ok(Err(ModelError::Cancelled)) => {
            let detail = "model download cancelled".to_string();
            if ready_tx.send(Err(detail.clone())).is_err() {
                // The caller has already given up; exit silently.
                running.store(false, Ordering::Release);
                return;
            }
            callback.on_error(detail);
            running.store(false, Ordering::Release);
            callback.on_stopped();
            return;
        }
        Ok(Err(error)) => {
            let detail = format!("model download failed: {error}");
            if ready_tx.send(Err(detail.clone())).is_err() {
                running.store(false, Ordering::Release);
                return;
            }
            callback.on_error(detail);
            running.store(false, Ordering::Release);
            callback.on_stopped();
            return;
        }
        Err(error) => {
            let detail = format!("model download task failed: {error}");
            if ready_tx.send(Err(detail.clone())).is_err() {
                running.store(false, Ordering::Release);
                return;
            }
            callback.on_error(detail);
            running.store(false, Ordering::Release);
            callback.on_stopped();
            return;
        }
    };

    let asr_language = language.clone();
    let asr_model_dir = model_dir.clone();
    let asr = match tokio::task::spawn_blocking(move || {
        build_asr(&asr_model_dir, &asr_model, &asr_language)
    })
    .await
    {
        Ok(Ok(asr)) => asr,
        Ok(Err(error)) => {
            let detail = format!("{error}");
            if ready_tx.send(Err(detail.clone())).is_err() {
                running.store(false, Ordering::Release);
                return;
            }
            callback.on_error(detail);
            running.store(false, Ordering::Release);
            callback.on_stopped();
            return;
        }
        Err(error) => {
            let detail = format!("ASR loading task failed: {error}");
            if ready_tx.send(Err(detail.clone())).is_err() {
                running.store(false, Ordering::Release);
                return;
            }
            callback.on_error(detail);
            running.store(false, Ordering::Release);
            callback.on_stopped();
            return;
        }
    };

    let model_dir_for_endpoint = model_dir.clone();
    let mut endpoint = match tokio::task::spawn_blocking(move || {
        default_endpoint_with_config_and_cache_dir(
            VadEndpointConfig::default(),
            &model_dir_for_endpoint,
        )
    })
    .await
    {
        Ok(endpoint) => endpoint,
        Err(error) => {
            let detail = format!("endpoint loading task failed: {error}");
            if ready_tx.send(Err(detail.clone())).is_err() {
                running.store(false, Ordering::Release);
                return;
            }
            callback.on_error(detail);
            running.store(false, Ordering::Release);
            callback.on_stopped();
            return;
        }
    };

    // Notify the caller that ASR/VAD are loaded and we are about to start
    // consuming chunks. From this point on `feed_pcm_chunk` will be accepted.
    if ready_tx.send(Ok(())).is_err() {
        // Caller has already given up on us; stop without further callbacks.
        running.store(false, Ordering::Release);
        return;
    }

    // Point the sherpa-kws loader at the downloaded model directory. `SherpaKws`
    // falls back to the desktop `~/.cache/takusu/models` when this is empty,
    // which does not exist on Android and would silently degrade the wake-word
    // backend to streaming-ASR text matching. The path must be the model
    // sub-directory itself: `SherpaKws::new` scans it directly for
    // `tokens.txt` and the encoder/decoder/joiner ONNX files.
    let mut kws_config = KwsConfig::for_wenetspeech();
    if !kws_dir.as_os_str().is_empty() {
        kws_config.model_dir = kws_dir.to_string_lossy().into_owned();
    }

    let config = AmbientConfig {
        enabled: true,
        wake_word: wake_word.clone(),
        wake_word_backend,
        kws: kws_config,
        asr_language: language,
        max_utterance_seconds: 60,
        pre_speech_buffer_ms,
        sample_rate: SHERPA_SAMPLE_RATE,
        verify_speaker: use_speaker_verification,
    };

    let stop_rx = stop_tx.subscribe();
    let pipeline_stop_rx = stop_tx.subscribe();
    // Build the pipeline (wake detector, VAD endpoint, ASR) once and reuse it
    // across captures: `run_with_chunks` resets the wake detector and endpoint
    // at the start of each utterance, so re-creating it per capture would
    // re-load the ~31 MB sherpa-kws spotter between every wake word on-device.
    let mut pipeline = match AmbientPipeline::new(
        config.clone(),
        &mut *endpoint,
        Arc::clone(&asr),
        pipeline_stop_rx,
    )
    .await
    {
        Ok(pipeline) => pipeline,
        Err(error) => {
            // The caller already got the ready signal (models loaded); surface
            // this failure through the callback and the stop path instead.
            callback.on_error(format!("{error}"));
            running.store(false, Ordering::Release);
            callback.on_stopped();
            return;
        }
    };
    // The models and detector are loaded; only now start advertising a
    // listening state to the notification.
    callback.on_listening();

    while !*stop_rx.borrow() && running.load(Ordering::Acquire) {
        let pipeline_result = run_pipeline_iteration(&mut pipeline, &callback, &mut feed_rx).await;

        match pipeline_result {
            Ok(Some(AmbientResult { text, samples })) => {
                callback.on_wake_word(wake_word.clone());
                callback.on_transcribing();
                // When speaker verification is disabled the samples are empty,
                // avoiding an unnecessary multi-MB FFI copy.
                let result_samples = if use_speaker_verification {
                    samples
                } else {
                    Vec::new()
                };
                callback.on_result(text, result_samples);
            }
            Ok(None) => {
                // Stream ended or was stopped; exit the loop.
                break;
            }
            Err(AmbientError::Cancelled) => {
                break;
            }
            Err(error) => {
                callback.on_error(format!("{error}"));
                break;
            }
        }
    }

    running.store(false, Ordering::Release);
    callback.on_stopped();
}

/// Run one wake-word to command capture, forwarding the persistent feed channel
/// into the pipeline's per-iteration chunk channel.
async fn run_pipeline_iteration(
    pipeline: &mut AmbientPipeline<'_>,
    callback: &Arc<dyn AmbientCallback>,
    feed_rx: &mut mpsc::UnboundedReceiver<Vec<f32>>,
) -> Result<Option<AmbientResult>, AmbientError> {
    // A bounded queue between the forwarder and the pipeline. When the ASR is
    // slower than real time the pipeline stops consuming, so an unbounded
    // channel here would keep accumulating raw audio (hundreds of MB over a
    // long conversation). Once full, incoming chunks are dropped: this bounds
    // memory, at the cost of losing (possibly command) audio while the ASR is
    // saturated.
    let (chunk_tx, chunk_rx) = mpsc::channel::<Vec<f32>>(MAX_BUFFERED_CHUNKS);

    callback.on_listening();

    let mut fut = Box::pin(pipeline.run_with_chunks(chunk_rx));

    let result: Result<Option<AmbientResult>, AmbientError> = 'pipeline: loop {
        tokio::select! {
            chunk = feed_rx.recv() => {
                match chunk {
                    Some(samples) => {
                        // Drop on saturation (the pipeline is consuming slower
                        // than real time, e.g. ASR under load) so the queue
                        // cannot grow without bound.
                        let _ = chunk_tx.try_send(samples);
                    }
                    None => {
                        // The caller stopped feeding audio.
                        drop(chunk_tx);
                        let result = fut.await;
                        break 'pipeline result;
                    }
                }
            }
            res = &mut fut => {
                break 'pipeline res;
            }
        }
    };

    result
}
