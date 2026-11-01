//! Application-level error types for the SEG-Y viewer.

use thiserror::Error;

/// Result alias for `app` operations.
pub type Result<T> = std::result::Result<T, AppError>;

/// Errors returned by the SEG-Y viewer application controller and CLI tools.
#[derive(Debug, Error)]
pub enum AppError {
    /// Underlying SEG-Y parser/reader error.
    #[error("SEG-Y error: {0}")]
    Segy(#[from] segy_core::SegyError),

    /// Underlying headless renderer or PNG encoder error.
    #[error("render error: {0}")]
    Render(#[from] render::RenderError),

    /// Standard I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Invalid command-line argument.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// Attempted to render or export when no SEG-Y file is loaded.
    #[error("no SEG-Y file is currently loaded")]
    NoFileLoaded,

    /// Automated scenario runner failure (panic, render error, blank frame, or frame budget exceeded).
    #[error("scenario failed: {0}")]
    ScenarioFailed(String),
}
