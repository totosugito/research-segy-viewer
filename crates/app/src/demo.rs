//! Feature demo: an ordered list of captioned steps that exercises every drawing feature of the
//! viewer. Each step both *changes* the app state and *verifies* the feature took effect, so the
//! same list serves as a headless feature test and as the script for the demo video
//! (`cargo run --release -p app --example demo_video`).

use render::{
    AgcConfig, ClipStrategy, ColormapKind, DisplayMode, NormScope, Polarity, TraceBalance,
    WiggleFillMode,
};
use segy_core::AxisHeaderKey;

use crate::gui::SegyViewerApp;

/// Result of one demo step: `Err` carries a human-readable reason the feature did not work.
pub type StepResult = std::result::Result<(), String>;

/// A single demo step.
pub struct DemoStep {
    /// Caption shown on screen while the step is displayed.
    pub caption: &'static str,
    /// How long the step stays on screen in the video (milliseconds).
    pub hold_ms: u64,
    /// Applies the step and checks the feature worked.
    pub run: fn(&mut SegyViewerApp, &mut DemoOverlay) -> StepResult,
}

/// Presentation-only state drawn on top of the section while a demo runs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DemoOverlay {
    /// Caption banner text.
    pub caption: String,
    /// 1-based index of the current step.
    pub index: usize,
    /// Total number of steps.
    pub total: usize,
    /// Demo mouse pointer as fractions `(x, y)` of the plot area.
    pub cursor: Option<(f32, f32)>,
    /// Whether a click ring is drawn around the pointer.
    pub click: bool,
    /// Pointer position in window pixels, drawn above every panel (used when real input events
    /// are injected, e.g. by the primary-feature demo).
    pub pointer: Option<(f32, f32)>,
    /// Whether a mouse button is currently held (draws the click ring around `pointer`).
    pub pressed: bool,
    /// Rubber-band rectangle as fractions `(x0, y0, x1, y1)` of the plot area.
    pub zoom_box: Option<(f32, f32, f32, f32)>,
}

fn ensure(cond: bool, msg: &str) -> StepResult {
    if cond { Ok(()) } else { Err(msg.to_string()) }
}

fn pixel(app: &SegyViewerApp, fx: f32, fy: f32) -> (f32, f32) {
    let vp = app.controller().viewport();
    (fx * vp.width_px() as f32, fy * vp.height_px() as f32)
}

fn move_cursor(app: &mut SegyViewerApp, ov: &mut DemoOverlay, fx: f32, fy: f32) -> StepResult {
    ov.cursor = Some((fx, fy));
    ov.click = false;
    let (x, y) = pixel(app, fx, fy);
    ensure(
        app.controller_mut().update_cursor_at_pixel(x, y).is_some(),
        "cursor readout missing",
    )
}

macro_rules! step {
    ($caption:expr, $hold:expr, |$app:ident, $ov:ident| $body:block) => {
        DemoStep {
            caption: $caption,
            hold_ms: $hold,
            run: |$app: &mut SegyViewerApp, $ov: &mut DemoOverlay| -> StepResult {
                let _ = (&$app, &$ov);
                $body
            },
        }
    };
}

/// Shorter hold for the in-between frames of an animation (pointer move, growing rectangle).
const TICK: u64 = 700;
/// Standard hold for a feature.
const HOLD: u64 = 2200;

/// Returns the full ordered demo script.
pub fn demo_steps() -> Vec<DemoStep> {
    let mut v = Vec::new();
    v.extend(intro_steps());
    v.extend(amplitude_steps());
    v.extend(colormap_steps());
    v.extend(navigation_steps());
    v.extend(wiggle_steps());
    v.extend(selection_steps());
    v.extend(axes_steps());
    v.extend(output_steps());
    v
}

fn intro_steps() -> Vec<DemoStep> {
    vec![
        step!(
            "SEG-Y Viewer: read-only SEG-Y (rev 0/1) with fast wiggle and image drawing",
            3500,
            |app, ov| {
                ov.cursor = None;
                app.controller_mut().set_display_mode(DisplayMode::Image);
                app.controller_mut().fit_to_data();
                ensure(app.controller().has_file(), "no file loaded")
            }
        ),
        step!(
            "Image mode (variable density): every trace of the file in one view",
            HOLD,
            |app, ov| {
                ensure(
                    app.controller().viewport().display_mode() == DisplayMode::Image,
                    "mode is not Image",
                )
            }
        ),
        step!(
            "Hover for a live readout: trace, header value, time and amplitude",
            TICK + 500,
            |app, ov| { move_cursor(app, ov, 0.30, 0.35) }
        ),
        step!(
            "The readout follows the pointer anywhere on the section",
            HOLD,
            |app, ov| {
                move_cursor(app, ov, 0.62, 0.55)?;
                ensure(
                    app.controller().last_cursor_readout().is_some(),
                    "readout not stored",
                )?;
                ov.cursor = None;
                app.controller_mut().clear_cursor();
                Ok(())
            }
        ),
    ]
}

fn amplitude_steps() -> Vec<DemoStep> {
    vec![
        step!("Gain +12 dB: amplify the display", HOLD, |app, ov| {
            app.controller_mut().set_gain_db(12.0);
            ensure(
                (app.controller().render_params().amplitude.gain_db - 12.0).abs() < 1e-4,
                "gain not applied",
            )
        }),
        step!("Gain -6 dB: attenuate", HOLD, |app, ov| {
            app.controller_mut().set_gain_db(-6.0);
            ensure(
                (app.controller().render_params().amplitude.gain_db + 6.0).abs() < 1e-4,
                "gain not applied",
            )
        }),
        step!("Gain back to 0 dB", 1500, |app, ov| {
            app.controller_mut().set_gain_db(0.0);
            Ok(())
        }),
        step!(
            "Clip at 2 x RMS: stronger contrast, big amplitudes saturate",
            HOLD,
            |app, ov| {
                app.controller_mut()
                    .set_clip_strategy(ClipStrategy::Rms { multiple: 2.0 });
                ensure(
                    matches!(
                        app.controller().render_params().amplitude.clip,
                        ClipStrategy::Rms { .. }
                    ),
                    "RMS clip not applied",
                )
            }
        ),
        step!(
            "Clip at the absolute peak: no saturation",
            HOLD,
            |app, ov| {
                app.controller_mut().set_clip_strategy(ClipStrategy::Peak);
                ensure(
                    app.controller().render_params().amplitude.clip == ClipStrategy::Peak,
                    "peak clip not applied",
                )
            }
        ),
        step!("Clip at the 99th percentile (default)", HOLD, |app, ov| {
            app.controller_mut()
                .set_clip_strategy(ClipStrategy::Percentile { percentile: 99.0 });
            Ok(())
        }),
        step!(
            "Per-trace scope: each trace normalised on its own",
            HOLD,
            |app, ov| {
                app.controller_mut().set_norm_scope(NormScope::PerTrace);
                ensure(
                    app.controller().render_params().amplitude.scope == NormScope::PerTrace,
                    "per-trace scope not applied",
                )
            }
        ),
        step!(
            "Global scope: one reference for the whole section",
            1800,
            |app, ov| {
                app.controller_mut().set_norm_scope(NormScope::Global);
                Ok(())
            }
        ),
        step!("DC removal: subtract each trace's mean", HOLD, |app, ov| {
            app.controller_mut().set_dc_removal(true);
            ensure(
                app.controller().render_params().amplitude.dc_removal,
                "DC removal not applied",
            )
        }),
        step!("DC removal off", 1200, |app, ov| {
            app.controller_mut().set_dc_removal(false);
            Ok(())
        }),
        step!(
            "Sliding-window AGC: balance amplitudes along time",
            HOLD + 600,
            |app, ov| {
                app.controller_mut().set_agc(AgcConfig {
                    enabled: true,
                    window_samples: 125,
                });
                ensure(
                    app.controller().render_params().amplitude.agc.enabled,
                    "AGC not applied",
                )
            }
        ),
        step!("AGC off", 1200, |app, ov| {
            app.controller_mut().set_agc(AgcConfig::off());
            Ok(())
        }),
        step!(
            "Trace balance (RMS): equalise energy between traces",
            HOLD,
            |app, ov| {
                app.controller_mut().set_trace_balance(TraceBalance::Rms);
                ensure(
                    app.controller().render_params().amplitude.trace_balance == TraceBalance::Rms,
                    "trace balance not applied",
                )
            }
        ),
        step!("Trace balance off", 1200, |app, ov| {
            app.controller_mut().set_trace_balance(TraceBalance::Off);
            Ok(())
        }),
        step!(
            "Polarity reversed: swap black and white",
            HOLD,
            |app, ov| {
                app.controller_mut().set_polarity(Polarity::Reversed);
                ensure(
                    app.controller().render_params().amplitude.polarity == Polarity::Reversed,
                    "polarity not applied",
                )
            }
        ),
        step!("Polarity normal", 1500, |app, ov| {
            app.controller_mut().set_polarity(Polarity::Normal);
            Ok(())
        }),
    ]
}

fn colormap_steps() -> Vec<DemoStep> {
    vec![
        step!("Colormap: Seismic (blue - white - red)", HOLD, |app, ov| {
            app.controller_mut()
                .set_colormap_kind(ColormapKind::SeismicRwb);
            ensure(
                app.controller().render_params().colormap.kind == ColormapKind::SeismicRwb,
                "colormap not applied",
            )
        }),
        step!("Reverse colormap swaps the colours", HOLD, |app, ov| {
            app.controller_mut().set_colormap_reversed(true);
            ensure(
                app.controller().render_params().colormap.reversed,
                "reverse not applied",
            )
        }),
        step!("Reverse colormap off", 1200, |app, ov| {
            app.controller_mut().set_colormap_reversed(false);
            Ok(())
        }),
        step!("Contrast 2.0: stretch the colour range", HOLD, |app, ov| {
            app.controller_mut().set_colormap_contrast(2.0);
            ensure(
                (app.controller().render_params().colormap.contrast - 2.0).abs() < 1e-4,
                "contrast not applied",
            )
        }),
        step!("Bias +0.3: shift the zero level", HOLD, |app, ov| {
            app.controller_mut().set_colormap_contrast(1.0);
            app.controller_mut().set_colormap_bias(0.3);
            ensure(
                (app.controller().render_params().colormap.bias - 0.3).abs() < 1e-4,
                "bias not applied",
            )
        }),
        step!("Colormap: Viridis", HOLD, |app, ov| {
            app.controller_mut().set_colormap_bias(0.0);
            app.controller_mut()
                .set_colormap_kind(ColormapKind::Viridis);
            ensure(
                app.controller().render_params().colormap.kind == ColormapKind::Viridis,
                "colormap not applied",
            )
        }),
        step!("Colormap: Rainbow", HOLD, |app, ov| {
            app.controller_mut()
                .set_colormap_kind(ColormapKind::Rainbow);
            ensure(
                app.controller().render_params().colormap.kind == ColormapKind::Rainbow,
                "colormap not applied",
            )
        }),
        step!("Colormap: Brown - white - black", HOLD, |app, ov| {
            app.controller_mut()
                .set_colormap_kind(ColormapKind::BrownWhiteBlack);
            ensure(
                app.controller().render_params().colormap.kind == ColormapKind::BrownWhiteBlack,
                "colormap not applied",
            )
        }),
        step!("Colormap: Grayscale (default)", 1800, |app, ov| {
            app.controller_mut()
                .set_colormap_kind(ColormapKind::Grayscale);
            Ok(())
        }),
    ]
}

fn navigation_steps() -> Vec<DemoStep> {
    vec![
        step!(
            "Mouse wheel zooms around the pointer",
            TICK + 500,
            |app, ov| { move_cursor(app, ov, 0.5, 0.4) }
        ),
        step!("Zoom in (wheel up)", HOLD, |app, ov| {
            let before = app.controller().viewport().visible_trace_range().len();
            let (x, y) = pixel(app, 0.5, 0.4);
            app.controller_mut().zoom_at_pixel(2.0, 2.0, x, y);
            ensure(
                app.controller().viewport().visible_trace_range().len() < before,
                "zoom did not narrow the range",
            )
        }),
        step!(
            "Shift + wheel zooms the trace axis only",
            HOLD,
            |app, ov| {
                let before = app.controller().viewport().visible_trace_range().len();
                let (x, y) = pixel(app, 0.5, 0.4);
                app.controller_mut().zoom_at_pixel(1.6, 1.0, x, y);
                ensure(
                    app.controller().viewport().visible_trace_range().len() < before,
                    "horizontal zoom failed",
                )
            }
        ),
        step!("Alt + wheel zooms the time axis only", HOLD, |app, ov| {
            let before = app.controller().viewport().visible_sample_range().len();
            let (x, y) = pixel(app, 0.5, 0.4);
            app.controller_mut().zoom_at_pixel(1.0, 1.6, x, y);
            ensure(
                app.controller().viewport().visible_sample_range().len() < before,
                "vertical zoom failed",
            )
        }),
        step!(
            "Middle / right button drag pans the section",
            TICK,
            |app, ov| {
                ov.cursor = Some((0.6, 0.5));
                let before = app.controller().viewport().visible_trace_range();
                app.controller_mut().pan_by_screen_drag_px(-120.0, -40.0);
                ensure(
                    app.controller().viewport().visible_trace_range() != before,
                    "pan did not move the view",
                )
            }
        ),
        step!("... keep dragging", TICK, |app, ov| {
            ov.cursor = Some((0.5, 0.48));
            app.controller_mut().pan_by_screen_drag_px(-120.0, -40.0);
            Ok(())
        }),
        step!("... release", HOLD, |app, ov| {
            ov.cursor = None;
            Ok(())
        }),
        step!("Fit All returns to the full section", HOLD, |app, ov| {
            app.controller_mut().fit_to_data();
            ensure(
                app.controller().viewport().visible_trace_range().len() == 1592,
                "fit did not restore all traces",
            )
        }),
        step!(
            "Zoom box: drag a rectangle with the left button",
            TICK,
            |app, ov| {
                ov.cursor = Some((0.50, 0.30));
                ov.click = true;
                ov.zoom_box = Some((0.50, 0.30, 0.50, 0.30));
                Ok(())
            }
        ),
        step!("... the rectangle follows the pointer", TICK, |app, ov| {
            ov.cursor = Some((0.52, 0.48));
            ov.click = true;
            ov.zoom_box = Some((0.50, 0.30, 0.52, 0.48));
            Ok(())
        }),
        step!("... select the area to enlarge", TICK + 400, |app, ov| {
            ov.cursor = Some((0.54, 0.62));
            ov.click = true;
            ov.zoom_box = Some((0.50, 0.30, 0.54, 0.62));
            Ok(())
        }),
        step!(
            "Release: the view zooms exactly to that trace and time range",
            HOLD + 800,
            |app, ov| {
                let before = app.controller().viewport().visible_trace_range().len();
                let (x0, y0) = pixel(app, 0.50, 0.30);
                let (x1, y1) = pixel(app, 0.54, 0.62);
                ov.cursor = None;
                ov.click = false;
                ov.zoom_box = None;
                ensure(
                    app.controller_mut().zoom_to_pixel_rect(x0, y0, x1, y1),
                    "zoom box rejected",
                )?;
                let after = app.controller().viewport().visible_trace_range().len();
                ensure(after < before / 10, "zoom box did not zoom in enough")
            }
        ),
    ]
}

fn wiggle_steps() -> Vec<DemoStep> {
    vec![
        step!(
            "Wiggle mode: each trace drawn as a waveform with filled lobes",
            HOLD + 800,
            |app, ov| {
                app.controller_mut().set_display_mode(DisplayMode::Wiggle);
                ensure(
                    app.controller().viewport().display_mode() == DisplayMode::Wiggle,
                    "wiggle mode not applied",
                )
            }
        ),
        step!("Fill: negative lobes", HOLD, |app, ov| {
            app.controller_mut()
                .set_wiggle_fill(WiggleFillMode::Negative);
            ensure(
                app.controller().render_params().wiggle_fill == WiggleFillMode::Negative,
                "fill not applied",
            )
        }),
        step!("Fill: both lobes", HOLD, |app, ov| {
            app.controller_mut().set_wiggle_fill(WiggleFillMode::Both);
            ensure(
                app.controller().render_params().wiggle_fill == WiggleFillMode::Both,
                "fill not applied",
            )
        }),
        step!("Fill: line only", HOLD, |app, ov| {
            app.controller_mut()
                .set_wiggle_fill(WiggleFillMode::LineOnly);
            ensure(
                app.controller().render_params().wiggle_fill == WiggleFillMode::LineOnly,
                "fill not applied",
            )
        }),
        step!("Fill: positive lobes (default)", 1800, |app, ov| {
            app.controller_mut()
                .set_wiggle_fill(WiggleFillMode::Positive);
            Ok(())
        }),
        step!(
            "Larger excursion: wiggles overlap their neighbours",
            HOLD,
            |app, ov| {
                app.controller_mut().set_wiggle_excursion_spacings(2.5);
                ensure(
                    (app.controller().render_params().wiggle_excursion_spacings - 2.5).abs() < 1e-4,
                    "excursion not applied",
                )
            }
        ),
        step!("Smaller excursion", HOLD, |app, ov| {
            app.controller_mut().set_wiggle_excursion_spacings(0.8);
            Ok(())
        }),
        step!("Hide the wiggle outline", HOLD, |app, ov| {
            app.controller_mut().set_wiggle_excursion_spacings(1.45);
            app.controller_mut().set_wiggle_draw_line(false);
            ensure(
                !app.controller().render_params().wiggle_draw_line,
                "outline toggle not applied",
            )
        }),
        step!("Outline back on", 1500, |app, ov| {
            app.controller_mut().set_wiggle_draw_line(true);
            Ok(())
        }),
        step!(
            "Overlay: wiggles drawn over the image",
            HOLD + 600,
            |app, ov| {
                app.controller_mut().set_display_mode(DisplayMode::Overlay);
                ensure(
                    app.controller().viewport().display_mode() == DisplayMode::Overlay,
                    "overlay mode not applied",
                )
            }
        ),
        step!("Trace step 2: draw every second trace", HOLD, |app, ov| {
            app.controller_mut().set_trace_decimate_step(2);
            ensure(
                app.controller().viewport().trace_decimate_step() == 2,
                "trace step not applied",
            )
        }),
        step!("Trace step back to 1", 1500, |app, ov| {
            app.controller_mut().set_trace_decimate_step(1);
            Ok(())
        }),
        step!("Reverse trace order: right to left", HOLD, |app, ov| {
            app.controller_mut().set_reverse_traces(true);
            ensure(
                app.controller().viewport().reverse_traces(),
                "reverse not applied",
            )
        }),
        step!("Normal trace order", 1500, |app, ov| {
            app.controller_mut().set_reverse_traces(false);
            Ok(())
        }),
    ]
}

fn selection_steps() -> Vec<DemoStep> {
    vec![
        step!("Click a trace to select it", TICK + 500, |app, ov| {
            app.controller_mut().set_display_mode(DisplayMode::Image);
            move_cursor(app, ov, 0.5, 0.5)?;
            ov.click = true;
            Ok(())
        }),
        step!(
            "The selected trace is drawn as its own waveform; its headers and amplitude are in the status bar",
            HOLD + 1500,
            |app, ov| {
                let (x, _) = pixel(app, 0.5, 0.5);
                ensure(
                    app.controller_mut().click_select_at_pixel(x).is_some(),
                    "click did not select a trace",
                )?;
                ensure(
                    app.controller().selected_trace_waveform().is_some(),
                    "no waveform for the selected trace",
                )?;
                ov.click = false;
                Ok(())
            }
        ),
        step!("Right arrow: next trace", TICK + 300, |app, ov| {
            let before = selected_index(app)?;
            app.controller_mut().move_selection(1);
            ensure(
                selected_index(app)? == before + 1,
                "arrow did not move selection",
            )
        }),
        step!("Right arrow again", TICK + 300, |app, ov| {
            app.controller_mut().move_selection(1);
            Ok(())
        }),
        step!("Left arrow: previous trace", TICK + 300, |app, ov| {
            let before = selected_index(app)?;
            app.controller_mut().move_selection(-1);
            ensure(
                selected_index(app)? + 1 == before,
                "arrow did not move selection",
            )
        }),
        step!("Esc clears the selection", HOLD, |app, ov| {
            app.controller_mut().clear_selection();
            ensure(
                app.controller().selected_trace().is_none(),
                "selection not cleared",
            )
        }),
        step!("Double-click: back to the full section", HOLD, |app, ov| {
            app.controller_mut().fit_to_data();
            ov.cursor = None;
            Ok(())
        }),
    ]
}

fn selected_index(app: &SegyViewerApp) -> std::result::Result<usize, String> {
    app.controller()
        .selected_trace()
        .map(|s| s.trace_index)
        .ok_or_else(|| "no trace selected".to_string())
}

fn axes_steps() -> Vec<DemoStep> {
    vec![
        step!(
            "X axis labels come from any trace header: CDP",
            HOLD,
            |app, ov| {
                app.controller_mut()
                    .set_horizontal_axis_key(AxisHeaderKey::Cdp);
                ensure(
                    app.controller().viewport().horizontal_axis_key() == AxisHeaderKey::Cdp,
                    "axis key not applied",
                )
            }
        ),
        step!(
            "Add label rows: up to five at once. Row 2: Field record (FFID)",
            HOLD,
            |app, ov| {
                ensure(
                    app.controller_mut()
                        .add_extra_axis_key(AxisHeaderKey::FieldRecord),
                    "could not add row",
                )
            }
        ),
        step!("Row 3: Offset", HOLD, |app, ov| {
            ensure(
                app.controller_mut()
                    .add_extra_axis_key(AxisHeaderKey::Offset),
                "could not add row",
            )
        }),
        step!("Row 4: Inline", HOLD, |app, ov| {
            ensure(
                app.controller_mut()
                    .add_extra_axis_key(AxisHeaderKey::Inline),
                "could not add row",
            )
        }),
        step!(
            "Row 5: any header field by byte position (custom i32 @ byte 21)",
            HOLD + 800,
            |app, ov| {
                ensure(
                    app.controller_mut()
                        .add_extra_axis_key(AxisHeaderKey::CustomI32 { byte_offset: 20 }),
                    "could not add row",
                )?;
                ensure(
                    !app.controller_mut()
                        .add_extra_axis_key(AxisHeaderKey::Crossline),
                    "a sixth row must be refused",
                )
            }
        ),
        step!("Rows can be removed again", HOLD, |app, ov| {
            for row in (0..4).rev() {
                ensure(
                    app.controller_mut().remove_extra_axis_key(row),
                    "remove failed",
                )?;
            }
            ensure(
                app.controller().extra_axis_keys().is_empty(),
                "rows not removed",
            )
        }),
        step!("Grid lines off", 1600, |app, ov| {
            app.set_show_grid(false);
            Ok(())
        }),
        step!("Grid lines on", 1500, |app, ov| {
            app.set_show_grid(true);
            Ok(())
        }),
        step!(
            "Drag tool: switch the left button to Pan",
            HOLD,
            |app, ov| {
                app.set_pan_tool(true);
                Ok(())
            }
        ),
        step!("Drag tool: back to Zoom box", 1500, |app, ov| {
            app.set_pan_tool(false);
            Ok(())
        }),
    ]
}

fn output_steps() -> Vec<DemoStep> {
    vec![
        step!(
            "Headers window: textual and binary file headers (read-only)",
            3000,
            |app, ov| {
                app.set_show_headers_window(true);
                ensure(app.controller().file().is_some(), "no file")
            }
        ),
        step!("Close the headers window", 1200, |app, ov| {
            app.set_show_headers_window(false);
            Ok(())
        }),
        step!(
            "Save PNG exports the section at any resolution, offscreen",
            HOLD + 800,
            |app, ov| {
                let path = std::env::temp_dir().join("segy_viewer_demo_section.png");
                let image = app
                    .controller_mut()
                    .save_png_with_dimensions(&path, 1600, 900)
                    .map_err(|e| e.to_string())?;
                ensure(
                    image.width == 1600 && image.height == 900,
                    "PNG has the wrong size",
                )?;
                ensure(
                    std::fs::metadata(&path).is_ok_and(|m| m.len() > 10_000),
                    "PNG file missing or tiny",
                )
            }
        ),
        step!(
            "Read-only SEG-Y viewer: wiggle, image and overlay drawing",
            3500,
            |app, ov| {
                app.controller_mut().set_display_mode(DisplayMode::Image);
                app.controller_mut().fit_to_data();
                Ok(())
            }
        ),
    ]
}
