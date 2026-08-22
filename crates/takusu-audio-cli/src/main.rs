use std::io::BufRead;
use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};
#[cfg(feature = "hush")]
use takusu_audio::hush::Hush;
use takusu_audio::{
    ExecutionProvider, RecordConfig, SHERPA_SAMPLE_RATE, SherpaOnnxModel, StreamingRecorder,
    SttBackend, SttRuntimeConfig, read_wav, record, write_wav,
};

#[derive(Parser)]
#[command(
    name = "takusu-audio",
    version,
    about = "Audio recording and speech-to-text CLI"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Record audio from the microphone (Press Enter to stop)
    Record {
        /// Output WAV file
        #[arg(short, long, default_value = "record.wav")]
        output: PathBuf,

        /// Maximum recording duration in seconds
        #[arg(long, default_value_t = 300.0)]
        max_duration: f64,

        /// Stop automatically ~500 ms after speech ends (VAD endpointing)
        #[arg(long, default_value_t = false)]
        endpoint: bool,
    },

    /// Transcribe a WAV audio file using Sherpa-ONNX
    Transcribe {
        /// Path to WAV audio file
        audio: PathBuf,

        /// Path to Sherpa-ONNX model directory (omit to download on first run)
        #[arg(long)]
        sherpa_model_dir: Option<PathBuf>,

        /// Sherpa-ONNX model family
        #[arg(long, value_enum, default_value = "sense-voice")]
        sherpa_model: SherpaOnnxModel,

        /// Language hint (auto, zh, en, ja, ko, etc.)
        #[arg(long, default_value = "auto")]
        sherpa_language: String,

        /// Use Sherpa-ONNX SenseVoice ITN
        #[arg(long, action = clap::ArgAction::Set, default_value = "true")]
        sherpa_use_itn: bool,

        /// Number of threads for Sherpa-ONNX inference
        #[arg(long, default_value_t = 2)]
        sherpa_num_threads: i32,

        /// ONNX provider for Sherpa-ONNX (cpu, cuda, coreml)
        #[arg(long, value_enum, default_value = "cpu")]
        sherpa_provider: ExecutionProvider,

        /// Use streaming (chunked) transcription; defaults to true for nemotron-ja
        #[arg(long)]
        streaming: Option<bool>,
    },

    /// Record from microphone and transcribe with Sherpa-ONNX (Press Enter to stop)
    Listen {
        /// Output WAV file (saved even after transcription)
        #[arg(short, long, default_value = "record.wav")]
        output: PathBuf,

        /// Maximum recording duration in seconds
        #[arg(long, default_value_t = 120.0)]
        max_duration: f64,

        /// Path to Sherpa-ONNX model directory (omit to download on first run)
        #[arg(long)]
        sherpa_model_dir: Option<PathBuf>,

        /// Sherpa-ONNX model family
        #[arg(long, value_enum, default_value = "sense-voice")]
        sherpa_model: SherpaOnnxModel,

        /// Language hint (auto, zh, en, ja, ko, etc.)
        #[arg(long, default_value = "auto")]
        sherpa_language: String,

        /// Use Sherpa-ONNX SenseVoice ITN
        #[arg(long, action = clap::ArgAction::Set, default_value = "true")]
        sherpa_use_itn: bool,

        /// Number of threads for Sherpa-ONNX inference
        #[arg(long, default_value_t = 2)]
        sherpa_num_threads: i32,

        /// ONNX provider for Sherpa-ONNX (cpu, cuda, coreml)
        #[arg(long, value_enum, default_value = "cpu")]
        sherpa_provider: ExecutionProvider,

        /// Use streaming (chunked) transcription; defaults to true for nemotron-ja
        #[arg(long)]
        streaming: Option<bool>,
    },

    #[cfg(feature = "hush")]
    /// Enhance a WAV file with the Hush denoiser
    Hush {
        /// Path to Hush ONNX model directory (omit to download on first run)
        #[arg(long)]
        model_dir: Option<PathBuf>,

        /// Input WAV file
        input: PathBuf,

        /// Output WAV file
        #[arg(short, long, default_value = "enhanced.wav")]
        output: PathBuf,

        /// Target RMS for input normalization (0 disables normalization)
        #[arg(long, default_value = "0.1")]
        target_rms: f32,

        /// Do not restore the original loudness after denoising
        #[arg(long, default_value_t = false)]
        no_restore: bool,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Record {
            output,
            max_duration,
            endpoint,
        } => {
            let config = takusu_audio::RecordConfig {
                max_duration: Duration::from_secs_f64(max_duration),
                ..Default::default()
            };

            let samples = if endpoint {
                listen_endpoint(&config).await
            } else {
                match record(&config) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("Recording error: {e}");
                        std::process::exit(1);
                    }
                }
            };

            eprintln!(
                "Recorded {} samples ({:.1}s)",
                samples.len(),
                samples.len() as f64 / SHERPA_SAMPLE_RATE as f64
            );
            write_wav(&output, &samples, SHERPA_SAMPLE_RATE).unwrap_or_else(|e| {
                eprintln!("Failed to write WAV: {e}");
                std::process::exit(1);
            });
            eprintln!("Saved to {}", output.display());
        }

        Commands::Transcribe {
            audio,
            sherpa_model_dir,
            sherpa_model,
            sherpa_language,
            sherpa_use_itn,
            sherpa_num_threads,
            sherpa_provider,
            streaming,
        } => {
            let samples = read_wav(&audio).unwrap_or_else(|e| {
                eprintln!("Failed to read WAV: {e}");
                std::process::exit(1);
            });
            eprintln!("Loaded {} samples from {}", samples.len(), audio.display());

            let stt_config = SttRuntimeConfig {
                backend: SttBackend::Sherpa,
                model: sherpa_model,
                model_dir: sherpa_model_dir,
                language: sherpa_language.clone(),
                use_itn: sherpa_use_itn,
                num_threads: sherpa_num_threads,
                provider: sherpa_provider,
                sample_rate: SHERPA_SAMPLE_RATE as i32,
            };

            let use_streaming = streaming.unwrap_or_else(|| stt_config.default_streaming());
            let text = if use_streaming {
                transcribe_streaming(&samples, stt_config, &sherpa_language, true).await
            } else {
                transcribe_offline(&samples, stt_config).await
            };
            println!("{text}");
        }

        Commands::Listen {
            output,
            max_duration,
            sherpa_model_dir,
            sherpa_model,
            sherpa_language,
            sherpa_use_itn,
            sherpa_num_threads,
            sherpa_provider,
            streaming,
        } => {
            let stt_config = SttRuntimeConfig {
                backend: SttBackend::Sherpa,
                model: sherpa_model,
                model_dir: sherpa_model_dir,
                language: sherpa_language.clone(),
                use_itn: sherpa_use_itn,
                num_threads: sherpa_num_threads,
                provider: sherpa_provider,
                sample_rate: SHERPA_SAMPLE_RATE as i32,
            };

            let use_streaming = streaming.unwrap_or_else(|| stt_config.default_streaming());
            let text = if use_streaming {
                listen_streaming(stt_config, &sherpa_language, max_duration, &output).await
            } else {
                let config = RecordConfig {
                    max_duration: Duration::from_secs_f64(max_duration),
                    ..Default::default()
                };

                let samples = match record(&config) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("Recording error: {e}");
                        std::process::exit(1);
                    }
                };

                if samples.is_empty() {
                    eprintln!("No audio recorded.");
                    std::process::exit(1);
                }

                eprintln!(
                    "Recorded {} samples ({:.1}s)",
                    samples.len(),
                    samples.len() as f64 / SHERPA_SAMPLE_RATE as f64
                );
                write_wav(&output, &samples, SHERPA_SAMPLE_RATE).unwrap_or_else(|e| {
                    eprintln!("Failed to write WAV: {e}");
                    std::process::exit(1);
                });
                eprintln!("Saved to {}", output.display());

                transcribe_offline(&samples, stt_config).await
            };
            println!("{text}");
        }

        #[cfg(feature = "hush")]
        Commands::Hush {
            model_dir,
            input,
            output,
            target_rms,
            no_restore,
        } => {
            let samples = read_wav(&input).unwrap_or_else(|e| {
                eprintln!("Failed to read WAV: {e}");
                std::process::exit(1);
            });
            eprintln!("Loaded {} samples from {}", samples.len(), input.display());

            let model_dir = match model_dir {
                Some(path) => path,
                None => {
                    eprintln!("Downloading Hush model on first run...");
                    let path = tokio::task::spawn_blocking(|| {
                        let cache =
                            takusu_audio::ModelCache::default_dir().map_err(|e| e.to_string())?;
                        cache.ensure("hush").map_err(|e| e.to_string())
                    })
                    .await
                    .unwrap_or_else(|e| {
                        eprintln!("Model download error: {e}");
                        std::process::exit(1);
                    })
                    .unwrap_or_else(|e| {
                        eprintln!("Model cache error: {e}");
                        std::process::exit(1);
                    });
                    eprintln!("Hush model cached at {}", path.display());
                    path
                }
            };

            let mut hush = Hush::from_model_dir(&model_dir).unwrap_or_else(|e| {
                eprintln!("Hush model error: {e}");
                std::process::exit(1);
            });
            let target = if target_rms > 0.0 {
                Some(target_rms)
            } else {
                None
            };
            hush.set_target_rms(target);
            hush.set_restore_loudness(!no_restore);

            let start = std::time::Instant::now();
            let enhanced = hush.enhance(&samples).unwrap_or_else(|e| {
                eprintln!("Hush enhancement error: {e}");
                std::process::exit(1);
            });
            eprintln!("Done in {:.1}s.", start.elapsed().as_secs_f64());
            write_wav(&output, &enhanced, SHERPA_SAMPLE_RATE).unwrap_or_else(|e| {
                eprintln!("Failed to write WAV: {e}");
                std::process::exit(1);
            });
            eprintln!("Saved to {}", output.display());
        }
    }
}

async fn listen_streaming(
    stt_config: SttRuntimeConfig,
    language: &str,
    max_duration: f64,
    output: &std::path::Path,
) -> String {
    #[cfg(not(feature = "sherpa"))]
    {
        let _ = (stt_config, language, max_duration, output);
        eprintln!("Sherpa-ONNX backend requires the 'sherpa' feature at compile time");
        std::process::exit(1);
    }

    #[cfg(feature = "sherpa")]
    {
        let language = language.to_string();
        let output = output.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let handle = tokio::runtime::Handle::try_current().unwrap_or_else(|e| {
                eprintln!("No tokio runtime: {e}");
                std::process::exit(1)
            });

            eprintln!("Loading Sherpa-ONNX model...");
            let start = std::time::Instant::now();
            let asr = stt_config.build_streaming().unwrap_or_else(|e| {
                eprintln!("Sherpa-ONNX model error: {e}");
                std::process::exit(1)
            });
            eprintln!("Model loaded in {:.1}s.", start.elapsed().as_secs_f64());

            handle.block_on(async {
                eprintln!("Recording... Press Enter to stop.");
                let mut stream = asr.start_stream(&language).await.unwrap_or_else(|e| {
                    eprintln!("Sherpa-ONNX stream error: {e}");
                    std::process::exit(1)
                });

                let (recorder, mut chunk_rx) = StreamingRecorder::start(RecordConfig {
                    max_duration: Duration::from_secs_f64(max_duration),
                    ..Default::default()
                })
                .unwrap_or_else(|e| {
                    eprintln!("Recording error: {e}");
                    std::process::exit(1)
                });

                let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel();
                tokio::task::spawn_blocking(move || {
                    let _ = std::io::stdin().lock().read_line(&mut String::new());
                    let _ = stop_tx.send(());
                });

                let mut all_samples = Vec::new();
                let mut last = String::new();

                let maybe_text: Option<String> = loop {
                    tokio::select! {
                        chunk = chunk_rx.recv() => {
                            match chunk {
                                Some(chunk) => {
                                    all_samples.extend_from_slice(&chunk);
                                    stream.accept_waveform(&chunk);
                                    let text = stream.text();
                                    if !text.is_empty() && text != last {
                                        eprintln!("> {text}");
                                        last = text;
                                    }
                                }
                                None => break Some(stream.finish().await.unwrap_or_else(|e| {
                                    eprintln!("Sherpa-ONNX transcription error: {e}");
                                    std::process::exit(1)
                                })),
                            }
                        }
                        _ = &mut stop_rx => {
                            recorder.stop();
                            break None;
                        }
                    }
                };

                let text = match maybe_text {
                    Some(text) => text,
                    None => {
                        while let Some(chunk) = chunk_rx.recv().await {
                            all_samples.extend_from_slice(&chunk);
                            stream.accept_waveform(&chunk);
                            let text = stream.text();
                            if !text.is_empty() && text != last {
                                eprintln!("> {text}");
                                last = text;
                            }
                        }
                        stream.finish().await.unwrap_or_else(|e| {
                            eprintln!("Sherpa-ONNX transcription error: {e}");
                            std::process::exit(1)
                        })
                    }
                };

                recorder.join().unwrap_or_else(|e| {
                    eprintln!("Recording error: {e}");
                    std::process::exit(1)
                });

                eprintln!(
                    "Recorded {} samples ({:.1}s)",
                    all_samples.len(),
                    all_samples.len() as f64 / SHERPA_SAMPLE_RATE as f64
                );
                write_wav(&output, &all_samples, SHERPA_SAMPLE_RATE).unwrap_or_else(|e| {
                    eprintln!("Failed to write WAV: {e}");
                    std::process::exit(1)
                });
                eprintln!("Saved to {}", output.display());

                text
            })
        })
        .await
        .unwrap_or_else(|e| {
            eprintln!("Streaming transcription task failed: {e}");
            std::process::exit(1)
        })
    }
}

/// Record from the microphone until VAD endpointing detects the end of speech
/// (a few hundred ms after the user stops talking). This is the on-device smoke
/// test for WI-12 VAD endpointing: run it, speak, then stop and observe that
/// recording stops ~0.5 s later, printing the segment boundaries. Ctrl-C
/// gracefully stops and still returns the captured audio.
///
/// The stream is left *unnormalized* so the VAD (Silero by default, else
/// raw-energy) sees real input levels instead of the recorder's fixed-RMS boost
/// that turns silence into "speech".
async fn listen_endpoint(config: &takusu_audio::RecordConfig) -> Vec<f32> {
    use takusu_audio::VadEvent;

    let mut raw_config = config.clone();
    raw_config.normalize_audio = false;

    let (recorder, mut rx) =
        takusu_audio::StreamingRecorder::start(raw_config).unwrap_or_else(|e| {
            eprintln!("Recorder error: {e}");
            std::process::exit(1);
        });

    eprintln!("Initializing VAD (downloads the Silero model on first run)...");
    let mut endpoint = takusu_audio::default_endpoint_async().await;

    let mut samples = Vec::new();
    let start = std::time::Instant::now();
    eprintln!("Listening; recording stops ~500 ms after speech ends. Ctrl-C to stop.");
    loop {
        tokio::select! {
            chunk = rx.recv() => {
                match chunk {
                    Some(chunk) => {
                        samples.extend_from_slice(&chunk);
                        match endpoint.push(&chunk) {
                            Some(VadEvent::SpeechStart) => eprintln!(
                                "[+{:.2}s] speech start",
                                start.elapsed().as_secs_f64()
                            ),
                            Some(VadEvent::SpeechEnd) => {
                                eprintln!(
                                    "[+{:.2}s] speech end (endpoint)",
                                    start.elapsed().as_secs_f64()
                                );
                                recorder.stop();
                                break;
                            }
                            None => {}
                        }
                    }
                    None => break,
                }
            }
            _ = tokio::signal::ctrl_c() => {
                eprintln!("interrupted; capturing what was recorded");
                recorder.stop();
                break;
            }
        }
    }
    let _ = tokio::task::spawn_blocking(move || recorder.join()).await;
    samples
}

async fn transcribe_offline(samples: &[f32], stt_config: SttRuntimeConfig) -> String {
    #[cfg(not(feature = "sherpa"))]
    {
        let _ = (samples, stt_config);
        eprintln!("Sherpa-ONNX backend requires the 'sherpa' feature at compile time");
        std::process::exit(1);
    }

    #[cfg(feature = "sherpa")]
    {
        let samples = samples.to_vec();
        tokio::task::spawn_blocking(move || {
            eprintln!("Loading Sherpa-ONNX model...");
            let start = std::time::Instant::now();
            let asr = stt_config.build().unwrap_or_else(|e| {
                eprintln!("Sherpa-ONNX model error: {e}");
                std::process::exit(1);
            });
            eprintln!("Model loaded in {:.1}s.", start.elapsed().as_secs_f64());

            let handle = tokio::runtime::Handle::try_current().unwrap_or_else(|e| {
                eprintln!("No tokio runtime: {e}");
                std::process::exit(1)
            });

            eprintln!(
                "Transcribing ({} samples, {:.1}s) with Sherpa-ONNX...",
                samples.len(),
                samples.len() as f64 / SHERPA_SAMPLE_RATE as f64
            );
            let start = std::time::Instant::now();
            let text = handle
                .block_on(asr.transcribe(&samples))
                .unwrap_or_else(|e| {
                    eprintln!("Sherpa-ONNX transcription error: {e}");
                    std::process::exit(1)
                });
            eprintln!("Done in {:.1}s.", start.elapsed().as_secs_f64());
            text
        })
        .await
        .unwrap_or_else(|e| {
            eprintln!("Transcription task failed: {e}");
            std::process::exit(1)
        })
    }
}

async fn transcribe_streaming(
    samples: &[f32],
    stt_config: SttRuntimeConfig,
    language: &str,
    print_partial: bool,
) -> String {
    #[cfg(not(feature = "sherpa"))]
    {
        let _ = (samples, stt_config, language, print_partial);
        eprintln!("Sherpa-ONNX backend requires the 'sherpa' feature at compile time");
        std::process::exit(1);
    }

    #[cfg(feature = "sherpa")]
    {
        let samples = samples.to_vec();
        let language = language.to_string();
        tokio::task::spawn_blocking(move || {
            let handle = tokio::runtime::Handle::try_current().unwrap_or_else(|e| {
                eprintln!("No tokio runtime: {e}");
                std::process::exit(1)
            });

            eprintln!("Loading Sherpa-ONNX model...");
            let start = std::time::Instant::now();
            let asr = stt_config.build_streaming().unwrap_or_else(|e| {
                eprintln!("Sherpa-ONNX model error: {e}");
                std::process::exit(1);
            });
            eprintln!("Model loaded in {:.1}s.", start.elapsed().as_secs_f64());

            eprintln!("Transcribing (streaming)...");
            let start = std::time::Instant::now();
            let mut stream = handle
                .block_on(asr.start_stream(&language))
                .unwrap_or_else(|e| {
                    eprintln!("Sherpa-ONNX stream error: {e}");
                    std::process::exit(1)
                });

            let chunk_size = (SHERPA_SAMPLE_RATE as usize * 160) / 1000;
            let mut last = String::new();
            for chunk in samples.chunks(chunk_size) {
                stream.accept_waveform(chunk);
                if print_partial {
                    let text = stream.text();
                    if !text.is_empty() && text != last {
                        eprintln!("> {text}");
                        last = text;
                    }
                }
            }

            let text = tokio::runtime::Handle::current()
                .block_on(stream.finish())
                .unwrap_or_else(|e| {
                    eprintln!("Sherpa-ONNX transcription error: {e}");
                    std::process::exit(1)
                });
            eprintln!("Done in {:.1}s.", start.elapsed().as_secs_f64());
            text
        })
        .await
        .unwrap_or_else(|e| {
            eprintln!("Streaming transcription task failed: {e}");
            std::process::exit(1)
        })
    }
}
