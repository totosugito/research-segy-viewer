use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use tiverse_mmap::{Mmap, MmapOptions, ReadOnly};

use crate::ebcdic::{TEXTUAL_HEADER_LEN, TextualHeader};
use crate::error::{Result, SegyError};
use crate::header::{
    AxisHeaderKey, BinaryHeader, FILE_HEADER_LEN, SampleFormat, TRACE_HEADER_LEN, TraceHeader,
    TraceHeaderView,
};
use crate::sample::decode_samples;

/// Maximum number of variable-length trace entries allowed in the fallback offset table.
const MAX_VARIABLE_TRACES: usize = 10_000_000;

enum Backing {
    Mmap(Mmap<ReadOnly>),
    Bytes(Arc<[u8]>),
}

impl std::fmt::Debug for Backing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mmap(mmap) => f
                .debug_struct("Backing::Mmap")
                .field("len", &mmap.as_slice().len())
                .finish(),
            Self::Bytes(bytes) => f
                .debug_struct("Backing::Bytes")
                .field("len", &bytes.len())
                .finish(),
        }
    }
}

impl Backing {
    #[inline]
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Mmap(mmap) => mmap.as_slice(),
            Self::Bytes(bytes) => bytes,
        }
    }
}

/// Byte offset and sample count for a single trace in a variable-length SEG-Y file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraceLocation {
    /// Byte offset of the 240-byte trace header from the start of the file.
    pub offset: u64,
    /// Number of samples in this trace.
    pub samples: u16,
}

/// Lazy trace index: constant-stride O(1) math when traces have fixed length,
/// or a compact offset table when traces have variable length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceIndex {
    /// Constant-stride fast path (zero per-trace heap allocation).
    FixedStride {
        /// Byte offset of trace 0 from the start of the file.
        first_trace_offset: usize,
        /// Byte stride between consecutive traces (`240 + samples_per_trace * bytes_per_sample`).
        trace_stride: usize,
        /// Number of samples in every trace.
        samples_per_trace: u16,
        /// Total number of traces in the file.
        trace_count: usize,
    },
    /// Offset table used only when trace headers override `ns` with varying lengths.
    VariableLength {
        /// Per-trace offset and sample count.
        traces: Vec<TraceLocation>,
    },
}

impl TraceIndex {
    /// Returns the total number of indexed traces.
    #[inline]
    pub fn len(&self) -> usize {
        match self {
            Self::FixedStride { trace_count, .. } => *trace_count,
            Self::VariableLength { traces } => traces.len(),
        }
    }

    /// Returns `true` if there are zero traces in the file.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns `(trace_header_offset, sample_count)` for `trace_index`.
    #[inline]
    pub fn locate(&self, trace_index: usize) -> Result<(usize, u16)> {
        let count = self.len();
        if trace_index >= count {
            return Err(SegyError::TraceOutOfBounds {
                trace_index,
                trace_count: count,
            });
        }
        match self {
            Self::FixedStride {
                first_trace_offset,
                trace_stride,
                samples_per_trace,
                ..
            } => {
                let offset = trace_index
                    .checked_mul(*trace_stride)
                    .and_then(|delta| first_trace_offset.checked_add(delta))
                    .ok_or(SegyError::TraceOutOfBounds {
                        trace_index,
                        trace_count: count,
                    })?;
                Ok((offset, *samples_per_trace))
            }
            Self::VariableLength { traces } => {
                let loc = traces.get(trace_index).ok_or(SegyError::TraceOutOfBounds {
                    trace_index,
                    trace_count: count,
                })?;
                Ok((loc.offset as usize, loc.samples))
            }
        }
    }
}

/// Read-only SEG-Y (rev 0/1) file handle backed by a read-only memory map or byte buffer.
#[derive(Debug)]
pub struct SegyFile {
    backing: Backing,
    textual_header: TextualHeader,
    binary_header: BinaryHeader,
    trace_index: TraceIndex,
}

impl SegyFile {
    /// Memory-maps a SEG-Y file read-only from disk and parses its file headers and lazy trace index.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path_ref = path.as_ref();
        let meta = std::fs::metadata(path_ref)?;
        let len = meta.len();
        if len < FILE_HEADER_LEN as u64 {
            return Err(SegyError::FileTooSmall {
                actual_bytes: len as usize,
                min_bytes: FILE_HEADER_LEN,
            });
        }
        let mmap = MmapOptions::new()
            .path(path_ref)
            .map_readonly()
            .map_err(|err| SegyError::Mmap(err.to_string()))?;
        Self::from_backing(Backing::Mmap(mmap))
    }

    /// Parses a SEG-Y file from an in-memory byte slice/buffer (useful for synthetic tests).
    pub fn from_bytes(bytes: impl Into<Arc<[u8]>>) -> Result<Self> {
        Self::from_backing(Backing::Bytes(bytes.into()))
    }

    fn from_backing(backing: Backing) -> Result<Self> {
        let data = backing.as_slice();
        if data.len() < FILE_HEADER_LEN {
            return Err(SegyError::FileTooSmall {
                actual_bytes: data.len(),
                min_bytes: FILE_HEADER_LEN,
            });
        }
        let textual_header = TextualHeader::parse(&data[..TEXTUAL_HEADER_LEN])?;
        let binary_header = BinaryHeader::parse(&data[TEXTUAL_HEADER_LEN..FILE_HEADER_LEN])?;
        let first_trace_offset = compute_first_trace_offset(&binary_header, data.len())?;
        let trace_index = build_trace_index(data, first_trace_offset, &binary_header)?;

        Ok(Self {
            backing,
            textual_header,
            binary_header,
            trace_index,
        })
    }

    /// Returns the total size of the SEG-Y file in bytes.
    #[inline]
    pub fn file_size(&self) -> usize {
        self.backing.as_slice().len()
    }

    /// Returns the parsed 3200-byte textual file header.
    #[inline]
    pub fn textual_header(&self) -> &TextualHeader {
        &self.textual_header
    }

    /// Returns the parsed 400-byte binary file header.
    #[inline]
    pub fn binary_header(&self) -> &BinaryHeader {
        &self.binary_header
    }

    /// Returns the data sample format from the binary header.
    #[inline]
    pub fn sample_format(&self) -> SampleFormat {
        self.binary_header.sample_format
    }

    /// Returns the default sample interval in microseconds (`dt`) from the binary header.
    #[inline]
    pub fn sample_interval_us(&self) -> u16 {
        self.binary_header.sample_interval_us
    }

    /// Returns the nominal number of samples per trace (`ns`) from the binary header.
    #[inline]
    pub fn samples_per_trace(&self) -> u16 {
        self.binary_header.samples_per_trace
    }

    /// Returns the total number of traces in the file.
    #[inline]
    pub fn trace_count(&self) -> usize {
        self.trace_index.len()
    }

    /// Returns `true` if traces are indexed via the O(1) constant-stride fast path.
    #[inline]
    pub const fn is_fixed_stride(&self) -> bool {
        matches!(self.trace_index, TraceIndex::FixedStride { .. })
    }

    /// Returns the underlying lazy [`TraceIndex`].
    #[inline]
    pub fn trace_index(&self) -> &TraceIndex {
        &self.trace_index
    }

    /// Returns a zero-copy [`TraceHeaderView`] for `trace_index` (reads only requested header offsets).
    #[inline]
    pub fn trace_header_view(&self, trace_index: usize) -> Result<TraceHeaderView<'_>> {
        let (offset, _) = self.trace_index.locate(trace_index)?;
        let data = self.backing.as_slice();
        let end = offset
            .checked_add(TRACE_HEADER_LEN)
            .ok_or(SegyError::TruncatedTrace {
                trace_index,
                offset,
                needed_bytes: TRACE_HEADER_LEN,
                file_size: data.len(),
            })?;
        let slice = data.get(offset..end).ok_or(SegyError::TruncatedTrace {
            trace_index,
            offset,
            needed_bytes: TRACE_HEADER_LEN,
            file_size: data.len(),
        })?;
        TraceHeaderView::new(slice)
    }

    /// Parses and returns the [`TraceHeader`] for `trace_index`.
    pub fn trace_header(&self, trace_index: usize) -> Result<TraceHeader> {
        self.trace_header_view(trace_index)?.to_trace_header()
    }

    /// Reads only the chosen horizontal axis header key for `trace_index`.
    #[inline]
    pub fn trace_axis_value(&self, trace_index: usize, key: AxisHeaderKey) -> Result<i64> {
        if key == AxisHeaderKey::TraceNumber {
            let count = self.trace_count();
            if trace_index >= count {
                return Err(SegyError::TraceOutOfBounds {
                    trace_index,
                    trace_count: count,
                });
            }
            return Ok((trace_index as i64).saturating_add(1));
        }
        self.trace_header_view(trace_index)?
            .axis_value(trace_index, key)
    }

    /// Returns the number of samples in `trace_index`.
    #[inline]
    pub fn trace_sample_count(&self, trace_index: usize) -> Result<usize> {
        let (_, ns) = self.trace_index.locate(trace_index)?;
        Ok(usize::from(ns))
    }

    /// Decodes all samples for `trace_index` into a newly allocated `Vec<f32>`.
    pub fn read_trace_samples(&self, trace_index: usize) -> Result<Vec<f32>> {
        let mut out = Vec::new();
        self.read_trace_samples_into(trace_index, &mut out)?;
        Ok(out)
    }

    /// Decodes all samples for `trace_index` into `out` (reusing its allocation).
    pub fn read_trace_samples_into(&self, trace_index: usize, out: &mut Vec<f32>) -> Result<()> {
        let (_, ns) = self.trace_index.locate(trace_index)?;
        self.read_trace_sample_window_into(trace_index, 0..usize::from(ns), out)
    }

    /// Decodes only the sample sub-range `sample_range` (`start..end`) of `trace_index` into `out`.
    pub fn read_trace_sample_window_into(
        &self,
        trace_index: usize,
        sample_range: Range<usize>,
        out: &mut Vec<f32>,
    ) -> Result<()> {
        let (trace_offset, ns_u16) = self.trace_index.locate(trace_index)?;
        let ns = usize::from(ns_u16);
        if sample_range.start > sample_range.end || sample_range.end > ns {
            return Err(SegyError::SampleRangeOutOfBounds {
                start: sample_range.start,
                end: sample_range.end,
                sample_count: ns,
            });
        }
        let window_count = sample_range.end - sample_range.start;
        let bps = self.binary_header.sample_format.bytes_per_sample();
        let samples_start_offset = trace_offset
            .checked_add(TRACE_HEADER_LEN)
            .and_then(|base| {
                sample_range
                    .start
                    .checked_mul(bps)
                    .and_then(|delta| base.checked_add(delta))
            })
            .ok_or(SegyError::TruncatedTrace {
                trace_index,
                offset: trace_offset,
                needed_bytes: TRACE_HEADER_LEN.saturating_add(ns.saturating_mul(bps)),
                file_size: self.file_size(),
            })?;
        let window_bytes = window_count
            .checked_mul(bps)
            .ok_or(SegyError::TruncatedTrace {
                trace_index,
                offset: trace_offset,
                needed_bytes: TRACE_HEADER_LEN.saturating_add(ns.saturating_mul(bps)),
                file_size: self.file_size(),
            })?;
        let samples_end_offset =
            samples_start_offset
                .checked_add(window_bytes)
                .ok_or(SegyError::TruncatedTrace {
                    trace_index,
                    offset: trace_offset,
                    needed_bytes: TRACE_HEADER_LEN.saturating_add(ns.saturating_mul(bps)),
                    file_size: self.file_size(),
                })?;

        let data = self.backing.as_slice();
        let raw_window = data.get(samples_start_offset..samples_end_offset).ok_or(
            SegyError::TruncatedTrace {
                trace_index,
                offset: trace_offset,
                needed_bytes: TRACE_HEADER_LEN.saturating_add(ns.saturating_mul(bps)),
                file_size: data.len(),
            },
        )?;

        decode_samples(
            raw_window,
            self.binary_header.sample_format,
            window_count,
            out,
        )
    }
}

fn compute_first_trace_offset(binary_header: &BinaryHeader, file_size: usize) -> Result<usize> {
    // Only interpret `extended_textual_headers` when revision >= 1.0 (0x0100).
    if binary_header.revision_raw < 0x0100 {
        return Ok(FILE_HEADER_LEN);
    }
    if binary_header.extended_textual_headers < 0 {
        return Err(SegyError::InvalidExtendedHeaderCount(
            binary_header.extended_textual_headers,
        ));
    }
    let ext_count = binary_header.extended_textual_headers as usize;
    let ext_bytes =
        ext_count
            .checked_mul(TEXTUAL_HEADER_LEN)
            .ok_or(SegyError::InvalidExtendedHeaderCount(
                binary_header.extended_textual_headers,
            ))?;
    let total =
        FILE_HEADER_LEN
            .checked_add(ext_bytes)
            .ok_or(SegyError::InvalidExtendedHeaderCount(
                binary_header.extended_textual_headers,
            ))?;
    if total > file_size {
        return Err(SegyError::TruncatedExtendedHeaders {
            expected_end: total,
            file_size,
        });
    }
    Ok(total)
}

fn build_trace_index(
    data: &[u8],
    first_trace_offset: usize,
    binary_header: &BinaryHeader,
) -> Result<TraceIndex> {
    let file_size = data.len();
    let rem = file_size.saturating_sub(first_trace_offset);
    let default_ns = binary_header.samples_per_trace;
    let bps = binary_header.sample_format.bytes_per_sample();
    let trace_stride = TRACE_HEADER_LEN
        .checked_add(usize::from(default_ns).saturating_mul(bps))
        .ok_or(SegyError::InvalidSamplesPerTrace(default_ns))?;

    if rem == 0 {
        return Ok(TraceIndex::FixedStride {
            first_trace_offset,
            trace_stride,
            samples_per_trace: default_ns,
            trace_count: 0,
        });
    }

    if rem < TRACE_HEADER_LEN {
        return Err(SegyError::TruncatedTrace {
            trace_index: 0,
            offset: first_trace_offset,
            needed_bytes: TRACE_HEADER_LEN,
            file_size,
        });
    }

    // If the rev 1 fixed-length flag is explicitly set, validate clean stride division.
    if binary_header.is_fixed_length_flag_set() {
        if !rem.is_multiple_of(trace_stride) {
            let trace_index = rem / trace_stride;
            let offset =
                first_trace_offset.saturating_add(trace_index.saturating_mul(trace_stride));
            return Err(SegyError::TruncatedTrace {
                trace_index,
                offset,
                needed_bytes: trace_stride,
                file_size,
            });
        }
        return Ok(TraceIndex::FixedStride {
            first_trace_offset,
            trace_stride,
            samples_per_trace: default_ns,
            trace_count: rem / trace_stride,
        });
    }

    // Fast path for rev 0 / fixed-length files where `rem` is an exact multiple of `trace_stride`
    // and spot-checked trace headers (first, second, last) do not override `ns` to a different value.
    if rem.is_multiple_of(trace_stride)
        && can_use_fixed_stride_fast_path(data, first_trace_offset, trace_stride, default_ns)
    {
        return Ok(TraceIndex::FixedStride {
            first_trace_offset,
            trace_stride,
            samples_per_trace: default_ns,
            trace_count: rem / trace_stride,
        });
    }

    // Variable-length fallback: scan only the 240-byte trace headers to build the offset table.
    let mut traces = Vec::new();
    let mut offset = first_trace_offset;
    let mut trace_index: usize = 0;

    while offset < file_size {
        if traces.len() >= MAX_VARIABLE_TRACES {
            return Err(SegyError::AllocationLimitExceeded {
                requested_samples: traces.len().saturating_add(1),
                max_samples: MAX_VARIABLE_TRACES,
            });
        }
        let header_end = offset
            .checked_add(TRACE_HEADER_LEN)
            .ok_or(SegyError::TruncatedTrace {
                trace_index,
                offset,
                needed_bytes: TRACE_HEADER_LEN,
                file_size,
            })?;
        let header_slice = data
            .get(offset..header_end)
            .ok_or(SegyError::TruncatedTrace {
                trace_index,
                offset,
                needed_bytes: TRACE_HEADER_LEN,
                file_size,
            })?;
        let view = TraceHeaderView::new(header_slice)?;
        let ns_override = view.ns_override();
        let ns = if ns_override > 0 {
            ns_override
        } else {
            default_ns
        };
        if ns == 0 {
            return Err(SegyError::InvalidSamplesPerTrace(0));
        }
        let trace_bytes = TRACE_HEADER_LEN
            .checked_add(usize::from(ns).saturating_mul(bps))
            .ok_or(SegyError::TruncatedTrace {
                trace_index,
                offset,
                needed_bytes: usize::MAX,
                file_size,
            })?;
        let next_offset = offset
            .checked_add(trace_bytes)
            .ok_or(SegyError::TruncatedTrace {
                trace_index,
                offset,
                needed_bytes: trace_bytes,
                file_size,
            })?;
        if next_offset > file_size {
            return Err(SegyError::TruncatedTrace {
                trace_index,
                offset,
                needed_bytes: trace_bytes,
                file_size,
            });
        }

        traces.push(TraceLocation {
            offset: offset as u64,
            samples: ns,
        });
        offset = next_offset;
        trace_index = trace_index.saturating_add(1);
    }

    // If all scanned traces ended up having `default_ns`, promote to FixedStride so the offset Vec can be dropped.
    if traces.iter().all(|t| t.samples == default_ns) {
        return Ok(TraceIndex::FixedStride {
            first_trace_offset,
            trace_stride,
            samples_per_trace: default_ns,
            trace_count: traces.len(),
        });
    }

    Ok(TraceIndex::VariableLength { traces })
}

fn can_use_fixed_stride_fast_path(
    data: &[u8],
    first_trace_offset: usize,
    trace_stride: usize,
    default_ns: u16,
) -> bool {
    let rem = data.len().saturating_sub(first_trace_offset);
    let count = rem / trace_stride;
    if count == 0 {
        return true;
    }
    let indices_to_check = [0, 1.min(count - 1), count / 2, count - 1];
    for idx in indices_to_check {
        let Some(offset) = idx
            .checked_mul(trace_stride)
            .and_then(|d| first_trace_offset.checked_add(d))
        else {
            return false;
        };
        let Some(header_slice) = data.get(offset..offset.saturating_add(TRACE_HEADER_LEN)) else {
            return false;
        };
        let Ok(view) = TraceHeaderView::new(header_slice) else {
            return false;
        };
        let ns_override = view.ns_override();
        if ns_override != 0 && ns_override != default_ns {
            return false;
        }
    }
    true
}
