//! Variable-density (image) seismic renderer:
//! - Precomputed 256/4096-entry colormap LUTs
//! - Rayon column-parallel trace decoding, amplitude processing, and resampling
//! - Min/max envelope decimation when zoomed out (horizontally and/or vertically)
//! - 2-D bilinear interpolation when zoomed in
//! - Viewport-sized output buffer only (never allocates a full `traces × samples` image)

use std::ops::Range;

use rayon::prelude::*;
use segy_core::SegyFile;

use crate::amplitude::{
    AmplitudeParams, AmplitudeScratch, NormScope, compute_global_reference,
    process_trace_with_scratch,
};
use crate::colormap::{ColormapConfig, ColormapLut};
use crate::error::{RenderError, Result};

/// Maximum allowed viewport width or height in pixels (`16384 × 16384`).
pub const MAX_VIEWPORT_DIMENSION: u32 = 16_384;

/// Maximum number of evenly-spaced traces sampled when estimating a global reference amplitude
/// across a large file without a precomputed `global_reference`.
const MAX_GLOBAL_STAT_TRACES: usize = 128;

/// Abstract read-only source of seismic traces for headless rendering.
pub trait TraceDataSource: Sync {
    /// Total number of traces available in the dataset.
    fn trace_count(&self) -> usize;

    /// Nominal number of samples per trace.
    fn sample_count(&self) -> usize;

    /// Reads and decodes `sample_range` (`start..end`) of `trace_index` into `out`.
    fn read_trace_window(
        &self,
        trace_index: usize,
        sample_range: Range<usize>,
        out: &mut Vec<f32>,
    ) -> Result<()>;
}

impl TraceDataSource for SegyFile {
    #[inline]
    fn trace_count(&self) -> usize {
        Self::trace_count(self)
    }

    #[inline]
    fn sample_count(&self) -> usize {
        usize::from(self.samples_per_trace())
    }

    #[inline]
    fn read_trace_window(
        &self,
        trace_index: usize,
        sample_range: Range<usize>,
        out: &mut Vec<f32>,
    ) -> Result<()> {
        let trace_ns = self.trace_sample_count(trace_index)?;
        if sample_range.start >= trace_ns {
            out.clear();
            out.resize(sample_range.end.saturating_sub(sample_range.start), 0.0);
            return Ok(());
        }
        let clamped_end = sample_range.end.min(trace_ns);
        self.read_trace_sample_window_into(trace_index, sample_range.start..clamped_end, out)?;
        let requested_len = sample_range.end.saturating_sub(sample_range.start);
        if out.len() < requested_len {
            out.resize(requested_len, 0.0);
        }
        Ok(())
    }
}

/// In-memory slice adapter implementing [`TraceDataSource`] for synthetic tests and benchmarks.
#[derive(Debug, Clone, Copy)]
pub struct SliceTraceSource<'a, S: AsRef<[f32]> + Sync> {
    traces: &'a [S],
    samples_per_trace: usize,
}

impl<'a, S: AsRef<[f32]> + Sync> SliceTraceSource<'a, S> {
    /// Wraps a slice of traces (e.g. `&[Vec<f32>]` or `&[&[f32]]`).
    pub fn new(traces: &'a [S]) -> Self {
        let samples_per_trace = traces.first().map_or(0, |t| t.as_ref().len());
        Self {
            traces,
            samples_per_trace,
        }
    }
}

impl<S: AsRef<[f32]> + Sync> TraceDataSource for SliceTraceSource<'_, S> {
    #[inline]
    fn trace_count(&self) -> usize {
        self.traces.len()
    }

    #[inline]
    fn sample_count(&self) -> usize {
        self.samples_per_trace
    }

    #[inline]
    fn read_trace_window(
        &self,
        trace_index: usize,
        sample_range: Range<usize>,
        out: &mut Vec<f32>,
    ) -> Result<()> {
        let trace = self
            .traces
            .get(trace_index)
            .ok_or(RenderError::InvalidTraceRange {
                start: trace_index,
                end: trace_index.saturating_add(1),
                total_traces: self.traces.len(),
            })?
            .as_ref();
        if sample_range.start > sample_range.end || sample_range.end > trace.len() {
            return Err(RenderError::InvalidSampleRange {
                start: sample_range.start,
                end: sample_range.end,
                total_samples: trace.len(),
            });
        }
        out.clear();
        out.extend_from_slice(&trace[sample_range]);
        Ok(())
    }
}

/// Envelope reduction rule used when multiple samples or traces fall into a single pixel (`zoomed out`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DecimationMode {
    /// Picks the extremum (`min` or `max`) with the larger absolute magnitude (`|max| >= |min| ? max : min`).
    /// Preserves both strong positive peaks and strong negative troughs without zero-mean washout.
    #[default]
    DominantExtremum,
    /// When a pixel bin straddles zero (`min < 0 && max > 0`), alternates `max` on even pixel rows
    /// and `min` on odd pixel rows so high-frequency wavelets retain both peak and trough bands.
    MinMaxParity,
}

/// Viewport dimensions and visible seismic data window (`trace_range × sample_range`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageViewport {
    /// Viewport width in pixels (`1..=MAX_VIEWPORT_DIMENSION`).
    pub width: u32,
    /// Viewport height in pixels (`1..=MAX_VIEWPORT_DIMENSION`).
    pub height: u32,
    /// Visible 0-based trace range `start..end`.
    pub trace_range: Range<usize>,
    /// Visible 0-based sample range `start..end`.
    pub sample_range: Range<usize>,
    /// Reverse horizontal trace ordering (`true` = right-to-left).
    pub reverse_traces: bool,
}

impl ImageViewport {
    /// Creates a new [`ImageViewport`] for `width × height` pixels covering `trace_range × sample_range`.
    pub const fn new(
        width: u32,
        height: u32,
        trace_range: Range<usize>,
        sample_range: Range<usize>,
    ) -> Self {
        Self {
            width,
            height,
            trace_range,
            sample_range,
            reverse_traces: false,
        }
    }

    /// Sets whether the horizontal trace axis is reversed.
    #[inline]
    pub const fn with_reverse_traces(mut self, reverse_traces: bool) -> Self {
        self.reverse_traces = reverse_traces;
        self
    }

    /// Returns the required RGBA byte length (`width * height * 4`) for this viewport.
    #[inline]
    pub fn rgba_byte_len(&self) -> Result<usize> {
        self.validate_dimensions()?;
        let pixels = (self.width as usize).saturating_mul(self.height as usize);
        Ok(pixels.saturating_mul(4))
    }

    fn validate_dimensions(&self) -> Result<()> {
        if self.width == 0
            || self.height == 0
            || self.width > MAX_VIEWPORT_DIMENSION
            || self.height > MAX_VIEWPORT_DIMENSION
        {
            return Err(RenderError::InvalidViewport {
                width: self.width,
                height: self.height,
                max_dimension: MAX_VIEWPORT_DIMENSION,
            });
        }
        Ok(())
    }

    fn validate_against_source(&self, source: &dyn TraceDataSource) -> Result<()> {
        self.validate_dimensions()?;
        let total_traces = source.trace_count();
        if self.trace_range.start >= self.trace_range.end || self.trace_range.end > total_traces {
            return Err(RenderError::InvalidTraceRange {
                start: self.trace_range.start,
                end: self.trace_range.end,
                total_traces,
            });
        }
        let total_samples = source.sample_count();
        if self.sample_range.start >= self.sample_range.end || self.sample_range.end > total_samples
        {
            return Err(RenderError::InvalidSampleRange {
                start: self.sample_range.start,
                end: self.sample_range.end,
                total_samples,
            });
        }
        Ok(())
    }
}

/// Complete rendering parameters for variable-density image mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageRenderParams {
    /// Amplitude normalization, clipping, gain, and AGC settings.
    pub amplitude: AmplitudeParams,
    /// Colormap LUT configuration.
    pub colormap: ColormapConfig,
    /// Min/max envelope decimation rule when zoomed out.
    pub decimation: DecimationMode,
    /// Maximum number of traces decoded per pixel column when horizontally zoomed out (`>= 1`).
    pub max_traces_per_column: usize,
    /// Optional precomputed global reference amplitude (used when `amplitude.scope == NormScope::Global`).
    pub global_reference: Option<f32>,
}

impl Default for ImageRenderParams {
    fn default() -> Self {
        Self {
            amplitude: AmplitudeParams::default(),
            colormap: ColormapConfig::default(),
            decimation: DecimationMode::DominantExtremum,
            max_traces_per_column: 4,
            global_reference: None,
        }
    }
}

impl ImageRenderParams {
    /// Sets the amplitude pipeline configuration.
    #[inline]
    pub const fn with_amplitude(mut self, amplitude: AmplitudeParams) -> Self {
        self.amplitude = amplitude;
        self
    }

    /// Sets the colormap configuration.
    #[inline]
    pub const fn with_colormap(mut self, colormap: ColormapConfig) -> Self {
        self.colormap = colormap;
        self
    }

    /// Sets the zoomed-out envelope decimation mode.
    #[inline]
    pub const fn with_decimation(mut self, decimation: DecimationMode) -> Self {
        self.decimation = decimation;
        self
    }

    /// Sets the maximum number of traces decoded per pixel column when horizontally zoomed out.
    #[inline]
    pub const fn with_max_traces_per_column(mut self, max_traces_per_column: usize) -> Self {
        self.max_traces_per_column = if max_traces_per_column == 0 {
            1
        } else {
            max_traces_per_column
        };
        self
    }

    /// Sets a precomputed global reference amplitude.
    #[inline]
    pub const fn with_global_reference(mut self, global_reference: Option<f32>) -> Self {
        self.global_reference = global_reference;
        self
    }
}

/// Rendered viewport-sized RGBA image (`width * height * 4` bytes, row-major).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedImage {
    /// Viewport width in pixels.
    pub width: u32,
    /// Viewport height in pixels.
    pub height: u32,
    /// Row-major RGBA8 pixel buffer of exact length `width * height * 4`.
    pub rgba: Vec<u8>,
}

impl RenderedImage {
    /// Returns the `[R, G, B, A]` color at pixel coordinates `(x, y)`.
    #[inline]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let idx = ((y as usize) * (self.width as usize) + (x as usize)) * 4;
        let slice = self.rgba.get(idx..idx + 4)?;
        Some([slice[0], slice[1], slice[2], slice[3]])
    }
}

/// Computes the `(min, max)` amplitude envelope of `samples[start..end]`.
#[inline]
pub fn sample_bin_minmax(samples: &[f32], start: usize, end: usize) -> (f32, f32) {
    let Some(slice) = samples.get(start..end) else {
        return (0.0, 0.0);
    };
    let Some(&first) = slice.first() else {
        return (0.0, 0.0);
    };
    let mut min_v = first;
    let mut max_v = first;
    for &v in &slice[1..] {
        if v < min_v {
            min_v = v;
        }
        if v > max_v {
            max_v = v;
        }
    }
    (min_v, max_v)
}

/// Selects a single representative amplitude from a `(min_val, max_val)` pixel envelope.
#[inline]
pub fn select_decimated_amplitude(
    min_val: f32,
    max_val: f32,
    row_y: usize,
    mode: DecimationMode,
) -> f32 {
    match mode {
        DecimationMode::DominantExtremum => {
            if max_val.abs() >= min_val.abs() {
                max_val
            } else {
                min_val
            }
        }
        DecimationMode::MinMaxParity => {
            if min_val < 0.0 && max_val > 0.0 {
                if row_y.is_multiple_of(2) {
                    max_val
                } else {
                    min_val
                }
            } else if max_val.abs() >= min_val.abs() {
                max_val
            } else {
                min_val
            }
        }
    }
}

/// Vertically resamples a single processed trace (`trace_samples`) into `out_col` (length `height`):
/// - Uses min/max envelope decimation when `trace_samples.len() > height` (zoomed out vertically).
/// - Uses linear interpolation when `trace_samples.len() <= height` (zoomed in vertically).
pub fn resample_trace_vertical_into(
    trace_samples: &[f32],
    mode: DecimationMode,
    out_col: &mut [f32],
) {
    let height = out_col.len();
    let n = trace_samples.len();
    if height == 0 {
        return;
    }
    if n == 0 {
        out_col.fill(0.0);
        return;
    }
    if n == 1 {
        out_col.fill(trace_samples[0]);
        return;
    }

    if n > height {
        // Zoomed out vertically: min/max envelope decimation per vertical pixel bin.
        for (y, dst) in out_col.iter_mut().enumerate() {
            let s_start = (y * n) / height;
            let s_end = (((y + 1) * n) / height).max(s_start + 1).min(n);
            let (min_v, max_v) = sample_bin_minmax(trace_samples, s_start, s_end);
            *dst = select_decimated_amplitude(min_v, max_v, y, mode);
        }
    } else if n == height {
        out_col.copy_from_slice(trace_samples);
    } else {
        // Zoomed in vertically: linear interpolation between adjacent samples.
        let denom = (height - 1).max(1) as f32;
        let max_sample_idx = (n - 1) as f32;
        for (y, dst) in out_col.iter_mut().enumerate() {
            let pos = (y as f32 / denom) * max_sample_idx;
            let s0 = (pos.floor() as usize).min(n - 1);
            let s1 = (s0 + 1).min(n - 1);
            let ty = (pos - s0 as f32).clamp(0.0, 1.0);
            let v0 = trace_samples[s0];
            let v1 = trace_samples[s1];
            *dst = v0 + (v1 - v0) * ty;
        }
    }
}

#[derive(Default)]
struct ColumnWorkerScratch {
    trace_buf: Vec<f32>,
    amp_scratch: AmplitudeScratch,
    min_env: Vec<f32>,
    max_env: Vec<f32>,
}

/// Stateful variable-density image renderer that reuses viewport-sized scratch buffers across frames.
#[derive(Debug, Default, Clone)]
pub struct ImageRenderer {
    /// Column-major 12-bit LUT index buffer (`width * height` entries, never larger than viewport).
    col_lut: Vec<u16>,
    /// Vertically-resampled trace buffer used only when `visible_traces < width`
    /// (`visible_traces * height` floats, strictly smaller than `width * height`).
    zoomed_in_cols: Vec<f32>,
}

impl ImageRenderer {
    /// Creates a new [`ImageRenderer`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Renders `viewport` from `source` into a newly allocated viewport-sized [`RenderedImage`].
    pub fn render(
        &mut self,
        source: &dyn TraceDataSource,
        viewport: &ImageViewport,
        params: &ImageRenderParams,
    ) -> Result<RenderedImage> {
        let byte_len = viewport.rgba_byte_len()?;
        let mut rgba = vec![0u8; byte_len];
        self.render_into(source, viewport, params, &mut rgba)?;
        Ok(RenderedImage {
            width: viewport.width,
            height: viewport.height,
            rgba,
        })
    }

    /// Renders `viewport` from `source` directly into caller-supplied row-major `out_rgba`
    /// (`width * height * 4` bytes).
    pub fn render_into(
        &mut self,
        source: &dyn TraceDataSource,
        viewport: &ImageViewport,
        params: &ImageRenderParams,
        out_rgba: &mut [u8],
    ) -> Result<()> {
        viewport.validate_against_source(source)?;
        let expected_bytes = viewport.rgba_byte_len()?;
        if out_rgba.len() != expected_bytes {
            return Err(RenderError::BufferSizeMismatch {
                actual_bytes: out_rgba.len(),
                expected_bytes,
                width: viewport.width,
                height: viewport.height,
            });
        }

        let width = viewport.width as usize;
        let height = viewport.height as usize;
        let visible_traces = viewport.trace_range.end - viewport.trace_range.start;
        let visible_samples = viewport.sample_range.end - viewport.sample_range.start;

        let lut = ColormapLut::from_config(params.colormap);
        let global_ref = resolve_global_reference(source, viewport, params)?;

        self.col_lut.resize(width * height, 0);

        if visible_traces >= width {
            // Zoomed out or 1:1 horizontally: each pixel column `x` owns a disjoint trace range `[t_start..t_end)`.
            let max_traces_per_col = params.max_traces_per_column.max(1);
            let decimation = params.decimation;
            let amp_params = params.amplitude;
            let sample_range = viewport.sample_range.clone();
            let trace_start = viewport.trace_range.start;
            let trace_end = viewport.trace_range.end;
            let reverse_traces = viewport.reverse_traces;

            self.col_lut
                .par_chunks_exact_mut(height)
                .enumerate()
                .for_each_init(
                    ColumnWorkerScratch::default,
                    |scratch, (col_x, col_lut_slice)| {
                        let mapped_x = if reverse_traces {
                            width - 1 - col_x
                        } else {
                            col_x
                        };
                        let t_start = trace_start + (mapped_x * visible_traces) / width;
                        let t_end = (trace_start + ((mapped_x + 1) * visible_traces) / width)
                            .max(t_start + 1)
                            .min(trace_end);
                        let bin_traces = t_end - t_start;
                        let trace_step = (bin_traces / max_traces_per_col).max(1);

                        scratch.min_env.clear();
                        scratch.min_env.resize(height, f32::INFINITY);
                        scratch.max_env.clear();
                        scratch.max_env.resize(height, f32::NEG_INFINITY);

                        let mut decoded_any = false;
                        let mut t_idx = t_start;
                        let mut count = 0_usize;
                        while t_idx < t_end && count < max_traces_per_col {
                            if source
                                .read_trace_window(
                                    t_idx,
                                    sample_range.clone(),
                                    &mut scratch.trace_buf,
                                )
                                .is_ok()
                            {
                                process_trace_with_scratch(
                                    &mut scratch.trace_buf,
                                    &amp_params,
                                    global_ref,
                                    &mut scratch.amp_scratch,
                                );
                                accumulate_vertical_envelope(
                                    &scratch.trace_buf,
                                    visible_samples,
                                    height,
                                    &mut scratch.min_env,
                                    &mut scratch.max_env,
                                );
                                decoded_any = true;
                            }
                            t_idx = t_idx.saturating_add(trace_step);
                            count = count.saturating_add(1);
                        }

                        if decoded_any {
                            for (y, dst) in col_lut_slice.iter_mut().enumerate() {
                                let amp = select_decimated_amplitude(
                                    scratch.min_env[y],
                                    scratch.max_env[y],
                                    y,
                                    decimation,
                                );
                                *dst = ColormapLut::quantize_4096(amp);
                            }
                        } else {
                            col_lut_slice.fill(ColormapLut::quantize_4096(0.0));
                        }
                    },
                );
        } else {
            // Zoomed in horizontally (`visible_traces < width`):
            // 1. Decode and vertically resample each visible trace once in parallel
            //    (`visible_traces * height` floats < `width * height` viewport pixels).
            self.zoomed_in_cols.resize(visible_traces * height, 0.0);
            let decimation = params.decimation;
            let amp_params = params.amplitude;
            let sample_range = viewport.sample_range.clone();
            let trace_start = viewport.trace_range.start;

            self.zoomed_in_cols
                .par_chunks_exact_mut(height)
                .enumerate()
                .for_each_init(
                    ColumnWorkerScratch::default,
                    |scratch, (local_t, trace_col)| {
                        let trace_idx = trace_start + local_t;
                        if source
                            .read_trace_window(
                                trace_idx,
                                sample_range.clone(),
                                &mut scratch.trace_buf,
                            )
                            .is_ok()
                        {
                            process_trace_with_scratch(
                                &mut scratch.trace_buf,
                                &amp_params,
                                global_ref,
                                &mut scratch.amp_scratch,
                            );
                            resample_trace_vertical_into(&scratch.trace_buf, decimation, trace_col);
                        } else {
                            trace_col.fill(0.0);
                        }
                    },
                );

            // 2. Horizontally interpolate across the `width` columns in parallel (yielding full 2-D bilinear
            //    interpolation when also zoomed in vertically).
            let zoomed_cols = self.zoomed_in_cols.as_slice();
            let reverse_traces = viewport.reverse_traces;
            let denom = (width - 1).max(1) as f32;
            let max_trace_idx = (visible_traces - 1) as f32;

            self.col_lut
                .par_chunks_exact_mut(height)
                .enumerate()
                .for_each(|(col_x, col_lut_slice)| {
                    let mapped_x = if reverse_traces {
                        width - 1 - col_x
                    } else {
                        col_x
                    };
                    let pos = if visible_traces <= 1 {
                        0.0
                    } else {
                        (mapped_x as f32 / denom) * max_trace_idx
                    };
                    let idx0 = (pos.floor() as usize).min(visible_traces - 1);
                    let idx1 = (idx0 + 1).min(visible_traces - 1);
                    let tx = (pos - idx0 as f32).clamp(0.0, 1.0);

                    let col0 = &zoomed_cols[idx0 * height..(idx0 + 1) * height];
                    let col1 = &zoomed_cols[idx1 * height..(idx1 + 1) * height];

                    for y in 0..height {
                        let v0 = col0[y];
                        let v1 = col1[y];
                        let interp = v0 + (v1 - v0) * tx;
                        col_lut_slice[y] = ColormapLut::quantize_4096(interp);
                    }
                });
        }

        // Convert column-major 12-bit LUT indices into row-major RGBA8 pixels in parallel.
        let col_lut_ref = self.col_lut.as_slice();
        out_rgba
            .par_chunks_exact_mut(width * 4)
            .enumerate()
            .for_each(|(y, row_bytes)| {
                for (x, pixel_out) in row_bytes.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    let lut_idx = col_lut_ref[x * height + y];
                    *pixel_out = lut.sample_index_4096(lut_idx);
                }
            });

        Ok(())
    }
}

/// Stateless convenience helper that renders `viewport` from `source` into a [`RenderedImage`].
pub fn render_image(
    source: &dyn TraceDataSource,
    viewport: &ImageViewport,
    params: &ImageRenderParams,
) -> Result<RenderedImage> {
    let mut renderer = ImageRenderer::new();
    renderer.render(source, viewport, params)
}

/// Stateless convenience helper that renders `viewport` from `source` into `out_rgba`.
pub fn render_image_into(
    source: &dyn TraceDataSource,
    viewport: &ImageViewport,
    params: &ImageRenderParams,
    out_rgba: &mut [u8],
) -> Result<()> {
    let mut renderer = ImageRenderer::new();
    renderer.render_into(source, viewport, params, out_rgba)
}

#[inline]
fn accumulate_vertical_envelope(
    trace_samples: &[f32],
    visible_samples: usize,
    height: usize,
    min_env: &mut [f32],
    max_env: &mut [f32],
) {
    let n = trace_samples.len().min(visible_samples);
    if n == 0 || height == 0 {
        return;
    }
    if n > height {
        // Zoomed out vertically: min/max of each vertical sample bin.
        for y in 0..height {
            let s_start = (y * n) / height;
            let s_end = (((y + 1) * n) / height).max(s_start + 1).min(n);
            let (s_min, s_max) = sample_bin_minmax(trace_samples, s_start, s_end);
            if s_min < min_env[y] {
                min_env[y] = s_min;
            }
            if s_max > max_env[y] {
                max_env[y] = s_max;
            }
        }
    } else if n == height {
        for y in 0..height {
            let v = trace_samples[y];
            if v < min_env[y] {
                min_env[y] = v;
            }
            if v > max_env[y] {
                max_env[y] = v;
            }
        }
    } else {
        // Zoomed in vertically: linear sample interpolation at `y`.
        let denom = (height - 1).max(1) as f32;
        let max_sample_idx = (n - 1) as f32;
        for y in 0..height {
            let pos = (y as f32 / denom) * max_sample_idx;
            let s0 = (pos.floor() as usize).min(n - 1);
            let s1 = (s0 + 1).min(n - 1);
            let ty = (pos - s0 as f32).clamp(0.0, 1.0);
            let v = trace_samples[s0] + (trace_samples[s1] - trace_samples[s0]) * ty;
            if v < min_env[y] {
                min_env[y] = v;
            }
            if v > max_env[y] {
                max_env[y] = v;
            }
        }
    }
}

fn resolve_global_reference(
    source: &dyn TraceDataSource,
    viewport: &ImageViewport,
    params: &ImageRenderParams,
) -> Result<Option<f32>> {
    if params.amplitude.scope != NormScope::Global {
        return Ok(None);
    }
    if let Some(precomputed) = params
        .global_reference
        .filter(|r| r.is_finite() && *r > 0.0)
    {
        return Ok(Some(precomputed));
    }

    let visible_traces = viewport.trace_range.end - viewport.trace_range.start;
    let step = (visible_traces / MAX_GLOBAL_STAT_TRACES).max(1);
    let sample_count = (visible_traces / step).clamp(1, MAX_GLOBAL_STAT_TRACES);

    let mut sampled_traces = Vec::with_capacity(sample_count);
    let mut t_idx = viewport.trace_range.start;
    while t_idx < viewport.trace_range.end && sampled_traces.len() < MAX_GLOBAL_STAT_TRACES {
        let mut buf = Vec::new();
        source.read_trace_window(t_idx, viewport.sample_range.clone(), &mut buf)?;
        sampled_traces.push(buf);
        t_idx = t_idx.saturating_add(step);
    }

    Ok(Some(compute_global_reference(
        &sampled_traces,
        &params.amplitude,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amplitude::ClipStrategy;
    use crate::colormap::ColormapKind;
    use std::path::Path;

    #[test]
    fn exact_1to1_viewport_maps_samples_to_expected_lut_colors() -> Result<()> {
        // 3 traces x 3 samples:
        // Trace 0: [-10.0, 0.0, 10.0]
        // Trace 1: [0.0, 10.0, -10.0]
        // Trace 2: [10.0, -10.0, 0.0]
        let traces = vec![
            vec![-10.0_f32, 0.0, 10.0],
            vec![0.0, 10.0, -10.0],
            vec![10.0, -10.0, 0.0],
        ];
        let src = SliceTraceSource::new(&traces);
        let viewport = ImageViewport::new(3, 3, 0..3, 0..3);
        let params = ImageRenderParams::default()
            .with_amplitude(
                AmplitudeParams::default()
                    .with_scope(NormScope::Global)
                    .with_clip(ClipStrategy::Peak),
            )
            .with_colormap(ColormapConfig::new(ColormapKind::SeismicRwb));

        let img = render_image(&src, &viewport, &params)?;
        assert_eq!(img.rgba.len(), 3 * 3 * 4);

        // Trace 0 (col 0): y=0 is -1.0 (blue), y=1 is 0.0 (white), y=2 is +1.0 (red)
        assert_eq!(img.pixel(0, 0), Some([0, 0, 255, 255]));
        assert_eq!(img.pixel(0, 1), Some([255, 255, 255, 255]));
        assert_eq!(img.pixel(0, 2), Some([255, 0, 0, 255]));

        // Trace 1 (col 1): y=0 is 0.0 (white), y=1 is +1.0 (red), y=2 is -1.0 (blue)
        assert_eq!(img.pixel(1, 0), Some([255, 255, 255, 255]));
        assert_eq!(img.pixel(1, 1), Some([255, 0, 0, 255]));
        assert_eq!(img.pixel(1, 2), Some([0, 0, 255, 255]));

        // Trace 2 (col 2): y=0 is +1.0 (red), y=1 is -1.0 (blue), y=2 is 0.0 (white)
        assert_eq!(img.pixel(2, 0), Some([255, 0, 0, 255]));
        assert_eq!(img.pixel(2, 1), Some([0, 0, 255, 255]));
        assert_eq!(img.pixel(2, 2), Some([255, 255, 255, 255]));
        Ok(())
    }

    #[test]
    fn zoomed_out_minmax_decimation_preserves_spikes_across_samples_and_traces() -> Result<()> {
        // 20 traces x 100 samples, all 0.0 except:
        // - A strong positive spike (+1.0) at trace 3, sample 17 (falls into col 0, row 0 of a 2x2 viewport)
        // - A strong negative spike (-1.0) at trace 14, sample 83 (falls into col 1, row 1 of a 2x2 viewport)
        let mut traces = vec![vec![0.0_f32; 100]; 20];
        traces[3][17] = 1.0;
        traces[14][83] = -1.0;

        let src = SliceTraceSource::new(&traces);
        let viewport = ImageViewport::new(2, 2, 0..20, 0..100);
        let params = ImageRenderParams::default()
            .with_max_traces_per_column(10)
            .with_amplitude(
                AmplitudeParams::default()
                    .with_scope(NormScope::Global)
                    .with_clip(ClipStrategy::Absolute { limit: 1.0 }),
            )
            .with_colormap(ColormapConfig::new(ColormapKind::Grayscale));

        let img = render_image(&src, &viewport, &params)?;
        // Col 0, Row 0 captured the +1.0 spike -> black [0, 0, 0, 255]
        assert_eq!(img.pixel(0, 0), Some([0, 0, 0, 255]));
        // Col 0, Row 1 is all zeros -> mid-gray [128, 128, 128, 255]
        assert_eq!(img.pixel(0, 1), Some([128, 128, 128, 255]));
        // Col 1, Row 0 is all zeros -> mid-gray [128, 128, 128, 255]
        assert_eq!(img.pixel(1, 0), Some([128, 128, 128, 255]));
        // Col 1, Row 1 captured the -1.0 spike -> white [255, 255, 255, 255]
        assert_eq!(img.pixel(1, 1), Some([255, 255, 255, 255]));
        Ok(())
    }

    #[test]
    fn zoomed_in_bilinear_interpolation_produces_smooth_midpoint() -> Result<()> {
        // 2 traces x 2 samples zoomed into a 3x3 pixel viewport:
        // Corners: (-1, -1) at top, (+1, +1) at bottom.
        // Center pixel (1, 1) must bilinearly interpolate to 0.0 -> mid-gray [128, 128, 128, 255].
        let traces = vec![vec![-1.0_f32, 1.0], vec![-1.0_f32, 1.0]];
        let src = SliceTraceSource::new(&traces);
        let viewport = ImageViewport::new(3, 3, 0..2, 0..2);
        let params = ImageRenderParams::default()
            .with_amplitude(
                AmplitudeParams::default()
                    .with_scope(NormScope::Global)
                    .with_clip(ClipStrategy::Absolute { limit: 1.0 }),
            )
            .with_colormap(ColormapConfig::new(ColormapKind::Grayscale));

        let img = render_image(&src, &viewport, &params)?;
        assert_eq!(img.pixel(0, 0), Some([255, 255, 255, 255]));
        assert_eq!(img.pixel(1, 1), Some([128, 128, 128, 255]));
        assert_eq!(img.pixel(2, 2), Some([0, 0, 0, 255]));
        Ok(())
    }

    #[test]
    fn viewport_buffer_never_exceeds_viewport_size_and_validates_bounds() -> Result<()> {
        let traces = vec![vec![0.5_f32; 500]; 200];
        let src = SliceTraceSource::new(&traces);
        let viewport = ImageViewport::new(64, 48, 0..200, 0..500);
        let params = ImageRenderParams::default();

        let mut renderer = ImageRenderer::new();
        let img = renderer.render(&src, &viewport, &params)?;
        assert_eq!(img.rgba.len(), 64 * 48 * 4);
        assert_eq!(renderer.col_lut.len(), 64 * 48);
        assert!(renderer.zoomed_in_cols.is_empty());

        // Invalid dimensions or mismatched output buffer return errors without panicking.
        assert!(matches!(
            render_image(&src, &ImageViewport::new(0, 48, 0..10, 0..10), &params),
            Err(RenderError::InvalidViewport { .. })
        ));
        let mut wrong_buf = vec![0u8; 16];
        assert!(matches!(
            render_image_into(&src, &viewport, &params, &mut wrong_buf),
            Err(RenderError::BufferSizeMismatch { .. })
        ));
        Ok(())
    }

    #[test]
    fn sample_segy_file_renders_cleanly_in_parallel() -> Result<()> {
        let sample_path = Path::new("/home/toto/Downloads/3X_75_PR.SGY");
        if !sample_path.exists() {
            return Ok(());
        }
        let file = SegyFile::open(sample_path)?;
        let viewport = ImageViewport::new(
            320,
            240,
            0..file.trace_count(),
            0..usize::from(file.samples_per_trace()),
        );
        let params = ImageRenderParams::default()
            .with_colormap(ColormapConfig::new(ColormapKind::SeismicRwb));

        let img = render_image(&file, &viewport, &params)?;
        assert_eq!(img.rgba.len(), 320 * 240 * 4);
        // Verify non-uniform seismic image output (not all one color).
        let first_px = img.pixel(0, 0);
        assert!(
            img.rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|&px| Some(px) != first_px)
        );
        Ok(())
    }
}
