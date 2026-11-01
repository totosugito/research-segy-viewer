#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

//! Offscreen PNG snapshot generator for the SEG-Y viewer:
//! `cargo run -p app --example snapshot -- /home/toto/Downloads/3X_75_PR.SGY --mode wiggle out.png`

use std::process::ExitCode;

use app::{SnapshotCliArgs, count_distinct_colors, run_snapshot};

fn main() -> ExitCode {
    let args = match SnapshotCliArgs::parse_from(std::env::args().skip(1)) {
        Ok(parsed) => parsed,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };

    match run_snapshot(&args) {
        Ok(img) => {
            let distinct = count_distinct_colors(&img, 32);
            if img.width != args.width || img.height != args.height || distinct < 2 {
                eprintln!(
                    "error: snapshot validation failed (got {}x{}, distinct colors: {})",
                    img.width, img.height, distinct
                );
                return ExitCode::FAILURE;
            }
            println!(
                "Rendered {:?} snapshot ({}x{}, {}+ distinct colors) -> {}",
                args.mode,
                img.width,
                img.height,
                distinct,
                args.output_path.display()
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
