//! Pure headless viewport and axis model for seismic section display:
//! - Independent horizontal (trace) and vertical (time / sample) pan and cursor-anchored zoom
//! - Explicit time window (`ms`) and trace range selection + fit-to-data reset
//! - User trace skip / decimation (`trace_decimate_step`)
//! - Automatic `Wiggle -> Image` mode switch when trace spacing falls below `~2.0` pixels (toggleable)
//! - Nice-number (`1-2-5 × 10^k`) tick generation for time (`ms`) and trace/header axes
//! - Bidirectional pixel ↔ `(trace, sample, time_ms)` coordinate transforms and cursor readout

use std::ops::Range;

use segy_core::{AxisHeaderKey, SegyFile};

use crate::image::{ImageViewport, MAX_VIEWPORT_DIMENSION, TraceDataSource};

/// Default minimum horizontal spacing in pixels between rendered traces before wiggle mode
/// automatically switches to variable-density image mode.
pub const DEFAULT_AUTO_SWITCH_TRACE_SPACING_PX: f32 = 2.0;

/// Minimum allowed continuous trace span when zooming in horizontally.
const MIN_VISIBLE_TRACE_SPAN: f64 = 1.0;

/// Minimum allowed continuous sample span when zooming in vertically.
const MIN_VISIBLE_SAMPLE_SPAN: f64 = 2.0;

/// Primary seismic section rendering mode selected by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DisplayMode {
    /// Wiggle + variable-area trace display.
    Wiggle,
    /// Variable-density (VD) color/grayscale image display.
    #[default]
    Image,
    /// Wiggle + variable-area traces overlaid on top of a variable-density image.
    Overlay,
}

impl DisplayMode {
    /// Returns `true` if variable-density image rendering is active in this mode.
    #[inline]
    pub const fn includes_image(self) -> bool {
        matches!(self, Self::Image | Self::Overlay)
    }

    /// Returns `true` if wiggle trace rendering is active in this mode.
    #[inline]
    pub const fn includes_wiggle(self) -> bool {
        matches!(self, Self::Wiggle | Self::Overlay)
    }
}

/// Dataset geometry bounds (`total_traces`, `samples_per_trace`, `sample_interval_us`, `delay_recording_time_ms`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SectionDataBounds {
    /// Total number of traces in the dataset (`>= 1` for non-empty sections).
    pub total_traces: usize,
    /// Nominal number of samples per trace (`>= 1` for non-empty sections).
    pub samples_per_trace: usize,
    /// Sample interval in microseconds (`dt_us`, e.g. `8000` µs = `8.0` ms).
    pub sample_interval_us: u32,
    /// Recording delay time of sample `0` in milliseconds (default `0.0` ms).
    pub delay_recording_time_ms: f64,
}

impl SectionDataBounds {
    /// Creates new [`SectionDataBounds`] with zero delay recording time.
    pub const fn new(
        total_traces: usize,
        samples_per_trace: usize,
        sample_interval_us: u32,
    ) -> Self {
        Self {
            total_traces,
            samples_per_trace,
            sample_interval_us: if sample_interval_us == 0 {
                1000
            } else {
                sample_interval_us
            },
            delay_recording_time_ms: 0.0,
        }
    }

    /// Sets the delay recording time of sample 0 in milliseconds.
    #[inline]
    pub const fn with_delay_recording_time_ms(mut self, delay_ms: f64) -> Self {
        self.delay_recording_time_ms = if delay_ms.is_finite() { delay_ms } else { 0.0 };
        self
    }

    /// Extracts [`SectionDataBounds`] from an opened [`SegyFile`].
    pub fn from_segy(file: &SegyFile) -> Self {
        let delay_ms = file
            .trace_header(0)
            .map(|th| f64::from(th.delay_recording_time_ms))
            .unwrap_or(0.0);
        Self::new(
            file.trace_count(),
            usize::from(file.samples_per_trace()),
            u32::from(file.sample_interval_us()),
        )
        .with_delay_recording_time_ms(delay_ms)
    }

    /// Sample interval in milliseconds (`dt_ms > 0.0`).
    #[inline]
    pub fn dt_ms(&self) -> f64 {
        f64::from(self.sample_interval_us.max(1)) / 1000.0
    }

    /// Converts a continuous 0-based sample index to time in milliseconds.
    #[inline]
    pub fn sample_to_time_ms(&self, sample_index: f64) -> f64 {
        self.delay_recording_time_ms + sample_index * self.dt_ms()
    }

    /// Converts a time in milliseconds to a continuous 0-based sample index.
    #[inline]
    pub fn time_ms_to_sample(&self, time_ms: f64) -> f64 {
        (time_ms - self.delay_recording_time_ms) / self.dt_ms()
    }

    /// Returns the full `(start_ms, end_ms)` time range of the dataset.
    #[inline]
    pub fn full_time_range_ms(&self) -> (f64, f64) {
        let end_sample = (self.samples_per_trace.saturating_sub(1)) as f64;
        (
            self.delay_recording_time_ms,
            self.sample_to_time_ms(end_sample),
        )
    }
}

/// Computed tick mark and formatted label along a horizontal (trace/header) or vertical (time) axis.
#[derive(Debug, Clone, PartialEq)]
pub struct AxisTick {
    /// Pixel offset along the axis (`0.0..=axis_length_px`).
    pub position_px: f32,
    /// Numeric value at this tick (time in `ms` for vertical axis, or header/trace value for horizontal axis).
    pub value: f64,
    /// Associated 0-based trace index when this is a horizontal trace axis tick.
    pub trace_index: Option<usize>,
    /// Formatted human-readable tick label.
    pub label: String,
}

/// Cursor inspection readout at a viewport pixel coordinate `(x_px, y_px)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CursorReadout {
    /// 0-based trace index under the cursor.
    pub trace_index: usize,
    /// 1-based trace number (`trace_index + 1`).
    pub trace_number: usize,
    /// Value of the currently selected [`AxisHeaderKey`] for `trace_index`.
    pub header_value: i64,
    /// 0-based sample index under the cursor.
    pub sample_index: usize,
    /// Time in milliseconds under the cursor.
    pub time_ms: f64,
    /// Raw (unprocessed) sample amplitude at `(trace_index, sample_index)`.
    pub amplitude: f32,
}

/// Pure headless viewport controller managing pan, independent H/V zoom, time windowing,
/// trace decimation, axis tick computation, and automatic wiggle-to-image switching.
#[derive(Debug, Clone, PartialEq)]
pub struct SeismicViewport {
    width_px: u32,
    height_px: u32,
    bounds: SectionDataBounds,
    trace_start: f64,
    trace_end: f64,
    sample_start: f64,
    sample_end: f64,
    trace_decimate_step: usize,
    reverse_traces: bool,
    horizontal_axis_key: AxisHeaderKey,
    display_mode: DisplayMode,
    auto_switch_wiggle_to_image: bool,
    auto_switch_threshold_px: f32,
}

impl SeismicViewport {
    /// Creates a new [`SeismicViewport`] initialized to fit the full `bounds` inside `width_px × height_px`.
    pub fn new(width_px: u32, height_px: u32, bounds: SectionDataBounds) -> Self {
        let clamped_w = width_px.clamp(1, MAX_VIEWPORT_DIMENSION);
        let clamped_h = height_px.clamp(1, MAX_VIEWPORT_DIMENSION);
        let max_traces = (bounds.total_traces.max(1)) as f64;
        let max_samples = (bounds.samples_per_trace.max(1)) as f64;

        Self {
            width_px: clamped_w,
            height_px: clamped_h,
            bounds,
            trace_start: 0.0,
            trace_end: max_traces,
            sample_start: 0.0,
            sample_end: max_samples,
            trace_decimate_step: 1,
            reverse_traces: false,
            horizontal_axis_key: AxisHeaderKey::TraceNumber,
            display_mode: DisplayMode::Image,
            auto_switch_wiggle_to_image: true,
            auto_switch_threshold_px: DEFAULT_AUTO_SWITCH_TRACE_SPACING_PX,
        }
    }

    /// Returns the viewport width in pixels.
    #[inline]
    pub const fn width_px(&self) -> u32 {
        self.width_px
    }

    /// Returns the viewport height in pixels.
    #[inline]
    pub const fn height_px(&self) -> u32 {
        self.height_px
    }

    /// Updates the viewport canvas pixel dimensions (`1..=MAX_VIEWPORT_DIMENSION`).
    #[inline]
    pub fn set_dimensions(&mut self, width_px: u32, height_px: u32) {
        self.width_px = width_px.clamp(1, MAX_VIEWPORT_DIMENSION);
        self.height_px = height_px.clamp(1, MAX_VIEWPORT_DIMENSION);
    }

    /// Returns the underlying dataset bounds.
    #[inline]
    pub const fn bounds(&self) -> SectionDataBounds {
        self.bounds
    }

    /// Returns the user-selected [`DisplayMode`].
    #[inline]
    pub const fn display_mode(&self) -> DisplayMode {
        self.display_mode
    }

    /// Sets the user-selected [`DisplayMode`].
    #[inline]
    pub fn set_display_mode(&mut self, mode: DisplayMode) {
        self.display_mode = mode;
    }

    /// Returns whether automatic `Wiggle -> Image` switching is enabled when traces are `< threshold` pixels apart.
    #[inline]
    pub const fn auto_switch_wiggle_to_image(&self) -> bool {
        self.auto_switch_wiggle_to_image
    }

    /// Enables or disables automatic `Wiggle -> Image` switching when traces are dense.
    #[inline]
    pub fn set_auto_switch_wiggle_to_image(&mut self, enabled: bool) {
        self.auto_switch_wiggle_to_image = enabled;
    }

    /// Returns the minimum trace spacing threshold in pixels (`~2.0` px by default) below which
    /// wiggle rendering automatically switches to variable-density image rendering.
    #[inline]
    pub const fn auto_switch_threshold_px(&self) -> f32 {
        self.auto_switch_threshold_px
    }

    /// Sets the minimum trace spacing threshold in pixels for automatic wiggle-to-image switching.
    #[inline]
    pub fn set_auto_switch_threshold_px(&mut self, threshold_px: f32) {
        if threshold_px.is_finite() && threshold_px > 0.0 {
            self.auto_switch_threshold_px = threshold_px;
        }
    }

    /// Returns the user trace decimation step (`1` = every trace, `N` = every `N`-th trace).
    #[inline]
    pub const fn trace_decimate_step(&self) -> usize {
        self.trace_decimate_step
    }

    /// Sets the user trace decimation step (clamped to `>= 1`).
    #[inline]
    pub fn set_trace_decimate_step(&mut self, step: usize) {
        self.trace_decimate_step = step.max(1);
    }

    /// Returns `true` if horizontal trace order is reversed (right-to-left).
    #[inline]
    pub const fn reverse_traces(&self) -> bool {
        self.reverse_traces
    }

    /// Sets whether horizontal trace order is reversed.
    #[inline]
    pub fn set_reverse_traces(&mut self, reverse: bool) {
        self.reverse_traces = reverse;
    }

    /// Returns the currently selected horizontal axis header key.
    #[inline]
    pub const fn horizontal_axis_key(&self) -> AxisHeaderKey {
        self.horizontal_axis_key
    }

    /// Sets the horizontal axis header key (`TraceNumber`, `Cdp`, `Inline`, `Crossline`, `Offset`, etc.).
    #[inline]
    pub fn set_horizontal_axis_key(&mut self, key: AxisHeaderKey) {
        self.horizontal_axis_key = key;
    }

    /// Resets both horizontal (trace) and vertical (sample/time) windows to fit the entire dataset.
    pub fn fit_to_data(&mut self) {
        self.fit_horizontal();
        self.fit_vertical();
    }

    /// Resets the horizontal trace window to cover `[0..total_traces)`.
    pub fn fit_horizontal(&mut self) {
        self.trace_start = 0.0;
        self.trace_end = (self.bounds.total_traces.max(1)) as f64;
    }

    /// Resets the vertical sample/time window to cover `[0..samples_per_trace)`.
    pub fn fit_vertical(&mut self) {
        self.sample_start = 0.0;
        self.sample_end = (self.bounds.samples_per_trace.max(1)) as f64;
    }

    /// Returns the horizontal zoom ratio (`>= 1.0`, where `1.0` is full trace range).
    #[inline]
    pub fn zoom_ratio_horizontal(&self) -> f64 {
        let total = (self.bounds.total_traces.max(1)) as f64;
        let span = (self.trace_end - self.trace_start).max(MIN_VISIBLE_TRACE_SPAN);
        (total / span).max(1.0)
    }

    /// Returns the vertical zoom ratio (`>= 1.0`, where `1.0` is full sample/time range).
    #[inline]
    pub fn zoom_ratio_vertical(&self) -> f64 {
        let total = (self.bounds.samples_per_trace.max(1)) as f64;
        let span = (self.sample_end - self.sample_start).max(MIN_VISIBLE_SAMPLE_SPAN.min(total));
        (total / span).max(1.0)
    }

    /// Returns the discrete 0-based visible trace index range `start..end`.
    pub fn visible_trace_range(&self) -> Range<usize> {
        let total = self.bounds.total_traces;
        if total == 0 {
            return 0..0;
        }
        let start = (self.trace_start.floor().max(0.0) as usize).min(total - 1);
        let end = (self.trace_end.ceil().max(1.0) as usize)
            .max(start + 1)
            .min(total);
        start..end
    }

    /// Returns the discrete 0-based visible sample index range `start..end`.
    pub fn visible_sample_range(&self) -> Range<usize> {
        let total = self.bounds.samples_per_trace;
        if total == 0 {
            return 0..0;
        }
        let start = (self.sample_start.floor().max(0.0) as usize).min(total - 1);
        let end = (self.sample_end.ceil().max(1.0) as usize)
            .max(start + 1)
            .min(total);
        start..end
    }

    /// Returns the visible time window `(start_ms, end_ms)` in milliseconds corresponding to
    /// [`Self::visible_sample_range`].
    pub fn visible_time_window_ms(&self) -> (f64, f64) {
        let range = self.visible_sample_range();
        if range.is_empty() {
            return (
                self.bounds.delay_recording_time_ms,
                self.bounds.delay_recording_time_ms,
            );
        }
        let start_ms = self.bounds.sample_to_time_ms(range.start as f64);
        let end_ms = self
            .bounds
            .sample_to_time_ms((range.end.saturating_sub(1)) as f64);
        (start_ms, end_ms)
    }

    /// Sets the visible vertical window from an explicit time range `[start_ms, end_ms]` in milliseconds.
    pub fn set_time_window_ms(&mut self, start_ms: f64, end_ms: f64) {
        if !start_ms.is_finite() || !end_ms.is_finite() {
            return;
        }
        let lo_ms = start_ms.min(end_ms);
        let hi_ms = start_ms.max(end_ms);
        let s0 = self.bounds.time_ms_to_sample(lo_ms).floor();
        let s1 = self.bounds.time_ms_to_sample(hi_ms).ceil() + 1.0;
        self.set_continuous_sample_window(s0, s1);
    }

    /// Sets the visible horizontal window to the discrete 0-based trace range `start..end`.
    pub fn set_trace_window(&mut self, start: usize, end: usize) {
        let lo = start.min(end) as f64;
        let hi = start.max(end).max(start.saturating_add(1)) as f64;
        self.set_continuous_trace_window(lo, hi);
    }

    /// Returns the number of traces in the visible range after applying `trace_decimate_step`.
    #[inline]
    pub fn decimated_trace_count(&self) -> usize {
        let range = self.visible_trace_range();
        let visible = range.end.saturating_sub(range.start);
        if visible == 0 {
            return 0;
        }
        let step = self.trace_decimate_step.max(1);
        visible.div_ceil(step)
    }

    /// Returns the ordered 0-based trace indices selected after applying `trace_decimate_step`
    /// (and `reverse_traces` when enabled).
    pub fn decimated_trace_indices(&self) -> Vec<usize> {
        let range = self.visible_trace_range();
        let step = self.trace_decimate_step.max(1);
        let mut indices: Vec<usize> = range.step_by(step).collect();
        if self.reverse_traces {
            indices.reverse();
        }
        indices
    }

    /// Returns the horizontal spacing in pixels between adjacent rendered traces in the current viewport.
    #[inline]
    pub fn trace_spacing_px(&self) -> f32 {
        let count = self.decimated_trace_count();
        if count == 0 {
            return self.width_px as f32;
        }
        (self.width_px as f32) / (count as f32)
    }

    /// Returns `true` if the user selected `Wiggle` or `Overlay`, `auto_switch_wiggle_to_image` is enabled,
    /// and the current trace spacing is strictly less than `auto_switch_threshold_px` (`~2.0` px).
    #[inline]
    pub fn is_wiggle_auto_switched(&self) -> bool {
        self.auto_switch_wiggle_to_image
            && self.display_mode.includes_wiggle()
            && self.trace_spacing_px() < self.auto_switch_threshold_px
    }

    /// Returns the effective [`DisplayMode`] for rendering the current frame, automatically
    /// switching `Wiggle` / `Overlay` to [`DisplayMode::Image`] when traces are `< ~2.0` pixels apart.
    #[inline]
    pub fn effective_display_mode(&self) -> DisplayMode {
        if self.is_wiggle_auto_switched() {
            DisplayMode::Image
        } else {
            self.display_mode
        }
    }

    /// Pans the horizontal trace window by `delta_traces` (positive shifts toward higher trace indices).
    pub fn pan_traces(&mut self, delta_traces: f64) {
        if !delta_traces.is_finite() {
            return;
        }
        self.set_continuous_trace_window(
            self.trace_start + delta_traces,
            self.trace_end + delta_traces,
        );
    }

    /// Pans the vertical sample window by `delta_samples` (positive shifts toward deeper samples).
    pub fn pan_samples(&mut self, delta_samples: f64) {
        if !delta_samples.is_finite() {
            return;
        }
        self.set_continuous_sample_window(
            self.sample_start + delta_samples,
            self.sample_end + delta_samples,
        );
    }

    /// Pans the vertical window by `delta_ms` milliseconds.
    pub fn pan_time_ms(&mut self, delta_ms: f64) {
        if !delta_ms.is_finite() {
            return;
        }
        let delta_samples = delta_ms / self.bounds.dt_ms();
        self.pan_samples(delta_samples);
    }

    /// Pans both axes in response to a screen pointer drag of `(drag_dx_px, drag_dy_px)` pixels
    /// (dragging right/down pulls earlier traces/samples into view).
    pub fn pan_by_screen_drag_px(&mut self, drag_dx_px: f32, drag_dy_px: f32) {
        if !drag_dx_px.is_finite() || !drag_dy_px.is_finite() {
            return;
        }
        let trace_span = self.trace_end - self.trace_start;
        let sample_span = self.sample_end - self.sample_start;
        let sign_x = if self.reverse_traces { 1.0 } else { -1.0 };
        let delta_traces =
            sign_x * (f64::from(drag_dx_px) / f64::from(self.width_px.max(1))) * trace_span;
        let delta_samples =
            -(f64::from(drag_dy_px) / f64::from(self.height_px.max(1))) * sample_span;
        self.pan_traces(delta_traces);
        self.pan_samples(delta_samples);
    }

    /// Zooms the horizontal trace axis by `factor` (`> 1.0` zooms in, `< 1.0` zooms out),
    /// keeping the trace at normalized viewport ratio `anchor_ratio` (`0.0..=1.0` from left to right)
    /// fixed on screen.
    pub fn zoom_horizontal(&mut self, factor: f64, anchor_ratio: f64) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let screen_ratio = if anchor_ratio.is_finite() {
            anchor_ratio.clamp(0.0, 1.0)
        } else {
            0.5
        };
        let data_ratio = if self.reverse_traces {
            1.0 - screen_ratio
        } else {
            screen_ratio
        };
        let total = (self.bounds.total_traces.max(1)) as f64;
        let min_span = MIN_VISIBLE_TRACE_SPAN.min(total);
        let old_span = (self.trace_end - self.trace_start).clamp(min_span, total);
        let new_span = (old_span / factor).clamp(min_span, total);
        let anchor_trace = self.trace_start + data_ratio * old_span;
        let new_start = anchor_trace - data_ratio * new_span;
        self.set_continuous_trace_window(new_start, new_start + new_span);
    }

    /// Zooms the vertical sample/time axis by `factor` (`> 1.0` zooms in, `< 1.0` zooms out),
    /// keeping the sample at normalized viewport ratio `anchor_ratio` (`0.0..=1.0` from top to bottom)
    /// fixed on screen.
    pub fn zoom_vertical(&mut self, factor: f64, anchor_ratio: f64) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let ratio = if anchor_ratio.is_finite() {
            anchor_ratio.clamp(0.0, 1.0)
        } else {
            0.5
        };
        let total = (self.bounds.samples_per_trace.max(1)) as f64;
        let min_span = MIN_VISIBLE_SAMPLE_SPAN.min(total);
        let old_span = (self.sample_end - self.sample_start).clamp(min_span, total);
        let new_span = (old_span / factor).clamp(min_span, total);
        let anchor_sample = self.sample_start + ratio * old_span;
        let new_start = anchor_sample - ratio * new_span;
        self.set_continuous_sample_window(new_start, new_start + new_span);
    }

    /// Applies independent horizontal (`zoom_h`) and vertical (`zoom_v`) zoom factors anchored
    /// at viewport pixel coordinate `(cursor_x_px, cursor_y_px)`.
    pub fn zoom_at_pixel(&mut self, zoom_h: f64, zoom_v: f64, cursor_x_px: f32, cursor_y_px: f32) {
        let rx = (f64::from(cursor_x_px) / f64::from(self.width_px.max(1))).clamp(0.0, 1.0);
        let ry = (f64::from(cursor_y_px) / f64::from(self.height_px.max(1))).clamp(0.0, 1.0);
        self.zoom_horizontal(zoom_h, rx);
        self.zoom_vertical(zoom_v, ry);
    }

    /// Converts the current viewport state into an [`ImageViewport`] for the image and wiggle renderers.
    pub fn to_image_viewport(&self) -> ImageViewport {
        ImageViewport {
            width: self.width_px,
            height: self.height_px,
            trace_range: self.visible_trace_range(),
            sample_range: self.visible_sample_range(),
            reverse_traces: self.reverse_traces,
        }
    }

    /// Maps a horizontal pixel coordinate `x_px` (`0.0..width_px`) to a 0-based dataset trace index.
    pub fn pixel_to_trace_index(&self, x_px: f32) -> Option<usize> {
        if !x_px.is_finite() || x_px < 0.0 || x_px >= self.width_px as f32 {
            return None;
        }
        let range = self.visible_trace_range();
        if range.is_empty() {
            return None;
        }
        let count = range.end - range.start;
        let norm_x = (x_px / (self.width_px as f32)).clamp(0.0, 0.999_999_9);
        let data_ratio = if self.reverse_traces {
            1.0 - norm_x
        } else {
            norm_x
        };
        let local_idx = ((data_ratio * (count as f32)).floor() as usize).min(count - 1);
        Some(range.start + local_idx)
    }

    /// Maps a vertical pixel coordinate `y_px` (`0.0..height_px`) to a 0-based dataset sample index.
    pub fn pixel_to_sample_index(&self, y_px: f32) -> Option<usize> {
        if !y_px.is_finite() || y_px < 0.0 || y_px >= self.height_px as f32 {
            return None;
        }
        let range = self.visible_sample_range();
        if range.is_empty() {
            return None;
        }
        let count = range.end - range.start;
        let norm_y = (y_px / (self.height_px as f32)).clamp(0.0, 0.999_999_9);
        let local_idx = ((norm_y * (count as f32)).round() as usize).min(count - 1);
        Some(range.start + local_idx)
    }

    /// Maps a vertical pixel coordinate `y_px` (`0.0..height_px`) to continuous time in milliseconds.
    pub fn pixel_to_time_ms(&self, y_px: f32) -> Option<f64> {
        if !y_px.is_finite() || y_px < 0.0 || y_px >= self.height_px as f32 {
            return None;
        }
        let (t0, t1) = self.visible_time_window_ms();
        let denom = f64::from((self.height_px.saturating_sub(1)).max(1));
        let ratio = (f64::from(y_px) / denom).clamp(0.0, 1.0);
        Some(t0 + ratio * (t1 - t0))
    }

    /// Maps a 0-based `trace_index` to its horizontal cell-center pixel coordinate in the viewport.
    pub fn trace_index_to_pixel_x(&self, trace_index: usize) -> f32 {
        let range = self.visible_trace_range();
        let count = range.end.saturating_sub(range.start).max(1);
        let clamped = trace_index.clamp(range.start, range.end.saturating_sub(1));
        let local = clamped - range.start;
        let slot = if self.reverse_traces {
            count - 1 - local
        } else {
            local
        };
        ((slot as f32) + 0.5) * ((self.width_px as f32) / (count as f32))
    }

    /// Maps a time in milliseconds `time_ms` to a vertical pixel coordinate in `[0.0, height_px - 1]`.
    pub fn time_ms_to_pixel_y(&self, time_ms: f64) -> f32 {
        let (t0, t1) = self.visible_time_range_for_ticks();
        let span = t1 - t0;
        if span.abs() <= 1e-9 || self.height_px <= 1 {
            return 0.0;
        }
        let ratio = ((time_ms - t0) / span).clamp(0.0, 1.0);
        (ratio * f64::from(self.height_px - 1)) as f32
    }

    /// Computes vertical time-axis ticks (in milliseconds) spaced roughly `target_spacing_px` apart.
    pub fn compute_time_axis_ticks(&self, target_spacing_px: f32) -> Vec<AxisTick> {
        let (t_start, t_end) = self.visible_time_range_for_ticks();
        compute_linear_ticks(
            t_start,
            t_end,
            self.height_px,
            target_spacing_px.max(16.0),
            false,
        )
    }

    /// Computes horizontal trace-axis ticks spaced roughly `target_spacing_px` apart.
    ///
    /// When `segy_file` is provided and `self.horizontal_axis_key` is a trace header field
    /// (such as `Cdp`, `Inline`, `Crossline`, or `Offset`), each tick label is populated from that
    /// trace's header value; otherwise the 1-based trace number is used.
    pub fn compute_trace_axis_ticks(
        &self,
        segy_file: Option<&SegyFile>,
        target_spacing_px: f32,
    ) -> Vec<AxisTick> {
        let range = self.visible_trace_range();
        if range.is_empty() || self.width_px == 0 {
            return Vec::new();
        }
        let visible_count = range.end - range.start;
        let desired_ticks = ((self.width_px as f32) / target_spacing_px.max(24.0))
            .floor()
            .max(1.0) as usize;
        let raw_step = (visible_count as f64) / (desired_ticks as f64);
        let step = nice_integer_step(raw_step).max(1);

        // Align 1-based trace numbers `(trace_idx + 1)` to multiples of `step` (always including trace 1 if visible).
        let first_trace_num = range.start + 1;
        let last_trace_num = range.end;
        let first_aligned_num = if step == 1 {
            first_trace_num
        } else {
            let rem = first_trace_num % step;
            if rem == 0 {
                first_trace_num
            } else {
                first_trace_num + (step - rem)
            }
        };

        let mut ticks = Vec::new();
        let mut trace_num = first_aligned_num;
        while trace_num <= last_trace_num {
            let trace_idx = trace_num - 1;
            let pos_x = self.trace_index_to_pixel_x(trace_idx);
            let header_val = segy_file
                .and_then(|f| f.trace_axis_value(trace_idx, self.horizontal_axis_key).ok())
                .unwrap_or(trace_num as i64);
            ticks.push(AxisTick {
                position_px: pos_x,
                value: header_val as f64,
                trace_index: Some(trace_idx),
                label: header_val.to_string(),
            });
            trace_num = trace_num.saturating_add(step);
        }

        // If the window is narrow and no multiple of `step` landed inside, emit the start trace.
        if ticks.is_empty() {
            let trace_idx = range.start;
            let pos_x = self.trace_index_to_pixel_x(trace_idx);
            let header_val = segy_file
                .and_then(|f| f.trace_axis_value(trace_idx, self.horizontal_axis_key).ok())
                .unwrap_or((trace_idx + 1) as i64);
            ticks.push(AxisTick {
                position_px: pos_x,
                value: header_val as f64,
                trace_index: Some(trace_idx),
                label: header_val.to_string(),
            });
        }

        if self.reverse_traces {
            ticks.sort_by(|a, b| a.position_px.total_cmp(&b.position_px));
        }

        ticks
    }

    /// Queries the trace index, header value, sample index, time (`ms`), and raw sample amplitude
    /// under viewport pixel `(x_px, y_px)`.
    pub fn cursor_readout(
        &self,
        source: &dyn TraceDataSource,
        segy_file: Option<&SegyFile>,
        x_px: f32,
        y_px: f32,
    ) -> Option<CursorReadout> {
        let trace_index = self.pixel_to_trace_index(x_px)?;
        let sample_index = self.pixel_to_sample_index(y_px)?;
        let time_ms = self.pixel_to_time_ms(y_px)?;
        let trace_number = trace_index + 1;
        let header_value = segy_file
            .and_then(|f| {
                f.trace_axis_value(trace_index, self.horizontal_axis_key)
                    .ok()
            })
            .unwrap_or(trace_number as i64);

        let mut sample_buf = Vec::with_capacity(1);
        source
            .read_trace_window(trace_index, sample_index..sample_index + 1, &mut sample_buf)
            .ok()?;
        let amplitude = sample_buf.first().copied().unwrap_or(0.0);

        Some(CursorReadout {
            trace_index,
            trace_number,
            header_value,
            sample_index,
            time_ms,
            amplitude,
        })
    }

    fn visible_time_range_for_ticks(&self) -> (f64, f64) {
        let (t0, t1) = self.visible_time_window_ms();
        if (t1 - t0).abs() > 1e-9 {
            (t0, t1)
        } else {
            (t0, t0 + self.bounds.dt_ms())
        }
    }

    fn set_continuous_trace_window(&mut self, start: f64, end: f64) {
        let total = (self.bounds.total_traces.max(1)) as f64;
        let min_span = MIN_VISIBLE_TRACE_SPAN.min(total);
        let span = (end - start).clamp(min_span, total);
        let clamped_start = start.clamp(0.0, (total - span).max(0.0));
        self.trace_start = clamped_start;
        self.trace_end = (clamped_start + span).min(total);
    }

    fn set_continuous_sample_window(&mut self, start: f64, end: f64) {
        let total = (self.bounds.samples_per_trace.max(1)) as f64;
        let min_span = MIN_VISIBLE_SAMPLE_SPAN.min(total);
        let span = (end - start).clamp(min_span, total);
        let clamped_start = start.clamp(0.0, (total - span).max(0.0));
        self.sample_start = clamped_start;
        self.sample_end = (clamped_start + span).min(total);
    }
}

/// Computes a human-friendly `1, 2, 5 × 10^k` step size close to `raw_step`.
pub fn nice_step_size(raw_step: f64) -> f64 {
    if !raw_step.is_finite() || raw_step <= 0.0 {
        return 1.0;
    }
    let exp = raw_step.log10().floor();
    let pow10 = 10.0_f64.powf(exp);
    let frac = raw_step / pow10;
    let nice_frac = if frac <= 1.5 {
        1.0
    } else if frac <= 3.5 {
        2.0
    } else if frac <= 7.5 {
        5.0
    } else {
        10.0
    };
    nice_frac * pow10
}

fn nice_integer_step(raw_step: f64) -> usize {
    let step = nice_step_size(raw_step.max(1.0)).round() as usize;
    step.max(1)
}

/// Computes nicely-spaced linear axis ticks across `[min_val, max_val]` mapped onto `length_px` pixels.
pub fn compute_linear_ticks(
    min_val: f64,
    max_val: f64,
    length_px: u32,
    target_spacing_px: f32,
    reverse: bool,
) -> Vec<AxisTick> {
    if !min_val.is_finite() || !max_val.is_finite() || length_px == 0 {
        return Vec::new();
    }
    let lo = min_val.min(max_val);
    let hi = min_val.max(max_val);
    let span = hi - lo;
    if span <= 1e-9 {
        return vec![AxisTick {
            position_px: 0.0,
            value: lo,
            trace_index: None,
            label: format_tick_value(lo, 1.0),
        }];
    }

    let desired_count = ((length_px as f32) / target_spacing_px.max(16.0))
        .floor()
        .max(1.0) as usize;
    let step = nice_step_size(span / (desired_count as f64));
    if step <= 0.0 || !step.is_finite() {
        return Vec::new();
    }

    let first_k = (lo / step).ceil() as i64;
    let last_k = (hi / step).floor() as i64;
    let max_px = (length_px.saturating_sub(1)) as f64;

    let mut ticks = Vec::new();
    for k in first_k..=last_k {
        let val = (k as f64) * step;
        let clean_val = if val.abs() < step * 1e-9 { 0.0 } else { val };
        let norm = ((clean_val - lo) / span).clamp(0.0, 1.0);
        let ratio = if reverse { 1.0 - norm } else { norm };
        let pos_px = (ratio * max_px) as f32;
        ticks.push(AxisTick {
            position_px: pos_px,
            value: clean_val,
            trace_index: None,
            label: format_tick_value(clean_val, step),
        });
    }

    if reverse {
        ticks.sort_by(|a, b| a.position_px.total_cmp(&b.position_px));
    }

    ticks
}

fn format_tick_value(val: f64, step: f64) -> String {
    if step >= 1.0 && (val - val.round()).abs() < 1e-6 {
        format!("{}", val.round() as i64)
    } else if step >= 0.1 {
        format!("{val:.1}")
    } else if step >= 0.01 {
        format!("{val:.2}")
    } else {
        format!("{val:.3}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::SliceTraceSource;
    use std::path::Path;

    #[test]
    fn independent_hv_zoom_and_pan_preserve_anchors_and_clamp_bounds() {
        // 1000 traces x 500 samples, dt = 4000 us (4 ms) => 0..1996 ms
        let bounds = SectionDataBounds::new(1000, 500, 4000);
        let mut vp = SeismicViewport::new(800, 600, bounds);

        assert_eq!(vp.visible_trace_range(), 0..1000);
        assert_eq!(vp.visible_sample_range(), 0..500);
        assert!((vp.zoom_ratio_horizontal() - 1.0).abs() < 1e-6);
        assert!((vp.zoom_ratio_vertical() - 1.0).abs() < 1e-6);

        // Zoom horizontally 4x around center (0.5), leave vertical untouched (1x)
        vp.zoom_horizontal(4.0, 0.5);
        assert!((vp.zoom_ratio_horizontal() - 4.0).abs() < 1e-5);
        assert!((vp.zoom_ratio_vertical() - 1.0).abs() < 1e-5);
        assert_eq!(vp.visible_trace_range(), 375..625);
        assert_eq!(vp.visible_sample_range(), 0..500);

        // Zoom vertically 2x anchored at the top edge (0.0)
        vp.zoom_vertical(2.0, 0.0);
        assert!((vp.zoom_ratio_vertical() - 2.0).abs() < 1e-5);
        assert_eq!(vp.visible_sample_range(), 0..250);

        // Pan traces right by +100 traces and pan time down by +400 ms (+100 samples)
        vp.pan_traces(100.0);
        vp.pan_time_ms(400.0);
        assert_eq!(vp.visible_trace_range(), 475..725);
        assert_eq!(vp.visible_sample_range(), 100..350);

        // Pan past upper bounds clamps cleanly to dataset end without changing span
        vp.pan_traces(10_000.0);
        vp.pan_samples(10_000.0);
        assert_eq!(vp.visible_trace_range(), 750..1000);
        assert_eq!(vp.visible_sample_range(), 250..500);

        // Fit-to-data restores the full section
        vp.fit_to_data();
        assert_eq!(vp.visible_trace_range(), 0..1000);
        assert_eq!(vp.visible_sample_range(), 0..500);
    }

    #[test]
    fn time_window_and_trace_decimate_update_ranges_and_spacing() {
        // 500 traces x 1001 samples at 2 ms (0..2000 ms)
        let bounds = SectionDataBounds::new(500, 1001, 2000);
        let mut vp = SeismicViewport::new(500, 400, bounds);

        vp.set_time_window_ms(400.0, 1200.0);
        let sample_range = vp.visible_sample_range();
        assert_eq!(sample_range, 200..601);
        let (t0, t1) = vp.visible_time_window_ms();
        assert!((t0 - 400.0).abs() < 1e-6);
        assert!((t1 - 1200.0).abs() < 1e-6);

        // Decimate by step = 5 => 500 / 5 = 100 traces rendered across 500 px => 5.0 px spacing
        vp.set_trace_decimate_step(5);
        assert_eq!(vp.decimated_trace_count(), 100);
        assert!((vp.trace_spacing_px() - 5.0).abs() < 1e-5);
        let indices = vp.decimated_trace_indices();
        assert_eq!(indices.len(), 100);
        assert_eq!(indices[0], 0);
        assert_eq!(indices[1], 5);
        assert_eq!(indices[99], 495);
    }

    #[test]
    fn auto_switch_wiggle_to_image_triggers_below_two_pixels_per_trace() {
        let bounds = SectionDataBounds::new(1000, 500, 4000);
        let mut vp = SeismicViewport::new(600, 400, bounds);
        vp.set_display_mode(DisplayMode::Wiggle);

        // 1000 traces in 600 px => 0.6 px/trace (< 2.0 px) => auto-switches to Image
        assert!((vp.trace_spacing_px() - 0.6).abs() < 1e-5);
        assert!(vp.is_wiggle_auto_switched());
        assert_eq!(vp.effective_display_mode(), DisplayMode::Image);

        // Disabling the toggle keeps Wiggle mode even when dense
        vp.set_auto_switch_wiggle_to_image(false);
        assert!(!vp.is_wiggle_auto_switched());
        assert_eq!(vp.effective_display_mode(), DisplayMode::Wiggle);
        vp.set_auto_switch_wiggle_to_image(true);

        // Zooming in horizontally by 4x (250 traces in 600 px => 2.4 px/trace >= 2.0 px) restores Wiggle
        vp.zoom_horizontal(4.0, 0.5);
        assert!((vp.trace_spacing_px() - 2.4).abs() < 1e-5);
        assert!(!vp.is_wiggle_auto_switched());
        assert_eq!(vp.effective_display_mode(), DisplayMode::Wiggle);

        // Or zooming out back to 1000 traces but setting trace_decimate_step = 4 (250 traces => 2.4 px/trace)
        vp.fit_horizontal();
        assert_eq!(vp.effective_display_mode(), DisplayMode::Image);
        vp.set_trace_decimate_step(4);
        assert_eq!(vp.effective_display_mode(), DisplayMode::Wiggle);
    }

    #[test]
    fn time_and_trace_axis_ticks_produce_clean_monotonic_labels() {
        let bounds = SectionDataBounds::new(1592, 751, 8000); // 0..6000 ms
        let vp = SeismicViewport::new(800, 600, bounds);

        let time_ticks = vp.compute_time_axis_ticks(60.0);
        assert!(time_ticks.len() >= 5 && time_ticks.len() <= 15);
        assert_eq!(time_ticks.first().map(|t| t.label.as_str()), Some("0"));
        assert_eq!(time_ticks.last().map(|t| t.label.as_str()), Some("6000"));
        for w in time_ticks.windows(2) {
            assert!(w[1].position_px > w[0].position_px);
            assert!(w[1].value > w[0].value);
        }

        let trace_ticks = vp.compute_trace_axis_ticks(None, 80.0);
        assert!(trace_ticks.len() >= 5 && trace_ticks.len() <= 20);
        for w in trace_ticks.windows(2) {
            assert!(w[1].position_px > w[0].position_px);
            assert!(w[1].value > w[0].value);
        }
    }

    #[test]
    fn cursor_readout_and_sample_segy_header_ticks_match_data()
    -> Result<(), Box<dyn std::error::Error>> {
        let traces = vec![
            vec![10.0_f32, 20.0, 30.0, 40.0],
            vec![50.0_f32, 60.0, 70.0, 80.0],
        ];
        let src = SliceTraceSource::new(&traces);
        let bounds = SectionDataBounds::new(2, 4, 4000);
        let vp = SeismicViewport::new(100, 100, bounds);

        // Left half of viewport is trace 0, bottom of viewport is sample 3 (time = 12 ms)
        let r0 = vp.cursor_readout(&src, None, 25.0, 0.0);
        assert!(r0.is_some());
        if let Some(ro) = r0 {
            assert_eq!(ro.trace_index, 0);
            assert_eq!(ro.trace_number, 1);
            assert_eq!(ro.sample_index, 0);
            assert!((ro.time_ms - 0.0).abs() < 1e-5);
            assert!((ro.amplitude - 10.0).abs() < 1e-5);
        }

        let r1 = vp.cursor_readout(&src, None, 75.0, 99.0);
        assert!(r1.is_some());
        if let Some(ro) = r1 {
            assert_eq!(ro.trace_index, 1);
            assert_eq!(ro.trace_number, 2);
            assert_eq!(ro.sample_index, 3);
            assert!((ro.time_ms - 12.0).abs() < 1e-5);
            assert!((ro.amplitude - 80.0).abs() < 1e-5);
        }

        let sample_path = Path::new("/home/toto/Downloads/3X_75_PR.SGY");
        if sample_path.exists() {
            let file = SegyFile::open(sample_path)?;
            let segy_bounds = SectionDataBounds::from_segy(&file);
            assert_eq!(segy_bounds.total_traces, 1592);
            assert_eq!(segy_bounds.samples_per_trace, 751);
            assert_eq!(segy_bounds.sample_interval_us, 8000);

            let mut segy_vp = SeismicViewport::new(1000, 700, segy_bounds);
            segy_vp.set_horizontal_axis_key(AxisHeaderKey::Cdp);
            let cdp_ticks = segy_vp.compute_trace_axis_ticks(Some(&file), 100.0);
            assert!(!cdp_ticks.is_empty());
        }
        Ok(())
    }

    #[test]
    fn reverse_traces_and_hostile_nan_or_zero_bounds_never_panic() {
        let empty_bounds = SectionDataBounds::new(0, 0, 0);
        let mut empty_vp = SeismicViewport::new(0, 0, empty_bounds);
        assert_eq!(empty_vp.width_px(), 1);
        assert_eq!(empty_vp.height_px(), 1);
        assert_eq!(empty_vp.visible_trace_range(), 0..0);
        assert_eq!(empty_vp.visible_sample_range(), 0..0);
        assert!(empty_vp.compute_trace_axis_ticks(None, 50.0).is_empty());
        empty_vp.zoom_horizontal(f64::NAN, f64::INFINITY);
        empty_vp.zoom_vertical(-5.0, f64::NAN);
        empty_vp.pan_traces(f64::NAN);
        empty_vp.pan_time_ms(f64::INFINITY);
        empty_vp.pan_by_screen_drag_px(f32::NAN, f32::NEG_INFINITY);

        // Reversed trace orientation maps left screen edge to high trace index and right edge to low trace index
        let bounds = SectionDataBounds::new(100, 200, 2000);
        let mut vp = SeismicViewport::new(200, 200, bounds);
        vp.set_reverse_traces(true);
        assert_eq!(vp.pixel_to_trace_index(1.0), Some(99));
        assert_eq!(vp.pixel_to_trace_index(199.0), Some(0));

        let rev_ticks = vp.compute_trace_axis_ticks(None, 50.0);
        for w in rev_ticks.windows(2) {
            assert!(w[1].position_px >= w[0].position_px);
            assert!(w[1].value < w[0].value);
        }
    }
}
