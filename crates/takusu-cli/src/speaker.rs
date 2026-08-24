//! Speaker voiceprint enrollment and verification for the takusu CLI.

use std::path::PathBuf;

use clap::{Args, Subcommand};
use takusu_audio::{
    AudioError, DEFAULT_SPEAKER_MODEL_ID, DEFAULT_VERIFY_THRESHOLD, ExecutionProvider, ModelCache,
    SpeakerConfig, SpeakerError, SpeakerVerifier, read_wav,
};
use takusu_local_lib::error::{AppError, BadRequestKind};

/// Speaker voiceprint subcommands.
#[derive(Subcommand)]
pub enum SpeakerCommands {
    /// Enroll a speaker from one or more WAV files.
    Enroll(EnrollArgs),

    /// Verify a WAV file against an enrolled speaker.
    Verify(VerifyArgs),

    /// Delete an enrolled speaker.
    Delete(DeleteArgs),

    /// List enrolled speakers.
    List(ListArgs),
}

#[derive(Args)]
pub struct EnrollArgs {
    /// Speaker name.
    #[arg(short, long, default_value = "default")]
    pub name: String,

    /// WAV files to enroll (multiple files are averaged).
    #[arg(required = true)]
    pub audio: Vec<PathBuf>,

    /// Path to the speaker embedding model directory (omit to download on first run).
    #[arg(long)]
    pub model_dir: Option<PathBuf>,

    /// Directory to persist voiceprints.
    #[arg(long)]
    pub voice_dir: Option<PathBuf>,

    /// Number of threads for ONNX inference.
    #[arg(long, default_value_t = 1)]
    pub num_threads: i32,

    /// ONNX execution provider.
    #[arg(long, value_enum, default_value = "cpu")]
    pub provider: ExecutionProvider,

    /// Verification threshold.
    #[arg(long, default_value_t = DEFAULT_VERIFY_THRESHOLD)]
    pub threshold: f32,
}

#[derive(Args)]
pub struct VerifyArgs {
    /// Speaker name.
    #[arg(short, long, default_value = "default")]
    pub name: String,

    /// WAV file to verify.
    pub audio: PathBuf,

    /// Path to the speaker embedding model directory.
    #[arg(long)]
    pub model_dir: Option<PathBuf>,

    /// Directory to persist voiceprints.
    #[arg(long)]
    pub voice_dir: Option<PathBuf>,

    /// Number of threads for ONNX inference.
    #[arg(long, default_value_t = 1)]
    pub num_threads: i32,

    /// ONNX execution provider.
    #[arg(long, value_enum, default_value = "cpu")]
    pub provider: ExecutionProvider,

    /// Verification threshold.
    #[arg(long, default_value_t = DEFAULT_VERIFY_THRESHOLD)]
    pub threshold: f32,
}

#[derive(Args)]
pub struct DeleteArgs {
    /// Speaker name.
    #[arg(short, long, default_value = "default")]
    pub name: String,

    /// Directory to persist voiceprints.
    #[arg(long)]
    pub voice_dir: Option<PathBuf>,

    /// Path to the speaker embedding model directory.
    #[arg(long)]
    pub model_dir: Option<PathBuf>,
}

#[derive(Args)]
pub struct ListArgs {
    /// Directory to persist voiceprints.
    #[arg(long)]
    pub voice_dir: Option<PathBuf>,

    /// Path to the speaker embedding model directory.
    #[arg(long)]
    pub model_dir: Option<PathBuf>,
}

pub async fn run_speaker(command: SpeakerCommands) -> Result<(), AppError> {
    match command {
        SpeakerCommands::Enroll(args) => {
            let (verifier, _model_dir) = load_speaker_verifier(
                args.model_dir,
                args.voice_dir.unwrap_or_else(default_voice_dir),
                args.num_threads,
                args.provider,
                args.threshold,
            )
            .await?;

            let samples: Vec<Vec<f32>> = args
                .audio
                .iter()
                .map(|path| read_wav(path).map_err(|e| audio_error(path, e)))
                .collect::<Result<Vec<_>, _>>()?;

            if samples.is_empty() {
                return Err(AppError::BadRequest(BadRequestKind::Other(
                    "no audio files provided".into(),
                )));
            }

            let refs: Vec<&[f32]> = samples.iter().map(|v| v.as_slice()).collect();
            verifier
                .enroll_list(&args.name, &refs)
                .map_err(speaker_error)?;
            println!("Enrolled speaker: {}", args.name);
        }

        SpeakerCommands::Verify(args) => {
            let (verifier, _model_dir) = load_speaker_verifier(
                args.model_dir,
                args.voice_dir.unwrap_or_else(default_voice_dir),
                args.num_threads,
                args.provider,
                args.threshold,
            )
            .await?;

            let samples = read_wav(&args.audio).map_err(|e| audio_error(&args.audio, e))?;

            let result = verifier
                .verify(&args.name, &samples)
                .map_err(speaker_error)?;

            println!(
                "score={:.4} accepted={} speaker={}",
                result.score,
                result.accepted,
                result.speaker.as_deref().unwrap_or("?")
            );

            if !result.accepted {
                return Err(AppError::BadRequest(BadRequestKind::Other(
                    "verification rejected".into(),
                )));
            }
        }

        SpeakerCommands::Delete(args) => {
            let (verifier, _model_dir) = load_speaker_verifier(
                args.model_dir,
                args.voice_dir.unwrap_or_else(default_voice_dir),
                1,
                ExecutionProvider::Cpu,
                DEFAULT_VERIFY_THRESHOLD,
            )
            .await?;

            verifier.remove(&args.name).map_err(speaker_error)?;
            println!("Deleted speaker: {}", args.name);
        }

        SpeakerCommands::List(args) => {
            let (verifier, _model_dir) = load_speaker_verifier(
                args.model_dir,
                args.voice_dir.unwrap_or_else(default_voice_dir),
                1,
                ExecutionProvider::Cpu,
                DEFAULT_VERIFY_THRESHOLD,
            )
            .await?;

            let speakers = verifier.list();
            if speakers.is_empty() {
                println!("No enrolled speakers.");
            } else {
                for name in speakers {
                    println!("{name}");
                }
            }
        }
    }

    Ok(())
}

fn default_voice_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(dir).join("takusu").join("voiceprint");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("takusu")
            .join("voiceprint");
    }
    PathBuf::from("takusu").join("voiceprint")
}

async fn load_speaker_verifier(
    model_dir: Option<PathBuf>,
    voice_dir: PathBuf,
    num_threads: i32,
    provider: ExecutionProvider,
    verify_threshold: f32,
) -> Result<(SpeakerVerifier, PathBuf), AppError> {
    let cache = tokio::task::spawn_blocking(|| {
        ModelCache::default_dir().map_err(|e| format!("model cache error: {e}"))
    })
    .await
    .map_err(|e| AppError::Internal(format!("model cache task failed: {e}")))?
    .map_err(AppError::Internal)?;

    let model_dir = match model_dir {
        Some(path) => path,
        None => cache
            .ensure(DEFAULT_SPEAKER_MODEL_ID)
            .map_err(|e| AppError::Internal(format!("model download error: {e}")))?,
    };
    let model_path = model_dir.join("model.onnx");

    let config = SpeakerConfig {
        model_id: DEFAULT_SPEAKER_MODEL_ID.to_string(),
        num_threads,
        provider,
        verify_threshold,
        voice_dir: None,
    };

    let verifier = tokio::task::spawn_blocking(move || {
        SpeakerVerifier::new(config, &model_path, Some(voice_dir))
    })
    .await
    .map_err(|e| AppError::Internal(format!("speaker verifier task failed: {e}")))?
    .map_err(speaker_error)?;

    Ok((verifier, model_dir))
}

fn audio_error(path: &std::path::Path, e: AudioError) -> AppError {
    AppError::BadRequest(BadRequestKind::Other(format!(
        "failed to read WAV {}: {e}",
        path.display()
    )))
}

fn speaker_error(e: SpeakerError) -> AppError {
    match e {
        SpeakerError::ModelNotFound(_)
        | SpeakerError::InputTooShort
        | SpeakerError::InvalidName(_)
        | SpeakerError::EnrollFailed => AppError::BadRequest(BadRequestKind::Other(e.to_string())),
        SpeakerError::NoSpeakers | SpeakerError::SpeakerNotFound(_) => {
            AppError::NotFound(e.to_string())
        }
        _ => AppError::Internal(e.to_string()),
    }
}
