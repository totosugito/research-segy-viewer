#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

//! Headless feature test: runs the whole demo script (the same steps the demo video shows) and
//! requires every step to both change the app state and verify that the feature worked.

use app::{AppCliArgs, DemoOverlay, Result, SegyViewerApp, demo_steps};

const SAMPLE_SEGY_PATH: &str = "/home/toto/Downloads/3X_75_PR.SGY";

#[test]
fn every_demo_step_applies_and_verifies_its_feature() -> Result<()> {
    if !std::path::Path::new(SAMPLE_SEGY_PATH).exists() {
        return Ok(());
    }
    let mut app = SegyViewerApp::new(AppCliArgs::parse_from([SAMPLE_SEGY_PATH])?);
    app.controller_mut().set_viewport_dimensions(900, 600);

    let steps = demo_steps();
    assert!(
        steps.len() >= 60,
        "demo should cover many features, got {}",
        steps.len()
    );

    let mut overlay = DemoOverlay::default();
    let mut failures = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        assert!(!step.caption.is_empty());
        assert!(step.hold_ms >= 500, "step {} is too short to read", i + 1);
        overlay.caption = step.caption.to_string();
        overlay.index = i + 1;
        overlay.total = steps.len();
        if let Err(reason) = (step.run)(&mut app, &mut overlay) {
            failures.push(format!("step {} '{}': {reason}", i + 1, step.caption));
        }
    }
    assert!(
        failures.is_empty(),
        "demo steps failed:\n{}",
        failures.join("\n")
    );
    Ok(())
}

#[test]
fn demo_script_is_long_enough_to_read_but_not_endless() {
    let total_ms: u64 = demo_steps().iter().map(|s| s.hold_ms).sum();
    assert!(
        (90_000..=240_000).contains(&total_ms),
        "demo length {total_ms} ms is outside 1.5-4 minutes"
    );
}
