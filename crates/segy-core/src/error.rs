use thiserror::Error;

/// Result alias for `segy-core` operations.
pub type Result<T> = std::result::Result<T, SegyError>;

/// Errors returned while opening, indexing, or decoding a SEG-Y file.
#[derive(Debug, Error)]
pub enum SegyError {
    /// Underlying filesystem I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Memory-mapping failed.
    #[error("memory map error: {0}")]
    Mmap(String),

    /// File is smaller than the mandatory 3600-byte SEG-Y file header.
    #[error("file too small: got {actual_bytes} bytes, minimum is {min_bytes} bytes")]
    FileTooSmall {
        /// Actual byte length of the input.
        actual_bytes: usize,
        /// Minimum required byte length.
        min_bytes: usize,
    },

    /// Binary header specifies an unsupported or invalid sample format code.
    #[error("unsupported SEG-Y data sample format code: {0}")]
    UnsupportedSampleFormat(u16),

    /// Samples per trace (`ns`) is zero or invalid.
    #[error("invalid samples per trace (ns): {0}")]
    InvalidSamplesPerTrace(u16),

    /// Sample interval (`dt`) is zero or invalid.
    #[error("invalid sample interval (dt) in microseconds: {0}")]
    InvalidSampleInterval(u16),

    /// Extended textual header count in rev 1 binary header is negative/unsupported.
    #[error("invalid extended textual header count: {0}")]
    InvalidExtendedHeaderCount(i16),

    /// Extended textual headers extend past the end of the file.
    #[error(
        "truncated extended textual headers: expected {expected_end} bytes, file is {file_size} bytes"
    )]
    TruncatedExtendedHeaders {
        /// Expected byte offset after all extended textual headers.
        expected_end: usize,
        /// Total byte length of the file.
        file_size: usize,
    },

    /// Requested trace index is out of range.
    #[error("trace index {trace_index} out of bounds (trace count: {trace_count})")]
    TraceOutOfBounds {
        /// Requested 0-based trace index.
        trace_index: usize,
        /// Total number of traces in the file.
        trace_count: usize,
    },

    /// Requested sample range `[start..end)` is invalid or out of bounds for the trace.
    #[error("sample range {start}..{end} out of bounds for trace with {sample_count} samples")]
    SampleRangeOutOfBounds {
        /// Start sample index (inclusive).
        start: usize,
        /// End sample index (exclusive).
        end: usize,
        /// Total number of samples in the trace.
        sample_count: usize,
    },

    /// Trace header or sample payload is truncated before the end of the trace.
    #[error(
        "truncated trace {trace_index} at byte offset {offset}: needed {needed_bytes} bytes, file size is {file_size} bytes"
    )]
    TruncatedTrace {
        /// 0-based trace index where truncation was encountered.
        trace_index: usize,
        /// Byte offset of the trace start.
        offset: usize,
        /// Bytes required for the trace header + samples.
        needed_bytes: usize,
        /// Total byte length of the file.
        file_size: usize,
    },

    /// Requested trace header byte offset + field width exceeds the 240-byte trace header.
    #[error(
        "trace header byte offset {offset} (width {width}) out of bounds for 240-byte trace header"
    )]
    HeaderOffsetOutOfBounds {
        /// 0-based byte offset in the trace header.
        offset: usize,
        /// Field width in bytes (2 or 4).
        width: usize,
    },

    /// Requested decode allocation exceeds safety cap.
    #[error(
        "sample allocation request ({requested_samples} samples) exceeds limit ({max_samples} samples)"
    )]
    AllocationLimitExceeded {
        /// Number of samples requested.
        requested_samples: usize,
        /// Maximum allowed samples per decode call.
        max_samples: usize,
    },
}
