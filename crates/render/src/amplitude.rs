//! Pure seismic amplitude processing pipeline:
//! DC removal, sliding-window RMS AGC, trace balance, per-trace/global RMS or percentile clipping,
//! gain in dB, and polarity flip.

/// Minimum positive amplitude threshold to avoid division by zero on dead/silent traces.
pub const MIN_AMP_EPSILON: f32 = 1e-30;

/// Maximum number of samples collected into the global percentile selection buffer.
/// Larger datasets are uniformly strided so global percentile estimation stays O(1) in memory.
const MAX_GLOBAL_PERCENTILE_SAMPLES: usize = 500_000;

/// Scope over which the reference clipping amplitude is computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NormScope {
    /// Normalize and clip all traces against a single global reference amplitude.
    #[default]
    Global,
    /// Normalize and clip each trace independently against its own reference amplitude.
    PerTrace,
}

/// Strategy used to determine the reference clipping amplitude (`1.0` after normalization).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ClipStrategy {
    /// Clip at `multiple * RMS` of the trace or section (e.g. `multiple = 2.0`).
    Rms {
        /// Positive multiplier applied to the RMS amplitude.
        multiple: f32,
    },
    /// Clip at the `percentile`-th percentile of `|amplitude|` in `(0.0, 100.0]` (e.g. `99.0`).
    Percentile {
        /// Percentile in `0.0..=100.0`.
        percentile: f32,
    },
    /// Clip at an explicit absolute amplitude threshold `limit`.
    Absolute {
        /// Positive amplitude threshold.
        limit: f32,
    },
    /// Clip at the maximum absolute amplitude (`100%` peak).
    Peak,
}

impl Default for ClipStrategy {
    fn default() -> Self {
        Self::Percentile { percentile: 99.0 }
    }
}

/// Trace-to-trace pre-balancing mode applied before global or per-trace clipping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TraceBalance {
    /// No trace pre-balancing.
    #[default]
    Off,
    /// Normalize each trace by its RMS energy.
    Rms,
    /// Normalize each trace by its mean absolute amplitude.
    MeanAbs,
    /// Normalize each trace by its peak (`max(|sample|)`) amplitude.
    Peak,
}

/// Sliding-window Automatic Gain Control (AGC) configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgcConfig {
    /// Whether sliding-window AGC is active.
    pub enabled: bool,
    /// Sliding window length in samples (clamped to `>= 1` when executed).
    pub window_samples: usize,
}

impl AgcConfig {
    /// Returns a disabled AGC configuration (default).
    #[inline]
    pub const fn off() -> Self {
        Self {
            enabled: false,
            window_samples: 125,
        }
    }

    /// Returns an enabled AGC configuration with the given sliding window length in samples.
    #[inline]
    pub const fn enabled(window_samples: usize) -> Self {
        Self {
            enabled: true,
            window_samples: if window_samples == 0 {
                1
            } else {
                window_samples
            },
        }
    }
}

impl Default for AgcConfig {
    fn default() -> Self {
        Self::off()
    }
}

/// Display polarity convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Polarity {
    /// Standard SEG-Y polarity (`+1.0`).
    #[default]
    Normal,
    /// Reversed / flipped polarity (`-1.0`).
    Reversed,
}

impl Polarity {
    /// Returns `+1.0` for [`Polarity::Normal`] and `-1.0` for [`Polarity::Reversed`].
    #[inline]
    pub const fn sign(self) -> f32 {
        match self {
            Self::Normal => 1.0,
            Self::Reversed => -1.0,
        }
    }

    /// Returns `true` if polarity is reversed.
    #[inline]
    pub const fn is_flipped(self) -> bool {
        matches!(self, Self::Reversed)
    }
}

/// Complete configuration for the seismic trace amplitude pipeline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AmplitudeParams {
    /// Gain in decibels (`0.0` dB = unity gain `1.0x`, `+6.02` dB ≈ `2.0x`, `-6.02` dB ≈ `0.5x`).
    pub gain_db: f32,
    /// Per-trace vs global normalization scope.
    pub scope: NormScope,
    /// Strategy for computing the `1.0` reference clipping amplitude.
    pub clip: ClipStrategy,
    /// Optional sliding-window Automatic Gain Control (AGC).
    pub agc: AgcConfig,
    /// Normal or flipped display polarity.
    pub polarity: Polarity,
    /// Subtract the mean amplitude (DC bias) from each trace before scaling.
    pub dc_removal: bool,
    /// Optional trace-to-trace energy balance applied before clipping.
    pub trace_balance: TraceBalance,
    /// Optional symmetric hard clip limit applied after normalization and gain (`Some(1.0)` clamps to `[-1.0, 1.0]`).
    pub clamp_limit: Option<f32>,
}

impl Default for AmplitudeParams {
    fn default() -> Self {
        Self {
            gain_db: 0.0,
            scope: NormScope::Global,
            clip: ClipStrategy::default(),
            agc: AgcConfig::off(),
            polarity: Polarity::Normal,
            dc_removal: false,
            trace_balance: TraceBalance::Off,
            clamp_limit: Some(1.0),
        }
    }
}

impl AmplitudeParams {
    /// Sets the gain in decibels (`dB`).
    #[inline]
    pub const fn with_gain_db(mut self, gain_db: f32) -> Self {
        self.gain_db = gain_db;
        self
    }

    /// Sets the normalization scope ([`NormScope::Global`] or [`NormScope::PerTrace`]).
    #[inline]
    pub const fn with_scope(mut self, scope: NormScope) -> Self {
        self.scope = scope;
        self
    }

    /// Sets the reference clipping strategy.
    #[inline]
    pub const fn with_clip(mut self, clip: ClipStrategy) -> Self {
        self.clip = clip;
        self
    }

    /// Sets the sliding-window AGC configuration.
    #[inline]
    pub const fn with_agc(mut self, agc: AgcConfig) -> Self {
        self.agc = agc;
        self
    }

    /// Sets the display polarity.
    #[inline]
    pub const fn with_polarity(mut self, polarity: Polarity) -> Self {
        self.polarity = polarity;
        self
    }

    /// Sets whether polarity is flipped.
    #[inline]
    pub const fn with_polarity_flip(mut self, flipped: bool) -> Self {
        self.polarity = if flipped {
            Polarity::Reversed
        } else {
            Polarity::Normal
        };
        self
    }

    /// Sets whether per-trace DC bias removal is enabled.
    #[inline]
    pub const fn with_dc_removal(mut self, dc_removal: bool) -> Self {
        self.dc_removal = dc_removal;
        self
    }

    /// Sets the trace-to-trace balance mode.
    #[inline]
    pub const fn with_trace_balance(mut self, trace_balance: TraceBalance) -> Self {
        self.trace_balance = trace_balance;
        self
    }

    /// Sets the post-gain symmetric hard clamp limit (`None` disables clamping).
    #[inline]
    pub const fn with_clamp_limit(mut self, clamp_limit: Option<f32>) -> Self {
        self.clamp_limit = clamp_limit;
        self
    }

    /// Converts `gain_db` into a linear multiplier (`10^(gain_db / 20)`).
    #[inline]
    pub fn gain_linear(&self) -> f32 {
        gain_db_to_linear(self.gain_db)
    }

    /// Returns `true` if the pipeline requires a per-trace pre-processing pass
    /// (DC removal, sliding-window AGC, or trace balance) prior to linear scaling.
    #[inline]
    pub const fn requires_prepass(&self) -> bool {
        self.dc_removal || self.agc.enabled || !matches!(self.trace_balance, TraceBalance::Off)
    }
}

/// Reusable scratch buffers for zero-allocation per-trace amplitude processing.
#[derive(Debug, Default, Clone)]
pub struct AmplitudeScratch {
    prefix_sq: Vec<f64>,
    agc_out: Vec<f32>,
    percentile_buf: Vec<f32>,
}

impl AmplitudeScratch {
    /// Creates an empty scratch state.
    pub fn new() -> Self {
        Self::default()
    }
}

/// Converts a gain value in decibels (`dB`) to a linear amplitude factor (`10^(gain_db / 20)`).
/// Non-finite inputs return `1.0`; extreme inputs are clamped to `[-240.0, 240.0]` dB.
#[inline]
pub fn gain_db_to_linear(gain_db: f32) -> f32 {
    if !gain_db.is_finite() || gain_db == 0.0 {
        return 1.0;
    }
    let clamped_db = gain_db.clamp(-240.0, 240.0);
    10.0_f32.powf(clamped_db * 0.05)
}

/// Sanitizes a floating-point sample so `NaN` and `±Inf` become `0.0`.
#[inline]
pub fn sanitize_sample(sample: f32) -> f32 {
    if sample.is_finite() { sample } else { 0.0 }
}

/// Computes the arithmetic mean (DC offset) of finite samples in `samples`.
/// Returns `0.0` if `samples` is empty.
pub fn trace_mean(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sum = 0.0_f64;
    let mut count = 0_usize;
    for &s in samples {
        if s.is_finite() {
            sum += f64::from(s);
            count += 1;
        }
    }
    if count == 0 {
        0.0
    } else {
        (sum / count as f64) as f32
    }
}

/// Subtracts the trace mean (DC bias) in place and sanitizes any non-finite samples to `0.0`.
pub fn remove_dc_in_place(samples: &mut [f32]) {
    let mean = trace_mean(samples);
    for s in samples.iter_mut() {
        *s = if s.is_finite() { *s - mean } else { 0.0 };
    }
}

/// Computes the Root-Mean-Square (RMS) amplitude of finite samples in `samples`.
/// Uses an `f64` accumulator so large sample values (up to `f32::MAX`) never overflow.
pub fn trace_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sum_sq = 0.0_f64;
    let mut count = 0_usize;
    for &s in samples {
        if s.is_finite() {
            let v = f64::from(s);
            sum_sq += v * v;
            count += 1;
        }
    }
    if count == 0 {
        0.0
    } else {
        (sum_sq / count as f64).sqrt() as f32
    }
}

/// Computes the mean absolute amplitude (`mean(|x|)`) of finite samples in `samples`.
pub fn trace_mean_abs(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sum_abs = 0.0_f64;
    let mut count = 0_usize;
    for &s in samples {
        if s.is_finite() {
            sum_abs += f64::from(s.abs());
            count += 1;
        }
    }
    if count == 0 {
        0.0
    } else {
        (sum_abs / count as f64) as f32
    }
}

/// Computes the maximum absolute amplitude (`max(|x|)`) of finite samples in `samples`.
pub fn trace_max_abs(samples: &[f32]) -> f32 {
    let mut max_abs = 0.0_f32;
    for &s in samples {
        if s.is_finite() {
            let a = s.abs();
            if a > max_abs {
                max_abs = a;
            }
        }
    }
    max_abs
}

/// Computes the `percentile`-th percentile (`0.0..=100.0`) of `|sample|` in O(N) time.
pub fn abs_percentile(samples: &[f32], percentile: f32) -> f32 {
    let mut scratch = Vec::new();
    abs_percentile_with_scratch(samples, percentile, &mut scratch)
}

/// Computes the `percentile`-th percentile (`0.0..=100.0`) of `|sample|` reusing `scratch`.
pub fn abs_percentile_with_scratch(
    samples: &[f32],
    percentile: f32,
    scratch: &mut Vec<f32>,
) -> f32 {
    scratch.clear();
    scratch.reserve(samples.len());
    for &s in samples {
        if s.is_finite() {
            scratch.push(s.abs());
        }
    }
    select_percentile_in_place(scratch, percentile)
}

fn select_percentile_in_place(values: &mut [f32], percentile: f32) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let pct = if percentile.is_finite() {
        percentile.clamp(0.0, 100.0)
    } else {
        99.0
    };
    let last = values.len() - 1;
    let rank = ((last as f64) * (f64::from(pct) / 100.0)).round() as usize;
    let target_idx = rank.min(last);
    let (_, &mut pivot, _) = values.select_nth_unstable_by(target_idx, f32::total_cmp);
    pivot
}

/// Applies O(N) sliding-window RMS Automatic Gain Control (AGC) in place.
///
/// For each sample `i`, the local RMS is computed over a centered window of length
/// `window_samples.max(1)` (clamped to trace boundaries) using a prefix sum of squares.
pub fn apply_agc_in_place(samples: &mut [f32], window_samples: usize) {
    let mut prefix_sq = Vec::new();
    let mut agc_out = Vec::new();
    apply_agc_with_scratch(samples, window_samples, &mut prefix_sq, &mut agc_out);
}

/// Applies O(N) sliding-window RMS AGC in place using caller-provided scratch buffers.
pub fn apply_agc_with_scratch(
    samples: &mut [f32],
    window_samples: usize,
    prefix_sq: &mut Vec<f64>,
    agc_out: &mut Vec<f32>,
) {
    let n = samples.len();
    if n == 0 {
        return;
    }
    let win = window_samples.max(1);
    let half_before = win / 2;
    let half_after = win - half_before;

    prefix_sq.clear();
    prefix_sq.reserve(n + 1);
    prefix_sq.push(0.0);

    let mut acc = 0.0_f64;
    for s in samples.iter_mut() {
        let clean = sanitize_sample(*s);
        *s = clean;
        let v = f64::from(clean);
        acc += v * v;
        prefix_sq.push(acc);
    }

    agc_out.clear();
    agc_out.reserve(n);
    for (i, &s) in samples.iter().enumerate() {
        let start = i.saturating_sub(half_before);
        let end = i.saturating_add(half_after).min(n);
        let count = end.saturating_sub(start).max(1);
        let sum_sq = (prefix_sq[end] - prefix_sq[start]).max(0.0);
        let rms = (sum_sq / count as f64).sqrt() as f32;
        let normalized = if rms > MIN_AMP_EPSILON { s / rms } else { 0.0 };
        agc_out.push(normalized);
    }

    samples.copy_from_slice(agc_out);
}

/// Applies trace-to-trace energy balancing in place according to `mode`.
pub fn apply_trace_balance_in_place(samples: &mut [f32], mode: TraceBalance) {
    if samples.is_empty() {
        return;
    }
    let denom = match mode {
        TraceBalance::Off => return,
        TraceBalance::Rms => trace_rms(samples),
        TraceBalance::MeanAbs => trace_mean_abs(samples),
        TraceBalance::Peak => trace_max_abs(samples),
    };
    if denom <= MIN_AMP_EPSILON {
        samples.fill(0.0);
        return;
    }
    let inv = 1.0 / denom;
    for s in samples.iter_mut() {
        *s = sanitize_sample(*s) * inv;
    }
}

/// Computes the positive reference clipping amplitude for a single trace.
pub fn compute_clip_reference(samples: &[f32], clip: ClipStrategy) -> f32 {
    let mut scratch = Vec::new();
    compute_clip_reference_with_scratch(samples, clip, &mut scratch)
}

/// Computes the positive reference clipping amplitude for a single trace reusing `scratch`.
pub fn compute_clip_reference_with_scratch(
    samples: &[f32],
    clip: ClipStrategy,
    scratch: &mut Vec<f32>,
) -> f32 {
    match clip {
        ClipStrategy::Rms { multiple } => {
            let mult = if multiple.is_finite() && multiple > 0.0 {
                multiple
            } else {
                1.0
            };
            trace_rms(samples) * mult
        }
        ClipStrategy::Percentile { percentile } => {
            abs_percentile_with_scratch(samples, percentile, scratch)
        }
        ClipStrategy::Absolute { limit } => {
            if limit.is_finite() && limit > 0.0 {
                limit
            } else {
                0.0
            }
        }
        ClipStrategy::Peak => trace_max_abs(samples),
    }
}

/// Applies the pre-clip per-trace stages (sanitization, DC removal, AGC, trace balance) in place.
pub fn preprocess_trace_with_scratch(
    samples: &mut [f32],
    params: &AmplitudeParams,
    scratch: &mut AmplitudeScratch,
) {
    if params.dc_removal {
        remove_dc_in_place(samples);
    } else {
        for s in samples.iter_mut() {
            *s = sanitize_sample(*s);
        }
    }

    if params.agc.enabled {
        apply_agc_with_scratch(
            samples,
            params.agc.window_samples,
            &mut scratch.prefix_sq,
            &mut scratch.agc_out,
        );
    }

    if params.trace_balance != TraceBalance::Off {
        apply_trace_balance_in_place(samples, params.trace_balance);
    }
}

/// Computes a single global reference clipping amplitude across a collection of traces.
///
/// If `params.requires_prepass()` is `true` (e.g., DC removal, AGC, or trace balance is enabled),
/// each trace is pre-processed in a temporary scratch buffer before measuring the global reference.
pub fn compute_global_reference<S: AsRef<[f32]>>(traces: &[S], params: &AmplitudeParams) -> f32 {
    if traces.is_empty() {
        return 0.0;
    }
    if let ClipStrategy::Absolute { limit } = params.clip {
        return if limit.is_finite() && limit > 0.0 {
            limit
        } else {
            0.0
        };
    }

    let needs_prepass = params.requires_prepass();
    let mut scratch = AmplitudeScratch::new();
    let mut trace_buf = Vec::new();

    match params.clip {
        ClipStrategy::Rms { multiple } => {
            let mult = if multiple.is_finite() && multiple > 0.0 {
                multiple
            } else {
                1.0
            };
            let mut sum_sq = 0.0_f64;
            let mut count = 0_usize;
            for trace in traces {
                let slice = if needs_prepass {
                    trace_buf.clear();
                    trace_buf.extend_from_slice(trace.as_ref());
                    preprocess_trace_with_scratch(&mut trace_buf, params, &mut scratch);
                    trace_buf.as_slice()
                } else {
                    trace.as_ref()
                };
                for &s in slice {
                    if s.is_finite() {
                        let v = f64::from(s);
                        sum_sq += v * v;
                        count += 1;
                    }
                }
            }
            if count == 0 {
                0.0
            } else {
                ((sum_sq / count as f64).sqrt() as f32) * mult
            }
        }
        ClipStrategy::Peak => {
            let mut max_abs = 0.0_f32;
            for trace in traces {
                let slice = if needs_prepass {
                    trace_buf.clear();
                    trace_buf.extend_from_slice(trace.as_ref());
                    preprocess_trace_with_scratch(&mut trace_buf, params, &mut scratch);
                    trace_buf.as_slice()
                } else {
                    trace.as_ref()
                };
                let t_max = trace_max_abs(slice);
                if t_max > max_abs {
                    max_abs = t_max;
                }
            }
            max_abs
        }
        ClipStrategy::Percentile { percentile } => {
            let total_samples: usize = traces.iter().map(|t| t.as_ref().len()).sum();
            if total_samples == 0 {
                return 0.0;
            }
            let step = (total_samples / MAX_GLOBAL_PERCENTILE_SAMPLES).max(1);
            let mut collected =
                Vec::with_capacity(total_samples.min(MAX_GLOBAL_PERCENTILE_SAMPLES));
            let mut idx = 0_usize;
            for trace in traces {
                let slice = if needs_prepass {
                    trace_buf.clear();
                    trace_buf.extend_from_slice(trace.as_ref());
                    preprocess_trace_with_scratch(&mut trace_buf, params, &mut scratch);
                    trace_buf.as_slice()
                } else {
                    trace.as_ref()
                };
                for &s in slice {
                    if idx.is_multiple_of(step) && s.is_finite() {
                        collected.push(s.abs());
                    }
                    idx = idx.wrapping_add(1);
                }
            }
            select_percentile_in_place(&mut collected, percentile)
        }
        ClipStrategy::Absolute { limit } => limit.max(0.0),
    }
}

/// Applies the full amplitude pipeline to `samples` in place, reusing `scratch`.
///
/// When `params.scope == NormScope::Global`, `global_ref` is used as the reference clipping
/// amplitude (falling back to the trace's own reference if `global_ref` is `None`).
pub fn process_trace_with_scratch(
    samples: &mut [f32],
    params: &AmplitudeParams,
    global_ref: Option<f32>,
    scratch: &mut AmplitudeScratch,
) {
    if samples.is_empty() {
        return;
    }

    preprocess_trace_with_scratch(samples, params, scratch);

    let ref_amp = match params.scope {
        NormScope::Global => global_ref.unwrap_or_else(|| {
            compute_clip_reference_with_scratch(samples, params.clip, &mut scratch.percentile_buf)
        }),
        NormScope::PerTrace => {
            compute_clip_reference_with_scratch(samples, params.clip, &mut scratch.percentile_buf)
        }
    };

    if !ref_amp.is_finite() || ref_amp <= MIN_AMP_EPSILON {
        samples.fill(0.0);
        return;
    }

    let scale = params.polarity.sign() * params.gain_linear() / ref_amp;
    apply_scale_and_clamp_in_place(samples, scale, params.clamp_limit);
}

/// Applies a linear scale factor and optional symmetric clamp (`[-limit, +limit]`) in place.
#[inline]
pub fn apply_scale_and_clamp_in_place(samples: &mut [f32], scale: f32, clamp_limit: Option<f32>) {
    if !scale.is_finite() || scale == 0.0 {
        samples.fill(0.0);
        return;
    }
    if let Some(limit) = clamp_limit.filter(|l| l.is_finite() && *l > 0.0) {
        let neg_limit = -limit;
        for s in samples.iter_mut() {
            *s = (*s * scale).clamp(neg_limit, limit);
        }
    } else {
        for s in samples.iter_mut() {
            *s *= scale;
        }
    }
}

/// Applies the full amplitude pipeline to `samples` in place.
pub fn process_trace_in_place(
    samples: &mut [f32],
    params: &AmplitudeParams,
    global_ref: Option<f32>,
) {
    let mut scratch = AmplitudeScratch::new();
    process_trace_with_scratch(samples, params, global_ref, &mut scratch);
}

/// Pure function that returns a newly allocated `Vec<f32>` with the amplitude pipeline applied.
pub fn process_trace(
    samples: &[f32],
    params: &AmplitudeParams,
    global_ref: Option<f32>,
) -> Vec<f32> {
    let mut out = samples.to_vec();
    process_trace_in_place(&mut out, params, global_ref);
    out
}

/// Pure function that processes a 2-D seismic section (`traces × samples`), automatically
/// computing the global reference amplitude when `params.scope == NormScope::Global`.
pub fn process_section<S: AsRef<[f32]>>(traces: &[S], params: &AmplitudeParams) -> Vec<Vec<f32>> {
    let global_ref = match params.scope {
        NormScope::Global => Some(compute_global_reference(traces, params)),
        NormScope::PerTrace => None,
    };
    let mut scratch = AmplitudeScratch::new();
    let mut out = Vec::with_capacity(traces.len());
    for trace in traces {
        let mut t = trace.as_ref().to_vec();
        process_trace_with_scratch(&mut t, params, global_ref, &mut scratch);
        out.push(t);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_db_conversion_matches_expected_ratios() {
        assert!((gain_db_to_linear(0.0) - 1.0).abs() < 1e-6);
        assert!((gain_db_to_linear(20.0) - 10.0).abs() < 1e-5);
        assert!((gain_db_to_linear(-20.0) - 0.1).abs() < 1e-6);
        assert!((gain_db_to_linear(6.020_6) - 2.0).abs() < 1e-5);
        assert!((gain_db_to_linear(f32::NAN) - 1.0).abs() < 1e-6);
        assert!(gain_db_to_linear(1000.0).is_finite());
    }

    #[test]
    fn dc_removal_subtracts_mean_and_cleans_non_finite() {
        let mut trace = vec![10.0, 12.0, 14.0, 16.0];
        assert!((trace_mean(&trace) - 13.0).abs() < 1e-6);
        remove_dc_in_place(&mut trace);
        assert_eq!(trace, vec![-3.0, -1.0, 1.0, 3.0]);
        assert!(trace_mean(&trace).abs() < 1e-6);

        let mut noisy = vec![5.0, f32::NAN, 15.0];
        remove_dc_in_place(&mut noisy);
        assert_eq!(noisy, vec![-5.0, 0.0, 5.0]);
    }

    #[test]
    fn rms_and_percentile_clip_behave_accurately() {
        let trace = vec![-2.0, 2.0, -2.0, 2.0];
        assert!((trace_rms(&trace) - 2.0).abs() < 1e-6);
        assert!((trace_max_abs(&trace) - 2.0).abs() < 1e-6);
        assert!((trace_mean_abs(&trace) - 2.0).abs() < 1e-6);

        let ramp: Vec<f32> = (0..=100).map(|i| i as f32).collect();
        assert!((abs_percentile(&ramp, 0.0) - 0.0).abs() < 1e-6);
        assert!((abs_percentile(&ramp, 50.0) - 50.0).abs() < 1e-6);
        assert!((abs_percentile(&ramp, 99.0) - 99.0).abs() < 1e-6);
        assert!((abs_percentile(&ramp, 100.0) - 100.0).abs() < 1e-6);
    }

    #[test]
    fn per_trace_vs_global_normalization_distinguishes_relative_energy() {
        let weak = vec![-1.0, 1.0, -1.0, 1.0];
        let strong = vec![-10.0, 10.0, -10.0, 10.0];
        let section = vec![weak.clone(), strong.clone()];

        // Per-trace peak normalization scales both traces to [-1, 1].
        let per_trace_params = AmplitudeParams::default()
            .with_scope(NormScope::PerTrace)
            .with_clip(ClipStrategy::Peak);
        let per_trace_out = process_section(&section, &per_trace_params);
        assert_eq!(per_trace_out[0], vec![-1.0, 1.0, -1.0, 1.0]);
        assert_eq!(per_trace_out[1], vec![-1.0, 1.0, -1.0, 1.0]);

        // Global peak normalization preserves the 1:10 amplitude ratio.
        let global_params = AmplitudeParams::default()
            .with_scope(NormScope::Global)
            .with_clip(ClipStrategy::Peak);
        let global_out = process_section(&section, &global_params);
        for (&got, &want) in global_out[0].iter().zip(&[-0.1, 0.1, -0.1, 0.1]) {
            assert!((got - want).abs() < 1e-6);
        }
        assert_eq!(global_out[1], vec![-1.0, 1.0, -1.0, 1.0]);
    }

    #[test]
    fn gain_polarity_and_hard_clamp_pipeline() {
        let trace = vec![-4.0, -2.0, 0.0, 2.0, 4.0];
        // Absolute clip at 4.0, +6.0206 dB (~2x gain), reversed polarity, clamp at 1.0:
        // -4.0 -> * (-2 / 4) = +2.0 -> clamped to +1.0
        // -2.0 -> * (-2 / 4) = +1.0 -> +1.0
        // +2.0 -> * (-2 / 4) = -1.0 -> -1.0
        let params = AmplitudeParams::default()
            .with_scope(NormScope::PerTrace)
            .with_clip(ClipStrategy::Absolute { limit: 4.0 })
            .with_gain_db(6.020_6)
            .with_polarity_flip(true)
            .with_clamp_limit(Some(1.0));

        let out = process_trace(&trace, &params, None);
        let expected = [1.0, 1.0, 0.0, -1.0, -1.0];
        for (got, want) in out.iter().zip(expected.iter()) {
            assert!((got - want).abs() < 1e-5, "got {got}, want {want}");
        }
    }

    #[test]
    fn sliding_window_agc_equalizes_decaying_trace() {
        // Construct a signal whose first half has amplitude 1.0 and second half has amplitude 100.0.
        let mut trace = Vec::with_capacity(200);
        for i in 0..100 {
            trace.push(if i % 2 == 0 { 1.0 } else { -1.0 });
        }
        for i in 0..100 {
            trace.push(if i % 2 == 0 { 100.0 } else { -100.0 });
        }

        let params = AmplitudeParams::default()
            .with_scope(NormScope::PerTrace)
            .with_clip(ClipStrategy::Rms { multiple: 2.0 })
            .with_agc(AgcConfig::enabled(20));

        let out = process_trace(&trace, &params, None);
        // Away from the transition at index 100, both halves have identical local RMS (1.0 after AGC)
        // and therefore equal normalized magnitudes (~0.5 with Rms { multiple: 2.0 }).
        let early_rms = trace_rms(&out[10..80]);
        let late_rms = trace_rms(&out[120..190]);
        assert!(
            (early_rms - late_rms).abs() < 0.02,
            "early_rms={early_rms}, late_rms={late_rms}"
        );
    }

    #[test]
    fn trace_balance_equalizes_traces_before_global_clip() {
        let trace_a = vec![-2.0, 2.0, -2.0, 2.0];
        let trace_b = vec![-50.0, 50.0, -50.0, 50.0];
        let section = vec![trace_a, trace_b];

        let params = AmplitudeParams::default()
            .with_scope(NormScope::Global)
            .with_trace_balance(TraceBalance::Rms)
            .with_clip(ClipStrategy::Rms { multiple: 1.0 });

        let out = process_section(&section, &params);
        assert_eq!(out[0], vec![-1.0, 1.0, -1.0, 1.0]);
        assert_eq!(out[1], vec![-1.0, 1.0, -1.0, 1.0]);
    }

    #[test]
    fn hostile_empty_zero_and_extreme_values_never_panic_or_nan() {
        let params = AmplitudeParams::default()
            .with_dc_removal(true)
            .with_agc(AgcConfig::enabled(0))
            .with_trace_balance(TraceBalance::Rms);

        assert!(process_trace(&[], &params, None).is_empty());

        let zeros = vec![0.0; 32];
        let out_zeros = process_trace(&zeros, &params, None);
        assert!(out_zeros.iter().all(|&v| v == 0.0));

        let extreme = vec![
            f32::MAX,
            -f32::MAX,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
        ];
        let out_extreme = process_trace(&extreme, &params, None);
        assert_eq!(out_extreme.len(), extreme.len());
        assert!(
            out_extreme
                .iter()
                .all(|v| v.is_finite() && (-1.0..=1.0).contains(v))
        );
    }
}
