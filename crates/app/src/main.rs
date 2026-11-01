#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

//! SEG-Y viewer desktop application entry point (`app [OPTIONS] [FILE.sgy]`).

use std::process::ExitCode;

use app::{AppCliArgs, SegyViewerApp};
use eframe::egui;

fn main() -> ExitCode {
    let args = match AppCliArgs::parse_from(std::env::args().skip(1)) {
        Ok(parsed) => parsed,
        Err(err) => {
            eprintln!("error: {err}");
            eprintln!(
                "usage: app [--mode wiggle|image|overlay] [--gain dB] [--colormap gray|seismic|viridis|rainbow|bwb] [--polarity normal|reversed] [file.sgy]"
            );
            return ExitCode::FAILURE;
        }
    };

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("SEG-Y Viewer")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([640.0, 400.0]),
        ..eframe::NativeOptions::default()
    };

    let run_res = eframe::run_native(
        "SEG-Y Viewer",
        native_options,
        Box::new(move |_cc| Ok(Box::new(SegyViewerApp::new(args)))),
    );

    match run_res {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("eframe error: {err}");
            ExitCode::FAILURE
        }
    }
}
