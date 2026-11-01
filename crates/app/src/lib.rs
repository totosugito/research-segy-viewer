#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

//! SEG-Y viewer application library (`cli`, headless `controller`, and thin `egui`/`eframe` `gui`).

pub mod cli;
pub mod controller;
pub mod error;
pub mod gui;

pub use cli::{
    AppCliArgs, DEFAULT_SNAPSHOT_HEIGHT, DEFAULT_SNAPSHOT_WIDTH, SnapshotCliArgs,
    parse_colormap_kind, parse_display_mode, parse_polarity,
};
pub use controller::{AppController, DEFAULT_VIEWPORT_HEIGHT, DEFAULT_VIEWPORT_WIDTH};
pub use error::{AppError, Result};
pub use gui::SegyViewerApp;

use render::{ClipStrategy, RenderedImage};

/// Executes an offscreen snapshot render according to `args`, saves the PNG to `args.output_path`,
/// and returns the rendered image.
pub fn run_snapshot(args: &SnapshotCliArgs) -> Result<RenderedImage> {
    let mut ctrl = AppController::new(args.width, args.height);
    ctrl.open_file(&args.input_path)?;
    ctrl.set_display_mode(args.mode);
    ctrl.set_gain_db(args.gain_db);
    ctrl.set_colormap_kind(args.colormap);
    ctrl.set_polarity(args.polarity);

    if let Some(pct) = args.clip_percentile {
        ctrl.set_clip_strategy(ClipStrategy::Percentile { percentile: pct });
    } else if let Some(mult) = args.clip_rms {
        ctrl.set_clip_strategy(ClipStrategy::Rms { multiple: mult });
    }

    ctrl.save_png_with_dimensions(&args.output_path, args.width, args.height)
}

/// Counts the number of distinct RGBA pixel values in `img` (up to `limit`) to verify non-blank rendering.
pub fn count_distinct_colors(img: &RenderedImage, limit: usize) -> usize {
    let mut seen: Vec<[u8; 4]> = Vec::with_capacity(limit.min(64));
    for px in img.rgba.as_chunks::<4>().0 {
        if !seen.contains(px) {
            seen.push(*px);
            if seen.len() >= limit {
                break;
            }
        }
    }
    seen.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use render::{ColormapKind, DisplayMode, Polarity, RenderLod, decode_rgba_png, load_rgba_png};
    use std::path::Path;

    #[test]
    fn smoke_test_gui_app_builds_and_initializes_from_cli() -> Result<()> {
        let args = AppCliArgs::parse_from([
            "--mode",
            "overlay",
            "--gain",
            "3.0",
            "--colormap",
            "seismic",
            "--polarity",
            "reversed",
        ])?;
        let app = SegyViewerApp::new(args);
        assert_eq!(
            app.controller().viewport().display_mode(),
            DisplayMode::Overlay
        );
        assert!((app.controller().render_params().amplitude.gain_db - 3.0).abs() < 1e-5);
        assert_eq!(
            app.controller().render_params().colormap.kind,
            ColormapKind::SeismicRwb
        );
        assert_eq!(
            app.controller().render_params().amplitude.polarity,
            Polarity::Reversed
        );
        Ok(())
    }

    #[test]
    fn sample_file_offscreen_snapshots_render_all_modes_to_valid_non_blank_pngs() -> Result<()> {
        let sample_path = Path::new("/home/toto/Downloads/3X_75_PR.SGY");
        if !sample_path.exists() {
            return Ok(());
        }

        let out_dir = std::env::temp_dir().join("segy_viewer_step7_test_out");
        std::fs::create_dir_all(&out_dir)?;

        let mut rendered_modes = Vec::new();
        for (mode_arg, filename, expected_mode) in [
            ("wiggle", "test_wiggle.png", DisplayMode::Wiggle),
            ("image", "test_image.png", DisplayMode::Image),
            ("overlay", "test_overlay.png", DisplayMode::Overlay),
        ] {
            let out_path = out_dir.join(filename);
            let out_str = out_path.to_string_lossy().into_owned();
            let cli = SnapshotCliArgs::parse_from([
                "/home/toto/Downloads/3X_75_PR.SGY",
                "--mode",
                mode_arg,
                "--width",
                "480",
                "--height",
                "320",
                &out_str,
            ])?;
            assert_eq!(cli.mode, expected_mode);

            let img = run_snapshot(&cli)?;
            assert_eq!(img.width, 480);
            assert_eq!(img.height, 320);
            assert_eq!(img.rgba.len(), 480 * 320 * 4);
            assert!(
                count_distinct_colors(&img, 16) >= 2,
                "mode {mode_arg} produced a blank uniform image"
            );

            // Verify the PNG file on disk decodes back to the exact same dimensions and pixels
            let disk_img = load_rgba_png(&out_path)?;
            assert_eq!(disk_img.width, 480);
            assert_eq!(disk_img.height, 320);
            assert_eq!(disk_img.rgba, img.rgba);

            let _ = std::fs::remove_file(&out_path);
            rendered_modes.push(img);
        }

        // Verify that wiggle, image, and overlay produce distinct visual outputs
        assert_ne!(rendered_modes[0].rgba, rendered_modes[1].rgba);
        assert_ne!(rendered_modes[1].rgba, rendered_modes[2].rgba);
        assert_ne!(rendered_modes[0].rgba, rendered_modes[2].rgba);

        Ok(())
    }

    #[test]
    fn background_worker_progressive_render_completes_without_blocking() -> Result<()> {
        let sample_path = Path::new("/home/toto/Downloads/3X_75_PR.SGY");
        if !sample_path.exists() {
            return Ok(());
        }

        let mut ctrl = AppController::new(320, 200);
        ctrl.open_file(sample_path)?;
        ctrl.set_display_mode(DisplayMode::Overlay);

        // Drive the non-blocking background worker until it settles at RenderLod::Full.
        for _ in 0..200 {
            if !ctrl.poll_background_render() && ctrl.rendered_lod() == Some(RenderLod::Full) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        assert_eq!(ctrl.rendered_lod(), Some(RenderLod::Full));
        assert!(!ctrl.needs_render());
        let cached = ctrl.cached_image();
        assert!(cached.is_some());
        if let Some(img) = cached {
            assert_eq!(img.width, 320);
            assert_eq!(img.height, 200);
            let png = img.to_png_bytes()?;
            let decoded = decode_rgba_png(&png)?;
            assert_eq!(decoded.width, 320);
            assert_eq!(decoded.height, 200);
        }

        // Polling again when nothing changed returns false immediately and does not re-render.
        let passes_before = ctrl.render_pass_count();
        assert!(!ctrl.poll_background_render());
        assert_eq!(ctrl.render_pass_count(), passes_before);

        Ok(())
    }
}
