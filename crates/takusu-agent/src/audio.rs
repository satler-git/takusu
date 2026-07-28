//! Application-level audio adapter for the takusu agent.
//!
//! This module is responsible for the push-to-talk loop:
//! record → transcribe → agent turn → synthesize → play.
//! It is not exposed as an LLM tool.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use takusu_audio::play::{PcmFormat, PlayError, StreamedAudioFormat, play_stream};
use takusu_audio::{
    CartesiaSonic, CartesiaSonicConfig, RecordConfig, SpeechToText, TextToSpeech, TtsBackend,
    TtsOptions, TtsRequest, normalize_for_tts, record,
};
use thiserror::Error;

use crate::{AgentError, AgentSession};

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("recording failed: {0}")]
    Record(String),
    #[error("transcription failed: {0}")]
    Transcribe(String),
    #[error("tts failed: {0}")]
    Tts(String),
    #[error("playback failed: {0}")]
    Play(String),
    #[error("audio backend {0} is not supported")]
    UnsupportedBackend(String),
    #[error("audio operation timed out")]
    Timeout,
}

impl From<takusu_audio::tts::TtsError> for AudioError {
    fn from(e: takusu_audio::tts::TtsError) -> Self {
        AudioError::Tts(e.to_string())
    }
}

impl From<PlayError> for AudioError {
    fn from(e: PlayError) -> Self {
        AudioError::Play(e.to_string())
    }
}

pub use crate::audio_config::{AudioConfig, SttConfig, TtsConfig};

/// Application-level audio adapter. Owns the agent session and the audio clients.
pub struct AudioAdapter {
    session: AgentSession,
    last_audio: AudioConfig,
    stt: Arc<dyn SpeechToText>,
    tts: Arc<dyn TextToSpeech>,
    tts_voice_id: String,
    tts_speed: Option<f32>,
    tts_format: StreamedAudioFormat,
}

impl AudioAdapter {
    /// Create an audio adapter from an existing agent session.
    pub async fn new(session: AgentSession) -> Result<Self, AudioError> {
        let audio = {
            let config = session.config.read().unwrap();
            config.audio.clone()
        };
        let (stt, tts, voice_id, speed, tts_format) = Self::build_audio(&audio).await?;
        Ok(Self {
            session,
            last_audio: audio,
            stt,
            tts,
            tts_voice_id: voice_id,
            tts_speed: speed,
            tts_format,
        })
    }

    /// Run the push-to-talk loop until interrupted or an unrecoverable error occurs.
    pub async fn run(&mut self, no_tts: bool) -> Result<(), AgentError> {
        loop {
            self.reconfigure_if_needed().await?;

            let samples = record_with_timeout(Duration::from_secs(60)).await?;
            if samples.is_empty() {
                continue;
            }

            let text =
                transcribe_with_timeout(Arc::clone(&self.stt), &samples, Duration::from_secs(120))
                    .await?;
            if text.trim().is_empty() {
                continue;
            }

            eprintln!("> {text}");

            let (tts_tx, mut tts_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            let tts = Arc::clone(&self.tts);
            let voice_id = self.tts_voice_id.clone();
            let speed = self.tts_speed;
            let tts_format = self.tts_format;
            let tts_language = self.last_audio.tts.language.clone();
            let no_tts_this_turn = no_tts || self.last_audio.tts.mute;
            let tts_player = tokio::spawn(async move {
                if no_tts_this_turn {
                    return Result::<(), AudioError>::Ok(());
                }
                while let Some(block) = tts_rx.recv().await {
                    if block.trim().is_empty() {
                        continue;
                    }
                    synthesize_and_play_with_timeout(
                        tts.as_ref(),
                        &block,
                        &voice_id,
                        &tts_language,
                        speed,
                        tts_format,
                        Duration::from_secs(120),
                    )
                    .await?;
                }
                Ok(())
            });

            let result = match self
                .session
                .run_turn_stream(
                    &text,
                    |_event| {},
                    |block| {
                        if !no_tts_this_turn {
                            let _ = tts_tx.send(block);
                        }
                    },
                )
                .await
            {
                Ok(result) => result,
                Err(e) => {
                    drop(tts_tx);
                    tts_player.abort();
                    return Err(e);
                }
            };

            // Drop the sender so the player task exits after processing all blocks.
            drop(tts_tx);
            tts_player
                .await
                .map_err(|e| AudioError::Play(format!("tts player task panicked: {e}")))??;

            println!("{}", result.text);
            if !result.changes.is_empty() {
                match serde_json::to_string_pretty(&result.changes) {
                    Ok(changes) => eprintln!("{changes}"),
                    Err(e) => eprintln!("changes: {e}"),
                }
            }
            if result.schedule_dirty {
                eprintln!("schedule dirty: true");
            }
        }
    }

    async fn reconfigure_if_needed(&mut self) -> Result<(), AudioError> {
        let current = {
            let config = self.session.config.read().unwrap();
            config.audio.clone()
        };
        if current == self.last_audio {
            return Ok(());
        }
        let (stt, tts, voice_id, speed, tts_format) = Self::build_audio(&current).await?;
        self.stt = stt;
        self.tts = tts;
        self.tts_voice_id = voice_id;
        self.tts_speed = speed;
        self.tts_format = tts_format;
        self.last_audio = current;
        Ok(())
    }

    async fn build_audio(
        audio: &AudioConfig,
    ) -> Result<
        (
            Arc<dyn SpeechToText>,
            Arc<dyn TextToSpeech>,
            String,
            Option<f32>,
            StreamedAudioFormat,
        ),
        AudioError,
    > {
        let stt_config = audio.stt.clone();
        let stt = tokio::task::spawn_blocking(move || build_stt(&stt_config))
            .await
            .map_err(|e| AudioError::Transcribe(format!("stt build task failed: {e}")))??;
        let supported = tokio::task::spawn_blocking(takusu_audio::play::default_output_config)
            .await
            .map_err(|e| AudioError::Play(format!("output config task failed: {e}")))??;
        let output_rate = supported.sample_rate();
        // The Cartesia /tts/bytes endpoint returns mono PCM when no channel
        // count is requested, so the stream format is always 1 channel.
        let tts_api_sample_rate = if audio.tts.sample_rate == 0 {
            output_rate
        } else {
            audio.tts.sample_rate
        };
        let (tts, voice_id, speed) = build_tts(&audio.tts, tts_api_sample_rate)?;
        let tts_format = StreamedAudioFormat {
            sample_rate: tts_api_sample_rate,
            channels: 1,
            pcm_format: PcmFormat::I16,
        };
        Ok((stt, tts, voice_id, speed, tts_format))
    }
}

fn build_stt(config: &SttConfig) -> Result<Arc<dyn SpeechToText>, AudioError> {
    let runtime_config = takusu_audio::SttRuntimeConfig {
        backend: config.backend,
        model: config.model,
        model_dir: if config.model_dir.is_empty() {
            None
        } else {
            Some(PathBuf::from(&config.model_dir))
        },
        language: config.language.clone(),
        use_itn: config.use_itn,
        num_threads: config.num_threads,
        provider: config.provider,
        sample_rate: config.sample_rate,
    };
    runtime_config
        .build()
        .map_err(|e| AudioError::Transcribe(e.to_string()))
}

type TtsBuildResult = Result<(Arc<dyn TextToSpeech>, String, Option<f32>), AudioError>;

fn build_tts(config: &TtsConfig, api_sample_rate: u32) -> TtsBuildResult {
    match config.backend {
        TtsBackend::Cartesia => {
            let api_key = if config.api_key.is_empty() {
                std::env::var(&config.api_key_env).unwrap_or_default()
            } else {
                config.api_key.clone()
            };
            if api_key.is_empty() && !config.mute {
                return Err(AudioError::Tts("missing Cartesia API key".to_string()));
            }
            let mut tts_config = CartesiaSonicConfig::new(api_key);
            tts_config.voice_id = config.voice_id.clone();
            tts_config.language = Some(config.language.clone());
            tts_config.output_format.sample_rate = api_sample_rate;
            tts_config.mute = config.mute;
            let voice_id = config.voice_id.clone();
            let speed = config.speed;
            Ok((Arc::new(CartesiaSonic::new(tts_config)), voice_id, speed))
        }
        // Android TTS is handled by the native mobile module, not by the
        // generic tokio-based AudioAdapter used on desktop.
        TtsBackend::Android => Err(AudioError::UnsupportedBackend("android".to_string())),
    }
}

async fn record_with_timeout(timeout: Duration) -> Result<Vec<f32>, AudioError> {
    let samples = tokio::task::spawn_blocking(move || {
        let config = RecordConfig {
            max_duration: timeout,
            ..Default::default()
        };
        record(&config)
    })
    .await
    .map_err(|e| AudioError::Record(format!("record task failed: {e}")))?
    .map_err(|e| AudioError::Record(e.to_string()))?;

    Ok(samples)
}

async fn transcribe_with_timeout(
    stt: Arc<dyn SpeechToText>,
    samples: &[f32],
    timeout: Duration,
) -> Result<String, AudioError> {
    let samples = samples.to_vec();
    tokio::time::timeout(
        timeout,
        tokio::task::spawn_blocking(move || {
            let handle = tokio::runtime::Handle::try_current()
                .map_err(|e| AudioError::Transcribe(e.to_string()))?;
            handle
                .block_on(stt.transcribe(&samples))
                .map_err(|e| AudioError::Transcribe(e.to_string()))
        }),
    )
    .await
    .map_err(|_| AudioError::Timeout)?
    .map_err(|e| AudioError::Transcribe(format!("transcribe task failed: {e}")))?
}

async fn synthesize_and_play_with_timeout(
    tts: &dyn TextToSpeech,
    text: &str,
    voice_id: &str,
    language: &str,
    speed: Option<f32>,
    format: StreamedAudioFormat,
    timeout: Duration,
) -> Result<(), AudioError> {
    // Lindera dictionary initialization can block on first use, so run the
    // normalization off the async runtime worker threads.
    let text = text.to_string();
    let language = language.to_string();
    let text =
        tokio::task::spawn_blocking(move || normalize_for_tts(&text, &language).into_owned())
            .await
            .map_err(|e| AudioError::Play(format!("tts normalization panicked: {e}")))?;
    let request = TtsRequest {
        text,
        voice: Some(voice_id.to_string()),
        reference_audio_path: None,
        options: TtsOptions {
            response_format: Some("pcm_s16le".to_string()),
            speed,
        },
    };

    let stream = tokio::time::timeout(timeout, tts.synthesize_stream(&request))
        .await
        .map_err(|_| AudioError::Timeout)?
        .map_err(|e| AudioError::Tts(e.to_string()))?;

    tokio::time::timeout(timeout, play_stream(stream, format))
        .await
        .map_err(|_| AudioError::Timeout)?
        .map_err(|e| AudioError::Play(e.to_string()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stt_config_defaults_to_sherpa() {
        use takusu_audio::{ExecutionProvider, SherpaOnnxModel, SttBackend};

        let config = SttConfig::default();
        assert_eq!(config.backend, SttBackend::Sherpa);
        assert_eq!(config.language, "ja");
        assert_eq!(config.model, SherpaOnnxModel::SenseVoice);
        assert!(config.use_itn);
        assert_eq!(config.num_threads, 2);
        assert_eq!(config.provider, ExecutionProvider::Cpu);
        assert_eq!(config.sample_rate, 16000);
    }

    #[test]
    fn tts_config_defaults_to_cartesia() {
        let config = TtsConfig::default();
        assert_eq!(config.backend, TtsBackend::Cartesia);
        assert_eq!(config.api_key_env, "CARTESIA_API_KEY");
        assert_eq!(config.sample_rate, 44100);
        assert!(!config.mute);
    }

    #[test]
    fn stt_config_rejects_unknown_backend_at_parse_time() {
        // With enum-typed backend, unknown values are rejected by serde
        // at config load time rather than at build time.
        let toml = r#"
[stt]
backend = "unknown"
"#;
        let result: Result<AudioConfig, _> = toml::from_str(toml);
        assert!(result.is_err());
    }

    #[test]
    fn stt_config_rejects_unknown_model_at_parse_time() {
        let toml = r#"
[stt]
backend = "sherpa"
model = "unknown"
"#;
        let result: Result<AudioConfig, _> = toml::from_str(toml);
        assert!(result.is_err());
    }

    #[test]
    fn stt_config_rejects_unknown_provider_at_parse_time() {
        let toml = r#"
[stt]
backend = "sherpa"
provider = "unknown"
"#;
        let result: Result<AudioConfig, _> = toml::from_str(toml);
        assert!(result.is_err());
    }

    #[test]
    fn tts_config_rejects_unknown_backend_at_parse_time() {
        let toml = r#"
[tts]
backend = "unknown"
"#;
        let result: Result<AudioConfig, _> = toml::from_str(toml);
        assert!(result.is_err());
    }

    #[test]
    fn build_tts_rejects_android_backend() {
        let config = TtsConfig {
            backend: TtsBackend::Android,
            ..TtsConfig::default()
        };
        let result = build_tts(&config, 44100);
        match result {
            Err(e) => assert!(e.to_string().contains("android")),
            Ok(_) => panic!("expected android backend to be rejected"),
        }
    }

    #[test]
    fn build_tts_allows_missing_api_key_when_muted() {
        let config = TtsConfig {
            api_key: String::new(),
            api_key_env: "NONEXISTENT_API_KEY".to_string(),
            mute: true,
            ..TtsConfig::default()
        };
        assert!(build_tts(&config, 44100).is_ok());
    }

    #[test]
    fn build_tts_rejects_missing_api_key_when_not_muted() {
        let config = TtsConfig {
            api_key: String::new(),
            api_key_env: "NONEXISTENT_API_KEY".to_string(),
            ..TtsConfig::default()
        };
        assert!(build_tts(&config, 44100).is_err());
    }
}
