use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
#[cfg(feature = "hush")]
use takusu_audio::hush::Hush;
use takusu_audio::{RecordConfig, SHERPA_SAMPLE_RATE, read_wav, record, write_wav};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum SherpaModel {
    SenseVoice,
    FunasrNano,
}

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
    },

    /// Transcribe a WAV audio file using Sherpa-ONNX
    Transcribe {
        /// Path to WAV audio file
        audio: PathBuf,

        /// Path to Sherpa-ONNX model directory (omit to download SenseVoice on first run)
        #[arg(long)]
        sherpa_model_dir: Option<PathBuf>,

        /// Sherpa-ONNX model family (sense-voice or funasr-nano)
        #[arg(long, value_enum, default_value = "sense-voice")]
        sherpa_model: SherpaModel,

        /// SenseVoice language (auto, zh, en, ja, ko)
        #[arg(long, default_value = "auto")]
        sherpa_language: String,

        /// Use Sherpa-ONNX SenseVoice ITN
        #[arg(long, action = clap::ArgAction::Set, default_value = "true")]
        sherpa_use_itn: bool,

        /// Number of threads for Sherpa-ONNX inference
        #[arg(long, default_value_t = 2)]
        sherpa_num_threads: i32,

        /// ONNX provider for Sherpa-ONNX (cpu, cuda, etc.)
        #[arg(long, default_value = "cpu")]
        sherpa_provider: String,
    },

    /// Record from microphone and transcribe with Sherpa-ONNX (Press Enter to stop)
    Listen {
        /// Output WAV file (saved even after transcription)
        #[arg(short, long, default_value = "record.wav")]
        output: PathBuf,

        /// Maximum recording duration in seconds
        #[arg(long, default_value_t = 120.0)]
        max_duration: f64,

        /// Path to Sherpa-ONNX model directory (omit to download SenseVoice on first run)
        #[arg(long)]
        sherpa_model_dir: Option<PathBuf>,

        /// Sherpa-ONNX model family (sense-voice or funasr-nano)
        #[arg(long, value_enum, default_value = "sense-voice")]
        sherpa_model: SherpaModel,

        /// SenseVoice language (auto, zh, en, ja, ko)
        #[arg(long, default_value = "auto")]
        sherpa_language: String,

        /// Use Sherpa-ONNX SenseVoice ITN
        #[arg(long, action = clap::ArgAction::Set, default_value = "true")]
        sherpa_use_itn: bool,

        /// Number of threads for Sherpa-ONNX inference
        #[arg(long, default_value_t = 2)]
        sherpa_num_threads: i32,

        /// ONNX provider for Sherpa-ONNX (cpu, cuda, etc.)
        #[arg(long, default_value = "cpu")]
        sherpa_provider: String,
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
        } => {
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
        } => {
            let samples = read_wav(&audio).unwrap_or_else(|e| {
                eprintln!("Failed to read WAV: {e}");
                std::process::exit(1);
            });
            eprintln!("Loaded {} samples from {}", samples.len(), audio.display());

            let text = transcribe_sherpa(
                &samples,
                sherpa_model_dir,
                sherpa_model,
                sherpa_language,
                sherpa_use_itn,
                sherpa_num_threads,
                sherpa_provider,
            )
            .await;
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
        } => {
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

            let text = transcribe_sherpa(
                &samples,
                sherpa_model_dir,
                sherpa_model,
                sherpa_language,
                sherpa_use_itn,
                sherpa_num_threads,
                sherpa_provider,
            )
            .await;
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

async fn transcribe_sherpa(
    samples: &[f32],
    sherpa_model_dir: Option<PathBuf>,
    sherpa_model: SherpaModel,
    sherpa_language: String,
    sherpa_use_itn: bool,
    sherpa_num_threads: i32,
    sherpa_provider: String,
) -> String {
    #[cfg(not(feature = "sherpa"))]
    {
        let _ = (
            samples,
            sherpa_model_dir,
            sherpa_model,
            sherpa_language,
            sherpa_use_itn,
            sherpa_num_threads,
            sherpa_provider,
        );
        eprintln!("Sherpa-ONNX backend requires the 'sherpa' feature at compile time");
        std::process::exit(1);
    }

    #[cfg(feature = "sherpa")]
    {
        use takusu_audio::{
            ModelCache, SherpaOnnxAsr, SherpaOnnxAsrConfig, SherpaOnnxModel, SpeechToText,
        };

        let model = match sherpa_model {
            SherpaModel::SenseVoice => SherpaOnnxModel::SenseVoice,
            SherpaModel::FunasrNano => SherpaOnnxModel::FunasrNano,
        };

        let model_dir = match sherpa_model_dir {
            Some(path) => path,
            None => {
                if matches!(model, SherpaOnnxModel::FunasrNano) {
                    eprintln!("--sherpa-model-dir is required for funasr-nano");
                    std::process::exit(1);
                }
                eprintln!("Downloading Sherpa-ONNX SenseVoice model on first run...");
                let path = tokio::task::spawn_blocking(|| {
                    let cache = ModelCache::default_dir().map_err(|e| e.to_string())?;
                    cache
                        .ensure("sherpa-sense-voice-int8")
                        .map_err(|e| e.to_string())
                })
                .await
                .unwrap_or_else(|e| {
                    eprintln!("Model download join error: {e}");
                    std::process::exit(1);
                })
                .unwrap_or_else(|e| {
                    eprintln!("Model download error: {e}");
                    std::process::exit(1);
                });
                eprintln!("Sherpa-ONNX model cached at {}", path.display());
                path
            }
        };

        let config = SherpaOnnxAsrConfig {
            model_dir,
            model,
            tokens: None,
            num_threads: sherpa_num_threads,
            provider: sherpa_provider,
            sample_rate: SHERPA_SAMPLE_RATE as i32,
            language: Some(sherpa_language),
            use_itn: sherpa_use_itn,
        };

        let samples = samples.to_vec();
        tokio::task::spawn_blocking(move || {
            eprintln!(
                "Loading Sherpa-ONNX model from {}...",
                config.model_dir.display()
            );
            let start = std::time::Instant::now();
            let asr = SherpaOnnxAsr::from_config(&config).unwrap_or_else(|e| {
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
