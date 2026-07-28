//! ONNX-based Hush speech enhancement (https://huggingface.co/weya-ai/hush).
//!
//! The model bundle is split into three ONNX files:
//!   - `enc.onnx`   - encoder producing ERB/DF skips, embedding, and c0
//!   - `erb_dec.onnx` - ERB mask decoder
//!   - `df_dec.onnx`  - deep-filter coefficient decoder
//!
//! This module mirrors the reference PyTorch inference in `scripts/infer_single.py`
//! from the Hush repository, using `libDF`-compatible STFT/ERB/ISTFT processing.

use std::path::{Path, PathBuf};

use ndarray::prelude::*;
use num_complex::Complex32;
use ort::session::Session;
use ort::value::Tensor;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use crate::wav::SHERPA_SAMPLE_RATE;

#[derive(Debug, thiserror::Error)]
pub enum HushError {
    #[error("ONNX runtime error: {0}")]
    Ort(#[from] ort::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("model not found: {0}")]
    MissingModel(String),
    #[error("invalid config: {0}")]
    Config(String),
    #[error("invalid audio: {0}")]
    Audio(String),
    #[error("shape error: {0}")]
    Shape(String),
}

/// Hush model configuration.
#[derive(Debug, Clone)]
pub struct HushConfig {
    pub sr: usize,
    pub fft_size: usize,
    pub hop_size: usize,
    pub nb_erb: usize,
    pub nb_df: usize,
    pub min_nb_erb_freqs: usize,
    pub norm_tau: f32,
    pub df_order: usize,
    pub df_lookahead: usize,
    pub conv_lookahead: usize,
    pub conv_ch: usize,
    /// Target RMS for optional input normalization. `None` disables normalization.
    pub target_rms: Option<f32>,
    /// Whether to scale the enhanced output back to the original loudness.
    pub restore_loudness: bool,
}

impl Default for HushConfig {
    fn default() -> Self {
        Self {
            sr: SHERPA_SAMPLE_RATE as usize,
            fft_size: 320,
            hop_size: 160,
            nb_erb: 32,
            nb_df: 64,
            min_nb_erb_freqs: 2,
            norm_tau: 1.0,
            df_order: 5,
            df_lookahead: 0,
            conv_lookahead: 0,
            conv_ch: 16,
            target_rms: Some(0.1),
            restore_loudness: true,
        }
    }
}

impl HushConfig {
    fn norm_alpha(&self) -> f32 {
        let dt = self.hop_size as f32 / self.sr as f32;
        f32::exp(-dt / self.norm_tau)
    }
}

/// Hush speech-enhancement model.
pub struct Hush {
    enc: Session,
    erb_dec: Session,
    df_dec: Session,
    config: HushConfig,
    erb_widths: Vec<usize>,
    window: Vec<f32>,
    fft_forward: std::sync::Arc<dyn RealToComplex<f32>>,
    fft_inverse: std::sync::Arc<dyn ComplexToReal<f32>>,
    freq_size: usize,
}

impl Hush {
    /// Load a Hush ONNX bundle from a directory containing `enc.onnx`, `erb_dec.onnx`,
    /// and `df_dec.onnx`.
    pub fn from_model_dir(dir: impl AsRef<Path>) -> Result<Self, HushError> {
        let dir = dir.as_ref();
        let config = HushConfig::default();

        let enc_path = dir.join("enc.onnx");
        let erb_dec_path = dir.join("erb_dec.onnx");
        let df_dec_path = dir.join("df_dec.onnx");
        for p in [&enc_path, &erb_dec_path, &df_dec_path] {
            if !p.exists() {
                return Err(HushError::MissingModel(p.display().to_string()));
            }
        }

        let mut fft = RealFftPlanner::<f32>::new();
        let forward = fft.plan_fft_forward(config.fft_size);
        let inverse = fft.plan_fft_inverse(config.fft_size);

        let freq_size = config.fft_size / 2 + 1;
        let erb_widths = erb_fb(
            config.sr,
            config.fft_size,
            config.nb_erb,
            config.min_nb_erb_freqs,
        );

        let window = vorbis_window(config.fft_size);

        init_onnx_runtime()?;

        let enc = Session::builder()?.commit_from_file(&enc_path)?;
        let erb_dec = Session::builder()?.commit_from_file(&erb_dec_path)?;
        let df_dec = Session::builder()?.commit_from_file(&df_dec_path)?;

        Ok(Self {
            enc,
            erb_dec,
            df_dec,
            config,
            erb_widths,
            window,
            fft_forward: forward,
            fft_inverse: inverse,
            freq_size,
        })
    }

    /// Configure the target RMS for input normalization.
    ///
    /// Set to `None` to disable normalization. A typical value for speech is
    /// `0.1` (-20 dBFS).
    pub fn set_target_rms(&mut self, target_rms: Option<f32>) {
        self.config.target_rms = target_rms.and_then(|v| if v > 0.0 { Some(v) } else { None });
    }

    /// Configure whether the enhanced output is scaled back to the original
    /// loudness. Ignored when normalization is disabled.
    pub fn set_restore_loudness(&mut self, restore: bool) {
        self.config.restore_loudness = restore;
    }

    /// Enhance a 16 kHz mono f32 audio buffer.
    ///
    /// If `target_rms` is configured, the input is RMS-normalized before the
    /// denoiser, and the output is optionally restored to the original
    /// loudness.
    pub fn enhance(&mut self, samples: &[f32]) -> Result<Vec<f32>, HushError> {
        if samples.is_empty() {
            return Err(HushError::Audio("empty input".to_string()));
        }

        if let Some(target) = self.config.target_rms {
            let (input_rms, input_peak) = rms_peak(samples);
            if input_peak > 0.0 && input_rms > 0.0 {
                let scale = (target / input_rms).min(1.0 / input_peak);
                let normalized: Vec<f32> = samples.iter().map(|&s| s * scale).collect();
                let enhanced = self.denoise(&normalized)?;
                if self.config.restore_loudness {
                    let inv = 1.0 / scale;
                    return Ok(enhanced.into_iter().map(|s| s * inv).collect());
                }
                return Ok(enhanced);
            }
        }

        self.denoise(samples)
    }

    fn denoise(&mut self, samples: &[f32]) -> Result<Vec<f32>, HushError> {
        if samples.is_empty() {
            return Err(HushError::Audio("empty input".to_string()));
        }

        let orig_len = samples.len();
        let n_fft = self.config.fft_size;
        let hop = self.config.hop_size;

        // Pad by fft_size and round to a full hop to avoid losing tail samples.
        let padded_len = (orig_len + n_fft).div_ceil(hop) * hop;
        let mut padded = samples.to_vec();
        padded.resize(padded_len, 0.0);

        let n_frames = padded_len / hop;

        let mut spec = Array3::<Complex32>::zeros((1, n_frames, self.freq_size));
        let mut scratch = self.fft_forward.make_scratch_vec();
        let mut analysis_mem = vec![0.0f32; n_fft - hop];
        let split = n_fft - hop;
        // realfft is unnormalized; wnorm matches libDF's vorbis-windowed STFT.
        let norm = (2.0 * hop as f32) / (n_fft as f32 * n_fft as f32);
        for t in 0..n_frames {
            let start = t * hop;
            let frame = &padded[start..start + hop];
            let mut buf = self.fft_forward.make_input_vec();
            for (i, &s) in analysis_mem.iter().enumerate() {
                buf[i] = s * self.window[i];
            }
            for (i, &s) in frame.iter().enumerate() {
                buf[split + i] = s * self.window[split + i];
            }
            let mut out = self.fft_forward.make_output_vec();
            self.fft_forward
                .process_with_scratch(&mut buf, &mut out, &mut scratch)
                .map_err(|e| HushError::Audio(format!("fft error: {e}")))?;
            for (f, c) in out.iter().enumerate() {
                spec[[0, t, f]] = *c * norm;
            }
            analysis_mem.rotate_left(hop);
            for (i, &s) in frame.iter().enumerate() {
                analysis_mem[split - hop + i] = s;
            }
        }

        let (feat_erb, feat_spec) = self.compute_features(&spec);

        let spec_enh = self.inference(&spec, &feat_erb, &feat_spec)?;

        let mut output = self.istft(&spec_enh, padded_len, n_frames * hop)?;

        // Delay compensation.
        let delay = n_fft - hop;
        if output.len() >= delay + orig_len {
            output = output[delay..delay + orig_len].to_vec();
        } else {
            output = output.get(delay..).unwrap_or(&[]).to_vec();
        }
        Ok(output)
    }

    fn compute_features(&self, spec: &Array3<Complex32>) -> (Array4<f32>, Array4<f32>) {
        let n_frames = spec.len_of(Axis(1));
        let alpha = self.config.norm_alpha();

        let mut erb_power = Array3::<f32>::zeros((1, n_frames, self.config.nb_erb));
        for t in 0..n_frames {
            for e in 0..self.config.nb_erb {
                let mut sum = 0.0f32;
                let start = self.erb_widths[..e].iter().sum::<usize>();
                for f in start..start + self.erb_widths[e] {
                    let c = spec[[0, t, f]];
                    sum += c.re * c.re + c.im * c.im;
                }
                erb_power[[0, t, e]] = sum / self.erb_widths[e] as f32;
            }
        }

        // log10 and mean norm, divided by 40.
        let mut state_erb: Vec<f32> = (0..self.config.nb_erb)
            .map(|i| -60.0 + (-90.0 - -60.0) * i as f32 / (self.config.nb_erb - 1).max(1) as f32)
            .collect();
        let mut feat_erb = Array4::<f32>::zeros((1, 1, n_frames, self.config.nb_erb));
        for t in 0..n_frames {
            for e in 0..self.config.nb_erb {
                let x = 10.0 * (erb_power[[0, t, e]] + 1e-10).log10();
                state_erb[e] = x * (1.0 - alpha) + state_erb[e] * alpha;
                let norm = (x - state_erb[e]) / 40.0;
                feat_erb[[0, 0, t, e]] = norm;
            }
        }

        let mut state_spec: Vec<f32> = (0..self.config.nb_df)
            .map(|i| 0.001 + (0.0001 - 0.001) * i as f32 / (self.config.nb_df - 1).max(1) as f32)
            .collect();
        let mut feat_spec = Array4::<f32>::zeros((1, 2, n_frames, self.config.nb_df));
        for t in 0..n_frames {
            for f in 0..self.config.nb_df {
                let c = spec[[0, t, f]];
                let mag = (c.re * c.re + c.im * c.im).sqrt();
                state_spec[f] = mag * (1.0 - alpha) + state_spec[f] * alpha;
                let scale = 1.0 / (state_spec[f].sqrt() + 1e-14);
                feat_spec[[0, 0, t, f]] = c.re * scale;
                feat_spec[[0, 1, t, f]] = c.im * scale;
            }
        }

        (feat_erb, feat_spec)
    }

    fn inference(
        &mut self,
        spec: &Array3<Complex32>,
        feat_erb: &Array4<f32>,
        feat_spec: &Array4<f32>,
    ) -> Result<Array3<Complex32>, HushError> {
        let n_frames = spec.len_of(Axis(1));

        fn to_4d(value: &ort::value::Value) -> Result<Array4<f32>, HushError> {
            let (shape, data) = value.try_extract_tensor::<f32>()?;
            let s: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
            if s.len() != 4 {
                return Err(HushError::Shape(format!("expected 4D output, got {:?}", s)));
            }
            Ok(Array4::<f32>::from_shape_vec((s[0], s[1], s[2], s[3]), data.to_vec()).unwrap())
        }

        let erb_input = Tensor::from_array(feat_erb.clone())?;
        let spec_input = Tensor::from_array(feat_spec.clone())?;
        let enc_outputs = self.enc.run(ort::inputs! {
            "feat_erb" => erb_input,
            "feat_spec" => spec_input,
        })?;

        let e0 = to_4d(&enc_outputs["e0"])?;
        let e1 = to_4d(&enc_outputs["e1"])?;
        let e2 = to_4d(&enc_outputs["e2"])?;
        let e3 = to_4d(&enc_outputs["e3"])?;
        let emb = {
            let (shape, data) = enc_outputs["emb"].try_extract_tensor::<f32>()?;
            let s: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
            if s.len() != 3 {
                return Err(HushError::Shape(format!("expected 3D emb, got {:?}", s)));
            }
            Array3::<f32>::from_shape_vec((s[0], s[1], s[2]), data.to_vec()).unwrap()
        };
        let c0 = to_4d(&enc_outputs["c0"])?;

        let m = {
            let erb_input = Tensor::from_array(emb.clone())?;
            let e3_input = Tensor::from_array(e3)?;
            let e2_input = Tensor::from_array(e2)?;
            let e1_input = Tensor::from_array(e1)?;
            let e0_input = Tensor::from_array(e0)?;
            let out = self.erb_dec.run(ort::inputs! {
                "emb" => erb_input,
                "e3" => e3_input,
                "e2" => e2_input,
                "e1" => e1_input,
                "e0" => e0_input,
            })?;
            to_4d(&out["m"])?
        };

        let df_coefs = {
            let emb_input = Tensor::from_array(emb)?;
            let c0_input = Tensor::from_array(c0)?;
            let out = self.df_dec.run(ort::inputs! {
                "emb" => emb_input,
                "c0" => c0_input,
            })?;
            to_4d(&out["coefs"])?
        };

        let mut spec_enh = spec.clone();
        let n_freqs = self.freq_size;
        let nb_df = self.config.nb_df;

        // ERB mask: expand to per-bin, divide by band width to match hush's Mask.matmul(erb_inv_fb).
        for t in 0..n_frames {
            let mut bin = 0usize;
            for e in 0..self.config.nb_erb {
                let width = self.erb_widths[e];
                let gain = m[[0, 0, t, e]] / width as f32;
                for f in bin..bin + width {
                    if f >= n_freqs {
                        break;
                    }
                    spec_enh[[0, t, f]] *= gain;
                }
                bin += width;
            }
        }

        // Deep filtering on the first nb_df bins.
        let order = self.config.df_order;
        let pad = order - 1;
        for t in 0..n_frames {
            for f in 0..nb_df {
                let mut re = 0.0f32;
                let mut im = 0.0f32;
                for k in 0..order {
                    let src_t = (t + k).saturating_sub(pad);
                    let c = spec[[0, src_t, f]];
                    let coef_re = df_coefs[[0, t, f, k * 2]];
                    let coef_im = df_coefs[[0, t, f, k * 2 + 1]];
                    re += c.re * coef_re - c.im * coef_im;
                    im += c.re * coef_im + c.im * coef_re;
                }
                spec_enh[[0, t, f]] = Complex32::new(re, im);
            }
        }

        Ok(spec_enh)
    }

    fn istft(
        &self,
        spec: &Array3<Complex32>,
        output_len: usize,
        _padded_len: usize,
    ) -> Result<Vec<f32>, HushError> {
        let n_frames = spec.len_of(Axis(1));
        let hop = self.config.hop_size;
        let n_fft = self.config.fft_size;
        let mut output = vec![0.0f32; n_frames * hop];
        let mut mem = vec![0.0f32; n_fft - hop];
        let mut scratch = self.fft_inverse.make_scratch_vec();

        for t in 0..n_frames {
            let mut c = spec.slice(s![0, t, ..]).to_vec();
            let mut buf = self.fft_inverse.make_output_vec();
            // realfft requires the DC and Nyquist bins to have zero imaginary part.
            if !c.is_empty() {
                c[0] = Complex32::new(c[0].re, 0.0);
                if let Some(last) = c.last_mut() {
                    *last = Complex32::new(last.re, 0.0);
                }
            }
            self.fft_inverse
                .process_with_scratch(&mut c, &mut buf, &mut scratch)
                .map_err(|e| HushError::Audio(format!("ifft error: {e}")))?;
            for (x, &w) in buf.iter_mut().zip(self.window.iter()) {
                *x *= w;
            }

            let (first, second) = buf.split_at(hop);
            for ((&x, &m), o) in first
                .iter()
                .zip(mem.iter())
                .zip(output[t * hop..(t + 1) * hop].iter_mut())
            {
                *o = x + m;
            }

            let split = mem.len() - hop;
            mem.rotate_left(hop);
            let (m_first, m_second) = mem.split_at_mut(split);
            let (s_first, s_second) = second.split_at(split);
            for (x, m) in s_first.iter().zip(m_first.iter_mut()) {
                *m += x;
            }
            for (x, m) in s_second.iter().zip(m_second.iter_mut()) {
                *m = *x;
            }
        }

        output.truncate(output_len);
        Ok(output)
    }
}

fn vorbis_window(size: usize) -> Vec<f32> {
    let half = size / 2;
    let pi = std::f64::consts::PI;
    (0..size)
        .map(|i| {
            let sin = (0.5 * pi * (i as f64 + 0.5) / half as f64).sin();
            ((0.5 * pi * sin * sin).sin()) as f32
        })
        .collect()
}

fn erb_fb(sr: usize, fft_size: usize, nb_bands: usize, min_nb_freqs: usize) -> Vec<usize> {
    let n_freqs = fft_size / 2 + 1;
    let nyq = (sr / 2) as f32;
    let freq_width = sr as f32 / fft_size as f32;
    let erb_low = freq2erb(0.0);
    let erb_high = freq2erb(nyq);
    let step = (erb_high - erb_low) / nb_bands as f32;
    let mut widths = vec![0usize; nb_bands];
    let mut prev = 0usize;
    let mut over = 0i32;
    for i in 1..=nb_bands {
        let f = erb2freq(erb_low + i as f32 * step);
        let fb = (f / freq_width).round() as usize;
        let mut nb = fb as i32 - prev as i32 - over;
        if nb < min_nb_freqs as i32 {
            over = min_nb_freqs as i32 - nb;
            nb = min_nb_freqs as i32;
        } else {
            over = 0;
        }
        widths[i - 1] = nb as usize;
        prev = fb;
    }
    widths[nb_bands - 1] += 1; // include Nyquist bin
    let total: usize = widths.iter().sum();
    if total > n_freqs {
        widths[nb_bands - 1] -= total - n_freqs;
    } else if total < n_freqs {
        widths[nb_bands - 1] += n_freqs - total;
    }
    widths
}

fn freq2erb(f_hz: f32) -> f32 {
    9.265 * (1.0 + f_hz / (24.7 * 9.265)).ln()
}

fn erb2freq(n_erb: f32) -> f32 {
    24.7 * 9.265 * ((n_erb / 9.265).exp() - 1.0)
}

fn rms_peak(samples: &[f32]) -> (f32, f32) {
    let sum_sq = samples.iter().map(|&s| s * s).sum::<f32>();
    let rms = (sum_sq / samples.len() as f32).sqrt();
    let peak = samples
        .iter()
        .map(|&s| s.abs())
        .fold(0.0f32, |a, b| a.max(b));
    (rms, peak)
}

fn init_onnx_runtime() -> Result<(), HushError> {
    if std::env::var("ORT_DYLIB_PATH").is_ok() {
        return Ok(());
    }
    if let Some(path) = find_onnxruntime_dylib() {
        ort::init_from(path)?.commit();
    }
    Ok(())
}

fn find_onnxruntime_dylib() -> Option<PathBuf> {
    let name = format!(
        "{}onnxruntime{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    );
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))?;
    for dir in [exe_dir.clone(), exe_dir.parent().map(PathBuf::from)?] {
        let candidate = dir.join(&name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}
