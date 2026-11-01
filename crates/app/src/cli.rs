//! Command-line argument parsing for the interactive SEG-Y viewer (`app <file.sgy>`)
//! and the offscreen snapshot tool (`snapshot <file.sgy> --mode wiggle <out.png>`).

use std::path::PathBuf;

use render::{ColormapKind, DisplayMode, MAX_VIEWPORT_DIMENSION, Polarity};

use crate::error::{AppError, Result};

/// Default snapshot image width in pixels.
pub const DEFAULT_SNAPSHOT_WIDTH: u32 = 1200;

/// Default snapshot image height in pixels.
pub const DEFAULT_SNAPSHOT_HEIGHT: u32 = 800;

/// Parsed CLI options for the main interactive viewer binary (`app [OPTIONS] [FILE.sgy]`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AppCliArgs {
    /// Optional SEG-Y file path to open immediately on startup without a file dialog.
    pub file_path: Option<PathBuf>,
    /// Optional initial display mode (`wiggle`, `image`, or `overlay`).
    pub mode: Option<DisplayMode>,
    /// Optional initial gain in decibels (`dB`).
    pub gain_db: Option<f32>,
    /// Optional initial colormap family.
    pub colormap: Option<ColormapKind>,
    /// Optional initial display polarity (`normal` or `reversed`).
    pub polarity: Option<Polarity>,
}

impl AppCliArgs {
    /// Parses CLI arguments from an iterator of strings (excluding `argv[0]`).
    pub fn parse_from<I, S>(args: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut parsed = Self::default();
        let mut iter = args.into_iter();

        while let Some(raw) = iter.next() {
            let arg = raw.as_ref();
            match arg {
                "--mode" | "-m" => {
                    let val = next_value(&mut iter, arg)?;
                    parsed.mode = Some(parse_display_mode(&val)?);
                }
                "--gain" | "-g" => {
                    let val = next_value(&mut iter, arg)?;
                    parsed.gain_db = Some(parse_f32_arg(arg, &val)?);
                }
                "--colormap" | "-c" => {
                    let val = next_value(&mut iter, arg)?;
                    parsed.colormap = Some(parse_colormap_kind(&val)?);
                }
                "--polarity" | "-p" => {
                    let val = next_value(&mut iter, arg)?;
                    parsed.polarity = Some(parse_polarity(&val)?);
                }
                other if other.starts_with('-') => {
                    return Err(AppError::InvalidArgument(format!(
                        "unrecognized flag '{other}'"
                    )));
                }
                positional => {
                    if parsed.file_path.is_some() {
                        return Err(AppError::InvalidArgument(format!(
                            "unexpected extra positional argument '{positional}'"
                        )));
                    }
                    parsed.file_path = Some(PathBuf::from(positional));
                }
            }
        }

        Ok(parsed)
    }
}

/// Parsed CLI options for the offscreen snapshot example (`snapshot <file.sgy> --mode wiggle <out.png>`).
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotCliArgs {
    /// Input SEG-Y file path.
    pub input_path: PathBuf,
    /// Output PNG file path.
    pub output_path: PathBuf,
    /// Rendering mode (`Wiggle`, `Image`, or `Overlay`).
    pub mode: DisplayMode,
    /// Output image width in pixels.
    pub width: u32,
    /// Output image height in pixels.
    pub height: u32,
    /// Gain in decibels (`dB`).
    pub gain_db: f32,
    /// Colormap family.
    pub colormap: ColormapKind,
    /// Display polarity.
    pub polarity: Polarity,
    /// Optional percentile clip override (`(0.0, 100.0]`).
    pub clip_percentile: Option<f32>,
    /// Optional RMS clip multiplier override (`> 0.0`).
    pub clip_rms: Option<f32>,
}

impl SnapshotCliArgs {
    /// Parses snapshot CLI arguments from an iterator of strings (excluding `argv[0]`).
    pub fn parse_from<I, S>(args: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut positionals: Vec<PathBuf> = Vec::new();
        let mut mode = DisplayMode::Image;
        let mut width = DEFAULT_SNAPSHOT_WIDTH;
        let mut height = DEFAULT_SNAPSHOT_HEIGHT;
        let mut gain_db = 0.0_f32;
        let mut colormap = ColormapKind::Grayscale;
        let mut polarity = Polarity::Normal;
        let mut clip_percentile = None;
        let mut clip_rms = None;

        let mut iter = args.into_iter();
        while let Some(raw) = iter.next() {
            let arg = raw.as_ref();
            match arg {
                "--mode" | "-m" => {
                    let val = next_value(&mut iter, arg)?;
                    mode = parse_display_mode(&val)?;
                }
                "--width" | "-w" => {
                    let val = next_value(&mut iter, arg)?;
                    width = parse_dimension_arg(arg, &val)?;
                }
                "--height" | "-h" => {
                    let val = next_value(&mut iter, arg)?;
                    height = parse_dimension_arg(arg, &val)?;
                }
                "--gain" | "-g" => {
                    let val = next_value(&mut iter, arg)?;
                    gain_db = parse_f32_arg(arg, &val)?;
                }
                "--colormap" | "-c" => {
                    let val = next_value(&mut iter, arg)?;
                    colormap = parse_colormap_kind(&val)?;
                }
                "--polarity" | "-p" => {
                    let val = next_value(&mut iter, arg)?;
                    polarity = parse_polarity(&val)?;
                }
                "--clip-percentile" => {
                    let val = next_value(&mut iter, arg)?;
                    clip_percentile = Some(parse_f32_arg(arg, &val)?);
                }
                "--clip-rms" => {
                    let val = next_value(&mut iter, arg)?;
                    clip_rms = Some(parse_f32_arg(arg, &val)?);
                }
                other if other.starts_with('-') => {
                    return Err(AppError::InvalidArgument(format!(
                        "unrecognized flag '{other}'"
                    )));
                }
                positional => {
                    positionals.push(PathBuf::from(positional));
                }
            }
        }

        if positionals.len() != 2 {
            return Err(AppError::InvalidArgument(
                "usage: snapshot <file.sgy> [--mode wiggle|image|overlay] [--width W] [--height H] <out.png>"
                    .to_string(),
            ));
        }

        let output_path = positionals.pop().unwrap_or_default();
        let input_path = positionals.pop().unwrap_or_default();

        Ok(Self {
            input_path,
            output_path,
            mode,
            width,
            height,
            gain_db,
            colormap,
            polarity,
            clip_percentile,
            clip_rms,
        })
    }
}

/// Parses a display mode name (`wiggle`, `image`, `vd`, `overlay`).
pub fn parse_display_mode(value: &str) -> Result<DisplayMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "wiggle" | "wig" | "va" => Ok(DisplayMode::Wiggle),
        "image" | "img" | "vd" => Ok(DisplayMode::Image),
        "overlay" | "both" | "wiggle+image" => Ok(DisplayMode::Overlay),
        other => Err(AppError::InvalidArgument(format!(
            "unknown display mode '{other}' (expected wiggle, image, or overlay)"
        ))),
    }
}

/// Parses a colormap name (`gray`, `grayscale`, `seismic`, `rwb`, `viridis`, `rainbow`, `bwb`).
pub fn parse_colormap_kind(value: &str) -> Result<ColormapKind> {
    match value.trim().to_ascii_lowercase().as_str() {
        "gray" | "grey" | "grayscale" | "greyscale" => Ok(ColormapKind::Grayscale),
        "seismic" | "rwb" | "bwr" | "seismic-rwb" => Ok(ColormapKind::SeismicRwb),
        "viridis" => Ok(ColormapKind::Viridis),
        "rainbow" => Ok(ColormapKind::Rainbow),
        "bwb" | "brown" | "brown-white-black" => Ok(ColormapKind::BrownWhiteBlack),
        other => Err(AppError::InvalidArgument(format!(
            "unknown colormap '{other}' (expected grayscale, seismic, viridis, rainbow, or bwb)"
        ))),
    }
}

/// Parses a polarity name (`normal`, `reversed`, `flip`, `flipped`).
pub fn parse_polarity(value: &str) -> Result<Polarity> {
    match value.trim().to_ascii_lowercase().as_str() {
        "normal" | "norm" | "standard" | "+" => Ok(Polarity::Normal),
        "reversed" | "reverse" | "rev" | "flip" | "flipped" | "-" => Ok(Polarity::Reversed),
        other => Err(AppError::InvalidArgument(format!(
            "unknown polarity '{other}' (expected normal or reversed)"
        ))),
    }
}

fn next_value<I, S>(iter: &mut I, flag: &str) -> Result<String>
where
    I: Iterator<Item = S>,
    S: AsRef<str>,
{
    iter.next()
        .map(|s| s.as_ref().to_string())
        .ok_or_else(|| AppError::InvalidArgument(format!("missing value for '{flag}'")))
}

fn parse_f32_arg(flag: &str, raw: &str) -> Result<f32> {
    let val: f32 = raw.parse().map_err(|_| {
        AppError::InvalidArgument(format!("invalid floating-point value '{raw}' for '{flag}'"))
    })?;
    if !val.is_finite() {
        return Err(AppError::InvalidArgument(format!(
            "non-finite value '{raw}' for '{flag}'"
        )));
    }
    Ok(val)
}

fn parse_dimension_arg(flag: &str, raw: &str) -> Result<u32> {
    let val: u32 = raw.parse().map_err(|_| {
        AppError::InvalidArgument(format!("invalid dimension '{raw}' for '{flag}'"))
    })?;
    if val == 0 || val > MAX_VIEWPORT_DIMENSION {
        return Err(AppError::InvalidArgument(format!(
            "dimension {val} for '{flag}' must be in 1..={MAX_VIEWPORT_DIMENSION}"
        )));
    }
    Ok(val)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_cli_parses_file_and_optional_flags() -> Result<()> {
        let empty = AppCliArgs::parse_from(Vec::<&str>::new())?;
        assert_eq!(empty, AppCliArgs::default());

        let file_only = AppCliArgs::parse_from(["/home/toto/Downloads/3X_75_PR.SGY"])?;
        assert_eq!(
            file_only.file_path,
            Some(PathBuf::from("/home/toto/Downloads/3X_75_PR.SGY"))
        );

        let full = AppCliArgs::parse_from([
            "test.sgy",
            "--mode",
            "overlay",
            "--gain",
            "6.0",
            "--colormap",
            "seismic",
            "--polarity",
            "reversed",
        ])?;
        assert_eq!(full.file_path, Some(PathBuf::from("test.sgy")));
        assert_eq!(full.mode, Some(DisplayMode::Overlay));
        assert_eq!(full.gain_db, Some(6.0));
        assert_eq!(full.colormap, Some(ColormapKind::SeismicRwb));
        assert_eq!(full.polarity, Some(Polarity::Reversed));
        Ok(())
    }

    #[test]
    fn snapshot_cli_parses_interleaved_positionals_and_flags() -> Result<()> {
        let args = SnapshotCliArgs::parse_from([
            "/home/toto/Downloads/3X_75_PR.SGY",
            "--mode",
            "wiggle",
            "out.png",
        ])?;
        assert_eq!(
            args.input_path,
            PathBuf::from("/home/toto/Downloads/3X_75_PR.SGY")
        );
        assert_eq!(args.output_path, PathBuf::from("out.png"));
        assert_eq!(args.mode, DisplayMode::Wiggle);
        assert_eq!(args.width, DEFAULT_SNAPSHOT_WIDTH);
        assert_eq!(args.height, DEFAULT_SNAPSHOT_HEIGHT);

        let custom = SnapshotCliArgs::parse_from([
            "--mode",
            "overlay",
            "--width",
            "640",
            "--height",
            "480",
            "--gain",
            "3.5",
            "--colormap",
            "viridis",
            "--polarity",
            "flipped",
            "in.sgy",
            "refs/out/overlay.png",
        ])?;
        assert_eq!(custom.input_path, PathBuf::from("in.sgy"));
        assert_eq!(custom.output_path, PathBuf::from("refs/out/overlay.png"));
        assert_eq!(custom.mode, DisplayMode::Overlay);
        assert_eq!(custom.width, 640);
        assert_eq!(custom.height, 480);
        assert!((custom.gain_db - 3.5).abs() < 1e-6);
        assert_eq!(custom.colormap, ColormapKind::Viridis);
        assert_eq!(custom.polarity, Polarity::Reversed);
        Ok(())
    }

    #[test]
    fn cli_rejects_bad_flags_and_missing_values() {
        assert!(AppCliArgs::parse_from(["--unknown"]).is_err());
        assert!(AppCliArgs::parse_from(["--mode"]).is_err());
        assert!(AppCliArgs::parse_from(["--mode", "3d"]).is_err());
        assert!(SnapshotCliArgs::parse_from(["only_one_path.sgy"]).is_err());
        assert!(SnapshotCliArgs::parse_from(["a.sgy", "b.png", "--width", "0"]).is_err());
    }
}
