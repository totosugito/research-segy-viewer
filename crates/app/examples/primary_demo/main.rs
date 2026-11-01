//! Renders the primary-feature demo (plot type, zoom, pan, selection, multi X axis) to an mp4.
//!
//! Real pointer, click, drag, wheel and key events are injected into the egui UI and every step
//! is rendered offscreen (`egui_kittest` + wgpu) to a PNG frame; `ffmpeg` joins the frames.
//!
//! ```text
//! cargo run --release -p app --example primary_demo -- [file.sgy] [--out DIR]
//! ```
//!
//! Output: `DIR/segy_viewer_primary.mp4` (default `refs/demo`).

mod script;

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use app::SegyViewerApp;
use egui_kittest::Harness;
use script::{Player, R, Sink, WINDOW, build_app};

const DEFAULT_SGY: &str = "/home/toto/Downloads/3X_75_PR.SGY";

struct VideoSink {
    frames_dir: PathBuf,
    list: String,
    count: usize,
    last: String,
}

impl Sink for VideoSink {
    fn frame(&mut self, h: &mut Harness<'static, SegyViewerApp>, ms: u64) -> R {
        // Let the progressive renderer finish and the texture upload before capturing.
        for _ in 0..400 {
            if !h.state().controller().needs_render() {
                break;
            }
            h.step();
            std::thread::sleep(Duration::from_millis(5));
        }
        h.step();
        let image = h.render()?;
        self.count += 1;
        let name = format!("frame_{:04}.png", self.count);
        image
            .save(self.frames_dir.join(&name))
            .map_err(|e| e.to_string())?;
        writeln!(
            self.list,
            "file 'frames/{name}'\nduration {:.3}",
            ms as f64 / 1000.0
        )
        .map_err(|e| e.to_string())?;
        self.last = name;
        if self.count.is_multiple_of(25) {
            println!("  {} frames rendered", self.count);
        }
        Ok(())
    }
}

fn run() -> Result<PathBuf, String> {
    let mut sgy = DEFAULT_SGY.to_string();
    let mut out = PathBuf::from("refs/demo/primary");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = PathBuf::from(args.next().ok_or("--out needs a value")?),
            other if !other.starts_with("--") => sgy = other.to_string(),
            other => return Err(format!("unknown option {other}")),
        }
    }
    let frames_dir = out.join("frames");
    std::fs::create_dir_all(&frames_dir).map_err(|e| e.to_string())?;

    let app = build_app(&sgy)?;
    let harness = Harness::builder()
        .with_size(WINDOW)
        .with_accessibility_check(false)
        .wgpu()
        .build_ui_state(|ui, state: &mut SegyViewerApp| state.draw_ui(ui), app);
    let sink = VideoSink {
        frames_dir,
        list: String::new(),
        count: 0,
        last: String::new(),
    };
    let mut player = Player::new(harness, sink);
    for _ in 0..4 {
        player.h.step();
    }
    player.run_primary()?;

    let mut list = std::mem::take(&mut player.sink.list);
    // The concat demuxer ignores the last duration unless the final file is repeated.
    writeln!(list, "file 'frames/{}'", player.sink.last).map_err(|e| e.to_string())?;
    let list_path = out.join("frames.txt");
    std::fs::write(&list_path, list).map_err(|e| e.to_string())?;

    let mp4 = out.join("segy_viewer_primary.mp4");
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
        Ok(path) => println!("primary demo written to {}", path.display()),
        Err(message) => {
            eprintln!("primary demo failed: {message}");
            std::process::exit(1);
        }
    }
}
