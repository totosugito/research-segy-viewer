#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

//! Primary features (zoom, pan, plot type, trace selection, multi X axis) exercised with real
//! injected pointer, wheel and key events: the same script that produces the demo video.

#[path = "../examples/primary_demo/script.rs"]
mod script;

use app::SegyViewerApp;
use egui_kittest::Harness;
use script::{Player, R, Sink, WINDOW, build_app};

const SAMPLE_SEGY_PATH: &str = "/home/toto/Downloads/3X_75_PR.SGY";

struct NoSink;

impl Sink for NoSink {
    fn frame(&mut self, _h: &mut Harness<'static, SegyViewerApp>, _ms: u64) -> R {
        Ok(())
    }
}

#[test]
fn primary_features_work_with_real_mouse_and_keyboard_input() -> Result<(), String> {
    if !std::path::Path::new(SAMPLE_SEGY_PATH).exists() {
        return Ok(());
    }
    let harness = Harness::builder()
        .with_size(WINDOW)
        .with_accessibility_check(false)
        .build_ui_state(
            |ui, state: &mut SegyViewerApp| state.draw_ui(ui),
            build_app(SAMPLE_SEGY_PATH)?,
        );
    let mut player = Player::new(harness, NoSink);
    for _ in 0..4 {
        player.h.step();
    }
    player.run_primary()
}
