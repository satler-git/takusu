//! Keyword spotting wake word detector.
//!
//! Provides a [`WakeWordDetector`] trait and two implementations:
//!
//! * [`SherpaKws`] — a tiny sherpa-onnx transducer keyword spotter for
//!   predefined Chinese/English phrases. This is the primary gate the design
//!   expects, but it requires a tokenized keyword string (see
//!   <https://k2-fsa.github.io/sherpa/onnx/kws/index.html>).
//! * [`AsrTextMatch`] — a fallback that runs a streaming ASR and triggers when
//!   the configured phrase appears in the partial transcript. Heavier, but
//!   works for Japanese and other languages without a pretrained KWS model.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use unicode_normalization::UnicodeNormalization;

use crate::stt::{AsrStream, ExecutionProvider, StreamingSpeechToText};
use crate::wav::{SHERPA_SAMPLE_RATE, normalize};

#[cfg(feature = "sherpa")]
use std::path::{Path, PathBuf};

#[cfg(feature = "sherpa")]
use crate::models::ModelCache;
#[cfg(feature = "sherpa")]
use sherpa_onnx::{KeywordSpotter, KeywordSpotterConfig, OnlineModelConfig};

/// Default KWS model, a Chinese/English zipformer spotter.
pub const DEFAULT_KWS_MODEL_ID: &str = "sherpa-kws-zipformer-wenetspeech-3.3m";

/// Default sample rate used by the keyword spotter.
pub const KWS_SAMPLE_RATE: i32 = SHERPA_SAMPLE_RATE as i32;

/// Target RMS for the ASR feed.
const ASR_TARGET_RMS: f32 = 0.1;

/// Configuration for the sherpa-onnx keyword spotter.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct KwsConfig {
    /// Model cache ID or empty to use `model_dir`.
    pub model_id: String,
    /// Inlined keywords, one per line, tokenized for the model.
    /// Example for the WenetSpeech BPE model:
    /// `t a k u s u @たくす`.
    /// When empty, `keyword` is tokenized automatically.
    pub keywords_buf: String,
    /// Path to a keywords file. If both are set, `keywords_buf` wins.
    pub keywords_file: String,
    /// The original keyword phrase. When `keywords_buf` is empty this is
    /// romanized and tokenized against the model's tokens file.
    pub keyword: String,
    /// Trigger threshold for each keyword (0..1); passed to sherpa-onnx.
    pub threshold: f32,
    /// Boosting score applied to keyword tokens.
    pub score: f32,
    /// Max active paths during beam search.
    pub max_active_paths: i32,
    /// Number of trailing blanks required after a keyword.
    pub num_trailing_blanks: i32,
    /// ONNX runtime threads.
    pub num_threads: i32,
    /// Execution provider.
    pub provider: ExecutionProvider,
    /// Preferred chunk size in the model file name ("16" or "8").
    pub chunk_size: String,
    /// Explicit local model directory; overrides the cache.
    pub model_dir: String,
}

impl KwsConfig {
    /// Sensible defaults for the shared WenetSpeech KWS model.
    pub fn for_wenetspeech() -> Self {
        Self {
            model_id: DEFAULT_KWS_MODEL_ID.into(),
            threshold: 0.25,
            score: 1.0,
            max_active_paths: 4,
            num_trailing_blanks: 1,
            num_threads: 2,
            provider: ExecutionProvider::Cpu,
            chunk_size: "16".into(),
            ..Default::default()
        }
    }

    /// True if the configuration can be used to load a sherpa-onnx spotter.
    pub fn is_sherpa(&self) -> bool {
        !self.model_id.is_empty() && (!self.keywords_buf.is_empty() || !self.keyword.is_empty())
    }
}

/// Errors from keyword spotting.
#[derive(Debug, Error)]
pub enum KwsError {
    #[error("model not found: {0}")]
    ModelNotFound(String),
    #[error("model cache error: {0}")]
    Cache(#[from] crate::models::ModelError),
    #[error("KWS model file missing: {0}")]
    ModelFileMissing(String),
    #[error("failed to create keyword spotter")]
    CreateFailed,
    #[error("invalid keywords: {0}")]
    InvalidKeywords(String),
    #[error("ASR start failed: {0}")]
    AsrStart(String),
    #[error("ASR finish failed: {0}")]
    AsrFinish(String),
    #[error(transparent)]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>),
}

/// A streaming wake word detector.
#[async_trait]
pub trait WakeWordDetector: Send {
    /// Feed a chunk of f32 samples and return `true` if the wake word fired.
    async fn push(&mut self, samples: &[f32]) -> Result<bool, KwsError>;
    /// Reset the detector state for the next utterance.
    async fn reset(&mut self) -> Result<(), KwsError>;
    /// Last matched keyword, if any.
    fn last_keyword(&self) -> Option<&str> {
        None
    }
}

/// Wake word detector that runs streaming ASR and matches the configured
/// phrase in the partial transcript. Heavier than a KWS model, but language
/// agnostic and works without a pretrained keyword spotter.
pub struct AsrTextMatch {
    asr: Arc<dyn StreamingSpeechToText>,
    language: String,
    phrase: String,
    stream: Option<Box<dyn AsrStream>>,
    detected: bool,
    last_text: String,
}

impl AsrTextMatch {
    /// Create a new text-match wake detector for `phrase`.
    pub async fn new(
        asr: Arc<dyn StreamingSpeechToText>,
        phrase: impl Into<String>,
        language: impl Into<String>,
    ) -> Result<Self, KwsError> {
        let phrase = phrase.into();
        let language = language.into();
        let mut stream = asr
            .start_stream(&language)
            .await
            .map_err(|e| KwsError::AsrStart(e.to_string()))?;
        // Warm the stream with an empty chunk so the model can report partial
        // text as soon as it sees speech.
        stream.accept_waveform(&[]);
        Ok(Self {
            asr,
            language,
            phrase,
            stream: Some(stream),
            detected: false,
            last_text: String::new(),
        })
    }

    /// Return the current partial transcript.
    pub fn last_text(&self) -> &str {
        &self.last_text
    }

    /// Normalize text for matching: NFC-Unicode-normalize, keep only
    /// letters/digits and whitespace, lowercase, and collapse runs of
    /// whitespace to a single space.
    pub(crate) fn normalize(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut pending_space = false;
        for c in text.nfc() {
            if c.is_alphanumeric() {
                if pending_space {
                    out.push(' ');
                    pending_space = false;
                }
                for lc in c.to_lowercase() {
                    out.push(lc);
                }
            } else if c.is_whitespace() && !out.is_empty() {
                pending_space = true;
            }
        }
        out
    }

    fn char_word_block(c: char) -> Option<u32> {
        if c.is_whitespace() || !c.is_alphanumeric() {
            return None;
        }
        if c.is_ascii_alphabetic() {
            return Some(0);
        }
        if c.is_ascii_digit() {
            return Some(6);
        }
        match c {
            '\u{3040}'..='\u{309F}' => Some(1), // Hiragana
            '\u{30A0}'..='\u{30FF}' | '\u{FF65}'..='\u{FF9F}' => Some(2), // Katakana
            '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}' => Some(3), // CJK Unified / compatibility
            '\u{AC00}'..='\u{D7AF}' => Some(4), // Hangul Syllables
            _ => Some(5),
        }
    }

    fn is_word_boundary(prev: Option<char>, next: Option<char>) -> bool {
        match (
            prev.and_then(Self::char_word_block),
            next.and_then(Self::char_word_block),
        ) {
            (None, _) | (_, None) => true,
            (Some(a), Some(b)) => a != b,
        }
    }

    fn matches(&self, text: &str) -> bool {
        let text = Self::normalize(text);
        let phrase = Self::normalize(&self.phrase);
        if phrase.is_empty() {
            return false;
        }
        text.match_indices(&phrase).any(|(i, _)| {
            let prev = if i == 0 {
                None
            } else {
                text[..i].chars().next_back()
            };
            let end = i + phrase.len();
            let next = if end == text.len() {
                None
            } else {
                text[end..].chars().next()
            };
            Self::is_word_boundary(prev, next)
        })
    }
}

#[async_trait]
impl WakeWordDetector for AsrTextMatch {
    async fn push(&mut self, samples: &[f32]) -> Result<bool, KwsError> {
        if self.detected {
            return Ok(true);
        }

        let stream = match self.stream.as_mut() {
            Some(s) => s,
            None => {
                let mut s = self
                    .asr
                    .start_stream(&self.language)
                    .await
                    .map_err(|e| KwsError::AsrStart(e.to_string()))?;
                s.accept_waveform(&[]);
                self.stream.insert(s)
            }
        };

        stream.accept_waveform(&normalize(samples, ASR_TARGET_RMS));
        let text = stream.text();
        if !text.is_empty() && text != self.last_text {
            self.last_text = text.clone();
            if self.matches(&text) {
                self.detected = true;
            }
        }
        Ok(self.detected)
    }

    fn last_keyword(&self) -> Option<&str> {
        Some(&self.phrase)
    }

    async fn reset(&mut self) -> Result<(), KwsError> {
        self.detected = false;
        self.last_text.clear();
        if let Some(mut old) = self.stream.take() {
            let _ = old
                .finish()
                .await
                .map_err(|e| KwsError::AsrFinish(e.to_string()))?;
        }
        let mut s = self
            .asr
            .start_stream(&self.language)
            .await
            .map_err(|e| KwsError::AsrStart(e.to_string()))?;
        s.accept_waveform(&[]);
        self.stream = Some(s);
        Ok(())
    }
}

#[cfg(feature = "sherpa")]
struct SherpaKwsInner {
    spotter: Arc<KeywordSpotter>,
    stream: sherpa_onnx::OnlineStream,
    config: KwsConfig,
    detected: bool,
    last_keyword: Option<String>,
    sample_rate: i32,
}

/// Sherpa-onnx keyword spotter wake word detector.
#[cfg(feature = "sherpa")]
pub struct SherpaKws {
    inner: SherpaKwsInner,
}

#[cfg(feature = "sherpa")]
impl SherpaKws {
    /// Load a sherpa-onnx keyword spotter from the model cache or a local
    /// directory.
    pub fn new(config: &KwsConfig) -> Result<Self, KwsError> {
        let model_dir = if config.model_dir.is_empty() {
            let cache = ModelCache::default_dir().map_err(KwsError::Cache)?;
            cache.ensure(&config.model_id)?
        } else {
            PathBuf::from(&config.model_dir)
        };
        let (encoder, decoder, joiner, tokens) =
            find_kws_model_files(&model_dir, &config.chunk_size)
                .map_err(|e| KwsError::ModelFileMissing(e.to_string()))?;

        let mut ks_config = KeywordSpotterConfig::default();
        ks_config.feat_config.sample_rate = KWS_SAMPLE_RATE;
        ks_config.feat_config.feature_dim = 80;

        let mut model_config = OnlineModelConfig::default();
        model_config.transducer.encoder = Some(path_to_string(&encoder));
        model_config.transducer.decoder = Some(path_to_string(&decoder));
        model_config.transducer.joiner = Some(path_to_string(&joiner));
        model_config.tokens = Some(path_to_string(&tokens));
        model_config.num_threads = config.num_threads;
        model_config.provider = Some(config.provider.to_string());
        ks_config.model_config = model_config;

        ks_config.max_active_paths = config.max_active_paths;
        ks_config.num_trailing_blanks = config.num_trailing_blanks;
        ks_config.keywords_score = config.score;
        ks_config.keywords_threshold = config.threshold;

        let keywords_buf = if !config.keywords_buf.is_empty() {
            config.keywords_buf.clone()
        } else if !config.keywords_file.is_empty() {
            config.keywords_file.clone()
        } else if !config.keyword.is_empty() {
            tokenize_keyword_for_model(&config.keyword, &tokens)?
        } else {
            return Err(KwsError::InvalidKeywords(
                "keywords_buf, keywords_file, or keyword must be set".into(),
            ));
        };
        ks_config.keywords_buf = Some(keywords_buf);

        let spotter = KeywordSpotter::create(&ks_config).ok_or(KwsError::CreateFailed)?;
        let stream = spotter.create_stream();

        Ok(Self {
            inner: SherpaKwsInner {
                spotter: Arc::new(spotter),
                stream,
                config: config.clone(),
                detected: false,
                last_keyword: None,
                sample_rate: KWS_SAMPLE_RATE,
            },
        })
    }

    /// Path to the cached model directory, for inspection or tests.
    pub fn model_dir(&self) -> PathBuf {
        PathBuf::from(&self.inner.config.model_dir)
    }
}

/// Convert a user-facing keyword into a space-separated token sequence for the
/// sherpa-onnx KWS model, using the model's `tokens.txt` as the vocabulary.
///
/// Japanese kana is romanized so that "たくす" becomes "t a k u s u", which
/// the Chinese/English WenetSpeech model can detect by its pronunciation.
/// Latin keywords are tokenized directly; unsupported characters become `<unk>`.
#[cfg(feature = "sherpa")]
fn tokenize_keyword_for_model(keyword: &str, tokens_path: &Path) -> Result<String, KwsError> {
    let content = std::fs::read_to_string(tokens_path)
        .map_err(|e| KwsError::ModelFileMissing(format!("failed to read tokens.txt: {e}")))?;

    let mut token_set: Vec<String> = content
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let token = parts.next()?;
            // Do not match special meta tokens; they cannot be decoded from audio.
            if token.starts_with('<') && token.ends_with('>') {
                None
            } else {
                Some(token.to_string())
            }
        })
        .collect();
    // Longest-match first so multichar pinyin like "sh" wins over "s" + "h".
    token_set.sort_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));

    let has_kana = keyword
        .chars()
        .any(|c| ('\u{3040}'..='\u{309F}').contains(&c) || ('\u{30A0}'..='\u{30FF}').contains(&c));

    let romanized = if has_kana {
        use romkan::Romkan;
        keyword.to_romaji()
    } else {
        keyword.to_string()
    };

    // Keep only letters/digits from the romanized form; drop spaces and
    // apostrophes (e.g. "kon'nichiwa") before tokenizing.
    let romanized: String = romanized
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect();

    let mut buf = String::new();
    let mut i = 0;
    while i < romanized.len() {
        let rest = &romanized[i..];
        let mut matched = false;
        for token in &token_set {
            if rest.starts_with(token) {
                if !buf.is_empty() {
                    buf.push(' ');
                }
                buf.push_str(token);
                i += token.len();
                matched = true;
                break;
            }
        }
        if !matched {
            // Skip one codepoint and substitute the model's unknown token.
            if !buf.is_empty() {
                buf.push(' ');
            }
            buf.push_str("<unk>");
            let step = romanized[i..].chars().next().map_or(1, |c| c.len_utf8());
            i += step;
        }
    }

    if buf.is_empty() || buf.contains("<unk>") {
        return Err(KwsError::InvalidKeywords(format!(
            "could not tokenize keyword '{keyword}' for KWS model"
        )));
    }

    Ok(format!("{buf} @{keyword}"))
}

#[cfg(feature = "sherpa")]
#[async_trait]
impl WakeWordDetector for SherpaKws {
    async fn push(&mut self, samples: &[f32]) -> Result<bool, KwsError> {
        if self.inner.detected {
            return Ok(true);
        }
        self.inner
            .stream
            .accept_waveform(self.inner.sample_rate, samples);
        while self.inner.spotter.is_ready(&self.inner.stream) {
            self.inner.spotter.decode(&self.inner.stream);
        }
        if let Some(result) = self.inner.spotter.get_result(&self.inner.stream)
            && !result.keyword.is_empty()
        {
            self.inner.last_keyword = Some(result.keyword.clone());
            self.inner.detected = true;
        }
        Ok(self.inner.detected)
    }

    async fn reset(&mut self) -> Result<(), KwsError> {
        self.inner.spotter.reset(&self.inner.stream);
        self.inner.detected = false;
        self.inner.last_keyword = None;
        Ok(())
    }

    fn last_keyword(&self) -> Option<&str> {
        self.inner.last_keyword.as_deref()
    }
}

#[cfg(feature = "sherpa")]
fn path_to_string(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[cfg(feature = "sherpa")]
fn find_kws_model_files(
    dir: &Path,
    preferred_chunk: &str,
) -> Result<(PathBuf, PathBuf, PathBuf, PathBuf), String> {
    let mut encoders = Vec::new();
    let mut decoders = Vec::new();
    let mut joiners = Vec::new();
    let mut tokens: Option<PathBuf> = None;

    for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name == "tokens.txt" {
            tokens = Some(path);
        } else if let Some(rest) = name.strip_prefix("encoder-")
            && rest.ends_with(".onnx")
        {
            encoders.push(path);
        } else if let Some(rest) = name.strip_prefix("decoder-")
            && rest.ends_with(".onnx")
        {
            decoders.push(path);
        } else if let Some(rest) = name.strip_prefix("joiner-")
            && rest.ends_with(".onnx")
        {
            joiners.push(path);
        }
    }

    let tokens = tokens.ok_or("tokens.txt not found")?;

    fn pick_with_preference(paths: &mut [PathBuf], preferred_chunk: &str) -> PathBuf {
        paths.sort();
        if let Some(p) = paths.iter().find(|p| {
            p.to_string_lossy()
                .contains(&format!("chunk-{preferred_chunk}"))
        }) {
            return p.clone();
        }
        paths.first().cloned().unwrap_or_default()
    }

    let encoder = pick_with_preference(&mut encoders, preferred_chunk);
    if encoder.as_os_str().is_empty() {
        return Err("encoder-*.onnx not found".into());
    }

    let stem = encoder
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("invalid encoder file name")?;
    let rest = stem
        .strip_prefix("encoder-")
        .and_then(|s| s.strip_suffix(".onnx"))
        .ok_or("invalid encoder file name")?;

    decoders.sort();
    let decoder = decoders
        .iter()
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n == format!("decoder-{rest}.onnx"))
        })
        .cloned()
        .or_else(|| {
            let preferred = format!("chunk-{preferred_chunk}");
            decoders
                .iter()
                .find(|p| p.to_string_lossy().contains(&preferred))
                .cloned()
                .or_else(|| decoders.first().cloned())
        })
        .ok_or("decoder-*.onnx not found")?;

    joiners.sort();
    let joiner = joiners
        .iter()
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n == format!("joiner-{rest}.onnx"))
        })
        .cloned()
        .or_else(|| {
            let preferred = format!("chunk-{preferred_chunk}");
            joiners
                .iter()
                .find(|p| p.to_string_lossy().contains(&preferred))
                .cloned()
                .or_else(|| joiners.first().cloned())
        })
        .ok_or("joiner-*.onnx not found")?;

    Ok((encoder, decoder, joiner, tokens))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct ScriptedWakeWord {
        fire_at: usize,
        seen: usize,
    }

    #[async_trait]
    impl WakeWordDetector for ScriptedWakeWord {
        async fn push(&mut self, _samples: &[f32]) -> Result<bool, KwsError> {
            self.seen += 1;
            Ok(self.seen >= self.fire_at)
        }

        async fn reset(&mut self) -> Result<(), KwsError> {
            self.seen = 0;
            Ok(())
        }
    }

    #[test]
    fn asr_text_match_normalizes() {
        assert_eq!(AsrTextMatch::normalize(" たくす、 "), "たくす");
        assert_eq!(AsrTextMatch::normalize("Takusu, Start"), "takusu start");
    }

    #[tokio::test]
    async fn scripted_wake_word_resets() {
        let mut det = ScriptedWakeWord {
            fire_at: 3,
            seen: 0,
        };
        assert!(!det.push(&[]).await.unwrap());
        assert!(!det.push(&[]).await.unwrap());
        assert!(det.push(&[]).await.unwrap());
        det.reset().await.unwrap();
        assert!(!det.push(&[]).await.unwrap());
    }

    #[cfg(feature = "sherpa")]
    #[test]
    fn tokenize_keyword_romanizes_japanese() {
        use std::io::Write;

        let mut tmp = std::env::temp_dir();
        tmp.push("takusu-kws-tokens.txt");
        let mut file = std::fs::File::create(&tmp).unwrap();
        // Vocabulary subset matching the WenetSpeech pinyin/letter tokens.
        for (i, token) in ["a", "k", "s", "t", "u", "<unk>", "<sos/eos>"]
            .iter()
            .enumerate()
        {
            writeln!(file, "{token} {i}").unwrap();
        }

        assert_eq!(
            tokenize_keyword_for_model("たくす", &tmp).unwrap(),
            "t a k u s u @たくす"
        );

        std::fs::remove_file(&tmp).unwrap();
    }
}
