//! Renders the feature demo to an mp4 without opening a window.
//!
//! Every step of [`app::demo_steps`] is applied to the real egui UI, rendered offscreen with
//! `egui_kittest` (wgpu) into a PNG frame, and `ffmpeg` joins the frames with each step's hold
//! time. Usage:
//!
//! ```text
//! cargo run --release -p app --example demo_video -- [file.sgy] [--out DIR] [--size WxH] [--speed F]
//! ```
//!
//! Output: `DIR/segy_viewer_demo.mp4` (default `refs/demo`), plus the frames in `DIR/frames`.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use app::{AppCliArgs, DemoOverlay, SegyViewerApp, demo_steps};
use eframe::egui::Vec2;
use egui_kittest::Harness;

const DEFAULT_SGY: &str = "/home/toto/Downloads/3X_75_PR.SGY";

struct Options {
    sgy: String,
    out: PathBuf,
    width: u32,
    height: u32,
    speed: f64,
}

fn parse_options() -> Result<Options, String> {
    let mut opts = Options {
        sgy: DEFAULT_SGY.to_string(),
        out: PathBuf::from("refs/demo"),
        width: 1280,
        height: 800,
        speed: 1.0,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => opts.out = PathBuf::from(args.next().ok_or("--out needs a value")?),
            "--size" => {
                let v = args.next().ok_or("--size needs WxH")?;
                let (w, h) = v.split_once('x').ok_or("--size must look like 1280x800")?;
                opts.width = w.parse().map_err(|_| "bad width")?;
                opts.height = h.parse().map_err(|_| "bad height")?;
            }
            "--speed" => {
                opts.speed = args
                    .next()
                    .ok_or("--speed needs a value")?
                    .parse()
                    .map_err(|_| "bad speed")?;
            }
            other if !other.starts_with("--") => opts.sgy = other.to_string(),
            other => return Err(format!("unknown option {other}")),
        }
    }
    if opts.speed <= 0.0 {
        return Err("--speed must be positive".to_string());
    }
    Ok(opts)
}

fn run() -> Result<PathBuf, String> {
    let opts = parse_options()?;
    let frames_dir = opts.out.join("frames");
    std::fs::create_dir_all(&frames_dir).map_err(|e| e.to_string())?;

    let args = AppCliArgs::parse_from([opts.sgy.as_str()]).map_err(|e| e.to_string())?;
    let app = SegyViewerApp::new(args);
    let mut harness = Harness::builder()
        .with_size(Vec2::new(opts.width as f32, opts.height as f32))
        .with_accessibility_check(false)
        .wgpu()
        .build_ui_state(|ui, state: &mut SegyViewerApp| state.draw_ui(ui), app);

    // First frames lay the canvas out so the viewport has its real size.
    for _ in 0..4 {
        harness.step();
    }

    let steps = demo_steps();
    let mut overlay = DemoOverlay::default();
    let mut list = String::new();
    let mut last_frame = String::new();

    for (i, step) in steps.iter().enumerate() {
        overlay.caption = step.caption.to_string();
        overlay.index = i + 1;
        overlay.total = steps.len();
        (step.run)(harness.state_mut(), &mut overlay)
            .map_err(|e| format!("step {} '{}': {e}", i + 1, step.caption))?;
        harness.state_mut().set_demo_overlay(Some(overlay.clone()));

        // Let the progressive renderer finish and the texture upload, then settle the layout.
        for _ in 0..400 {
            harness.step();
            if !harness.state().controller().needs_render() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        for _ in 0..3 {
            harness.step();
        }

        let image = harness.render()?;
        let name = format!("frame_{:03}.png", i + 1);
        image
            .save(frames_dir.join(&name))
            .map_err(|e| e.to_string())?;
        let seconds = step.hold_ms as f64 / 1000.0 / opts.speed;
        writeln!(list, "file 'frames/{name}'\nduration {seconds:.3}").map_err(|e| e.to_string())?;
        last_frame = name;
        println!(
            "[{:>2}/{}] {:.1}s  {}",
            i + 1,
            steps.len(),
            seconds,
            step.caption
        );
    }
    // The concat demuxer ignores the duration of the last entry unless it is repeated.
    writeln!(list, "file 'frames/{last_frame}'").map_err(|e| e.to_string())?;
    let list_path = opts.out.join("frames.txt");
    std::fs::write(&list_path, list).map_err(|e| e.to_string())?;

    let mp4 = opts.out.join("segy_viewer_demo.mp4");
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
        ])
        .arg(&list_path)
        .args([
            "-vf",
            "fps=25,scale=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p",
            "-c:v",
            "libx264",
            "-crf",
            "20",
            "-movflags",
            "+faststart",
        ])
        .arg(&mp4)
        .status()
        .map_err(|e| format!("could not run ffmpeg: {e}"))?;
    if !status.success() {
        return Err(format!("ffmpeg failed: {status}"));
    }
    Ok(mp4)
}

fn main() {
    match run() {
        Ok(path) => println!("demo video written to {}", path.display()),
        Err(message) => {
            eprintln!("demo video failed: {message}");
            std::process::exit(1);
        }
    }
}
