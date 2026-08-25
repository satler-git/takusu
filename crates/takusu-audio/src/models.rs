//! First-run model download and cache.
//!
//! `ModelCache` downloads ONNX model bundles from known URLs and extracts them
//! to a local directory. It is designed to work on both desktop Linux
//! (`~/.cache/takusu/models` by default) and Android (when given the app's
//! cache directory from the Kotlin layer).

use std::fs;
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bzip2::read::BzDecoder;
use flate2::read::GzDecoder;
use tar::Archive;
use thiserror::Error;

const HUSH_URL: &str = "https://huggingface.co/weya-ai/hush/resolve/main/onnx/advanced_dfnet16k_model_best_onnx.tar.gz";
const SHERPA_SENSE_VOICE_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17.tar.bz2";
const SHERPA_PARAKEET_CTC_JA_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt_ctc-0.6b-ja-35000-int8.tar.bz2";
const SHERPA_NEMOTRON_JA_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11.tar.bz2";
const SHERPA_SPEAKER_CAMPPLUS_ZH_EN_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx";
const SHERPA_KWS_WENETSPEECH_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/kws-models/sherpa-onnx-kws-zipformer-wenetspeech-3.3M-2024-01-01.tar.bz2";
const SILERO_VAD_URL: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx";

/// Archive compression used by a model bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    /// `.tar.gz`
    TarGz,
    /// `.tar.bz2`
    TarBz2,
}

/// Progress phase for model preparation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadStage {
    Downloading,
    Extracting,
    Verifying,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadProgress {
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub stage: DownloadStage,
}

pub type ProgressCallback = Arc<dyn Fn(DownloadProgress) + Send + Sync>;

/// Description of a downloadable model bundle.
#[derive(Debug, Clone, Copy)]
pub struct ModelSpec {
    pub id: &'static str,
    pub url: &'static str,
    pub format: ArchiveFormat,
    /// When set, the download is a single file (not an archive) written to
    /// `model_dir/<filename>`. Used for un-bundled models such as Silero VAD.
    pub single_file: Option<&'static str>,
    pub expected_files: &'static [&'static str],
    /// Optional expected file size in bytes. When set, `is_cached` validates
    /// the on-disk size so interrupted downloads that left a partial file are
    /// not accepted as complete.
    pub expected_size: Option<u64>,
}

impl ModelSpec {
    /// Whether this spec is a single-file download rather than an archive.
    pub fn is_single_file(&self) -> bool {
        self.single_file.is_some()
    }
}

const HUSH_SPEC: ModelSpec = ModelSpec {
    id: "hush",
    url: HUSH_URL,
    format: ArchiveFormat::TarGz,
    single_file: None,
    expected_files: &["enc.onnx", "erb_dec.onnx", "df_dec.onnx"],
    expected_size: None,
};

const SHERPA_SENSE_VOICE_SPEC: ModelSpec = ModelSpec {
    id: "sherpa-sense-voice-int8",
    url: SHERPA_SENSE_VOICE_URL,
    format: ArchiveFormat::TarBz2,
    single_file: None,
    expected_files: &["tokens.txt", "model.int8.onnx"],
    expected_size: None,
};

const SHERPA_PARAKEET_CTC_JA_SPEC: ModelSpec = ModelSpec {
    id: "sherpa-parakeet-ctc-ja-0.6b",
    url: SHERPA_PARAKEET_CTC_JA_URL,
    format: ArchiveFormat::TarBz2,
    single_file: None,
    expected_files: &["model.int8.onnx", "tokens.txt"],
    expected_size: None,
};

const SHERPA_NEMOTRON_JA_SPEC: ModelSpec = ModelSpec {
    id: "sherpa-nemotron-ja-0.6b",
    url: SHERPA_NEMOTRON_JA_URL,
    format: ArchiveFormat::TarBz2,
    single_file: None,
    expected_files: &[
        "encoder.int8.onnx",
        "decoder.int8.onnx",
        "joiner.int8.onnx",
        "tokens.txt",
    ],
    expected_size: None,
};

const SILERO_VAD_SPEC: ModelSpec = ModelSpec {
    id: "silero-vad",
    url: SILERO_VAD_URL,
    format: ArchiveFormat::TarGz, // unused for single-file downloads
    single_file: Some("silero_vad.onnx"),
    expected_files: &["silero_vad.onnx"],
    expected_size: Some(643_854),
};

const SHERPA_SPEAKER_CAMPPLUS_ZH_EN_SPEC: ModelSpec = ModelSpec {
    id: "sherpa-speaker-campplus-zh-en",
    url: SHERPA_SPEAKER_CAMPPLUS_ZH_EN_URL,
    format: ArchiveFormat::TarGz, // unused for single-file downloads
    single_file: Some("model.onnx"),
    expected_files: &["model.onnx"],
    expected_size: Some(28_281_164),
};

const SHERPA_KWS_WENETSPEECH_SPEC: ModelSpec = ModelSpec {
    id: "sherpa-kws-zipformer-wenetspeech-3.3m",
    url: SHERPA_KWS_WENETSPEECH_URL,
    format: ArchiveFormat::TarBz2,
    single_file: None,
    // The archive contains multiple encoder/decoder/joiner variants; the KWS
    // loader discovers the concrete files at runtime. Only tokens.txt is fixed.
    expected_files: &["tokens.txt"],
    expected_size: None,
};

const ALL_MODELS: [ModelSpec; 7] = [
    HUSH_SPEC,
    SHERPA_SENSE_VOICE_SPEC,
    SHERPA_PARAKEET_CTC_JA_SPEC,
    SHERPA_NEMOTRON_JA_SPEC,
    SILERO_VAD_SPEC,
    SHERPA_SPEAKER_CAMPPLUS_ZH_EN_SPEC,
    SHERPA_KWS_WENETSPEECH_SPEC,
];

/// Known downloadable models.
pub struct ModelRegistry;

impl ModelRegistry {
    /// Hush denoiser (DeepFilterNet3 ONNX, ~8 MB).
    pub const fn hush() -> ModelSpec {
        HUSH_SPEC
    }

    /// Sherpa-ONNX SenseVoice int8 ASR (~160 MB).
    pub const fn sherpa_sense_voice() -> ModelSpec {
        SHERPA_SENSE_VOICE_SPEC
    }

    /// Sherpa-ONNX NeMo Parakeet TDT CTC for Japanese (~...).
    pub const fn sherpa_parakeet_ja() -> ModelSpec {
        SHERPA_PARAKEET_CTC_JA_SPEC
    }

    /// Sherpa-ONNX Nemotron multilingual streaming transducer (~...).
    pub const fn sherpa_nemotron_ja() -> ModelSpec {
        SHERPA_NEMOTRON_JA_SPEC
    }

    /// sherpa-onnx Silero VAD (single `.onnx`, ~628 KB). Downloaded on first
    /// use by recording loops and by `download_model` on Android.
    pub const fn silero_vad() -> ModelSpec {
        SILERO_VAD_SPEC
    }

    /// sherpa-onnx 3D-Speaker CAM++ Chinese-English speaker embedding model
    /// (single `.onnx`, ~28 MB).
    pub const fn sherpa_speaker_campplus_zh_en() -> ModelSpec {
        SHERPA_SPEAKER_CAMPPLUS_ZH_EN_SPEC
    }

    /// sherpa-onnx Zipformer keyword spotting (WenetSpeech, Chinese-English
    /// transducer, ~31 MB).
    pub const fn sherpa_kws_wenetspeech() -> ModelSpec {
        SHERPA_KWS_WENETSPEECH_SPEC
    }

    /// All known models.
    pub fn all() -> &'static [ModelSpec] {
        &ALL_MODELS
    }

    /// Find a model by ID.
    pub fn find(id: &str) -> Option<ModelSpec> {
        ALL_MODELS.iter().find(|s| s.id == id).copied()
    }
}

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("unknown model id: {0}")]
    UnknownModel(String),
    #[error("model already present at {0} and `use_cache` is true")]
    AlreadyCached(PathBuf),
    #[error("cache directory could not be determined")]
    CacheDirNotFound,
    #[error("download failed: {0}")]
    Download(#[from] reqwest::Error),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("download size mismatch: expected {expected}, got {actual}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("archive extraction failed: {0}")]
    Extract(String),
    #[error("missing expected files after extraction: {0}")]
    MissingFiles(String),
}

/// Cache for downloaded model bundles.
#[derive(Debug, Clone)]
pub struct ModelCache {
    cache_dir: PathBuf,
}

impl ModelCache {
    /// Create a cache at an explicit directory.
    pub fn new(cache_dir: impl AsRef<Path>) -> Self {
        Self {
            cache_dir: cache_dir.as_ref().to_path_buf(),
        }
    }

    /// Create a cache at the default desktop location.
    ///
    /// On Android, the cache directory should be supplied explicitly via
    /// [`ModelCache::new`] (e.g. from `Context.getCacheDir()`).
    pub fn default_dir() -> Result<Self, ModelError> {
        let dir = default_cache_dir().ok_or(ModelError::CacheDirNotFound)?;
        let dir = dir.join("takusu").join("models");
        Ok(Self::new(dir))
    }

    /// Ensure the sherpa-onnx Silero VAD model is present, downloading the
    /// single `.onnx` file on first use, and return its path.
    ///
    /// This is now a registered model (`"silero-vad"`) usable through
    /// [`Self::ensure`], so the same path is shared by desktop recording loops
    /// and Android's `download_model`.
    pub fn ensure_silero_vad(&self) -> Result<PathBuf, ModelError> {
        let model_dir = self.ensure("silero-vad")?;
        Ok(model_dir.join("silero_vad.onnx"))
    }

    /// Check whether a model is already present and complete in the cache.
    pub fn is_cached(&self, id: &str) -> Result<bool, ModelError> {
        let spec = ModelRegistry::find(id).ok_or(ModelError::UnknownModel(id.to_string()))?;
        let model_dir = self.cache_dir.join(spec.id);
        Ok(model_dir.is_dir()
            && has_expected_files(&model_dir, spec.expected_files, spec.expected_size))
    }

    /// Ensure a model is available, downloading it if necessary.
    ///
    /// Returns the path to the extracted model directory.
    pub fn ensure(&self, id: &str) -> Result<PathBuf, ModelError> {
        self.ensure_with_progress(id, None)
    }

    /// Ensure a model is available while reporting throttled preparation progress.
    pub fn ensure_with_progress(
        &self,
        id: &str,
        progress: Option<ProgressCallback>,
    ) -> Result<PathBuf, ModelError> {
        let spec = ModelRegistry::find(id).ok_or(ModelError::UnknownModel(id.to_string()))?;
        let model_dir = self.cache_dir.join(spec.id);
        if model_dir.is_dir()
            && has_expected_files(&model_dir, spec.expected_files, spec.expected_size)
        {
            return Ok(model_dir);
        }
        self.download_and_extract(&spec, progress)?;
        if !has_expected_files(&model_dir, spec.expected_files, spec.expected_size) {
            return Err(ModelError::MissingFiles(model_dir.display().to_string()));
        }
        Ok(model_dir)
    }

    /// Force a re-download of a model.
    pub fn download(&self, id: &str) -> Result<PathBuf, ModelError> {
        self.download_with_progress(id, None)
    }

    pub fn download_with_progress(
        &self,
        id: &str,
        progress: Option<ProgressCallback>,
    ) -> Result<PathBuf, ModelError> {
        let spec = ModelRegistry::find(id).ok_or(ModelError::UnknownModel(id.to_string()))?;
        let model_dir = self.cache_dir.join(spec.id);
        if model_dir.is_dir() {
            fs::remove_dir_all(&model_dir)?;
        }
        self.download_and_extract(&spec, progress)?;
        Ok(model_dir)
    }

    fn download_and_extract(
        &self,
        spec: &ModelSpec,
        progress: Option<ProgressCallback>,
    ) -> Result<(), ModelError> {
        let model_dir = self.cache_dir.join(spec.id);
        fs::create_dir_all(&model_dir)?;

        let certs: Vec<reqwest::Certificate> = webpki_root_certs::TLS_SERVER_ROOT_CERTS
            .iter()
            .filter_map(|c| reqwest::Certificate::from_der(c.as_ref()).ok())
            .collect();
        let client = reqwest::blocking::Client::builder()
            .use_rustls_tls()
            .tls_certs_only(certs)
            .timeout(std::time::Duration::from_secs(600))
            .build()?;
        let request = client.get(spec.url);
        let mut response = request.send()?;
        if !response.status().is_success() {
            return Err(ModelError::Download(
                response.error_for_status().unwrap_err(),
            ));
        }

        let total_bytes = response.content_length();
        let mut downloaded_bytes = 0;
        let mut buffer = [0u8; 64 * 1024];

        if let Some(filename) = spec.single_file {
            // Single-file model: write to a temporary file, validate size, then
            // atomically rename it into place. This prevents a partial download
            // from being accepted as a complete model on the next run.
            let final_path = model_dir.join(filename);
            let tmp_path = model_dir.join(format!("{}.{}", filename, "tmp"));
            if tmp_path.exists() {
                fs::remove_file(&tmp_path)?;
            }
            let mut file = fs::File::create(&tmp_path)?;
            loop {
                let read = response.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                file.write_all(&buffer[..read])?;
                downloaded_bytes += read as u64;
                if let Some(callback) = &progress {
                    callback(DownloadProgress {
                        downloaded_bytes,
                        total_bytes,
                        stage: DownloadStage::Downloading,
                    });
                }
            }
            file.flush()?;
            drop(file);

            if let Some(expected) = total_bytes
                && downloaded_bytes != expected
            {
                fs::remove_file(&tmp_path)?;
                return Err(ModelError::SizeMismatch {
                    expected,
                    actual: downloaded_bytes,
                });
            }

            fs::rename(&tmp_path, &final_path)?;
            if let Some(callback) = &progress {
                callback(DownloadProgress {
                    downloaded_bytes,
                    total_bytes,
                    stage: DownloadStage::Verifying,
                });
            }
            return Ok(());
        }

        let archive_name = archive_name_from_url(spec.url);
        let archive_path = self.cache_dir.join(format!("{}.{}", spec.id, archive_name));
        let tmp_archive_path = self
            .cache_dir
            .join(format!("{}.{}.tmp", spec.id, archive_name));
        if tmp_archive_path.exists() {
            fs::remove_file(&tmp_archive_path)?;
        }
        let mut file = fs::File::create(&tmp_archive_path)?;
        loop {
            let read = response.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            file.write_all(&buffer[..read])?;
            downloaded_bytes += read as u64;
            if let Some(callback) = &progress {
                callback(DownloadProgress {
                    downloaded_bytes,
                    total_bytes,
                    stage: DownloadStage::Downloading,
                });
            }
        }
        file.flush()?;
        drop(file);

        if let Some(expected) = total_bytes
            && downloaded_bytes != expected
        {
            fs::remove_file(&tmp_archive_path)?;
            return Err(ModelError::SizeMismatch {
                expected,
                actual: downloaded_bytes,
            });
        }

        fs::rename(&tmp_archive_path, &archive_path)?;

        if let Some(callback) = &progress {
            callback(DownloadProgress {
                downloaded_bytes,
                total_bytes,
                stage: DownloadStage::Extracting,
            });
        }
        extract_archive(&archive_path, &model_dir, spec.format)?;

        if let Some(callback) = &progress {
            callback(DownloadProgress {
                downloaded_bytes,
                total_bytes,
                stage: DownloadStage::Verifying,
            });
        }

        // Some archives have a single top-level directory. If the model dir
        // contains only one directory and no expected files, move the contents
        // up one level.
        if let Some(top) = single_child_directory(&model_dir)
            && !has_expected_files_direct(&model_dir, spec.expected_files, spec.expected_size)
        {
            let temp = self.cache_dir.join(format!("{}.tmp", spec.id));
            if temp.exists() {
                fs::remove_dir_all(&temp)?;
            }
            fs::rename(&model_dir, &temp)?;
            fs::create_dir(&model_dir)?;
            for entry in fs::read_dir(temp.join(top.file_name().unwrap_or_default()))? {
                let entry = entry?;
                let from = entry.path();
                let to = model_dir.join(entry.file_name());
                fs::rename(from, to)?;
            }
            fs::remove_dir_all(&temp)?;
        }

        fs::remove_file(&archive_path)?;
        Ok(())
    }
}

fn default_cache_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("TAKUSU_MODEL_CACHE_DIR") {
        return Some(PathBuf::from(dir));
    }
    if let Ok(dir) = std::env::var("XDG_CACHE_HOME") {
        return Some(PathBuf::from(dir));
    }
    if let Ok(home) = std::env::var("HOME") {
        return Some(PathBuf::from(home).join(".cache"));
    }
    if let Ok(user) = std::env::var("USERPROFILE") {
        return Some(PathBuf::from(user).join("AppData").join("Local"));
    }
    None
}

fn archive_name_from_url(url: &str) -> String {
    url.rsplit('/')
        .next()
        .and_then(|s| s.split('?').next())
        .unwrap_or("archive")
        .to_string()
}

fn extract_archive(
    archive_path: &Path,
    dest_dir: &Path,
    format: ArchiveFormat,
) -> Result<(), ModelError> {
    let file = fs::File::open(archive_path)?;
    let reader = BufReader::new(file);
    fs::create_dir_all(dest_dir)?;

    match format {
        ArchiveFormat::TarGz => {
            let mut archive = Archive::new(GzDecoder::new(reader));
            unpack_entries(&mut archive, dest_dir)?;
        }
        ArchiveFormat::TarBz2 => {
            let mut archive = Archive::new(BzDecoder::new(reader));
            unpack_entries(&mut archive, dest_dir)?;
        }
    }
    Ok(())
}

fn unpack_entries<R: std::io::Read>(
    archive: &mut Archive<R>,
    dest_dir: &Path,
) -> Result<(), ModelError> {
    for entry in archive.entries()? {
        let mut entry = entry.map_err(|e| ModelError::Extract(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| ModelError::Extract(e.to_string()))?;
        if !is_safe_archive_path(&path) {
            continue;
        }
        entry
            .unpack_in(dest_dir)
            .map_err(|e| ModelError::Extract(e.to_string()))?;
    }
    Ok(())
}

fn is_safe_archive_path(path: &Path) -> bool {
    if path.is_absolute() {
        return false;
    }
    for comp in path.components() {
        if !matches!(comp, std::path::Component::Normal(_)) {
            return false;
        }
    }
    true
}

fn has_expected_files(dir: &Path, expected: &[&str], expected_size: Option<u64>) -> bool {
    expected
        .iter()
        .all(|name| find_file_recursive(dir, name, expected_size).is_some())
}

fn has_expected_files_direct(dir: &Path, expected: &[&str], expected_size: Option<u64>) -> bool {
    expected
        .iter()
        .all(|name| is_valid_model_file(&dir.join(name), expected_size))
}

fn find_file_recursive(dir: &Path, name: &str, expected_size: Option<u64>) -> Option<PathBuf> {
    let path = dir.join(name);
    if is_valid_model_file(&path, expected_size) {
        return Some(path);
    }
    for entry in fs::read_dir(dir).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();
        if path.is_dir()
            && let Some(found) = find_file_recursive(&path, name, expected_size)
        {
            return Some(found);
        }
    }
    None
}

fn is_valid_model_file(path: &Path, expected_size: Option<u64>) -> bool {
    let meta = match path.metadata() {
        Ok(m) if m.is_file() => m,
        _ => return false,
    };
    let len = meta.len();
    if len == 0 {
        return false;
    }
    if let Some(expected) = expected_size {
        return len == expected;
    }
    true
}

fn single_child_directory(dir: &Path) -> Option<PathBuf> {
    let entries: Vec<_> = fs::read_dir(dir).ok()?.flatten().collect();
    if entries.len() != 1 {
        return None;
    }
    let entry = entries.into_iter().next()?;
    if entry.path().is_dir() {
        Some(entry.path())
    } else {
        None
    }
}
