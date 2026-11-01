#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

//! Headless `egui_kittest` UI interaction tests for `SegyViewerApp`.

use app::{AppCliArgs, Result, SegyViewerApp};
use eframe::egui::Vec2;
use eframe::egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use render::{
    ClipStrategy, ColormapKind, DisplayMode, NormScope, Polarity, TraceBalance, WiggleFillMode,
};
use segy_core::AxisHeaderKey;

const SAMPLE_SEGY_PATH: &str = "/home/toto/Downloads/3X_75_PR.SGY";

fn build_harness_with_sample() -> Result<Harness<'static, SegyViewerApp>> {
    let args = AppCliArgs::parse_from([SAMPLE_SEGY_PATH])?;
    let app = SegyViewerApp::new(args);
    let harness = Harness::builder()
        .with_size(Vec2::new(1280.0, 820.0))
        .build_ui_state(|ui, state: &mut SegyViewerApp| state.draw_ui(ui), app);
    Ok(harness)
}

#[test]
fn kittest_sample_segy_mode_gain_and_clip_interactions() -> Result<()> {
    let mut h = build_harness_with_sample()?;
    assert!(h.state().controller().has_file());
    assert_eq!(
        h.state().controller().viewport().bounds().total_traces,
        1592
    );
    assert_eq!(
        h.state().controller().viewport().bounds().samples_per_trace,
        751
    );

    // 1. Mode buttons: Wiggle / Image / Overlay
    h.get_by_label("Wiggle").click();
    h.run();
    assert_eq!(
        h.state().controller().viewport().display_mode(),
        DisplayMode::Wiggle
    );

    h.get_by_label("Overlay").click();
    h.run();
    assert_eq!(
        h.state().controller().viewport().display_mode(),
        DisplayMode::Overlay
    );

    h.get_by_label("Image").click();
    h.run();
    assert_eq!(
        h.state().controller().viewport().display_mode(),
        DisplayMode::Image
    );

    // 2. Gain slider and 0 dB reset button
    h.get_by_role_and_label(Role::Slider, "Gain (dB)")
        .set_value_accesskit(9.5);
    h.run();
    assert!((h.state().controller().render_params().amplitude.gain_db - 9.5).abs() < 1e-4);

    h.get_by_label("0 dB").click();
    h.run();
    assert!(
        h.state()
            .controller()
            .render_params()
            .amplitude
            .gain_db
            .abs()
            < 1e-5
    );

    // 3. Clip strategy buttons and parameter sliders
    h.get_by_label("RMS").click();
    h.run();
    assert_eq!(
        h.state().controller().render_params().amplitude.clip,
        ClipStrategy::Rms { multiple: 2.5 }
    );

    h.get_by_role_and_label(Role::Slider, "× RMS")
        .set_value_accesskit(4.0);
    h.run();
    assert_eq!(
        h.state().controller().render_params().amplitude.clip,
        ClipStrategy::Rms { multiple: 4.0 }
    );

    h.get_by_label("Peak").click();
    h.run();
    assert_eq!(
        h.state().controller().render_params().amplitude.clip,
        ClipStrategy::Peak
    );

    h.get_by_label("Percentile").click();
    h.run();
    assert_eq!(
        h.state().controller().render_params().amplitude.clip,
        ClipStrategy::Percentile { percentile: 99.0 }
    );

    h.get_by_role_and_label(Role::Slider, "Percentile (%)")
        .set_value_accesskit(95.0);
    h.run();
    assert_eq!(
        h.state().controller().render_params().amplitude.clip,
        ClipStrategy::Percentile { percentile: 95.0 }
    );

    assert!(h.state().controller().last_error().is_none());
    Ok(())
}

#[test]
fn kittest_sample_segy_colormap_polarity_and_fit_buttons() -> Result<()> {
    let mut h = build_harness_with_sample()?;

    // 1. Colormap combo box and reverse toggle
    h.get_by_label("Colormap:").click();
    h.run();
    h.get_by_label(ColormapKind::SeismicRwb.label()).click();
    h.run();
    assert_eq!(
        h.state().controller().render_params().colormap.kind,
        ColormapKind::SeismicRwb
    );

    h.get_by_label("Colormap:").click();
    h.run();
    h.get_by_label("Viridis").click();
    h.run();
    assert_eq!(
        h.state().controller().render_params().colormap.kind,
        ColormapKind::Viridis
    );

    h.get_by_label("Reverse Colormap").click();
    h.run();
    assert!(h.state().controller().render_params().colormap.reversed);

    // 2. Polarity toggle button
    assert_eq!(
        h.state().controller().render_params().amplitude.polarity,
        Polarity::Normal
    );
    h.get_by_label("Polarity: Normal (+)").click();
    h.run();
    assert_eq!(
        h.state().controller().render_params().amplitude.polarity,
        Polarity::Reversed
    );

    h.get_by_label("Polarity: Reversed (−)").click();
    h.run();
    assert_eq!(
        h.state().controller().render_params().amplitude.polarity,
        Polarity::Normal
    );

    // 3. Zoom and Fit buttons (Fit H, Fit V, Fit All)
    h.state_mut()
        .controller_mut()
        .zoom_at_pixel(4.0, 3.0, 400.0, 300.0);
    h.run();
    assert!(h.state().controller().viewport().zoom_ratio_horizontal() > 3.5);
    assert!(h.state().controller().viewport().zoom_ratio_vertical() > 2.5);

    h.get_by_label("Fit H").click();
    h.run();
    assert!((h.state().controller().viewport().zoom_ratio_horizontal() - 1.0).abs() < 1e-5);
    assert_eq!(
        h.state().controller().viewport().visible_trace_range(),
        0..1592
    );
    assert!(h.state().controller().viewport().zoom_ratio_vertical() > 2.5);

    h.get_by_label("Fit V").click();
    h.run();
    assert!((h.state().controller().viewport().zoom_ratio_vertical() - 1.0).abs() < 1e-5);
    assert_eq!(
        h.state().controller().viewport().visible_sample_range(),
        0..751
    );

    h.state_mut()
        .controller_mut()
        .zoom_at_pixel(3.0, 2.5, 350.0, 250.0);
    h.run();
    assert!(h.state().controller().viewport().zoom_ratio_horizontal() > 2.5);
    assert!(h.state().controller().viewport().zoom_ratio_vertical() > 2.0);

    h.get_by_label("Fit All").click();
    h.run();
    assert!((h.state().controller().viewport().zoom_ratio_horizontal() - 1.0).abs() < 1e-5);
    assert!((h.state().controller().viewport().zoom_ratio_vertical() - 1.0).abs() < 1e-5);
    assert_eq!(
        h.state().controller().viewport().visible_trace_range(),
        0..1592
    );
    assert_eq!(
        h.state().controller().viewport().visible_sample_range(),
        0..751
    );

    assert!(h.state().controller().last_error().is_none());
    Ok(())
}

#[test]
fn kittest_sidebar_wiggle_geometry_and_headers_controls() -> Result<()> {
    let mut h = build_harness_with_sample()?;

    // Scope toggle
    h.get_by_label("Per-Trace").click();
    h.run();
    assert_eq!(
        h.state().controller().render_params().amplitude.scope,
        NormScope::PerTrace
    );
    h.get_by_label("Global").click();
    h.run();
    assert_eq!(
        h.state().controller().render_params().amplitude.scope,
        NormScope::Global
    );

    // DC Removal, AGC, and Trace Balance
    h.get_by_label("DC Removal").click();
    h.run();
    assert!(h.state().controller().render_params().amplitude.dc_removal);

    h.get_by_label("Sliding-Window AGC").click();
    h.run();
    assert!(h.state().controller().render_params().amplitude.agc.enabled);
    h.get_by_role_and_label(Role::Slider, "AGC Window (samples)")
        .set_value_accesskit(120.0);
    h.run();
    assert_eq!(
        h.state()
            .controller()
            .render_params()
            .amplitude
            .agc
            .window_samples,
        120
    );

    h.get_by_label("RMS Bal").click();
    h.run();
    assert_eq!(
        h.state()
            .controller()
            .render_params()
            .amplitude
            .trace_balance,
        TraceBalance::Rms
    );
    h.get_by_label("Off").click();
    h.run();
    assert_eq!(
        h.state()
            .controller()
            .render_params()
            .amplitude
            .trace_balance,
        TraceBalance::Off
    );

    // Wiggle fill mode buttons & excursion sliders
    h.get_by_label("Neg").click();
    h.run();
    assert_eq!(
        h.state().controller().render_params().wiggle_fill,
        WiggleFillMode::Negative
    );
    h.get_by_label("Both").click();
    h.run();
    assert_eq!(
        h.state().controller().render_params().wiggle_fill,
        WiggleFillMode::Both
    );
    h.get_by_label("None").click();
    h.run();
    assert_eq!(
        h.state().controller().render_params().wiggle_fill,
        WiggleFillMode::LineOnly
    );
    h.get_by_label("Pos").click();
    h.run();
    assert_eq!(
        h.state().controller().render_params().wiggle_fill,
        WiggleFillMode::Positive
    );

    h.get_by_role_and_label(Role::Slider, "Excursion")
        .set_value_accesskit(1.5);
    h.run();
    assert!(
        (h.state()
            .controller()
            .render_params()
            .wiggle_excursion_spacings
            - 1.5)
            .abs()
            < 1e-4
    );

    // X-Axis header key combo box, Trace Step, and Reverse Trace Order
    h.get_by_label("X-Axis:").click();
    h.run();
    h.get_by_label("CDP / Ensemble").click();
    h.run();
    assert_eq!(
        h.state().controller().viewport().horizontal_axis_key(),
        AxisHeaderKey::Cdp
    );

    h.get_by_role_and_label(Role::Slider, "Trace Step")
        .set_value_accesskit(4.0);
    h.run();
    assert_eq!(h.state().controller().viewport().trace_decimate_step(), 4);

    h.get_by_label("Reverse Trace Order").click();
    h.run();
    assert!(h.state().controller().viewport().reverse_traces());

    // Open and close Headers read-only window
    h.get_by_label("Headers").click();
    h.run();
    h.get_by_label("Headers").click();
    h.run();

    assert!(h.state().controller().last_error().is_none());
    Ok(())
}
