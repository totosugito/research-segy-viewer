//! Seismic wiggle and variable-area (VA) trace renderer:
//! - Pure headless geometry output (`Point2f`, `WigglePolygon`, `TraceWiggleGeometry`, `WiggleSectionGeometry`)
//! - Positive, negative, both-lobe variable-area polygon fill, or line-only mode
//! - Exact linear zero-crossing interpolation along the trace baseline
//! - Configurable excursion scale (in trace spacings) and symmetric flat-top clipping (`±N` trace spacings)
//! - Vertical min/max envelope decimation to `<= 2` points per vertical pixel when dense
//! - Fast-path parallel geometry generation and direct scanline RGBA rasterization

use rayon::prelude::*;

use crate::amplitude::{
    AmplitudeParams, AmplitudeScratch, MIN_AMP_EPSILON, NormScope, compute_global_reference,
    process_trace_with_scratch, sanitize_sample,
};
use crate::error::{RenderError, Result};
use crate::image::{ImageViewport, RenderedImage, TraceDataSource};

/// Small amplitude threshold around zero used to classify baseline crossings cleanly.
const ZERO_CROSSING_EPSILON: f32 = 1e-6;

/// Maximum number of evenly-spaced traces sampled when estimating a global reference amplitude.
const MAX_GLOBAL_STAT_TRACES: usize = 128;

/// 2-D floating-point coordinate in viewport pixel space (`x` right, `y` down).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point2f {
    /// Horizontal pixel coordinate (`0.0..width`).
    pub x: f32,
    /// Vertical pixel coordinate (`0.0..height`).
    pub y: f32,
}

impl Point2f {
    /// Creates a new [`Point2f`].
    #[inline]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// Returns `true` if both `x` and `y` are finite numbers.
    #[inline]
    pub const fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

/// Polarity of a filled variable-area wiggle lobe relative to the trace baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LobePolarity {
    /// Positive amplitude lobe (`x > baseline_x` when excursion > 0).
    Positive,
    /// Negative amplitude lobe (`x < baseline_x` when excursion > 0).
    Negative,
}

/// Variable-area lobe fill mode for wiggle traces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WiggleFillMode {
    /// Draw only the wiggle trace polyline without filling any lobes.
    LineOnly,
    /// Fill positive lobes (`amplitude > 0`) and draw the wiggle polyline.
    #[default]
    Positive,
    /// Fill negative lobes (`amplitude < 0`) and draw the wiggle polyline.
    Negative,
    /// Fill both positive and negative lobes and draw the wiggle polyline.
    Both,
}

impl WiggleFillMode {
    /// Returns `true` if positive lobes should be filled.
    #[inline]
    pub const fn fills_positive(self) -> bool {
        matches!(self, Self::Positive | Self::Both)
    }

    /// Returns `true` if negative lobes should be filled.
    #[inline]
    pub const fn fills_negative(self) -> bool {
        matches!(self, Self::Negative | Self::Both)
    }

    /// Returns `true` if any lobe polarity is filled.
    #[inline]
    pub const fn has_fill(self) -> bool {
        !matches!(self, Self::LineOnly)
    }
}

/// Closed variable-area polygon representing a single positive or negative lobe cut at the baseline.
///
/// The polygon starts at `(baseline_x, y_start)`, follows one or more excursion points on a single
/// side of the baseline in monotonically non-decreasing `y` order, and closes at `(baseline_x, y_end)`.
#[derive(Debug, Clone, PartialEq)]
pub struct WigglePolygon {
    /// Whether this polygon represents a positive or negative amplitude lobe.
    pub lobe: LobePolarity,
    /// Closed polygon boundary vertices (`len >= 3`).
    pub points: Vec<Point2f>,
}

impl WigglePolygon {
    /// Computes the unsigned geometric area of the polygon in square pixels using the shoelace formula.
    pub fn area(&self) -> f32 {
        let n = self.points.len();
        if n < 3 {
            return 0.0;
        }
        let mut twice_area = 0.0_f64;
        for i in 0..n {
            let p0 = self.points[i];
            let p1 = self.points[(i + 1) % n];
            twice_area += f64::from(p0.x) * f64::from(p1.y) - f64::from(p1.x) * f64::from(p0.y);
        }
        (twice_area.abs() * 0.5) as f32
    }
}

/// Pure path and polygon geometry generated for a single seismic trace.
#[derive(Debug, Clone, PartialEq)]
pub struct TraceWiggleGeometry {
    /// 0-based trace index in the source dataset.
    pub trace_index: usize,
    /// Horizontal pixel coordinate of this trace's zero-amplitude baseline.
    pub baseline_x: f32,
    /// Continuous wiggle polyline points `(x, y)` ordered from top (`y = 0`) to bottom (`y = height - 1`).
    /// Guaranteed to contain at most `2 * height` points when vertical min/max decimation is active.
    pub polyline: Vec<Point2f>,
    /// Variable-area lobe polygons cut along `x = baseline_x` with interpolated zero crossings.
    pub fill_polygons: Vec<WigglePolygon>,
}

/// Pure path and polygon geometry for an entire viewport section of wiggle traces.
#[derive(Debug, Clone, PartialEq)]
pub struct WiggleSectionGeometry {
    /// Viewport width in pixels.
    pub width: u32,
    /// Viewport height in pixels.
    pub height: u32,
    /// Horizontal spacing in pixels between adjacent rendered trace baselines.
    pub trace_spacing_px: f32,
    /// Per-trace wiggle polylines and fill polygons.
    pub traces: Vec<TraceWiggleGeometry>,
}

impl WiggleSectionGeometry {
    /// Total number of polyline points across all rendered traces.
    pub fn total_polyline_points(&self) -> usize {
        self.traces.iter().map(|t| t.polyline.len()).sum()
    }

    /// Total number of filled lobe polygons across all rendered traces.
    pub fn total_fill_polygons(&self) -> usize {
        self.traces.iter().map(|t| t.fill_polygons.len()).sum()
    }

    /// Total number of polygon boundary vertices across all rendered traces.
    pub fn total_polygon_vertices(&self) -> usize {
        self.traces
            .iter()
            .flat_map(|t| t.fill_polygons.iter())
            .map(|p| p.points.len())
            .sum()
    }
}

/// Complete configuration for wiggle and variable-area trace rendering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WiggleRenderParams {
    /// Amplitude normalization, clipping, gain, and AGC settings.
    pub amplitude: AmplitudeParams,
    /// Variable-area fill mode (`LineOnly`, `Positive`, `Negative`, or `Both`).
    pub fill_mode: WiggleFillMode,
    /// Whether to stroke the continuous wiggle trace line (`true` by default).
    pub draw_line: bool,
    /// Excursion scale in trace spacings per unit (`1.0`) normalized amplitude.
    pub excursion_spacings: f32,
    /// Symmetric clipping limit in trace spacings (`±clip_spacings`).
    pub clip_spacings: f32,
    /// Maximum vertical polyline points emitted per pixel row when dense (`1` or `2`; default `2`).
    pub max_points_per_pixel: u8,
    /// Optional cap on the number of traces rendered across the viewport when zoomed out.
    /// When `None`, defaults to at most `viewport.width` traces.
    pub max_rendered_traces: Option<usize>,
    /// Background RGBA8 color used when rasterizing standalone wiggle images.
    pub background_rgba: [u8; 4],
    /// Wiggle stroke line RGBA8 color.
    pub line_rgba: [u8; 4],
    /// Positive lobe fill RGBA8 color.
    pub positive_fill_rgba: [u8; 4],
    /// Negative lobe fill RGBA8 color.
    pub negative_fill_rgba: [u8; 4],
    /// Optional precomputed global reference amplitude (used when `amplitude.scope == NormScope::Global`).
    pub global_reference: Option<f32>,
}

impl Default for WiggleRenderParams {
    fn default() -> Self {
        Self {
            amplitude: AmplitudeParams::default(),
            fill_mode: WiggleFillMode::Positive,
            draw_line: true,
            excursion_spacings: 1.0,
            clip_spacings: 1.5,
            max_points_per_pixel: 2,
            max_rendered_traces: None,
            background_rgba: [255, 255, 255, 255],
            line_rgba: [0, 0, 0, 255],
            positive_fill_rgba: [0, 0, 0, 255],
            negative_fill_rgba: [30, 100, 200, 255],
            global_reference: None,
        }
    }
}

impl WiggleRenderParams {
    /// Sets the amplitude pipeline configuration.
    #[inline]
    pub const fn with_amplitude(mut self, amplitude: AmplitudeParams) -> Self {
        self.amplitude = amplitude;
        self
    }

    /// Sets the variable-area lobe fill mode.
    #[inline]
    pub const fn with_fill_mode(mut self, fill_mode: WiggleFillMode) -> Self {
        self.fill_mode = fill_mode;
        self
    }

    /// Sets whether the wiggle trace line is drawn.
    #[inline]
    pub const fn with_draw_line(mut self, draw_line: bool) -> Self {
        self.draw_line = draw_line;
        self
    }

    /// Sets the excursion scale in trace spacings per unit normalized amplitude.
    #[inline]
    pub const fn with_excursion_spacings(mut self, excursion_spacings: f32) -> Self {
        self.excursion_spacings = excursion_spacings;
        self
    }

    /// Sets the symmetric hard excursion clip in trace spacings (`±clip_spacings`).
    #[inline]
    pub const fn with_clip_spacings(mut self, clip_spacings: f32) -> Self {
        self.clip_spacings = clip_spacings;
        self
    }

    /// Sets the vertical decimation density cap (`1` or `2` points per vertical pixel).
    #[inline]
    pub const fn with_max_points_per_pixel(mut self, max_points_per_pixel: u8) -> Self {
        self.max_points_per_pixel = if max_points_per_pixel <= 1 { 1 } else { 2 };
        self
    }

    /// Sets an explicit cap on the number of traces rendered across the viewport.
    #[inline]
    pub const fn with_max_rendered_traces(mut self, max_rendered_traces: Option<usize>) -> Self {
        self.max_rendered_traces = max_rendered_traces;
        self
    }

    /// Sets the background, line, positive-lobe, and negative-lobe RGBA8 colors.
    #[inline]
    pub const fn with_colors(
        mut self,
        background_rgba: [u8; 4],
        line_rgba: [u8; 4],
        positive_fill_rgba: [u8; 4],
        negative_fill_rgba: [u8; 4],
    ) -> Self {
        self.background_rgba = background_rgba;
        self.line_rgba = line_rgba;
        self.positive_fill_rgba = positive_fill_rgba;
        self.negative_fill_rgba = negative_fill_rgba;
        self
    }

    /// Sets a precomputed global reference amplitude.
    #[inline]
    pub const fn with_global_reference(mut self, global_reference: Option<f32>) -> Self {
        self.global_reference = global_reference;
        self
    }

    #[inline]
    fn sanitized_excursion(&self) -> f32 {
        if self.excursion_spacings.is_finite() && self.excursion_spacings > 0.0 {
            self.excursion_spacings
        } else {
            0.0
        }
    }

    #[inline]
    fn sanitized_clip(&self) -> f32 {
        if self.clip_spacings.is_finite() && self.clip_spacings > 0.0 {
            self.clip_spacings
        } else {
            0.0
        }
    }
}

/// Computes the linearly interpolated vertical coordinate `y` where the segment from
/// `(d0, y0)` to `(d1, y1)` crosses the zero baseline (`d = 0`).
#[inline]
pub fn interpolate_zero_crossing_y(y0: f32, d0: f32, y1: f32, d1: f32) -> f32 {
    let denom = d1 - d0;
    if !denom.is_finite() || denom.abs() <= MIN_AMP_EPSILON {
        return 0.5 * (y0 + y1);
    }
    let t = (-d0 / denom).clamp(0.0, 1.0);
    y0 + t * (y1 - y0)
}

/// Maps a 0-based sample index `sample_idx` in `0..sample_count` to a vertical pixel coordinate in
/// `0.0..=(height - 1) as f32`.
#[inline]
pub fn sample_index_to_pixel_y(sample_idx: usize, sample_count: usize, height: u32) -> f32 {
    if height <= 1 || sample_count <= 1 {
        return 0.0;
    }
    let max_y = (height - 1) as f32;
    let max_idx = (sample_count - 1) as f32;
    ((sample_idx.min(sample_count - 1) as f32) / max_idx) * max_y
}

/// Decimates `samples` vertically so that at most `max_points_per_pixel` (`1` or `2`) points are
/// emitted per vertical pixel row when `samples.len() > height * max_points_per_pixel`.
///
/// Each output entry in `out_y_and_amp` is `(y_px, sample_amplitude)` ordered with strictly
/// non-decreasing `y_px`.
pub fn decimate_trace_vertical_into(
    samples: &[f32],
    height: u32,
    max_points_per_pixel: u8,
    out_y_and_amp: &mut Vec<(f32, f32)>,
) {
    out_y_and_amp.clear();
    let n = samples.len();
    if n == 0 || height == 0 {
        return;
    }

    let h = height as usize;
    let pts_per_px = usize::from(max_points_per_pixel.clamp(1, 2));
    let dense_threshold = h.saturating_mul(pts_per_px);

    if n <= dense_threshold {
        out_y_and_amp.reserve(n);
        for (idx, &amp) in samples.iter().enumerate() {
            let y_px = sample_index_to_pixel_y(idx, n, height);
            out_y_and_amp.push((y_px, sanitize_sample(amp)));
        }
        return;
    }

    out_y_and_amp.reserve(dense_threshold);
    for y in 0..h {
        let s_start = (y * n) / h;
        let s_end = (((y + 1) * n) / h).max(s_start + 1).min(n);
        let bin = &samples[s_start..s_end];

        let first_val = sanitize_sample(bin[0]);
        let mut min_idx = s_start;
        let mut min_val = first_val;
        let mut max_idx = s_start;
        let mut max_val = first_val;

        for (offset, &raw_v) in bin.iter().enumerate().skip(1) {
            let v = sanitize_sample(raw_v);
            let abs_idx = s_start + offset;
            if v < min_val {
                min_val = v;
                min_idx = abs_idx;
            }
            if v >= max_val {
                max_val = v;
                max_idx = abs_idx;
            }
        }

        if pts_per_px == 1 || min_idx == max_idx || (max_val - min_val).abs() <= MIN_AMP_EPSILON {
            // Emit 1 representative point for this pixel row.
            let chosen_idx = if y == 0 {
                0
            } else if y + 1 == h {
                n - 1
            } else if max_val.abs() >= min_val.abs() {
                max_idx
            } else {
                min_idx
            };
            let chosen_val = if max_val.abs() >= min_val.abs() {
                max_val
            } else {
                min_val
            };
            let y_px = sample_index_to_pixel_y(chosen_idx, n, height);
            out_y_and_amp.push((y_px, chosen_val));
        } else {
            // Emit both min and max in time (sample index) order so waveforms connect without reversal.
            let (idx0, val0, idx1, val1) = if min_idx < max_idx {
                (min_idx, min_val, max_idx, max_val)
            } else {
                (max_idx, max_val, min_idx, min_val)
            };
            let y0 = sample_index_to_pixel_y(idx0, n, height);
            let y1 = sample_index_to_pixel_y(idx1, n, height);
            out_y_and_amp.push((y0, val0));
            out_y_and_amp.push((y1, val1));
        }
    }
}

/// Builds the wiggle polyline and variable-area fill polygons for a single already-normalized trace.
pub fn build_trace_wiggle(
    trace_index: usize,
    baseline_x: f32,
    trace_spacing_px: f32,
    height: u32,
    processed_samples: &[f32],
    params: &WiggleRenderParams,
) -> TraceWiggleGeometry {
    let mut y_and_amp = Vec::new();
    build_trace_wiggle_with_scratch(
        trace_index,
        baseline_x,
        trace_spacing_px,
        height,
        processed_samples,
        params,
        &mut y_and_amp,
    )
}

/// Builds the wiggle polyline and variable-area fill polygons for a single already-normalized trace,
/// reusing `y_and_amp_scratch`.
pub fn build_trace_wiggle_with_scratch(
    trace_index: usize,
    baseline_x: f32,
    trace_spacing_px: f32,
    height: u32,
    processed_samples: &[f32],
    params: &WiggleRenderParams,
    y_and_amp_scratch: &mut Vec<(f32, f32)>,
) -> TraceWiggleGeometry {
    decimate_trace_vertical_into(
        processed_samples,
        height,
        params.max_points_per_pixel,
        y_and_amp_scratch,
    );

    let excursion = params.sanitized_excursion();
    let clip_spacings = params.sanitized_clip();
    let spacing = if trace_spacing_px.is_finite() && trace_spacing_px > 0.0 {
        trace_spacing_px
    } else {
        1.0
    };

    let mut polyline = Vec::with_capacity(y_and_amp_scratch.len());
    for &(y_px, amp) in y_and_amp_scratch.iter() {
        let disp_spacings = (amp * excursion).clamp(-clip_spacings, clip_spacings);
        let x_px = baseline_x + disp_spacings * spacing;
        polyline.push(Point2f::new(x_px, y_px));
    }

    let fill_polygons = if params.fill_mode.has_fill() {
        extract_lobe_polygons(&polyline, baseline_x, params.fill_mode)
    } else {
        Vec::new()
    };

    TraceWiggleGeometry {
        trace_index,
        baseline_x,
        polyline,
        fill_polygons,
    }
}

#[inline]
fn classify_deflection(dx: f32) -> i8 {
    if dx > ZERO_CROSSING_EPSILON {
        1
    } else if dx < -ZERO_CROSSING_EPSILON {
        -1
    } else {
        0
    }
}

#[inline]
fn should_emit_lobe(sign: i8, fill_mode: WiggleFillMode) -> bool {
    (sign > 0 && fill_mode.fills_positive()) || (sign < 0 && fill_mode.fills_negative())
}

#[inline]
fn sign_to_polarity(sign: i8) -> LobePolarity {
    if sign >= 0 {
        LobePolarity::Positive
    } else {
        LobePolarity::Negative
    }
}

/// Extracts closed variable-area lobe polygons from `polyline` cut against `baseline_x`
/// with linearly interpolated zero crossings.
pub fn extract_lobe_polygons(
    polyline: &[Point2f],
    baseline_x: f32,
    fill_mode: WiggleFillMode,
) -> Vec<WigglePolygon> {
    if !fill_mode.has_fill() || polyline.len() < 2 {
        return Vec::new();
    }

    let mut polygons = Vec::new();
    let mut active_sign: i8 = 0;
    let mut active_points: Vec<Point2f> = Vec::new();

    let close_active = |active_sign: &mut i8,
                        active_points: &mut Vec<Point2f>,
                        close_y: f32,
                        polygons: &mut Vec<WigglePolygon>| {
        if *active_sign != 0 {
            if should_emit_lobe(*active_sign, fill_mode) && active_points.len() >= 2 {
                active_points.push(Point2f::new(baseline_x, close_y));
                let points = std::mem::take(active_points);
                polygons.push(WigglePolygon {
                    lobe: sign_to_polarity(*active_sign),
                    points,
                });
            } else {
                active_points.clear();
            }
            *active_sign = 0;
        }
    };

    let p0 = polyline[0];
    let d0 = p0.x - baseline_x;
    let s0 = classify_deflection(d0);
    if s0 != 0 {
        active_sign = s0;
        if should_emit_lobe(s0, fill_mode) {
            active_points.push(Point2f::new(baseline_x, p0.y));
            active_points.push(p0);
        }
    }

    for window in polyline.windows(2) {
        let prev = window[0];
        let curr = window[1];
        let d_prev = prev.x - baseline_x;
        let d_curr = curr.x - baseline_x;
        let s_prev = classify_deflection(d_prev);
        let s_curr = classify_deflection(d_curr);

        if s_curr == 0 {
            if active_sign != 0 {
                close_active(&mut active_sign, &mut active_points, curr.y, &mut polygons);
            }
        } else if active_sign == 0 {
            // Starting a new lobe from baseline.
            let start_y = if s_prev == 0 {
                prev.y
            } else {
                interpolate_zero_crossing_y(prev.y, d_prev, curr.y, d_curr)
            };
            active_sign = s_curr;
            if should_emit_lobe(s_curr, fill_mode) {
                active_points.push(Point2f::new(baseline_x, start_y));
                active_points.push(curr);
            }
        } else if s_curr == active_sign {
            // Continuing the same lobe.
            if should_emit_lobe(s_curr, fill_mode) {
                active_points.push(curr);
            }
        } else {
            // Sign flipped directly from positive to negative or negative to positive:
            // interpolate the exact zero crossing on the baseline.
            let y_cross = interpolate_zero_crossing_y(prev.y, d_prev, curr.y, d_curr);
            close_active(&mut active_sign, &mut active_points, y_cross, &mut polygons);

            active_sign = s_curr;
            if should_emit_lobe(s_curr, fill_mode) {
                active_points.push(Point2f::new(baseline_x, y_cross));
                active_points.push(curr);
            }
        }
    }

    if let Some(last) = polyline.last()
        && active_sign != 0
    {
        close_active(&mut active_sign, &mut active_points, last.y, &mut polygons);
    }

    polygons
}

#[derive(Default)]
struct WiggleWorkerScratch {
    trace_buf: Vec<f32>,
    stack_buf: Vec<f32>,
    amp_scratch: AmplitudeScratch,
    y_and_amp: Vec<(f32, f32)>,
}

/// Computes pure wiggle path and variable-area polygon geometry for `viewport` across `source`.
pub fn render_wiggle_geometry(
    source: &dyn TraceDataSource,
    viewport: &ImageViewport,
    params: &WiggleRenderParams,
) -> Result<WiggleSectionGeometry> {
    validate_viewport_against_source(viewport, source)?;

    let width = viewport.width;
    let height = viewport.height;
    let visible_traces = viewport.trace_range.end - viewport.trace_range.start;
    let max_traces = params.max_rendered_traces.unwrap_or(width as usize).max(1);
    let rendered_traces = visible_traces.min(max_traces);
    let trace_spacing_px = (width as f32) / (rendered_traces as f32);

    let global_ref = resolve_wiggle_global_reference(source, viewport, params)?;
    // Disable hard [-1, 1] pre-clamp inside `AmplitudeParams` when wiggle `clip_spacings` governs excursion clipping.
    let amp_params = params.amplitude.with_clamp_limit(None);
    let sample_range = viewport.sample_range.clone();
    let trace_start = viewport.trace_range.start;
    let trace_end = viewport.trace_range.end;
    let reverse_traces = viewport.reverse_traces;

    let traces: Vec<TraceWiggleGeometry> = (0..rendered_traces)
        .into_par_iter()
        .map_init(WiggleWorkerScratch::default, |scratch, slot_idx| {
            let mapped_slot = if reverse_traces {
                rendered_traces - 1 - slot_idx
            } else {
                slot_idx
            };
            let trace_index = if rendered_traces == visible_traces {
                trace_start + mapped_slot
            } else {
                trace_start + (mapped_slot * visible_traces) / rendered_traces
            };
            let baseline_x = ((slot_idx as f32) + 0.5) * trace_spacing_px;

            let read_ok = if rendered_traces < visible_traces {
                let bin_end = (trace_start
                    + ((mapped_slot + 1) * visible_traces) / rendered_traces)
                    .max(trace_index + 1)
                    .min(trace_end);
                read_stacked_trace_window(
                    source,
                    trace_index..bin_end,
                    sample_range.clone(),
                    height,
                    &mut scratch.trace_buf,
                    &mut scratch.stack_buf,
                )
            } else {
                source
                    .read_trace_window(trace_index, sample_range.clone(), &mut scratch.trace_buf)
                    .is_ok()
            };

            if read_ok {
                process_trace_with_scratch(
                    &mut scratch.trace_buf,
                    &amp_params,
                    global_ref,
                    &mut scratch.amp_scratch,
                );
            } else {
                scratch.trace_buf.clear();
            }

            build_trace_wiggle_with_scratch(
                trace_index,
                baseline_x,
                trace_spacing_px,
                height,
                &scratch.trace_buf,
                params,
                &mut scratch.y_and_amp,
            )
        })
        .collect();

    Ok(WiggleSectionGeometry {
        width,
        height,
        trace_spacing_px,
        traces,
    })
}

fn read_stacked_trace_window(
    source: &dyn TraceDataSource,
    trace_bin: std::ops::Range<usize>,
    sample_range: std::ops::Range<usize>,
    height: u32,
    out_trace: &mut Vec<f32>,
    tmp_trace: &mut Vec<f32>,
) -> bool {
    let bin_len = trace_bin.end.saturating_sub(trace_bin.start);
    // Stack at most 3 contiguous traces around the bin center so dipping reflectors never
    // suffer destructive phase cancellation across wide trace bins.
    let max_stack = bin_len.clamp(1, 3);
    let center = trace_bin.start + bin_len / 2;
    let start_trace = center.saturating_sub(max_stack / 2).max(trace_bin.start);
    let end_trace = (start_trace + max_stack).min(trace_bin.end);

    out_trace.clear();
    let mut stacked_count = 0_usize;
    for t in start_trace..end_trace {
        if source
            .read_trace_window(t, sample_range.clone(), tmp_trace)
            .is_ok()
            && !tmp_trace.is_empty()
        {
            if stacked_count == 0 {
                out_trace.resize(tmp_trace.len(), 0.0);
                for (dst, &src) in out_trace.iter_mut().zip(tmp_trace.iter()) {
                    *dst = sanitize_sample(src);
                }
            } else {
                for (dst, &src) in out_trace.iter_mut().zip(tmp_trace.iter()) {
                    *dst += sanitize_sample(src);
                }
            }
            stacked_count += 1;
        }
    }

    if stacked_count == 0 {
        return false;
    }
    if stacked_count > 1 {
        let inv = 1.0 / (stacked_count as f32);
        for v in out_trace.iter_mut() {
            *v *= inv;
        }
    }

    // Apply a light 3-point [0.25, 0.5, 0.25] anti-aliasing filter when vertical sample count
    // approaches or exceeds vertical pixel height during horizontal trace decimation.
    if out_trace.len() >= (height as usize).max(4) {
        tmp_trace.clear();
        tmp_trace.extend_from_slice(out_trace);
        let n = tmp_trace.len();
        for i in 1..(n - 1) {
            out_trace[i] = 0.25 * tmp_trace[i - 1] + 0.5 * tmp_trace[i] + 0.25 * tmp_trace[i + 1];
        }
    }

    true
}

/// Rasterizes precomputed [`WiggleSectionGeometry`] (variable-area lobes and wiggle polylines)
/// into a caller-supplied row-major RGBA8 pixel buffer (`width * height * 4` bytes).
///
/// When `clear_background` is `false`, the wiggle traces and lobes are composited on top of the
/// existing pixels in `out_rgba` (enabling single-buffer Wiggle + Variable-Density Image overlay).
pub fn rasterize_wiggle_geometry_into(
    geometry: &WiggleSectionGeometry,
    params: &WiggleRenderParams,
    clear_background: bool,
    out_rgba: &mut [u8],
) -> Result<()> {
    let width = geometry.width as usize;
    let height = geometry.height as usize;
    let expected_bytes = width.saturating_mul(height).saturating_mul(4);
    if out_rgba.len() != expected_bytes {
        return Err(RenderError::BufferSizeMismatch {
            actual_bytes: out_rgba.len(),
            expected_bytes,
            width: geometry.width,
            height: geometry.height,
        });
    }

    if clear_background {
        let bg = params.background_rgba;
        for px in out_rgba.as_chunks_mut::<4>().0 {
            *px = bg;
        }
    }

    let w_f32 = (width.saturating_sub(1)) as f32;
    let h_f32 = (height.saturating_sub(1)) as f32;

    // 1. Scanline-fill positive/negative variable-area lobes along each trace.
    if params.fill_mode.has_fill() {
        for trace in &geometry.traces {
            rasterize_trace_lobes(trace, w_f32, h_f32, width, height, params, out_rgba);
        }
    }

    // 2. Stroke the continuous wiggle polylines on top of the lobe fills.
    if params.draw_line {
        for trace in &geometry.traces {
            rasterize_trace_stroke(
                trace,
                geometry.trace_spacing_px,
                w_f32,
                h_f32,
                width,
                height,
                params.line_rgba,
                out_rgba,
            );
        }
    }

    Ok(())
}

/// Stateful wiggle renderer that reuses internal buffers across frames.
#[derive(Debug, Default, Clone)]
pub struct WiggleRenderer {
    last_geometry: Option<WiggleSectionGeometry>,
}

impl WiggleRenderer {
    /// Creates a new [`WiggleRenderer`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Computes and caches the [`WiggleSectionGeometry`] for `viewport`.
    pub fn compute_geometry(
        &mut self,
        source: &dyn TraceDataSource,
        viewport: &ImageViewport,
        params: &WiggleRenderParams,
    ) -> Result<&WiggleSectionGeometry> {
        let geom = render_wiggle_geometry(source, viewport, params)?;
        Ok(self.last_geometry.insert(geom))
    }

    /// Renders `viewport` as a standalone RGBA8 [`RenderedImage`].
    pub fn render_image(
        &mut self,
        source: &dyn TraceDataSource,
        viewport: &ImageViewport,
        params: &WiggleRenderParams,
    ) -> Result<RenderedImage> {
        let byte_len = viewport.rgba_byte_len()?;
        let mut rgba = vec![0u8; byte_len];
        self.render_into(source, viewport, params, true, &mut rgba)?;
        Ok(RenderedImage {
            width: viewport.width,
            height: viewport.height,
            rgba,
        })
    }

    /// Renders `viewport` directly into `out_rgba` (clearing to `background_rgba` when
    /// `clear_background` is `true`, or compositing over an existing image when `false`).
    pub fn render_into(
        &mut self,
        source: &dyn TraceDataSource,
        viewport: &ImageViewport,
        params: &WiggleRenderParams,
        clear_background: bool,
        out_rgba: &mut [u8],
    ) -> Result<()> {
        let geom = render_wiggle_geometry(source, viewport, params)?;
        rasterize_wiggle_geometry_into(&geom, params, clear_background, out_rgba)?;
        self.last_geometry = Some(geom);
        Ok(())
    }
}

/// Stateless helper that renders wiggle traces into a viewport-sized [`RenderedImage`].
pub fn render_wiggle_image(
    source: &dyn TraceDataSource,
    viewport: &ImageViewport,
    params: &WiggleRenderParams,
) -> Result<RenderedImage> {
    let mut renderer = WiggleRenderer::new();
    renderer.render_image(source, viewport, params)
}

/// Stateless helper that renders wiggle traces directly into `out_rgba`.
pub fn render_wiggle_into(
    source: &dyn TraceDataSource,
    viewport: &ImageViewport,
    params: &WiggleRenderParams,
    clear_background: bool,
    out_rgba: &mut [u8],
) -> Result<()> {
    let mut renderer = WiggleRenderer::new();
    renderer.render_into(source, viewport, params, clear_background, out_rgba)
}

/// Evaluates the interpolated `x` at `y_center` and the `[x_min, x_max]` envelope of a monotonic
/// polyline across the vertical pixel band `[y_lo, y_hi]`.
fn evaluate_polyline_band(
    pts: &[Point2f],
    seg_cursor: &mut usize,
    y_lo: f32,
    y_center: f32,
    y_hi: f32,
) -> (f32, f32, f32) {
    let last_idx = pts.len().saturating_sub(1);
    while *seg_cursor < last_idx && pts[*seg_cursor + 1].y < y_lo {
        *seg_cursor += 1;
    }

    let x_lo = sample_polyline_at_y(pts, *seg_cursor, y_lo);
    let x_center = sample_polyline_at_y(pts, *seg_cursor, y_center);
    let x_hi = sample_polyline_at_y(pts, *seg_cursor, y_hi);

    let mut x_min = x_lo.min(x_center).min(x_hi);
    let mut x_max = x_lo.max(x_center).max(x_hi);

    let mut idx = *seg_cursor;
    while idx <= last_idx && pts[idx].y <= y_hi {
        if pts[idx].y >= y_lo {
            x_min = x_min.min(pts[idx].x);
            x_max = x_max.max(pts[idx].x);
        }
        idx += 1;
    }

    (x_center, x_min, x_max)
}

#[inline]
fn sample_polyline_at_y(pts: &[Point2f], start_idx: usize, y_target: f32) -> f32 {
    if pts.is_empty() {
        return 0.0;
    }
    if y_target <= pts[0].y {
        return pts[0].x;
    }
    let last = pts.len() - 1;
    if y_target >= pts[last].y {
        return pts[last].x;
    }
    let mut i = start_idx.min(last.saturating_sub(1));
    while i + 1 < pts.len() && pts[i + 1].y < y_target {
        i += 1;
    }
    let p0 = pts[i];
    let p1 = pts[(i + 1).min(last)];
    let dy = p1.y - p0.y;
    if dy.abs() <= MIN_AMP_EPSILON {
        0.5 * (p0.x + p1.x)
    } else {
        let t = ((y_target - p0.y) / dy).clamp(0.0, 1.0);
        p0.x + t * (p1.x - p0.x)
    }
}

fn rasterize_trace_lobes(
    trace: &TraceWiggleGeometry,
    max_x: f32,
    max_y: f32,
    width: usize,
    height: usize,
    params: &WiggleRenderParams,
    out_rgba: &mut [u8],
) {
    let pts = &trace.polyline;
    if pts.len() < 2 || height == 0 || width == 0 {
        return;
    }
    let baseline_x = trace.baseline_x.clamp(0.0, max_x);
    let y_start = pts[0].y.clamp(0.0, max_y).floor() as usize;
    let y_end = pts[pts.len() - 1]
        .y
        .clamp(0.0, max_y)
        .ceil()
        .min((height - 1) as f32) as usize;

    let mut seg_cursor = 0_usize;
    for y in y_start..=y_end {
        let y_f = y as f32;
        let y_lo = (y_f - 0.5).max(0.0);
        let y_hi = (y_f + 0.5).min(max_y);
        let (x_center, x_min, x_max) =
            evaluate_polyline_band(pts, &mut seg_cursor, y_lo, y_f, y_hi);
        let dx_center = x_center - baseline_x;

        if dx_center > ZERO_CROSSING_EPSILON && params.fill_mode.fills_positive() {
            let x_right = x_max.max(x_center).clamp(baseline_x, max_x);
            fill_antialiased_span(
                out_rgba,
                width,
                height,
                y,
                baseline_x,
                x_right,
                params.positive_fill_rgba,
            );
        } else if dx_center < -ZERO_CROSSING_EPSILON && params.fill_mode.fills_negative() {
            let x_left = x_min.min(x_center).clamp(0.0, baseline_x);
            fill_antialiased_span(
                out_rgba,
                width,
                height,
                y,
                x_left,
                baseline_x,
                params.negative_fill_rgba,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn rasterize_trace_stroke(
    trace: &TraceWiggleGeometry,
    trace_spacing_px: f32,
    max_x: f32,
    max_y: f32,
    width: usize,
    height: usize,
    line_color: [u8; 4],
    out_rgba: &mut [u8],
) {
    let pts = &trace.polyline;
    if pts.is_empty() || height == 0 || width == 0 {
        return;
    }
    if pts.len() == 1 {
        let p = pts[0];
        plot_pixel(
            out_rgba,
            width,
            height,
            p.x.round() as i32,
            p.y.round() as i32,
            line_color,
        );
        return;
    }

    let baseline_x = trace.baseline_x.clamp(0.0, max_x);
    let density_alpha = if trace_spacing_px >= 14.0 {
        1.0
    } else {
        (trace_spacing_px / 14.0).clamp(0.42, 1.0)
    };

    let y_start = pts[0].y.clamp(0.0, max_y).floor() as usize;
    let y_end = pts[pts.len() - 1]
        .y
        .clamp(0.0, max_y)
        .ceil()
        .min((height - 1) as f32) as usize;

    let mut seg_cursor = 0_usize;
    for y in y_start..=y_end {
        let y_f = y as f32;
        let y_lo = (y_f - 0.5).max(0.0);
        let y_hi = (y_f + 0.5).min(max_y);
        let (x_center, x_min, x_max) =
            evaluate_polyline_band(pts, &mut seg_cursor, y_lo, y_f, y_hi);
        let x0 = x_min.clamp(0.0, max_x);
        let x1 = x_max.clamp(0.0, max_x);
        let span = (x1 - x0).max(0.0);
        let excursion = (x_center - baseline_x)
            .abs()
            .max((x1 - baseline_x).abs())
            .max((x0 - baseline_x).abs());

        let mute_factor = if trace_spacing_px < 14.0 && excursion < 0.35 {
            0.38
        } else {
            1.0
        };

        if span < 0.85 {
            let px = x_center.round() as i32;
            blend_pixel(
                out_rgba,
                width,
                height,
                px,
                y as i32,
                line_color,
                density_alpha * mute_factor,
            );
        } else {
            let slope_alpha =
                (density_alpha * mute_factor * (1.25 / (1.0 + 0.42 * span))).clamp(0.22, 1.0);
            let px_start = (x0.floor().max(0.0) as usize).min(width - 1);
            let px_end = (x1.ceil().max(0.0) as usize).min(width - 1);
            for px in px_start..=px_end {
                blend_pixel(
                    out_rgba,
                    width,
                    height,
                    px as i32,
                    y as i32,
                    line_color,
                    slope_alpha,
                );
            }
        }
    }
}

#[inline]
fn fill_antialiased_span(
    out_rgba: &mut [u8],
    width: usize,
    height: usize,
    y: usize,
    x_left: f32,
    x_right: f32,
    color: [u8; 4],
) {
    let span = x_right - x_left;
    if span <= 0.05 || width == 0 || y >= height {
        return;
    }
    let start_px = (x_left.floor().max(0.0) as usize).min(width - 1);
    let end_px = (x_right.floor().max(0.0) as usize).min(width - 1);
    if start_px == end_px {
        let cov = span.clamp(0.0, 1.0);
        blend_pixel(
            out_rgba,
            width,
            height,
            start_px as i32,
            y as i32,
            color,
            cov,
        );
        return;
    }

    let left_cov = (((start_px + 1) as f32) - x_left).clamp(0.0, 1.0);
    blend_pixel(
        out_rgba,
        width,
        height,
        start_px as i32,
        y as i32,
        color,
        left_cov,
    );

    if start_px < end_px.saturating_sub(1) {
        let row_offset = y * width * 4;
        let inner_start = start_px + 1;
        let inner_end = end_px - 1;
        let span_bytes =
            &mut out_rgba[row_offset + inner_start * 4..row_offset + (inner_end + 1) * 4];
        for px in span_bytes.as_chunks_mut::<4>().0 {
            *px = color;
        }
    }

    let right_cov = (x_right - (end_px as f32)).clamp(0.0, 1.0);
    if right_cov > 0.02 {
        blend_pixel(
            out_rgba,
            width,
            height,
            end_px as i32,
            y as i32,
            color,
            right_cov,
        );
    }
}

#[inline]
fn plot_pixel(out_rgba: &mut [u8], width: usize, height: usize, x: i32, y: i32, color: [u8; 4]) {
    if x < 0 || y < 0 {
        return;
    }
    let ux = x as usize;
    let uy = y as usize;
    if ux >= width || uy >= height {
        return;
    }
    let idx = (uy * width + ux) * 4;
    if let Some(dst) = out_rgba.get_mut(idx..idx + 4) {
        dst.copy_from_slice(&color);
    }
}

#[inline]
fn blend_pixel(
    out_rgba: &mut [u8],
    width: usize,
    height: usize,
    x: i32,
    y: i32,
    color: [u8; 4],
    alpha: f32,
) {
    if alpha >= 0.999 {
        plot_pixel(out_rgba, width, height, x, y, color);
        return;
    }
    if x < 0 || y < 0 {
        return;
    }
    let ux = x as usize;
    let uy = y as usize;
    if ux >= width || uy >= height {
        return;
    }
    let idx = (uy * width + ux) * 4;
    if let Some(dst) = out_rgba.get_mut(idx..idx + 4) {
        let a = alpha.clamp(0.0, 1.0);
        let inv_a = 1.0 - a;
        dst[0] = ((color[0] as f32) * a + (dst[0] as f32) * inv_a).round() as u8;
        dst[1] = ((color[1] as f32) * a + (dst[1] as f32) * inv_a).round() as u8;
        dst[2] = ((color[2] as f32) * a + (dst[2] as f32) * inv_a).round() as u8;
        dst[3] = 255;
    }
}

fn validate_viewport_against_source(
    viewport: &ImageViewport,
    source: &dyn TraceDataSource,
) -> Result<()> {
    let _ = viewport.rgba_byte_len()?;
    let total_traces = source.trace_count();
    if viewport.trace_range.start >= viewport.trace_range.end
        || viewport.trace_range.end > total_traces
    {
        return Err(RenderError::InvalidTraceRange {
            start: viewport.trace_range.start,
            end: viewport.trace_range.end,
            total_traces,
        });
    }
    let total_samples = source.sample_count();
    if viewport.sample_range.start >= viewport.sample_range.end
        || viewport.sample_range.end > total_samples
    {
        return Err(RenderError::InvalidSampleRange {
            start: viewport.sample_range.start,
            end: viewport.sample_range.end,
            total_samples,
        });
    }
    Ok(())
}

fn resolve_wiggle_global_reference(
    source: &dyn TraceDataSource,
    viewport: &ImageViewport,
    params: &WiggleRenderParams,
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
    use crate::image::SliceTraceSource;
    use segy_core::SegyFile;
    use std::path::Path;

    #[test]
    fn zero_crossing_interpolation_splits_positive_and_negative_lobes_accurately() {
        // Sample 0 at y = 0.0 has amplitude -1.0; Sample 1 at y = 10.0 has amplitude +3.0.
        // Zero crossing occurs at t = 1 / (1 + 3) = 0.25 => y = 2.5.
        let samples = [-1.0_f32, 3.0];
        let baseline_x = 50.0_f32;
        let spacing = 10.0_f32;
        let height = 11_u32; // y in 0.0..=10.0

        // 1. Positive fill only
        let pos_params = WiggleRenderParams::default()
            .with_fill_mode(WiggleFillMode::Positive)
            .with_excursion_spacings(1.0)
            .with_clip_spacings(5.0);
        let pos_geom = build_trace_wiggle(0, baseline_x, spacing, height, &samples, &pos_params);
        assert_eq!(pos_geom.polyline.len(), 2);
        assert_eq!(pos_geom.fill_polygons.len(), 1);
        let poly = &pos_geom.fill_polygons[0];
        assert_eq!(poly.lobe, LobePolarity::Positive);
        assert_eq!(
            poly.points,
            vec![
                Point2f::new(50.0, 2.5),
                Point2f::new(80.0, 10.0),
                Point2f::new(50.0, 10.0),
            ]
        );
        // Triangle base = 30 px, height = 7.5 px => area = 112.5
        assert!((poly.area() - 112.5).abs() < 1e-4);

        // 2. Negative fill only
        let neg_params = pos_params.with_fill_mode(WiggleFillMode::Negative);
        let neg_geom = build_trace_wiggle(0, baseline_x, spacing, height, &samples, &neg_params);
        assert_eq!(neg_geom.fill_polygons.len(), 1);
        let neg_poly = &neg_geom.fill_polygons[0];
        assert_eq!(neg_poly.lobe, LobePolarity::Negative);
        assert_eq!(
            neg_poly.points,
            vec![
                Point2f::new(50.0, 0.0),
                Point2f::new(40.0, 0.0),
                Point2f::new(50.0, 2.5),
            ]
        );
        // Triangle base = 10 px, height = 2.5 px => area = 12.5
        assert!((neg_poly.area() - 12.5).abs() < 1e-4);

        // 3. Both positive and negative fill
        let both_params = pos_params.with_fill_mode(WiggleFillMode::Both);
        let both_geom = build_trace_wiggle(0, baseline_x, spacing, height, &samples, &both_params);
        assert_eq!(both_geom.fill_polygons.len(), 2);
        assert_eq!(both_geom.fill_polygons[0].lobe, LobePolarity::Negative);
        assert_eq!(both_geom.fill_polygons[1].lobe, LobePolarity::Positive);

        // 4. Line-only mode emits zero fill polygons while keeping the polyline intact
        let line_params = pos_params.with_fill_mode(WiggleFillMode::LineOnly);
        let line_geom = build_trace_wiggle(0, baseline_x, spacing, height, &samples, &line_params);
        assert!(line_geom.fill_polygons.is_empty());
        assert_eq!(line_geom.polyline.len(), 2);
    }

    #[test]
    fn excursion_scale_and_clipping_respect_trace_spacings() {
        let samples = [-4.0_f32, -0.5, 0.0, 0.5, 4.0];
        let baseline_x = 100.0_f32;
        let spacing = 20.0_f32;
        let params = WiggleRenderParams::default()
            .with_excursion_spacings(1.0)
            .with_clip_spacings(1.5);

        let geom = build_trace_wiggle(0, baseline_x, spacing, 20, &samples, &params);
        let xs: Vec<f32> = geom.polyline.iter().map(|p| p.x).collect();
        // -4.0 clipped to -1.5 spacings = 100 - 30 = 70
        // -0.5 => 100 - 10 = 90
        //  0.0 => 100
        // +0.5 => 100 + 10 = 110
        // +4.0 clipped to +1.5 spacings = 100 + 30 = 130
        assert_eq!(xs, vec![70.0, 90.0, 100.0, 110.0, 130.0]);
    }

    #[test]
    fn vertical_decimation_caps_polyline_to_at_most_two_points_per_pixel() {
        let height = 100_u32;
        let dense_samples: Vec<f32> = (0..2500).map(|i| ((i as f32) * 0.17).sin() * 2.0).collect();

        let params_2 = WiggleRenderParams::default().with_max_points_per_pixel(2);
        let geom_2 = build_trace_wiggle(0, 50.0, 10.0, height, &dense_samples, &params_2);
        assert!(geom_2.polyline.len() <= (height as usize) * 2);
        assert!(geom_2.polyline.len() > height as usize);
        // Verify y coordinates are strictly non-decreasing and within [0, height - 1].
        for w in geom_2.polyline.windows(2) {
            assert!(
                w[1].y >= w[0].y,
                "y must be monotonic: {:?} -> {:?}",
                w[0],
                w[1]
            );
        }
        assert!(!geom_2.fill_polygons.is_empty());

        let params_1 = WiggleRenderParams::default().with_max_points_per_pixel(1);
        let geom_1 = build_trace_wiggle(0, 50.0, 10.0, height, &dense_samples, &params_1);
        assert_eq!(geom_1.polyline.len(), height as usize);
    }

    #[test]
    fn section_geometry_and_rasterizer_render_expected_pixels() -> Result<()> {
        let traces = vec![
            vec![-1.0_f32, 1.0, -1.0, 1.0],
            vec![1.0_f32, -1.0, 1.0, -1.0],
        ];
        let src = SliceTraceSource::new(&traces);
        let viewport = ImageViewport::new(40, 20, 0..2, 0..4);
        let params = WiggleRenderParams::default()
            .with_amplitude(
                AmplitudeParams::default()
                    .with_scope(NormScope::Global)
                    .with_clip(ClipStrategy::Peak),
            )
            .with_fill_mode(WiggleFillMode::Both)
            .with_excursion_spacings(0.8)
            .with_clip_spacings(1.0);

        let geom = render_wiggle_geometry(&src, &viewport, &params)?;
        assert_eq!(geom.traces.len(), 2);
        assert!((geom.trace_spacing_px - 20.0).abs() < 1e-5);
        assert!((geom.traces[0].baseline_x - 10.0).abs() < 1e-5);
        assert!((geom.traces[1].baseline_x - 30.0).abs() < 1e-5);
        assert!(geom.total_fill_polygons() >= 6);

        let img = render_wiggle_image(&src, &viewport, &params)?;
        assert_eq!(img.rgba.len(), 40 * 20 * 4);
        // Verify that both line/positive pixels and negative fill pixels were drawn over the white background.
        let non_white = img
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|&&px| px != params.background_rgba)
            .count();
        assert!(non_white > 20);
        Ok(())
    }

    #[test]
    fn sample_file_3x_75_pr_renders_wiggle_geometry_and_rgba() -> Result<()> {
        let path = Path::new("/home/toto/Downloads/3X_75_PR.SGY");
        if !path.exists() {
            return Ok(());
        }
        let file = SegyFile::open(path)?;
        let viewport = ImageViewport::new(320, 240, 0..120, 0..file.samples_per_trace() as usize);
        let params = WiggleRenderParams::default()
            .with_fill_mode(WiggleFillMode::Positive)
            .with_excursion_spacings(1.2)
            .with_clip_spacings(2.0);

        let geom = render_wiggle_geometry(&file, &viewport, &params)?;
        assert_eq!(geom.traces.len(), 120);
        for trace in &geom.traces {
            assert!(trace.polyline.len() <= (viewport.height as usize) * 2);
            assert!(trace.polyline.iter().all(|p| p.is_finite()));
        }
        assert!(geom.total_fill_polygons() > 100);

        let img = render_wiggle_image(&file, &viewport, &params)?;
        assert_eq!(img.rgba.len(), 320 * 240 * 4);
        Ok(())
    }

    #[test]
    fn hostile_inputs_and_overlay_compositing_behave_safely() -> Result<()> {
        let traces = vec![vec![0.0_f32, f32::NAN, f32::INFINITY, -f32::INFINITY]];
        let src = SliceTraceSource::new(&traces);

        // Invalid viewport dimensions
        assert!(matches!(
            render_wiggle_geometry(
                &src,
                &ImageViewport::new(0, 10, 0..1, 0..4),
                &WiggleRenderParams::default()
            ),
            Err(RenderError::InvalidViewport { .. })
        ));

        // Invalid trace range
        assert!(matches!(
            render_wiggle_geometry(
                &src,
                &ImageViewport::new(10, 10, 0..5, 0..4),
                &WiggleRenderParams::default()
            ),
            Err(RenderError::InvalidTraceRange { .. })
        ));

        // Invalid sample range
        assert!(matches!(
            render_wiggle_geometry(
                &src,
                &ImageViewport::new(10, 10, 0..1, 2..5),
                &WiggleRenderParams::default()
            ),
            Err(RenderError::InvalidSampleRange { .. })
        ));

        // Non-finite excursion and clip parameters sanitize cleanly without NaN coordinates
        let weird_params = WiggleRenderParams::default()
            .with_excursion_spacings(f32::NAN)
            .with_clip_spacings(f32::INFINITY);
        let vp = ImageViewport::new(16, 16, 0..1, 0..4);
        let geom = render_wiggle_geometry(&src, &vp, &weird_params)?;
        assert!(geom.traces[0].polyline.iter().all(|p| p.is_finite()));

        // Buffer size mismatch on rasterization
        let mut short_buf = vec![0u8; 10];
        assert!(matches!(
            rasterize_wiggle_geometry_into(&geom, &weird_params, true, &mut short_buf),
            Err(RenderError::BufferSizeMismatch { .. })
        ));

        // Overlay compositing (`clear_background = false`) preserves untouched background pixels
        let sentinel = [12_u8, 34, 56, 255];
        let mut buf = sentinel.repeat(16 * 16);
        render_wiggle_into(&src, &vp, &WiggleRenderParams::default(), false, &mut buf)?;
        assert!(buf.as_chunks::<4>().0.contains(&sentinel));
        assert!(buf.as_chunks::<4>().0.contains(&[0, 0, 0, 255]));
        Ok(())
    }
}
