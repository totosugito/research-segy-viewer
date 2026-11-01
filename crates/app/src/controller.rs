//! Headless application controller managing SEG-Y file state, viewport, rendering parameters,
//! progressive non-blocking rendering, cursor readout, and offscreen PNG export.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use render::{
    AgcConfig, AxisTick, ClipStrategy, ColormapKind, CursorReadout, DisplayMode, NormScope,
    Polarity, RenderLod, RenderedImage, SectionDataBounds, SectionRenderParams, SectionRenderer,
    SeismicViewport, TraceBalance, WiggleFillMode,
};
use segy_core::{AxisHeaderKey, SegyFile, TraceHeader};

use crate::cli::AppCliArgs;
use crate::error::{AppError, Result};

/// Default initial viewport canvas width in pixels.
pub const DEFAULT_VIEWPORT_WIDTH: u32 = 1000;

/// Default initial viewport canvas height in pixels.
pub const DEFAULT_VIEWPORT_HEIGHT: u32 = 680;

/// Rubber-band rectangles smaller than this (in pixels, either axis) are ignored.
pub const MIN_ZOOM_BOX_PX: f32 = 6.0;

/// Summary of the trace the user selected by clicking on the section.
#[derive(Debug, Clone, PartialEq)]
pub struct TraceSelection {
    /// 0-based trace index.
    pub trace_index: usize,
    /// 1-based trace number.
    pub trace_number: usize,
    /// Parsed trace header, when readable.
    pub header: Option<TraceHeader>,
    /// Minimum raw sample amplitude.
    pub min: f32,
    /// Maximum raw sample amplitude.
    pub max: f32,
    /// Root-mean-square raw sample amplitude.
    pub rms: f32,
}

#[derive(Debug, Clone)]
struct WorkerJob {
    revision: u64,
    lod: RenderLod,
    file: Arc<SegyFile>,
    viewport: SeismicViewport,
    params: SectionRenderParams,
}

#[derive(Debug)]
struct WorkerOutput {
    revision: u64,
    lod: RenderLod,
    result: render::Result<RenderedImage>,
}

/// Headless controller for the SEG-Y viewer UI.
pub struct AppController {
    file: Option<Arc<SegyFile>>,
    file_path: Option<PathBuf>,
    viewport: SeismicViewport,
    render_params: SectionRenderParams,
    renderer: SectionRenderer,
    last_cursor: Option<CursorReadout>,
    selected_trace: Option<TraceSelection>,
    last_error: Option<String>,
    status_message: Option<String>,
    state_revision: u64,
    rendered_revision: Option<u64>,
    rendered_lod: Option<RenderLod>,
    cached_image: Option<Arc<RenderedImage>>,
    image_generation: u64,
    render_pass_count: u64,
    worker_tx: Sender<WorkerJob>,
    worker_rx: Receiver<WorkerOutput>,
    in_flight: Option<(u64, RenderLod)>,
}

impl Default for AppController {
    fn default() -> Self {
        Self::new(DEFAULT_VIEWPORT_WIDTH, DEFAULT_VIEWPORT_HEIGHT)
    }
}

impl AppController {
    /// Creates a new [`AppController`] with initial viewport dimensions `width_px × height_px`.
    pub fn new(width_px: u32, height_px: u32) -> Self {
        let bounds = SectionDataBounds::new(1, 1, 1000);
        let mut viewport = SeismicViewport::new(width_px, height_px, bounds);
        // Allow immediate Wiggle/Overlay rendering at full section view (with min_wiggle_spacing_px
        // keeping wiggle density clean); the user can toggle `<2 px` auto-switch in the UI.
        viewport.set_auto_switch_wiggle_to_image(false);

        let (worker_tx, worker_rx) = spawn_render_worker();

        Self {
            file: None,
            file_path: None,
            viewport,
            render_params: SectionRenderParams::default(),
            renderer: SectionRenderer::new(),
            last_cursor: None,
            selected_trace: None,
            last_error: None,
            status_message: None,
            state_revision: 1,
            rendered_revision: None,
            rendered_lod: None,
            cached_image: None,
            image_generation: 0,
            render_pass_count: 0,
            worker_tx,
            worker_rx,
            in_flight: None,
        }
    }

    /// Applies initial startup CLI arguments (`app [OPTIONS] [FILE.sgy]`).
    pub fn apply_cli_args(&mut self, args: &AppCliArgs) -> Result<()> {
        if let Some(mode) = args.mode {
            self.set_display_mode(mode);
        }
        if let Some(gain_db) = args.gain_db {
            self.set_gain_db(gain_db);
        }
        if let Some(colormap) = args.colormap {
            self.set_colormap_kind(colormap);
        }
        if let Some(polarity) = args.polarity {
            self.set_polarity(polarity);
        }
        if let Some(path) = &args.file_path {
            self.open_file(path)?;
        }
        Ok(())
    }

    /// Opens and memory-maps a SEG-Y file from disk, resetting the viewport to fit the dataset.
    pub fn open_file(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let path_buf = path.as_ref().to_path_buf();
        match SegyFile::open(&path_buf) {
            Ok(segy) => {
                self.set_segy_file(Arc::new(segy), Some(path_buf));
                Ok(())
            }
            Err(err) => {
                let msg = format!("Failed to open {}: {err}", path_buf.display());
                self.last_error = Some(msg);
                Err(AppError::Segy(err))
            }
        }
    }

    /// Attaches an already-opened [`SegyFile`] to the controller.
    pub fn set_segy_file(&mut self, file: Arc<SegyFile>, path: Option<PathBuf>) {
        let bounds = SectionDataBounds::from_segy(&file);
        let width = self.viewport.width_px();
        let height = self.viewport.height_px();
        let mode = self.viewport.display_mode();
        let auto_switch = self.viewport.auto_switch_wiggle_to_image();
        let axis_key = self.viewport.horizontal_axis_key();

        let mut vp = SeismicViewport::new(width, height, bounds);
        vp.set_display_mode(mode);
        vp.set_auto_switch_wiggle_to_image(auto_switch);
        vp.set_horizontal_axis_key(axis_key);

        let summary = format!(
            "Loaded {} traces × {} samples (dt = {:.2} ms, {:?})",
            file.trace_count(),
            file.samples_per_trace(),
            bounds.dt_ms(),
            file.sample_format()
        );

        self.file = Some(file);
        self.file_path = path;
        self.viewport = vp;
        self.render_params.global_reference = None;
        self.last_cursor = None;
        self.selected_trace = None;
        self.last_error = None;
        self.status_message = Some(summary);
        self.cached_image = None;
        self.rendered_revision = None;
        self.rendered_lod = None;
        self.in_flight = None;
        self.mark_dirty();
    }

    /// Returns `true` if a SEG-Y file is currently loaded.
    #[inline]
    pub fn has_file(&self) -> bool {
        self.file.is_some()
    }

    /// Returns a reference to the currently loaded [`SegyFile`], if any.
    #[inline]
    pub fn file(&self) -> Option<&Arc<SegyFile>> {
        self.file.as_ref()
    }

    /// Returns the path of the currently loaded SEG-Y file, if any.
    #[inline]
    pub fn file_path(&self) -> Option<&Path> {
        self.file_path.as_deref()
    }

    /// Returns the current [`SeismicViewport`].
    #[inline]
    pub const fn viewport(&self) -> &SeismicViewport {
        &self.viewport
    }

    /// Returns the current [`SectionRenderParams`].
    #[inline]
    pub const fn render_params(&self) -> &SectionRenderParams {
        &self.render_params
    }

    /// Returns the most recent cursor readout, if the pointer is over the seismic plot.
    #[inline]
    pub const fn last_cursor_readout(&self) -> Option<CursorReadout> {
        self.last_cursor
    }

    /// Returns the last error message, if any.
    #[inline]
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Clears the last error message.
    #[inline]
    pub fn clear_error(&mut self) {
        self.last_error = None;
    }

    /// Returns the latest status message, if any.
    #[inline]
    pub fn status_message(&self) -> Option<&str> {
        self.status_message.as_deref()
    }

    /// Returns the monotonic state revision counter.
    #[inline]
    pub const fn state_revision(&self) -> u64 {
        self.state_revision
    }

    /// Returns the total number of render passes completed since controller creation.
    #[inline]
    pub const fn render_pass_count(&self) -> u64 {
        self.render_pass_count
    }

    /// Returns the monotonic generation of the cached [`RenderedImage`] (incremented on each update).
    #[inline]
    pub const fn image_generation(&self) -> u64 {
        self.image_generation
    }

    /// Returns the [`RenderLod`] of the currently cached image, if any.
    #[inline]
    pub const fn rendered_lod(&self) -> Option<RenderLod> {
        self.rendered_lod
    }

    /// Returns the currently cached viewport-sized [`RenderedImage`], if any.
    #[inline]
    pub fn cached_image(&self) -> Option<&Arc<RenderedImage>> {
        self.cached_image.as_ref()
    }

    /// Returns `true` if the cached image does not yet match `state_revision` at [`RenderLod::Full`].
    #[inline]
    pub fn needs_render(&self) -> bool {
        self.file.is_some()
            && (self.rendered_revision != Some(self.state_revision)
                || self.rendered_lod != Some(RenderLod::Full))
    }

    /// Updates the viewport canvas pixel dimensions (`width_px × height_px`).
    pub fn set_viewport_dimensions(&mut self, width_px: u32, height_px: u32) {
        let clamped_w = width_px.max(1);
        let clamped_h = height_px.max(1);
        if self.viewport.width_px() != clamped_w || self.viewport.height_px() != clamped_h {
            self.viewport.set_dimensions(clamped_w, clamped_h);
            self.mark_dirty();
        }
    }

    /// Sets the primary display mode (`Wiggle`, `Image`, or `Overlay`).
    pub fn set_display_mode(&mut self, mode: DisplayMode) {
        if self.viewport.display_mode() != mode {
            self.viewport.set_display_mode(mode);
            self.mark_dirty();
        }
    }

    /// Sets the display gain in decibels (`dB`).
    pub fn set_gain_db(&mut self, gain_db: f32) {
        if !gain_db.is_finite() {
            return;
        }
        let clamped = gain_db.clamp(-120.0, 120.0);
        if (self.render_params.amplitude.gain_db - clamped).abs() > 1e-5 {
            self.render_params.amplitude.gain_db = clamped;
            self.mark_dirty();
        }
    }

    /// Sets the amplitude clipping strategy (`Percentile`, `Rms`, `Peak`, or `Absolute`).
    pub fn set_clip_strategy(&mut self, clip: ClipStrategy) {
        if self.render_params.amplitude.clip != clip {
            self.render_params.amplitude.clip = clip;
            self.render_params.global_reference = None;
            self.mark_dirty();
        }
    }

    /// Sets the normalization scope (`Global` or `PerTrace`).
    pub fn set_norm_scope(&mut self, scope: NormScope) {
        if self.render_params.amplitude.scope != scope {
            self.render_params.amplitude.scope = scope;
            self.mark_dirty();
        }
    }

    /// Sets the display polarity (`Normal` or `Reversed`).
    pub fn set_polarity(&mut self, polarity: Polarity) {
        if self.render_params.amplitude.polarity != polarity {
            self.render_params.amplitude.polarity = polarity;
            self.mark_dirty();
        }
    }

    /// Toggles the display polarity between `Normal` and `Reversed`.
    pub fn toggle_polarity(&mut self) {
        let next = match self.render_params.amplitude.polarity {
            Polarity::Normal => Polarity::Reversed,
            Polarity::Reversed => Polarity::Normal,
        };
        self.set_polarity(next);
    }

    /// Sets the sliding-window AGC configuration.
    pub fn set_agc(&mut self, agc: AgcConfig) {
        if self.render_params.amplitude.agc != agc {
            self.render_params.amplitude.agc = agc;
            self.render_params.global_reference = None;
            self.mark_dirty();
        }
    }

    /// Sets whether per-trace DC offset removal is enabled.
    pub fn set_dc_removal(&mut self, enabled: bool) {
        if self.render_params.amplitude.dc_removal != enabled {
            self.render_params.amplitude.dc_removal = enabled;
            self.render_params.global_reference = None;
            self.mark_dirty();
        }
    }

    /// Sets the trace-to-trace energy balance mode.
    pub fn set_trace_balance(&mut self, balance: TraceBalance) {
        if self.render_params.amplitude.trace_balance != balance {
            self.render_params.amplitude.trace_balance = balance;
            self.render_params.global_reference = None;
            self.mark_dirty();
        }
    }

    /// Sets the variable-density colormap family (`Grayscale`, `SeismicRwb`, `Viridis`, `Rainbow`, `BrownWhiteBlack`).
    pub fn set_colormap_kind(&mut self, kind: ColormapKind) {
        if self.render_params.colormap.kind != kind {
            self.render_params.colormap.kind = kind;
            self.mark_dirty();
        }
    }

    /// Sets whether the variable-density colormap direction is reversed.
    pub fn set_colormap_reversed(&mut self, reversed: bool) {
        if self.render_params.colormap.reversed != reversed {
            self.render_params.colormap.reversed = reversed;
            self.mark_dirty();
        }
    }

    /// Sets the colormap center bias in `[-1.0, 1.0]`.
    pub fn set_colormap_bias(&mut self, bias: f32) {
        if !bias.is_finite() {
            return;
        }
        let clamped = bias.clamp(-1.0, 1.0);
        if (self.render_params.colormap.bias - clamped).abs() > 1e-5 {
            self.render_params.colormap.bias = clamped;
            self.mark_dirty();
        }
    }

    /// Sets the colormap contrast slope multiplier (`0.05..=50.0`).
    pub fn set_colormap_contrast(&mut self, contrast: f32) {
        if !contrast.is_finite() || contrast <= 0.0 {
            return;
        }
        let clamped = contrast.clamp(0.05, 50.0);
        if (self.render_params.colormap.contrast - clamped).abs() > 1e-5 {
            self.render_params.colormap.contrast = clamped;
            self.mark_dirty();
        }
    }

    /// Sets the variable-area wiggle lobe fill mode (`Positive`, `Negative`, `Both`, or `LineOnly`).
    pub fn set_wiggle_fill(&mut self, fill: WiggleFillMode) {
        if self.render_params.wiggle_fill != fill {
            self.render_params.wiggle_fill = fill;
            self.mark_dirty();
        }
    }

    /// Sets whether the continuous wiggle polyline stroke is drawn.
    pub fn set_wiggle_draw_line(&mut self, draw_line: bool) {
        if self.render_params.wiggle_draw_line != draw_line {
            self.render_params.wiggle_draw_line = draw_line;
            self.mark_dirty();
        }
    }

    /// Sets the wiggle excursion scale in trace spacings.
    pub fn set_wiggle_excursion_spacings(&mut self, spacings: f32) {
        if !spacings.is_finite() || spacings <= 0.0 {
            return;
        }
        let clamped = spacings.clamp(0.05, 20.0);
        if (self.render_params.wiggle_excursion_spacings - clamped).abs() > 1e-5 {
            self.render_params.wiggle_excursion_spacings = clamped;
            self.mark_dirty();
        }
    }

    /// Sets the symmetric wiggle excursion clip limit in trace spacings.
    pub fn set_wiggle_clip_spacings(&mut self, spacings: f32) {
        if !spacings.is_finite() || spacings <= 0.0 {
            return;
        }
        let clamped = spacings.clamp(0.1, 20.0);
        if (self.render_params.wiggle_clip_spacings - clamped).abs() > 1e-5 {
            self.render_params.wiggle_clip_spacings = clamped;
            self.mark_dirty();
        }
    }

    /// Enables or disables automatic `Wiggle -> Image` switching when traces are `< ~2.0` pixels apart.
    pub fn set_auto_switch_wiggle_to_image(&mut self, enabled: bool) {
        if self.viewport.auto_switch_wiggle_to_image() != enabled {
            self.viewport.set_auto_switch_wiggle_to_image(enabled);
            self.mark_dirty();
        }
    }

    /// Sets the user trace decimation step (`1` = every trace, `N` = every `N`-th trace).
    pub fn set_trace_decimate_step(&mut self, step: usize) {
        let clamped = step.max(1);
        if self.viewport.trace_decimate_step() != clamped {
            self.viewport.set_trace_decimate_step(clamped);
            self.mark_dirty();
        }
    }

    /// Sets whether horizontal trace order is reversed (right-to-left).
    pub fn set_reverse_traces(&mut self, reverse: bool) {
        if self.viewport.reverse_traces() != reverse {
            self.viewport.set_reverse_traces(reverse);
            self.mark_dirty();
        }
    }

    /// Sets the horizontal axis header key (`TraceNumber`, `Cdp`, `Inline`, `Crossline`, `Offset`, etc.).
    pub fn set_horizontal_axis_key(&mut self, key: AxisHeaderKey) {
        if self.viewport.horizontal_axis_key() != key {
            self.viewport.set_horizontal_axis_key(key);
        }
    }

    /// Sets the visible vertical time window in milliseconds.
    pub fn set_time_window_ms(&mut self, start_ms: f64, end_ms: f64) {
        let before = self.viewport.visible_sample_range();
        self.viewport.set_time_window_ms(start_ms, end_ms);
        if self.viewport.visible_sample_range() != before {
            self.mark_dirty();
        }
    }

    /// Pans the viewport in response to a mouse drag of `(drag_dx_px, drag_dy_px)` pixels.
    pub fn pan_by_screen_drag_px(&mut self, drag_dx_px: f32, drag_dy_px: f32) {
        if drag_dx_px == 0.0 && drag_dy_px == 0.0 {
            return;
        }
        let prev_t = self.viewport.visible_trace_range();
        let prev_s = self.viewport.visible_sample_range();
        self.viewport.pan_by_screen_drag_px(drag_dx_px, drag_dy_px);
        if self.viewport.visible_trace_range() != prev_t
            || self.viewport.visible_sample_range() != prev_s
        {
            self.mark_dirty();
        }
    }

    /// Zooms horizontally (`zoom_h`) and vertically (`zoom_v`) anchored at `(cursor_x_px, cursor_y_px)`.
    pub fn zoom_at_pixel(&mut self, zoom_h: f64, zoom_v: f64, cursor_x_px: f32, cursor_y_px: f32) {
        let prev_t = self.viewport.visible_trace_range();
        let prev_s = self.viewport.visible_sample_range();
        self.viewport
            .zoom_at_pixel(zoom_h, zoom_v, cursor_x_px, cursor_y_px);
        if self.viewport.visible_trace_range() != prev_t
            || self.viewport.visible_sample_range() != prev_s
        {
            self.mark_dirty();
        }
    }

    /// Zooms the viewport to the pixel rectangle `(x0, y0)`–`(x1, y1)` (viewport-local pixels).
    ///
    /// Returns `false` (and changes nothing) when the rectangle is smaller than
    /// [`MIN_ZOOM_BOX_PX`] on either axis or does not map onto data.
    pub fn zoom_to_pixel_rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) -> bool {
        if !self.has_file()
            || (x1 - x0).abs() < MIN_ZOOM_BOX_PX
            || (y1 - y0).abs() < MIN_ZOOM_BOX_PX
        {
            return false;
        }
        let max_x = (self.viewport.width_px() as f32 - 0.01).max(0.0);
        let max_y = (self.viewport.height_px() as f32 - 0.01).max(0.0);
        let (xa, xb) = (x0.clamp(0.0, max_x), x1.clamp(0.0, max_x));
        let (ya, yb) = (y0.clamp(0.0, max_y), y1.clamp(0.0, max_y));
        let (Some(ta), Some(tb)) = (
            self.viewport.pixel_to_trace_index(xa),
            self.viewport.pixel_to_trace_index(xb),
        ) else {
            return false;
        };
        let (Some(ma), Some(mb)) = (
            self.viewport.pixel_to_time_ms(ya),
            self.viewport.pixel_to_time_ms(yb),
        ) else {
            return false;
        };
        let prev_t = self.viewport.visible_trace_range();
        let prev_s = self.viewport.visible_sample_range();
        self.viewport
            .set_trace_window(ta.min(tb), ta.max(tb).saturating_add(1));
        self.viewport.set_time_window_ms(ma, mb);
        if self.viewport.visible_trace_range() != prev_t
            || self.viewport.visible_sample_range() != prev_s
        {
            self.mark_dirty();
        }
        true
    }

    /// Returns the currently selected trace summary, if any.
    pub fn selected_trace(&self) -> Option<&TraceSelection> {
        self.selected_trace.as_ref()
    }

    /// Selects trace `trace_index` (0-based) and computes its amplitude summary.
    /// Returns `false` when the index is out of range or no file is loaded.
    pub fn select_trace(&mut self, trace_index: usize) -> bool {
        let Some(file) = self.file.as_deref() else {
            return false;
        };
        if trace_index >= file.trace_count() {
            return false;
        }
        let (mut min, mut max, mut sum_sq, mut n) =
            (f32::INFINITY, f32::NEG_INFINITY, 0.0_f64, 0_u64);
        if let Ok(samples) = file.read_trace_samples(trace_index) {
            for v in samples.into_iter().filter(|v| v.is_finite()) {
                min = min.min(v);
                max = max.max(v);
                sum_sq += f64::from(v) * f64::from(v);
                n += 1;
            }
        }
        let (min, max, rms) = if n == 0 {
            (0.0, 0.0, 0.0)
        } else {
            (min, max, (sum_sq / n as f64).sqrt() as f32)
        };
        self.selected_trace = Some(TraceSelection {
            trace_index,
            trace_number: trace_index + 1,
            header: file.trace_header(trace_index).ok(),
            min,
            max,
            rms,
        });
        true
    }

    /// Clicking at viewport-local `x_px`: selects the trace under it, or deselects when it is
    /// already the selected trace. Returns the selected trace index afterwards.
    pub fn click_select_at_pixel(&mut self, x_px: f32) -> Option<usize> {
        let idx = self.viewport.pixel_to_trace_index(x_px)?;
        if self.selected_trace.as_ref().map(|s| s.trace_index) == Some(idx) {
            self.selected_trace = None;
            return None;
        }
        self.select_trace(idx).then_some(idx)
    }

    /// Clears the trace selection.
    pub fn clear_selection(&mut self) {
        self.selected_trace = None;
    }

    /// Moves the selection by `delta` traces (clamped to the file) and pans the view so the
    /// selected trace stays visible. Does nothing without an existing selection.
    pub fn move_selection(&mut self, delta: isize) {
        let (Some(sel), Some(file)) = (self.selected_trace.as_ref(), self.file.as_deref()) else {
            return;
        };
        let last = file.trace_count().saturating_sub(1);
        let next = sel.trace_index.saturating_add_signed(delta).min(last);
        if !self.select_trace(next) {
            return;
        }
        let range = self.viewport.visible_trace_range();
        if next < range.start {
            self.viewport.pan_traces(next as f64 - range.start as f64);
            self.mark_dirty();
        } else if next >= range.end {
            self.viewport
                .pan_traces((next + 1) as f64 - range.end as f64);
            self.mark_dirty();
        }
    }

    /// Viewport-local x pixel of the selected trace, if it is inside the visible trace range.
    pub fn selected_trace_pixel_x(&self) -> Option<f32> {
        let sel = self.selected_trace.as_ref()?;
        let range = self.viewport.visible_trace_range();
        range
            .contains(&sel.trace_index)
            .then(|| self.viewport.trace_index_to_pixel_x(sel.trace_index))
    }

    /// Resets both horizontal and vertical axes to fit the full dataset.
    pub fn fit_to_data(&mut self) {
        let prev_t = self.viewport.visible_trace_range();
        let prev_s = self.viewport.visible_sample_range();
        self.viewport.fit_to_data();
        if self.viewport.visible_trace_range() != prev_t
            || self.viewport.visible_sample_range() != prev_s
        {
            self.mark_dirty();
        }
    }

    /// Resets the horizontal axis to cover all traces.
    pub fn fit_horizontal(&mut self) {
        let prev_t = self.viewport.visible_trace_range();
        self.viewport.fit_horizontal();
        if self.viewport.visible_trace_range() != prev_t {
            self.mark_dirty();
        }
    }

    /// Resets the vertical axis to cover the full time/sample range.
    pub fn fit_vertical(&mut self) {
        let prev_s = self.viewport.visible_sample_range();
        self.viewport.fit_vertical();
        if self.viewport.visible_sample_range() != prev_s {
            self.mark_dirty();
        }
    }

    /// Computes vertical time-axis ticks (`ms`) for the current viewport.
    pub fn time_axis_ticks(&self, target_spacing_px: f32) -> Vec<AxisTick> {
        self.viewport.compute_time_axis_ticks(target_spacing_px)
    }

    /// Computes horizontal trace/header-axis ticks for the current viewport.
    pub fn trace_axis_ticks(&self, target_spacing_px: f32) -> Vec<AxisTick> {
        self.viewport
            .compute_trace_axis_ticks(self.file.as_deref(), target_spacing_px)
    }

    /// Updates and returns the cursor readout at viewport pixel coordinate `(x_px, y_px)`.
    pub fn update_cursor_at_pixel(&mut self, x_px: f32, y_px: f32) -> Option<CursorReadout> {
        let file = self.file.as_deref()?;
        let readout = self.viewport.cursor_readout(file, Some(file), x_px, y_px);
        self.last_cursor = readout;
        readout
    }

    /// Clears the cursor readout when the pointer leaves the seismic plot area.
    pub fn clear_cursor(&mut self) {
        self.last_cursor = None;
    }

    /// Executes one synchronous progressive render step if the view/parameters have changed:
    /// - Step 1 after a change renders a fast `RenderLod::Coarse` preview into a viewport-sized buffer.
    /// - Step 2 refines the buffer to `RenderLod::Full`.
    /// - Subsequent calls when nothing has changed do zero work and return `Ok(false)`.
    pub fn step_progressive_render(&mut self) -> Result<bool> {
        let Some(file) = self.file.clone() else {
            return Ok(false);
        };
        let Some(next_lod) = self.next_needed_lod() else {
            return Ok(false);
        };

        let mut params = self.render_params;
        params.lod = next_lod;
        let img = self
            .renderer
            .render(file.as_ref(), &self.viewport, &params)?;
        self.commit_rendered_image(self.state_revision, next_lod, img);
        Ok(true)
    }

    /// Renders `RenderLod::Full` synchronously if the cached image is not already up-to-date at `Full`
    /// for the current state revision, returning the cached [`RenderedImage`].
    pub fn render_full_now(&mut self) -> Result<Arc<RenderedImage>> {
        let Some(file) = self.file.clone() else {
            return Err(AppError::NoFileLoaded);
        };
        if self.rendered_revision == Some(self.state_revision)
            && self.rendered_lod == Some(RenderLod::Full)
            && let Some(cached) = &self.cached_image
        {
            return Ok(Arc::clone(cached));
        }

        let mut params = self.render_params;
        params.lod = RenderLod::Full;
        let img = self
            .renderer
            .render(file.as_ref(), &self.viewport, &params)?;
        self.commit_rendered_image(self.state_revision, RenderLod::Full, img);
        self.cached_image
            .as_ref()
            .cloned()
            .ok_or(AppError::NoFileLoaded)
    }

    /// Polls the non-blocking background render worker and dispatches progressive (`Coarse` then `Full`)
    /// jobs only when the view or parameters have changed.
    ///
    /// Returns `true` if a background render or refinement pass is still pending (indicating that
    /// the UI should request a repaint).
    pub fn poll_background_render(&mut self) -> bool {
        while let Ok(output) = self.worker_rx.try_recv() {
            if self.in_flight == Some((output.revision, output.lod)) {
                self.in_flight = None;
            }
            match output.result {
                Ok(img) => {
                    // Accept if this matches the current revision or if we have no image yet.
                    if output.revision == self.state_revision || self.cached_image.is_none() {
                        self.commit_rendered_image(output.revision, output.lod, img);
                    }
                }
                Err(err) => {
                    self.last_error = Some(err.to_string());
                }
            }
        }

        let Some(file) = self.file.as_ref() else {
            return false;
        };
        let Some(next_lod) = self.next_needed_lod() else {
            return false;
        };

        let target = (self.state_revision, next_lod);
        if self.in_flight != Some(target) {
            let job = WorkerJob {
                revision: self.state_revision,
                lod: next_lod,
                file: Arc::clone(file),
                viewport: self.viewport.clone(),
                params: self.render_params.with_lod(next_lod),
            };
            if self.worker_tx.send(job).is_ok() {
                self.in_flight = Some(target);
            } else {
                // Fallback if worker channel closed: step synchronously.
                let _ = self.step_progressive_render();
            }
        }

        self.needs_render() || self.in_flight.is_some()
    }

    /// Invalidates the cached rendered image so the next render pass recomputes from scratch.
    pub fn invalidate_render_cache(&mut self) {
        self.rendered_revision = None;
        self.rendered_lod = None;
        self.cached_image = None;
        self.mark_dirty();
    }

    /// Renders the current viewport at full quality offscreen and saves it to `path` as a PNG.
    pub fn save_png(&mut self, path: impl AsRef<Path>) -> Result<RenderedImage> {
        let width = self.viewport.width_px();
        let height = self.viewport.height_px();
        self.save_png_with_dimensions(path, width, height)
    }

    /// Renders the current view and parameters offscreen at arbitrary resolution `width × height`
    /// and saves the resulting RGBA8 PNG to `path`.
    pub fn save_png_with_dimensions(
        &mut self,
        path: impl AsRef<Path>,
        width: u32,
        height: u32,
    ) -> Result<RenderedImage> {
        let Some(file) = self.file.clone() else {
            return Err(AppError::NoFileLoaded);
        };
        if let Some(parent) = path.as_ref().parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let img = if width == self.viewport.width_px()
            && height == self.viewport.height_px()
            && self.rendered_revision == Some(self.state_revision)
            && self.rendered_lod == Some(RenderLod::Full)
            && let Some(cached) = &self.cached_image
        {
            (**cached).clone()
        } else {
            let mut offscreen_vp = self.viewport.clone();
            offscreen_vp.set_dimensions(width, height);
            let params = self.render_params.with_lod(RenderLod::Full);
            self.renderer
                .render(file.as_ref(), &offscreen_vp, &params)?
        };
        img.save_png(path.as_ref())?;
        self.status_message = Some(format!(
            "Saved {}×{} PNG to {}",
            img.width,
            img.height,
            path.as_ref().display()
        ));
        Ok(img)
    }

    fn next_needed_lod(&self) -> Option<RenderLod> {
        self.file.as_ref()?;
        if self.rendered_revision != Some(self.state_revision) {
            Some(RenderLod::Coarse)
        } else if self.rendered_lod != Some(RenderLod::Full) {
            Some(RenderLod::Full)
        } else {
            None
        }
    }

    fn commit_rendered_image(&mut self, revision: u64, lod: RenderLod, img: RenderedImage) {
        self.rendered_revision = Some(revision);
        self.rendered_lod = Some(lod);
        self.cached_image = Some(Arc::new(img));
        self.image_generation = self.image_generation.wrapping_add(1);
        self.render_pass_count = self.render_pass_count.wrapping_add(1);
    }

    #[inline]
    fn mark_dirty(&mut self) {
        self.state_revision = self.state_revision.wrapping_add(1);
    }
}

fn spawn_render_worker() -> (Sender<WorkerJob>, Receiver<WorkerOutput>) {
    let (job_tx, job_rx) = mpsc::channel::<WorkerJob>();
    let (out_tx, out_rx) = mpsc::channel::<WorkerOutput>();

    std::thread::spawn(move || {
        let mut renderer = SectionRenderer::new();
        while let Ok(mut job) = job_rx.recv() {
            // Drain any newer jobs queued while the previous render was running so we never
            // waste time rendering stale intermediate slider/pan states.
            while let Ok(newer) = job_rx.try_recv() {
                job = newer;
            }
            let result = renderer.render(job.file.as_ref(), &job.viewport, &job.params);
            let output = WorkerOutput {
                revision: job.revision,
                lod: job.lod,
                result,
            };
            if out_tx.send(output).is_err() {
                break;
            }
        }
    });

    (job_tx, out_rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use segy_core::{FILE_HEADER_LEN, TEXTUAL_HEADER_LEN, TRACE_HEADER_LEN};

    fn make_synthetic_segy(traces: usize, samples: usize) -> Result<Arc<SegyFile>> {
        let mut bytes = vec![0u8; FILE_HEADER_LEN];
        bytes[..TEXTUAL_HEADER_LEN].fill(0x40);
        let bin = &mut bytes[TEXTUAL_HEADER_LEN..FILE_HEADER_LEN];
        bin[16..18].copy_from_slice(&4000_u16.to_be_bytes()); // 4 ms
        bin[20..22].copy_from_slice(&(samples as u16).to_be_bytes());
        bin[24..26].copy_from_slice(&5_u16.to_be_bytes()); // IEEE f32
        bin[300..302].copy_from_slice(&0x0100_u16.to_be_bytes());
        bin[302..304].copy_from_slice(&1_u16.to_be_bytes());

        for t in 0..traces {
            let mut th = [0u8; TRACE_HEADER_LEN];
            th[0..4].copy_from_slice(&((t as i32) + 1).to_be_bytes());
            th[4..8].copy_from_slice(&((t as i32) + 1).to_be_bytes());
            th[20..24].copy_from_slice(&(1000 + (t as i32) * 10).to_be_bytes()); // CDP
            th[36..40].copy_from_slice(&((t as i32) * 50).to_be_bytes()); // Offset
            th[114..116].copy_from_slice(&(samples as u16).to_be_bytes());
            th[116..118].copy_from_slice(&4000_u16.to_be_bytes());
            bytes.extend_from_slice(&th);
            for s in 0..samples {
                let val = (((s as f32) * 0.25 + (t as f32) * 0.35).sin()) * 50.0;
                bytes.extend_from_slice(&val.to_be_bytes());
            }
        }

        let segy = SegyFile::from_bytes(bytes)?;
        Ok(Arc::new(segy))
    }

    #[test]
    fn rect_zoom_maps_pixels_to_trace_and_time_window_and_ignores_tiny_boxes() -> Result<()> {
        let mut ctrl = AppController::new(200, 100);
        ctrl.set_segy_file(make_synthetic_segy(100, 200)?, None);

        assert!(!ctrl.zoom_to_pixel_rect(10.0, 10.0, 12.0, 80.0));
        assert!(!ctrl.zoom_to_pixel_rect(10.0, 10.0, 80.0, 13.0));
        assert_eq!(ctrl.viewport().visible_trace_range(), 0..100);

        // Right half horizontally, top half vertically.
        assert!(ctrl.zoom_to_pixel_rect(100.0, 0.0, 199.0, 50.0));
        let traces = ctrl.viewport().visible_trace_range();
        assert!(traces.start >= 49 && traces.start <= 51, "{traces:?}");
        assert_eq!(traces.end, 100);
        let (t0, t1) = ctrl.viewport().visible_time_window_ms();
        assert!(t0 < 1.0);
        assert!(t1 < 0.55 * 200.0 * 4.0, "t1={t1}");

        // Reversed corner order gives the same window.
        let mut ctrl2 = AppController::new(200, 100);
        ctrl2.set_segy_file(make_synthetic_segy(100, 200)?, None);
        assert!(ctrl2.zoom_to_pixel_rect(199.0, 50.0, 100.0, 0.0));
        assert_eq!(
            ctrl2.viewport().visible_trace_range(),
            ctrl.viewport().visible_trace_range()
        );

        ctrl.fit_to_data();
        assert_eq!(ctrl.viewport().visible_trace_range(), 0..100);
        Ok(())
    }

    #[test]
    fn click_selects_toggles_and_arrow_moves_selection() -> Result<()> {
        let mut ctrl = AppController::new(200, 100);
        assert_eq!(ctrl.click_select_at_pixel(50.0), None);
        ctrl.set_segy_file(make_synthetic_segy(100, 200)?, None);

        let idx = ctrl.click_select_at_pixel(50.0);
        assert_eq!(idx, Some(25));
        let sel = ctrl
            .selected_trace()
            .cloned()
            .ok_or(AppError::NoFileLoaded)?;
        assert_eq!(sel.trace_number, 26);
        assert!(sel.max > sel.min && sel.rms > 0.0);
        assert_eq!(sel.header.map(|h| h.ensemble_number), Some(1000 + 25 * 10));
        assert!(ctrl.selected_trace_pixel_x().is_some());

        // Clicking the same trace toggles off.
        assert_eq!(ctrl.click_select_at_pixel(50.0), None);
        assert!(ctrl.selected_trace().is_none());

        ctrl.select_trace(0);
        ctrl.move_selection(-1);
        assert_eq!(ctrl.selected_trace().map(|s| s.trace_index), Some(0));
        ctrl.move_selection(5);
        assert_eq!(ctrl.selected_trace().map(|s| s.trace_index), Some(5));
        ctrl.move_selection(1_000);
        assert_eq!(ctrl.selected_trace().map(|s| s.trace_index), Some(99));
        assert!(!ctrl.select_trace(100));
        ctrl.clear_selection();
        assert!(ctrl.selected_trace().is_none());
        Ok(())
    }

    #[test]
    fn progressive_rendering_runs_coarse_then_full() -> Result<()> {
        let mut ctrl = AppController::new(160, 120);
        assert!(!ctrl.has_file());
        assert!(!ctrl.step_progressive_render()?);

        let segy = make_synthetic_segy(24, 64)?;
        ctrl.set_segy_file(segy, None);
        assert!(ctrl.has_file());
        assert!(ctrl.needs_render());

        // Pass 1: Coarse LOD preview
        assert!(ctrl.step_progressive_render()?);
        assert_eq!(ctrl.rendered_lod(), Some(RenderLod::Coarse));
        assert_eq!(ctrl.render_pass_count(), 1);
        if let Some(img) = ctrl.cached_image() {
            assert_eq!(img.width, 160);
            assert_eq!(img.height, 120);
        }

        // Pass 2: Full LOD refinement
        assert!(ctrl.step_progressive_render()?);
        assert_eq!(ctrl.rendered_lod(), Some(RenderLod::Full));
        assert_eq!(ctrl.render_pass_count(), 2);
        assert!(!ctrl.needs_render());
        Ok(())
    }

    #[test]
    fn unchanged_state_skips_rerender_until_parameter_changes() -> Result<()> {
        let mut ctrl = AppController::new(160, 120);
        let segy = make_synthetic_segy(24, 64)?;
        ctrl.set_segy_file(segy, None);

        let _ = ctrl.render_full_now()?;
        assert_eq!(ctrl.render_pass_count(), 1);
        assert!(!ctrl.needs_render());

        // Unchanged state -> zero work!
        assert!(!ctrl.step_progressive_render()?);
        assert_eq!(ctrl.render_pass_count(), 1);

        // Setting the exact same mode or gain does NOT mark dirty or re-render.
        ctrl.set_display_mode(DisplayMode::Image);
        ctrl.set_gain_db(0.0);
        assert!(!ctrl.needs_render());
        assert!(!ctrl.step_progressive_render()?);
        assert_eq!(ctrl.render_pass_count(), 1);

        // Changing mode to Wiggle marks dirty and triggers Coarse -> Full again.
        ctrl.set_display_mode(DisplayMode::Wiggle);
        assert!(ctrl.needs_render());
        assert!(ctrl.step_progressive_render()?);
        assert!(ctrl.step_progressive_render()?);
        assert_eq!(ctrl.rendered_lod(), Some(RenderLod::Full));
        assert_eq!(ctrl.render_pass_count(), 3);
        Ok(())
    }

    #[test]
    fn controller_controls_pan_zoom_axes_and_cursor_readout() -> Result<()> {
        let mut ctrl = AppController::new(200, 100);
        let segy = make_synthetic_segy(40, 50)?;
        ctrl.set_segy_file(segy, None);

        // Mode toggles, gain, clip, colormap, polarity
        ctrl.set_display_mode(DisplayMode::Overlay);
        ctrl.set_gain_db(6.0);
        ctrl.set_clip_strategy(ClipStrategy::Rms { multiple: 2.0 });
        ctrl.set_norm_scope(NormScope::PerTrace);
        ctrl.set_colormap_kind(ColormapKind::SeismicRwb);
        ctrl.set_colormap_reversed(true);
        ctrl.toggle_polarity();
        assert_eq!(ctrl.render_params().amplitude.polarity, Polarity::Reversed);

        let img = ctrl.render_full_now()?;
        assert_eq!(img.width, 200);
        assert_eq!(img.height, 100);

        // Pan and zoom
        ctrl.zoom_at_pixel(2.0, 2.0, 100.0, 50.0);
        assert!(ctrl.viewport().zoom_ratio_horizontal() > 1.9);
        assert!(ctrl.viewport().zoom_ratio_vertical() > 1.9);

        ctrl.pan_by_screen_drag_px(-20.0, -10.0);
        assert!(ctrl.needs_render());

        // Axes ticks and cursor readout
        ctrl.set_horizontal_axis_key(AxisHeaderKey::Cdp);
        let time_ticks = ctrl.time_axis_ticks(30.0);
        let trace_ticks = ctrl.trace_axis_ticks(50.0);
        assert!(!time_ticks.is_empty());
        assert!(!trace_ticks.is_empty());

        let readout = ctrl.update_cursor_at_pixel(50.0, 25.0);
        assert!(readout.is_some());
        if let Some(r) = readout {
            assert!(r.trace_number >= 1 && r.trace_number <= 40);
            assert!(r.header_value >= 1000);
            assert!(r.time_ms >= 0.0);
            assert!(r.amplitude.is_finite());
        }

        ctrl.fit_to_data();
        assert_eq!(ctrl.viewport().visible_trace_range(), 0..40);
        assert_eq!(ctrl.viewport().visible_sample_range(), 0..50);
        Ok(())
    }
}
