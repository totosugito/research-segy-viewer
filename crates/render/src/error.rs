use thiserror::Error;

/// Result alias for `render` operations.
pub type Result<T> = std::result::Result<T, RenderError>;

/// Errors returned by headless seismic rendering operations.
#[derive(Debug, Error)]
pub enum RenderError {
    /// Underlying SEG-Y parser/reader error.
    #[error("SEG-Y error: {0}")]
    Segy(#[from] segy_core::SegyError),

    /// Viewport width or height is zero or exceeds safety limits.
    #[error("invalid viewport dimensions: {width}x{height} (max {max_dimension})")]
    InvalidViewport {
        /// Requested viewport width in pixels.
        width: u32,
        /// Requested viewport height in pixels.
        height: u32,
        /// Maximum allowed dimension in pixels.
        max_dimension: u32,
    },

    /// Requested trace range `[start..end)` is empty or out of bounds.
    #[error("invalid trace range {start}..{end} (total traces: {total_traces})")]
    InvalidTraceRange {
        /// Inclusive start trace index.
        start: usize,
        /// Exclusive end trace index.
        end: usize,
        /// Total available traces.
        total_traces: usize,
    },

    /// Requested sample range `[start..end)` is empty or out of bounds.
    #[error("invalid sample range {start}..{end} (total samples: {total_samples})")]
    InvalidSampleRange {
        /// Inclusive start sample index.
        start: usize,
        /// Exclusive end sample index.
        end: usize,
        /// Total available samples per trace.
        total_samples: usize,
    },

    /// Caller-supplied RGBA pixel buffer does not match the exact viewport byte size (`width * height * 4`).
    #[error(
        "output RGBA buffer length mismatch: got {actual_bytes} bytes, expected {expected_bytes} bytes for {width}x{height} viewport"
    )]
    BufferSizeMismatch {
        /// Actual byte length of the supplied buffer.
        actual_bytes: usize,
        /// Expected byte length (`width * height * 4`).
        expected_bytes: usize,
        /// Viewport width in pixels.
        width: u32,
        /// Viewport height in pixels.
        height: u32,
    },
}
