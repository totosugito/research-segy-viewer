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

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use app::{
    AppCliArgs, ScenarioReport, SegyViewerApp, finalize_scenario_report, run_scenario_headless,
};
use eframe::egui;
use segy_core::SegyFile;

fn main() -> ExitCode {
    let args = match AppCliArgs::parse_from(std::env::args().skip(1)) {
        Ok(parsed) => parsed,
        Err(err) => {
            eprintln!("error: {err}");
            eprintln!(
                "usage: app [--mode wiggle|image|overlay] [--gain dB] [--colormap gray|seismic|viridis|rainbow|bwb] [--polarity normal|reversed] [--scenario smoke] [--scenario-fast] [file.sgy]"
            );
            return ExitCode::FAILURE;
        }
    };

    if args.scenario.is_some() {
        let Some(path) = &args.file_path else {
            eprintln!("error: --scenario requires a SEG-Y input file path");
            return ExitCode::FAILURE;
        };
        if let Err(err) = SegyFile::open(path) {
            eprintln!("error: failed to open '{}': {err}", path.display());
            return ExitCode::FAILURE;
        }
        if args.scenario_fast {
            return match run_scenario_headless(&args) {
                Ok(_) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("error: {err}");
                    ExitCode::FAILURE
                }
            };
        }
    }

    let is_scenario = args.scenario.is_some();
    let scenario_report = Arc::new(Mutex::new(ScenarioReport::default()));
    let report_for_app = Arc::clone(&scenario_report);

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("SEG-Y Viewer")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([640.0, 400.0]),
        ..eframe::NativeOptions::default()
    };

    let run_res = catch_unwind(AssertUnwindSafe(|| {
        eframe::run_native(
            "SEG-Y Viewer",
            native_options,
            Box::new(move |_cc| {
                Ok(Box::new(SegyViewerApp::new_with_scenario_report(
                    args,
                    report_for_app,
                )))
            }),
        )
    }));

    match run_res {
        Ok(Ok(())) => {
            if is_scenario {
                let report = scenario_report
                    .lock()
                    .map(|g| g.clone())
                    .unwrap_or_default();
                if let Err(err) = finalize_scenario_report(report) {
                    eprintln!("error: {err}");
                    return ExitCode::FAILURE;
                }
            }
            ExitCode::SUCCESS
        }
        Ok(Err(err)) => {
            eprintln!("eframe error: {err}");
            ExitCode::FAILURE
        }
        Err(_) => {
            eprintln!("error: fatal panic in application window loop");
            ExitCode::FAILURE
        }
    }
}
