//! Duration-distribution estimation for resident-agent interventions.

use std::f64::consts::PI;

pub const USUAL_SURVIVAL: f64 = 0.15;
pub const REPLAN_SURVIVAL: f64 = 0.03;
pub const ATTENTION_SHIFT: f64 = 1.0;
pub const REPLAN_SHIFT: f64 = 2.0;
pub const LIKELIHOOD_NOISE_C: f64 = 1.0;
pub const DEFAULT_FALLBACK_MEAN_MINUTES: f64 = 30.0;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum InterventionBand {
    Usual,
    Attention,
    Replan,
}

impl InterventionBand {
    pub const NORMAL: Self = Self::Usual;
    pub const CAUTION: Self = Self::Attention;

    pub fn is_intervention(self) -> bool {
        !matches!(self, Self::Usual)
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct DurationDistribution {
    pub mu: f64,
    pub sigma: f64,
}

impl DurationDistribution {
    pub fn new(mu: f64, sigma: f64) -> Self {
        Self {
            mu: mu.max(0.0),
            sigma: sigma.max(0.0),
        }
    }

    pub fn mean_minutes(self) -> f64 {
        truncated_normal_moments(self).0
    }

    pub fn stddev_minutes(self) -> f64 {
        truncated_normal_moments(self).1
    }

    pub fn survival_probability(self, active_elapsed_minutes: f64) -> f64 {
        survival_probability(self, active_elapsed_minutes)
    }

    pub fn band_at(self, active_elapsed_minutes: f64) -> InterventionBand {
        intervention_band(self.survival_probability(active_elapsed_minutes))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProgressPosterior {
    pub prior: DurationDistribution,
    pub posterior: DurationDistribution,
    pub projection_minutes: f64,
    pub noise_variance: f64,
    pub prior_mean_minutes: f64,
    pub prior_stddev_minutes: f64,
    pub posterior_mean_minutes: f64,
    pub posterior_stddev_minutes: f64,
    pub prior_shift_z: f64,
    pub band: InterventionBand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstimatorError {
    InvalidQuantityFraction,
    InvalidActiveMinutes,
}

impl std::fmt::Display for EstimatorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidQuantityFraction => f.write_str("quantity fraction must be in (0, 1]"),
            Self::InvalidActiveMinutes => f.write_str("active minutes must not be negative"),
        }
    }
}

impl std::error::Error for EstimatorError {}

pub fn truncated_normal_moments(distribution: DurationDistribution) -> (f64, f64) {
    let distribution = DurationDistribution::new(distribution.mu, distribution.sigma);
    if distribution.sigma == 0.0 {
        return (distribution.mu, 0.0);
    }

    let alpha = -distribution.mu / distribution.sigma;
    let normalization = normal_cdf(-alpha).max(f64::MIN_POSITIVE);
    let lambda = normal_pdf(alpha) / normalization;
    let mean = distribution.mu + distribution.sigma * lambda;
    let variance =
        (distribution.sigma * distribution.sigma * (1.0 + alpha * lambda - lambda * lambda))
            .max(0.0);
    (mean, variance.sqrt())
}

pub fn survival_probability(
    distribution: DurationDistribution,
    active_elapsed_minutes: f64,
) -> f64 {
    if active_elapsed_minutes <= 0.0 {
        return 1.0;
    }
    if distribution.sigma == 0.0 {
        return f64::from(active_elapsed_minutes < distribution.mu);
    }

    let normalization = normal_cdf(distribution.mu / distribution.sigma);
    if normalization <= f64::MIN_POSITIVE {
        return 0.0;
    }
    let tail = normal_cdf((distribution.mu - active_elapsed_minutes) / distribution.sigma);
    (tail / normalization).clamp(0.0, 1.0)
}

pub fn intervention_band(survival: f64) -> InterventionBand {
    if survival > USUAL_SURVIVAL {
        InterventionBand::Usual
    } else if survival > REPLAN_SURVIVAL {
        InterventionBand::Attention
    } else {
        InterventionBand::Replan
    }
}

pub fn next_crossing_active_minutes(
    distribution: DurationDistribution,
    active_elapsed_minutes: f64,
) -> Option<f64> {
    let elapsed = active_elapsed_minutes.max(0.0);
    for boundary in [USUAL_SURVIVAL, REPLAN_SURVIVAL] {
        let crossing = inverse_survival(distribution, boundary)?;
        if crossing > elapsed + 1e-9 {
            return Some(crossing);
        }
    }
    None
}

pub fn next_crossing_time(
    distribution: DurationDistribution,
    active_elapsed_minutes: f64,
    now_minutes: f64,
) -> Option<f64> {
    next_crossing_active_minutes(distribution, active_elapsed_minutes)
        .map(|crossing| now_minutes + crossing - active_elapsed_minutes.max(0.0))
}

pub fn progress_posterior(
    prior: DurationDistribution,
    active_elapsed_minutes: f64,
    quantity_fraction: f64,
) -> Result<ProgressPosterior, EstimatorError> {
    if active_elapsed_minutes < 0.0 {
        return Err(EstimatorError::InvalidActiveMinutes);
    }
    if !(0.0 < quantity_fraction && quantity_fraction <= 1.0) {
        return Err(EstimatorError::InvalidQuantityFraction);
    }

    let prior = DurationDistribution::new(prior.mu, prior.sigma);
    let projection_minutes = (active_elapsed_minutes / quantity_fraction).max(0.0);
    let prior_mean_minutes = prior.mean_minutes();
    let prior_stddev_minutes = prior.stddev_minutes();
    let noise_variance = LIKELIHOOD_NOISE_C * prior.sigma * prior.sigma * (1.0 - quantity_fraction)
        / quantity_fraction;

    let posterior = if prior.sigma == 0.0 || noise_variance <= f64::EPSILON {
        DurationDistribution::new(projection_minutes, 0.0)
    } else {
        let prior_variance = prior.sigma * prior.sigma;
        let posterior_variance = 1.0 / (1.0 / prior_variance + 1.0 / noise_variance);
        let posterior_mu =
            posterior_variance * (prior.mu / prior_variance + projection_minutes / noise_variance);
        DurationDistribution::new(posterior_mu, posterior_variance.sqrt())
    };

    let posterior_mean_minutes = posterior.mean_minutes();
    let posterior_stddev_minutes = posterior.stddev_minutes();
    let prior_shift_z = if prior_stddev_minutes > f64::EPSILON {
        (posterior_mean_minutes - prior_mean_minutes) / prior_stddev_minutes
    } else if posterior_mean_minutes > prior_mean_minutes + f64::EPSILON {
        f64::INFINITY
    } else {
        0.0
    };
    let band = if prior_shift_z >= REPLAN_SHIFT {
        InterventionBand::Replan
    } else if prior_shift_z >= ATTENTION_SHIFT {
        InterventionBand::Attention
    } else {
        InterventionBand::Usual
    };

    Ok(ProgressPosterior {
        prior,
        posterior,
        projection_minutes,
        noise_variance,
        prior_mean_minutes,
        prior_stddev_minutes,
        posterior_mean_minutes,
        posterior_stddev_minutes,
        prior_shift_z,
        band,
    })
}

/// Conditional expected remaining time `E[T - e | T > e]` for a positive-support
/// truncated-normal duration `T` and observed elapsed time `e`.
///
/// For `sigma > 0` this is the mean residual life of the underlying normal
/// distribution at `e`. For `sigma == 0` it is the remaining deterministic
/// duration, or `0.0` if the elapsed time already meets or exceeds it.
pub fn conditional_expected_remaining_minutes(
    distribution: DurationDistribution,
    active_elapsed_minutes: f64,
) -> Option<f64> {
    if active_elapsed_minutes < 0.0 {
        return None;
    }
    if distribution.sigma == 0.0 {
        return if active_elapsed_minutes >= distribution.mu {
            Some(0.0)
        } else {
            Some(distribution.mu - active_elapsed_minutes)
        };
    }

    let alpha = (active_elapsed_minutes - distribution.mu) / distribution.sigma;
    let survival = (1.0 - normal_cdf(alpha)).max(f64::MIN_POSITIVE);
    let mills_ratio = normal_pdf(alpha) / survival;
    let conditional_mean = distribution.mu + distribution.sigma * mills_ratio;
    Some((conditional_mean - active_elapsed_minutes).max(0.0))
}

pub fn effective_distribution(
    mu_minutes: f64,
    sigma_minutes: f64,
    task_kind_prior: Option<DurationDistribution>,
) -> DurationDistribution {
    if sigma_minutes > 0.0 {
        DurationDistribution::new(mu_minutes, sigma_minutes)
    } else if let Some(prior) = task_kind_prior {
        prior
    } else {
        fallback_distribution(if mu_minutes > 0.0 {
            mu_minutes
        } else {
            DEFAULT_FALLBACK_MEAN_MINUTES
        })
    }
}

pub fn fallback_distribution(mean_minutes: f64) -> DurationDistribution {
    let mean = mean_minutes.max(1.0);
    DurationDistribution::new(mean, mean * 0.5)
}

fn inverse_survival(distribution: DurationDistribution, survival: f64) -> Option<f64> {
    if !(0.0 < survival && survival < 1.0) {
        return None;
    }
    if distribution.sigma == 0.0 {
        return (distribution.mu > 0.0).then_some(distribution.mu);
    }

    let normalization = normal_cdf(distribution.mu / distribution.sigma);
    let cdf = 1.0 - survival * normalization;
    let standard = normal_inverse_cdf(cdf.clamp(f64::MIN_POSITIVE, 1.0 - f64::EPSILON));
    Some((distribution.mu + distribution.sigma * standard).max(0.0))
}

fn normal_pdf(x: f64) -> f64 {
    (-0.5 * x * x).exp() / (2.0 * PI).sqrt()
}

fn normal_cdf(x: f64) -> f64 {
    0.5 * (1.0 + erf(x / 2.0_f64.sqrt()))
}

fn erf(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.3275911 * x);
    let polynomial = (((((1.061405429 * t - 1.453152027) * t) + 1.421413741) * t - 0.284496736)
        * t
        + 0.254829592)
        * t;
    sign * (1.0 - polynomial * (-x * x).exp())
}

fn horner<const N: usize>(coefficients: &[f64; N], x: f64) -> f64 {
    coefficients
        .iter()
        .copied()
        .fold(0.0, |value, coefficient| value * x + coefficient)
}

fn normal_inverse_cdf(p: f64) -> f64 {
    const A: [f64; 6] = [
        -3.969683028665376e1,
        2.209460984245205e2,
        -2.759285104469687e2,
        1.38357751867269e2,
        -3.066479806614716e1,
        2.506628277459239,
    ];
    const B: [f64; 5] = [
        -5.447609879822406e1,
        1.615858368580409e2,
        -1.556989798598866e2,
        6.680131188771972e1,
        -1.328068155288572e1,
    ];
    const C: [f64; 6] = [
        -7.784894002430293e-3,
        -3.223964580411365e-1,
        -2.400758277161838e0,
        -2.549732539343734e0,
        4.374664141464968e0,
        2.938163982698783e0,
    ];
    const D: [f64; 4] = [
        7.784695709041462e-3,
        3.224671290700398e-1,
        2.445134137142996e0,
        3.754408661907416e0,
    ];

    let p = p.clamp(f64::MIN_POSITIVE, 1.0 - f64::EPSILON);
    if p < 0.02425 {
        let q = (-2.0 * p.ln()).sqrt();
        let numerator = horner(&C, q);
        let denominator = horner(&D, q) * q + 1.0;
        return numerator / denominator;
    }
    if p > 1.0 - 0.02425 {
        let q = (-2.0 * (1.0 - p).ln()).sqrt();
        let numerator = horner(&C, q);
        let denominator = horner(&D, q) * q + 1.0;
        return -numerator / denominator;
    }

    let q = p - 0.5;
    let r = q * q;
    let numerator = horner(&A, r) * q;
    let denominator = horner(&B, r) * r + 1.0;
    numerator / denominator
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncated_moments_are_normalized() {
        let distribution = DurationDistribution::new(10.0, 20.0);
        let (mean, sigma) = truncated_normal_moments(distribution);
        assert!(mean > 10.0);
        assert!(sigma > 0.0);
    }

    #[test]
    fn survival_is_monotonic_and_uses_the_bands() {
        let distribution = DurationDistribution::new(60.0, 10.0);
        assert!(distribution.survival_probability(10.0) > distribution.survival_probability(70.0));
        assert_eq!(intervention_band(0.2), InterventionBand::Usual);
        assert_eq!(intervention_band(0.1), InterventionBand::Attention);
        assert_eq!(intervention_band(0.02), InterventionBand::Replan);
    }

    #[test]
    fn crossing_time_inverts_survival() {
        let distribution = DurationDistribution::new(60.0, 10.0);
        let crossing = next_crossing_active_minutes(distribution, 0.0).unwrap();
        assert!((distribution.survival_probability(crossing) - USUAL_SURVIVAL).abs() < 1e-5);
        assert!(next_crossing_time(distribution, crossing + 1.0, 100.0).is_some());
    }

    #[test]
    fn progress_posterior_tightens_and_reports_shift() {
        let result = progress_posterior(DurationDistribution::new(60.0, 20.0), 50.0, 0.5).unwrap();
        assert!(result.posterior_stddev_minutes < result.prior_stddev_minutes);
        assert!(result.posterior_mean_minutes > result.prior_mean_minutes);
        assert!(result.prior_shift_z > 0.0);
    }

    #[test]
    fn complete_progress_is_degenerate() {
        let result = progress_posterior(DurationDistribution::new(60.0, 20.0), 45.0, 1.0).unwrap();
        assert_eq!(result.projection_minutes, 45.0);
        assert_eq!(result.posterior_stddev_minutes, 0.0);
    }

    #[test]
    fn conditional_remaining_uses_mean_residual_life() {
        let distribution = DurationDistribution::new(60.0, 10.0);
        let at_zero = conditional_expected_remaining_minutes(distribution, 0.0).unwrap();
        // At e = 0 the condition T > 0 is always true, so the conditional mean
        // equals the unconditional truncated mean.
        assert!((at_zero - distribution.mean_minutes()).abs() < 1e-9);

        let at_mean = conditional_expected_remaining_minutes(distribution, 60.0).unwrap();
        // Above the mean, the conditional expectation still leaves some positive
        // remaining time (mean residual life of a normal at its mean is ~0.8 sigma).
        assert!(at_mean > 0.0);
        assert!(at_mean < at_zero);

        let at_80 = conditional_expected_remaining_minutes(distribution, 80.0).unwrap();
        assert!(at_80 > 0.0 && at_80 < at_mean);
    }

    #[test]
    fn conditional_remaining_is_degenerate_for_zero_sigma() {
        let distribution = DurationDistribution::new(60.0, 0.0);
        assert_eq!(
            conditional_expected_remaining_minutes(distribution, 30.0),
            Some(30.0)
        );
        assert_eq!(
            conditional_expected_remaining_minutes(distribution, 60.0),
            Some(0.0)
        );
        assert_eq!(
            conditional_expected_remaining_minutes(distribution, 90.0),
            Some(0.0)
        );
        assert_eq!(
            conditional_expected_remaining_minutes(distribution, -1.0),
            None
        );
    }

    #[test]
    fn zero_sigma_uses_a_prior_or_wide_fallback() {
        let fallback = effective_distribution(10.0, 0.0, None);
        assert_eq!(fallback, DurationDistribution::new(10.0, 5.0));
        let default = effective_distribution(0.0, 0.0, None);
        assert_eq!(default, DurationDistribution::new(30.0, 15.0));
        let prior = DurationDistribution::new(90.0, 12.0);
        assert_eq!(effective_distribution(10.0, 0.0, Some(prior)), prior);
    }
}
