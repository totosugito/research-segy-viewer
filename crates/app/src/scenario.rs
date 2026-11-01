//! Live and headless automated UI scenario runner (`--scenario smoke` and `--scenario-fast`).
//!
//! Executes a deterministic sequence of user-visible steps (mode switches, H/V zoom, pan,
//! gain, colormap, polarity, reverse traces, horizontal axis key change, and PNG export to
//! `refs/out/`), printing one line per step with frame time in milliseconds and failing
//! with a clear message on any panic, error, blank render, or frame time > 50 ms (warning > 16 ms).

use std::io::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Pos2, RawInput, Rect, Vec2};
use render::{ColormapKind, DisplayMode, Polarity};
use segy_core::AxisHeaderKey;

use crate::cli::{AppCliArgs, ScenarioName};
use crate::controller::AppController;
use crate::count_distinct_colors;
use crate::error::{AppError, Result};
use crate::gui::SegyViewerApp;

/// Default visual pause between steps in live window mode (`--scenario smoke`).
pub const DEFAULT_SCENARIO_STEP_PAUSE_MS: u64 = 700;

/// Frame time threshold (in milliseconds) above which a step emits a `WARN` tag.
pub const SCENARIO_WARN_FRAME_MS: f64 = 16.0;

/// Maximum allowed frame time (in milliseconds) per step before failing the scenario.
pub const SCENARIO_MAX_FRAME_MS: f64 = 50.0;

/// Default PNG output path written during the `smoke` scenario's Save PNG step.
pub const DEFAULT_SCENARIO_PNG_PATH: &str = "refs/out/smoke_section.png";

/// Resolves `refs/out/smoke_section.png` relative to the workspace root so both CLI runs and
/// crate-level `cargo test` runs write to the root `refs/out/` directory.
pub fn resolve_scenario_png_path() -> PathBuf {
    let rel = Path::new(DEFAULT_SCENARIO_PNG_PATH);
    if Path::new("crates").is_dir() {
        return rel.to_path_buf();
    }
    let workspace_root = Path::new("../..");
    if workspace_root.join("crates").is_dir() {
        return workspace_root.join(rel);
    }
    rel.to_path_buf()
}

/// Ordered steps executed by [`ScenarioName::Smoke`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScenarioStepKind {
    /// Step 1: Verify loaded file and render initial full-section Image display.
    OpenInitialImage,
    /// Step 2: Switch to Wiggle display mode.
    SwitchModeWiggle,
    /// Step 3: Switch to Wiggle + Image Overlay display mode.
    SwitchModeOverlay,
    /// Step 4: Switch back to variable-density Image display mode.
    SwitchModeImage,
    /// Step 5: Zoom in horizontally (2.5×) and vertically (2.0×).
    ZoomHvIn,
    /// Step 6: Pan across traces and time samples.
    PanSection,
    /// Step 7: Rubber-band zoom into the central half of the visible section.
    RectZoom,
    /// Step 8: Select the trace under the section centre, like a mouse click.
    SelectTrace,
    /// Step 9: Increase display gain to +6.0 dB.
    GainPlus6Db,
    /// Step 8: Switch colormap to diverging Seismic (Blue-White-Red).
    ColormapSeismic,
    /// Step 9: Flip display polarity to Reversed.
    PolarityReversed,
    /// Step 10: Reverse horizontal trace order (right-to-left).
    ReverseTraces,
    /// Step 11: Switch X-axis header key to CDP / Ensemble and fit full section in Overlay mode.
    XAxisKeyCdp,
    /// Step 12: Save PNG snapshot to `refs/out/smoke_section.png` and verify non-blank output.
    SavePngRefsOut,
}

impl ScenarioStepKind {
    /// Complete ordered step sequence for [`ScenarioName::Smoke`].
    pub const ALL: [Self; 14] = [
        Self::OpenInitialImage,
        Self::SwitchModeWiggle,
        Self::SwitchModeOverlay,
        Self::SwitchModeImage,
        Self::ZoomHvIn,
        Self::PanSection,
        Self::RectZoom,
        Self::SelectTrace,
        Self::GainPlus6Db,
        Self::ColormapSeismic,
        Self::PolarityReversed,
        Self::ReverseTraces,
        Self::XAxisKeyCdp,
        Self::SavePngRefsOut,
    ];

    /// Short machine- and human-readable step identifier printed in the scenario log.
    #[inline]
    pub const fn name(self) -> &'static str {
        match self {
            Self::OpenInitialImage => "open_initial_image",
            Self::SwitchModeWiggle => "switch_mode_wiggle",
            Self::SwitchModeOverlay => "switch_mode_overlay",
            Self::SwitchModeImage => "switch_mode_image",
            Self::ZoomHvIn => "zoom_hv_in",
            Self::PanSection => "pan_section",
            Self::RectZoom => "rect_zoom",
            Self::SelectTrace => "select_trace",
            Self::GainPlus6Db => "gain_plus_6db",
            Self::ColormapSeismic => "colormap_seismic",
            Self::PolarityReversed => "polarity_reversed",
            Self::ReverseTraces => "reverse_traces",
            Self::XAxisKeyCdp => "xaxis_key_cdp",
            Self::SavePngRefsOut => "save_png_refs_out",
        }
    }

    /// Applies the pre-render controller state transition for this step.
    pub fn apply(self, ctrl: &mut AppController) -> Result<String> {
        if let Some(err) = ctrl.last_error() {
            return Err(AppError::ScenarioFailed(err.to_string()));
        }
        if !ctrl.has_file() {
            return Err(AppError::NoFileLoaded);
        }

        match self {
            Self::OpenInitialImage => {
                ctrl.set_display_mode(DisplayMode::Image);
                ctrl.fit_to_data();
                ctrl.invalidate_render_cache();
                let vp = ctrl.viewport();
                Ok(format!(
                    "mode=Image traces={:?} samples={:?}",
                    vp.visible_trace_range(),
                    vp.visible_sample_range()
                ))
            }
            Self::SwitchModeWiggle => {
                ctrl.set_display_mode(DisplayMode::Wiggle);
                let vp = ctrl.viewport();
                Ok(format!(
                    "mode=Wiggle effective={:?}",
                    vp.effective_display_mode()
                ))
            }
            Self::SwitchModeOverlay => {
                ctrl.set_display_mode(DisplayMode::Overlay);
                let vp = ctrl.viewport();
                Ok(format!(
                    "mode=Overlay effective={:?}",
                    vp.effective_display_mode()
                ))
            }
            Self::SwitchModeImage => {
                ctrl.set_display_mode(DisplayMode::Image);
                Ok("mode=Image".to_string())
            }
            Self::ZoomHvIn => {
                let cx = (ctrl.viewport().width_px() as f32) * 0.5;
                let cy = (ctrl.viewport().height_px() as f32) * 0.5;
                ctrl.zoom_at_pixel(2.5, 2.0, cx, cy);
                let vp = ctrl.viewport();
                Ok(format!(
                    "zoom_h={:.2}x zoom_v={:.2}x traces={:?}",
                    vp.zoom_ratio_horizontal(),
                    vp.zoom_ratio_vertical(),
                    vp.visible_trace_range()
                ))
            }
            Self::PanSection => {
                ctrl.pan_by_screen_drag_px(-120.0, -80.0);
                let vp = ctrl.viewport();
                Ok(format!(
                    "pan traces={:?} samples={:?}",
                    vp.visible_trace_range(),
                    vp.visible_sample_range()
                ))
            }
            Self::RectZoom => {
                let before = ctrl.viewport().visible_trace_range();
                let (w, h) = (
                    ctrl.viewport().width_px() as f32,
                    ctrl.viewport().height_px() as f32,
                );
                if !ctrl.zoom_to_pixel_rect(w * 0.25, h * 0.25, w * 0.75, h * 0.75) {
                    return Err(AppError::ScenarioFailed(
                        "rectangle zoom was rejected".to_string(),
                    ));
                }
                let vp = ctrl.viewport();
                let after = vp.visible_trace_range();
                if after.len() >= before.len() {
                    return Err(AppError::ScenarioFailed(format!(
                        "rectangle zoom did not shrink trace range {before:?} -> {after:?}"
                    )));
                }
                Ok(format!(
                    "rect_zoom traces={after:?} samples={:?}",
                    vp.visible_sample_range()
                ))
            }
            Self::SelectTrace => {
                ctrl.clear_selection();
                let x = ctrl.viewport().width_px() as f32 * 0.5;
                let Some(idx) = ctrl.click_select_at_pixel(x) else {
                    return Err(AppError::ScenarioFailed(
                        "click did not select a trace".to_string(),
                    ));
                };
                let rms = ctrl.selected_trace().map_or(0.0, |sel| sel.rms);
                Ok(format!("selected trace_index={idx} rms={rms:.3e}"))
            }
            Self::GainPlus6Db => {
                ctrl.set_gain_db(6.0);
                Ok(format!(
                    "gain_db={:+.1}",
                    ctrl.render_params().amplitude.gain_db
                ))
            }
            Self::ColormapSeismic => {
                ctrl.set_colormap_kind(ColormapKind::SeismicRwb);
                Ok(format!("colormap={:?}", ctrl.render_params().colormap.kind))
            }
            Self::PolarityReversed => {
                ctrl.set_polarity(Polarity::Reversed);
                Ok(format!(
                    "polarity={:?}",
                    ctrl.render_params().amplitude.polarity
                ))
            }
            Self::ReverseTraces => {
                ctrl.set_reverse_traces(true);
                Ok(format!(
                    "reverse_traces={}",
                    ctrl.viewport().reverse_traces()
                ))
            }
            Self::XAxisKeyCdp => {
                ctrl.set_horizontal_axis_key(AxisHeaderKey::Cdp);
                ctrl.set_reverse_traces(false);
                ctrl.set_polarity(Polarity::Normal);
                ctrl.set_gain_db(0.0);
                ctrl.fit_to_data();
                ctrl.set_display_mode(DisplayMode::Overlay);
                let ticks = ctrl.trace_axis_ticks(90.0);
                let first_label = ticks.first().map_or("none", |t| t.label.as_str());
                Ok(format!(
                    "xaxis={:?} first_tick={} mode=Overlay",
                    ctrl.viewport().horizontal_axis_key(),
                    first_label
                ))
            }
            Self::SavePngRefsOut => Ok(format!("out={DEFAULT_SCENARIO_PNG_PATH}")),
        }
    }

    /// Executes any post-render validation or disk output for this step.
    pub fn finish_post_render(self, ctrl: &mut AppController) -> Result<()> {
        if self == Self::SavePngRefsOut {
            let out_path = resolve_scenario_png_path();
            let saved = ctrl.save_png(&out_path)?;
            let distinct = count_distinct_colors(&saved, 16);
            if distinct < 2 {
                return Err(AppError::ScenarioFailed(format!(
                    "saved PNG {} is blank ({distinct} distinct color)",
                    out_path.display()
                )));
            }
            let meta = std::fs::metadata(&out_path)?;
            if meta.len() <= 64 {
                return Err(AppError::ScenarioFailed(format!(
                    "saved PNG {} is unexpectedly small ({} bytes)",
                    out_path.display(),
                    meta.len()
                )));
            }
        }
        Ok(())
    }
}

/// Measured outcome of a single scenario step.
#[derive(Debug, Clone, PartialEq)]
pub struct ScenarioStepRecord {
    /// 1-based step index.
    pub index: usize,
    /// Total number of steps in the scenario.
    pub total: usize,
    /// Step identifier name.
    pub name: &'static str,
    /// Step state summary string.
    pub detail: String,
    /// Measured end-to-end step frame time in milliseconds.
    pub frame_ms: f64,
    /// Number of distinct RGBA colors observed in the rendered viewport buffer (capped at 16).
    pub distinct_colors: usize,
    /// `true` if `frame_ms > SCENARIO_WARN_FRAME_MS` (`16.0` ms).
    pub warned: bool,
    /// `true` if the step succeeded with non-blank output and `frame_ms <= SCENARIO_MAX_FRAME_MS` (`50.0` ms).
    pub passed: bool,
    /// Failure description if `passed == false`.
    pub failure_reason: Option<String>,
}

impl ScenarioStepRecord {
    /// Formats a single-line human- and machine-readable log entry for stdout.
    pub fn format_log_line(&self) -> String {
        let status = if !self.passed {
            "FAIL"
        } else if self.warned {
            "WARN(>16ms)"
        } else {
            "OK"
        };
        let suffix = match &self.failure_reason {
            Some(reason) => format!("{} | error={reason}", self.detail),
            None => self.detail.clone(),
        };
        format!(
            "[scenario:smoke {:02}/{:02}] {:<20} | frame_ms={:>6.2} | status={:<11} | colors={:>2}+ | {}",
            self.index, self.total, self.name, self.frame_ms, status, self.distinct_colors, suffix
        )
    }
}

/// Aggregated report for an entire scenario run.
#[derive(Debug, Clone, Default)]
pub struct ScenarioReport {
    /// Per-step execution records in order.
    pub steps: Vec<ScenarioStepRecord>,
    /// `true` if the scenario reached termination without early window close.
    pub finished: bool,
    /// First fatal failure message, if any.
    pub failure_message: Option<String>,
}

impl ScenarioReport {
    /// Returns `true` if all scenario steps completed and passed within the 50 ms frame budget.
    pub fn is_ok(&self) -> bool {
        self.finished
            && self.failure_message.is_none()
            && !self.steps.is_empty()
            && self.steps.iter().all(|s| s.passed)
    }

    /// Maximum single-step frame time in milliseconds across the run.
    pub fn max_frame_ms(&self) -> f64 {
        self.steps
            .iter()
            .map(|s| s.frame_ms)
            .fold(0.0_f64, f64::max)
    }

    /// Mean step frame time in milliseconds across the run.
    pub fn mean_frame_ms(&self) -> f64 {
        if self.steps.is_empty() {
            return 0.0;
        }
        let sum: f64 = self.steps.iter().map(|s| s.frame_ms).sum();
        sum / (self.steps.len() as f64)
    }

    /// Number of steps that exceeded [`SCENARIO_WARN_FRAME_MS`] (`16.0` ms).
    pub fn warn_count(&self) -> usize {
        self.steps.iter().filter(|s| s.warned).count()
    }
}

/// Stateful driver attached to [`SegyViewerApp`] when `--scenario` or `--scenario-fast` is active.
pub struct ScenarioDriver {
    scenario: ScenarioName,
    step_pause: Duration,
    max_frame_ms: f64,
    warmup_done: bool,
    next_step_idx: usize,
    next_step_due: Instant,
    report: Arc<Mutex<ScenarioReport>>,
}

impl ScenarioDriver {
    /// Creates a new [`ScenarioDriver`] sharing `report`.
    pub fn new(scenario: ScenarioName, fast: bool, report: Arc<Mutex<ScenarioReport>>) -> Self {
        let pause = if fast {
            Duration::ZERO
        } else {
            Duration::from_millis(DEFAULT_SCENARIO_STEP_PAUSE_MS)
        };
        Self {
            scenario,
            step_pause: pause,
            max_frame_ms: SCENARIO_MAX_FRAME_MS,
            warmup_done: false,
            next_step_idx: 0,
            next_step_due: Instant::now(),
            report,
        }
    }

    /// Overrides the maximum allowed frame time budget in milliseconds (used by tests to verify budget failure handling).
    pub fn set_max_frame_ms(&mut self, max_frame_ms: f64) {
        self.max_frame_ms = max_frame_ms;
    }

    /// Returns the scenario name being executed.
    #[inline]
    pub const fn scenario(&self) -> ScenarioName {
        self.scenario
    }

    /// Returns the configured inter-step pause duration.
    #[inline]
    pub const fn step_pause(&self) -> Duration {
        self.step_pause
    }

    /// Returns the maximum allowed frame time budget in milliseconds.
    #[inline]
    pub const fn max_frame_ms(&self) -> f64 {
        self.max_frame_ms
    }

    /// Returns `true` if warmup has completed.
    #[inline]
    pub const fn warmup_done(&self) -> bool {
        self.warmup_done
    }

    /// Marks the warmup frame as completed.
    #[inline]
    pub fn complete_warmup(&mut self) {
        self.warmup_done = true;
        self.next_step_due = Instant::now();
    }

    /// Returns the 0-based index of the next step to execute.
    #[inline]
    pub const fn next_step_idx(&self) -> usize {
        self.next_step_idx
    }

    /// Returns the [`Instant`] at which the next step (or window close) is due.
    #[inline]
    pub const fn next_step_due(&self) -> Instant {
        self.next_step_due
    }

    /// Advances to the next step and schedules `next_step_due`.
    #[inline]
    pub fn advance_step(&mut self) {
        self.next_step_idx += 1;
        self.next_step_due = Instant::now() + self.step_pause;
    }

    /// Appends a [`ScenarioStepRecord`] to the shared report and prints the step line to stdout.
    pub fn record_step(&self, record: ScenarioStepRecord) {
        let line = record.format_log_line();
        println!("{line}");
        let _ = std::io::stdout().flush();

        if let Ok(mut guard) = self.report.lock() {
            if !record.passed && guard.failure_message.is_none() {
                guard.failure_message = record.failure_reason.clone();
            }
            if !record.passed {
                guard.finished = true;
            }
            guard.steps.push(record);
        }
    }

    /// Marks the scenario run as finished in the shared report.
    pub fn mark_finished(&self) {
        if let Ok(mut guard) = self.report.lock() {
            guard.finished = true;
        }
    }

    /// Records a fatal panic or top-level error in the shared report.
    pub fn record_fatal_error(&self, message: String) {
        eprintln!("scenario error: {message}");
        if let Ok(mut guard) = self.report.lock() {
            if guard.failure_message.is_none() {
                guard.failure_message = Some(message);
            }
            guard.finished = true;
        }
    }

    /// Returns a snapshot clone of the current [`ScenarioReport`].
    pub fn report_snapshot(&self) -> ScenarioReport {
        self.report.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

/// Evaluates the outcome of a single step after its UI pass and synchronous render have completed.
pub fn evaluate_step_outcome(
    ctrl: &AppController,
    step_idx_zero_based: usize,
    total_steps: usize,
    step_kind: ScenarioStepKind,
    step_result: Result<String>,
    frame_ms: f64,
    max_frame_ms: f64,
) -> ScenarioStepRecord {
    let mut detail = String::new();
    let mut failure_reason: Option<String> = None;

    match step_result {
        Ok(d) => detail = d,
        Err(err) => {
            failure_reason = Some(err.to_string());
        }
    }

    if failure_reason.is_none()
        && let Some(ctrl_err) = ctrl.last_error()
    {
        failure_reason = Some(ctrl_err.to_string());
    }

    let distinct_colors = ctrl
        .cached_image()
        .map_or(0, |img| count_distinct_colors(img, 16));

    if failure_reason.is_none() && distinct_colors < 2 {
        failure_reason = Some(format!(
            "blank render on step '{}' (distinct colors: {distinct_colors})",
            step_kind.name()
        ));
    }

    let warned = frame_ms > SCENARIO_WARN_FRAME_MS;
    if failure_reason.is_none() && frame_ms > max_frame_ms {
        failure_reason = Some(format!(
            "step '{}' frame time {:.2} ms exceeded {:.1} ms limit",
            step_kind.name(),
            frame_ms,
            max_frame_ms
        ));
    }

    ScenarioStepRecord {
        index: step_idx_zero_based + 1,
        total: total_steps,
        name: step_kind.name(),
        detail,
        frame_ms,
        distinct_colors,
        warned,
        passed: failure_reason.is_none(),
        failure_reason,
    }
}

/// Validates a finished [`ScenarioReport`] and converts any failure or incomplete run into an [`AppError`].
pub fn finalize_scenario_report(report: ScenarioReport) -> Result<ScenarioReport> {
    if let Some(msg) = &report.failure_message {
        return Err(AppError::ScenarioFailed(msg.clone()));
    }
    if !report.finished || report.steps.len() < ScenarioStepKind::ALL.len() {
        return Err(AppError::ScenarioFailed(format!(
            "scenario interrupted after {}/{} steps",
            report.steps.len(),
            ScenarioStepKind::ALL.len()
        )));
    }
    println!(
        "[scenario:smoke summary] steps={}/{} status=PASS mean_frame_ms={:.2} max_frame_ms={:.2} warnings={}",
        report.steps.len(),
        ScenarioStepKind::ALL.len(),
        report.mean_frame_ms(),
        report.max_frame_ms(),
        report.warn_count()
    );
    Ok(report)
}

/// Runs the configured scenario headlessly with zero pauses (`--scenario-fast`), suitable for
/// automated tests and CI.
pub fn run_scenario_headless(args: &AppCliArgs) -> Result<ScenarioReport> {
    run_scenario_headless_with_budget(args, SCENARIO_MAX_FRAME_MS)
}

/// Runs the configured scenario headlessly with a custom maximum frame time budget `max_frame_ms`.
pub fn run_scenario_headless_with_budget(
    args: &AppCliArgs,
    max_frame_ms: f64,
) -> Result<ScenarioReport> {
    if args.file_path.is_none() {
        return Err(AppError::ScenarioFailed(
            "missing SEG-Y file path for scenario run".to_string(),
        ));
    }

    let mut fast_args = args.clone();
    if fast_args.scenario.is_none() {
        fast_args.scenario = Some(ScenarioName::Smoke);
    }
    fast_args.scenario_fast = true;

    let report_arc = Arc::new(Mutex::new(ScenarioReport::default()));
    let mut app = SegyViewerApp::new_with_scenario_report(fast_args, Arc::clone(&report_arc));
    app.set_scenario_max_frame_ms(max_frame_ms);

    if let Some(err) = app.controller().last_error() {
        return Err(AppError::ScenarioFailed(err.to_string()));
    }

    let ctx = egui::Context::default();
    let max_frames = ScenarioStepKind::ALL.len() + 8;

    for _ in 0..max_frames {
        let mut raw_input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 820.0))),
            ..Default::default()
        };
        let step_res = catch_unwind(AssertUnwindSafe(|| {
            let mut output = ctx.run_ui(raw_input.take(), |ui| {
                app.draw_ui(ui);
            });
            output.textures_delta.clear();
        }));

        if let Err(panic_payload) = step_res {
            let msg = format_panic_payload(panic_payload);
            return Err(AppError::ScenarioFailed(format!(
                "panic during scenario execution: {msg}"
            )));
        }

        let done = report_arc.lock().map(|g| g.finished).unwrap_or(true);
        if done {
            break;
        }
    }

    let report = report_arc.lock().map(|g| g.clone()).unwrap_or_default();
    finalize_scenario_report(report)
}

/// Extracts a human-readable message from a caught panic payload.
pub fn format_panic_payload(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_scenario_fast_runs_all_smoke_steps_under_budget() -> Result<()> {
        let sample = Path::new("/home/toto/Downloads/3X_75_PR.SGY");
        if !sample.exists() {
            return Ok(());
        }

        let args = AppCliArgs::parse_from([
            "/home/toto/Downloads/3X_75_PR.SGY",
            "--scenario",
            "smoke",
            "--scenario-fast",
        ])?;
        let report = run_scenario_headless(&args)?;
        assert!(report.is_ok());
        assert_eq!(report.steps.len(), ScenarioStepKind::ALL.len());
        for step in &report.steps {
            assert!(
                step.passed,
                "step {} failed: {:?}",
                step.name, step.failure_reason
            );
            assert!(step.distinct_colors >= 2);
            assert!(step.frame_ms <= SCENARIO_MAX_FRAME_MS);
        }
        assert!(resolve_scenario_png_path().exists());
        Ok(())
    }

    #[test]
    fn headless_scenario_fails_on_missing_file_or_impossible_frame_budget() -> Result<()> {
        let no_file = AppCliArgs::parse_from(["--scenario-fast"])?;
        assert!(run_scenario_headless(&no_file).is_err());

        let bad_file = AppCliArgs::parse_from(["/nonexistent/file.sgy", "--scenario-fast"])?;
        assert!(run_scenario_headless(&bad_file).is_err());

        let sample = Path::new("/home/toto/Downloads/3X_75_PR.SGY");
        if sample.exists() {
            let args =
                AppCliArgs::parse_from(["/home/toto/Downloads/3X_75_PR.SGY", "--scenario-fast"])?;
            let impossible_budget = run_scenario_headless_with_budget(&args, 0.0001);
            assert!(impossible_budget.is_err());
        }
        Ok(())
    }
}
