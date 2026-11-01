use crate::ebcdic::TEXTUAL_HEADER_LEN;
use crate::error::{Result, SegyError};

/// Length in bytes of a SEG-Y binary file header.
pub const BINARY_HEADER_LEN: usize = 400;
/// Length in bytes of a SEG-Y trace header.
pub const TRACE_HEADER_LEN: usize = 240;
/// Combined length in bytes of the mandatory textual + binary file headers (`3200 + 400 = 3600`).
pub const FILE_HEADER_LEN: usize = TEXTUAL_HEADER_LEN + BINARY_HEADER_LEN;

/// SEG-Y data sample format codes (bytes 3225–3226 of the binary header).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    /// Code 1: 4-byte IBM hexadecimal floating-point.
    IbmFloat32,
    /// Code 2: 4-byte two's complement integer (`i32`).
    Int32,
    /// Code 3: 2-byte two's complement integer (`i16`).
    Int16,
    /// Code 5: 4-byte IEEE 754 floating-point (`f32`).
    IeeeFloat32,
    /// Code 8: 1-byte two's complement integer (`i8`).
    Int8,
}

impl SampleFormat {
    /// Parses a SEG-Y sample format code (`1, 2, 3, 5, 8`).
    #[inline]
    pub const fn from_code(code: u16) -> Result<Self> {
        match code {
            1 => Ok(Self::IbmFloat32),
            2 => Ok(Self::Int32),
            3 => Ok(Self::Int16),
            5 => Ok(Self::IeeeFloat32),
            8 => Ok(Self::Int8),
            other => Err(SegyError::UnsupportedSampleFormat(other)),
        }
    }

    /// Returns the numeric SEG-Y format code (`1, 2, 3, 5, 8`).
    #[inline]
    pub const fn code(self) -> u16 {
        match self {
            Self::IbmFloat32 => 1,
            Self::Int32 => 2,
            Self::Int16 => 3,
            Self::IeeeFloat32 => 5,
            Self::Int8 => 8,
        }
    }

    /// Returns the encoded byte width of one sample.
    #[inline]
    pub const fn bytes_per_sample(self) -> usize {
        match self {
            Self::IbmFloat32 | Self::Int32 | Self::IeeeFloat32 => 4,
            Self::Int16 => 2,
            Self::Int8 => 1,
        }
    }
}

/// Measurement system specified in bytes 3255–3256 of the binary header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeasurementSystem {
    /// 1 = Meters.
    Meters,
    /// 2 = Feet.
    Feet,
    /// Unassigned or non-standard value.
    Unknown(u16),
}

impl MeasurementSystem {
    /// Converts the raw 16-bit code from bytes 3255–3256.
    #[inline]
    pub const fn from_code(code: u16) -> Self {
        match code {
            1 => Self::Meters,
            2 => Self::Feet,
            other => Self::Unknown(other),
        }
    }
}

/// Parsed 400-byte SEG-Y binary file header (bytes 3201–3600, 1-based).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BinaryHeader {
    /// Job identification number (bytes 3201–3204).
    pub job_id: i32,
    /// Line number (bytes 3205–3208).
    pub line_number: i32,
    /// Reel number (bytes 3209–3212).
    pub reel_number: i32,
    /// Number of data traces per ensemble (bytes 3213–3214).
    pub traces_per_ensemble: u16,
    /// Number of auxiliary traces per ensemble (bytes 3215–3216).
    pub aux_traces_per_ensemble: u16,
    /// Sample interval in microseconds (`dt`, bytes 3217–3218).
    pub sample_interval_us: u16,
    /// Number of samples per data trace (`ns`, bytes 3221–3222).
    pub samples_per_trace: u16,
    /// Data sample format code (bytes 3225–3226).
    pub sample_format: SampleFormat,
    /// Measurement system (bytes 3255–3256).
    pub measurement_system: MeasurementSystem,
    /// Raw SEG-Y format revision word (bytes 3501–3502; e.g. `0x0100` for rev 1.0).
    pub revision_raw: u16,
    /// Fixed-length trace flag (bytes 3503–3504; `1` = all traces guaranteed constant length).
    pub fixed_length_flag: u16,
    /// Number of 3200-byte extended textual file header records following the binary header (bytes 3505–3506).
    pub extended_textual_headers: i16,
}

impl BinaryHeader {
    /// Parses a 400-byte binary file header (big-endian).
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < BINARY_HEADER_LEN {
            return Err(SegyError::FileTooSmall {
                actual_bytes: bytes.len(),
                min_bytes: BINARY_HEADER_LEN,
            });
        }
        let job_id = read_i32_be(bytes, 0)?;
        let line_number = read_i32_be(bytes, 4)?;
        let reel_number = read_i32_be(bytes, 8)?;
        let traces_per_ensemble = read_u16_be(bytes, 12)?;
        let aux_traces_per_ensemble = read_u16_be(bytes, 14)?;
        let sample_interval_us = read_u16_be(bytes, 16)?;
        let samples_per_trace = read_u16_be(bytes, 20)?;
        let format_code = read_u16_be(bytes, 24)?;
        let sample_format = SampleFormat::from_code(format_code)?;
        let measurement_code = read_u16_be(bytes, 54)?;
        let measurement_system = MeasurementSystem::from_code(measurement_code);
        let revision_raw = read_u16_be(bytes, 300)?;
        let fixed_length_flag = read_u16_be(bytes, 302)?;
        let extended_textual_headers = read_i16_be(bytes, 304)?;

        if samples_per_trace == 0 {
            return Err(SegyError::InvalidSamplesPerTrace(0));
        }

        Ok(Self {
            job_id,
            line_number,
            reel_number,
            traces_per_ensemble,
            aux_traces_per_ensemble,
            sample_interval_us,
            samples_per_trace,
            sample_format,
            measurement_system,
            revision_raw,
            fixed_length_flag,
            extended_textual_headers,
        })
    }

    /// Returns the `(major, minor)` SEG-Y revision numbers decoded from `revision_raw`.
    #[inline]
    pub const fn revision_major_minor(&self) -> (u8, u8) {
        (
            (self.revision_raw >> 8) as u8,
            (self.revision_raw & 0xFF) as u8,
        )
    }

    /// Returns `true` if the rev 1 fixed-length trace flag (`1`) is explicitly set.
    #[inline]
    pub const fn is_fixed_length_flag_set(&self) -> bool {
        self.fixed_length_flag == 1
    }
}

/// Applies a SEG-Y trace header scalar (`>0` multiplies, `<0` divides by `|scalar|`, `0` leaves unchanged).
#[inline]
pub fn apply_segy_scalar(value: i32, scalar: i16) -> f64 {
    if scalar > 0 {
        f64::from(value) * f64::from(scalar)
    } else if scalar < 0 {
        f64::from(value) / f64::from(-i32::from(scalar))
    } else {
        f64::from(value)
    }
}

/// Header key selectable as the horizontal trace axis during rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisHeaderKey {
    /// 1-based trace index within the file (`1..=trace_count`).
    TraceNumber,
    /// Trace sequence number within line (bytes 1–4, 0-based `0..4`).
    SequenceLine,
    /// Trace sequence number within file (bytes 5–8, 0-based `4..8`).
    SequenceFile,
    /// Original field record number (bytes 9–12, 0-based `8..12`).
    FieldRecord,
    /// Ensemble / CDP number (bytes 21–24, 0-based `20..24`).
    Cdp,
    /// Source-receiver offset distance (bytes 37–40, 0-based `36..40`).
    Offset,
    /// 3-D inline number (bytes 189–192, 0-based `188..192`).
    Inline,
    /// 3-D crossline number (bytes 193–196, 0-based `192..196`).
    Crossline,
    /// Arbitrary big-endian `i16` at 0-based byte offset `byte_offset` (`0..=238`).
    CustomI16 {
        /// 0-based byte offset within the 240-byte trace header.
        byte_offset: usize,
    },
    /// Arbitrary big-endian `i32` at 0-based byte offset `byte_offset` (`0..=236`).
    CustomI32 {
        /// 0-based byte offset within the 240-byte trace header.
        byte_offset: usize,
    },
}

/// Zero-copy view over a single 240-byte big-endian SEG-Y trace header.
/// Allows reading only the fields needed for drawing without decoding the entire header.
#[derive(Debug, Clone, Copy)]
pub struct TraceHeaderView<'a> {
    bytes: &'a [u8],
}

impl<'a> TraceHeaderView<'a> {
    /// Wraps a 240-byte trace header slice.
    pub fn new(bytes: &'a [u8]) -> Result<Self> {
        if bytes.len() < TRACE_HEADER_LEN {
            return Err(SegyError::FileTooSmall {
                actual_bytes: bytes.len(),
                min_bytes: TRACE_HEADER_LEN,
            });
        }
        Ok(Self {
            bytes: &bytes[..TRACE_HEADER_LEN],
        })
    }

    /// Returns the underlying 240-byte slice.
    #[inline]
    pub const fn as_bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// Reads a big-endian `i16` at the given 0-based byte offset (`0..=238`).
    #[inline]
    pub fn read_i16(&self, offset: usize) -> Result<i16> {
        read_i16_be(self.bytes, offset)
    }

    /// Reads a big-endian `u16` at the given 0-based byte offset (`0..=238`).
    #[inline]
    pub fn read_u16(&self, offset: usize) -> Result<u16> {
        read_u16_be(self.bytes, offset)
    }

    /// Reads a big-endian `i32` at the given 0-based byte offset (`0..=236`).
    #[inline]
    pub fn read_i32(&self, offset: usize) -> Result<i32> {
        read_i32_be(self.bytes, offset)
    }

    /// Number of samples in this trace (`ns` override at bytes 115–116, 0-based `114..116`).
    #[inline]
    pub fn ns_override(&self) -> u16 {
        u16::from_be_bytes([self.bytes[114], self.bytes[115]])
    }

    /// Sample interval in microseconds for this trace (`dt` override at bytes 117–118, 0-based `116..118`).
    #[inline]
    pub fn dt_override_us(&self) -> u16 {
        u16::from_be_bytes([self.bytes[116], self.bytes[117]])
    }

    /// Reads the value of the chosen horizontal axis key for a given 0-based `trace_index`.
    #[inline]
    pub fn axis_value(&self, trace_index: usize, key: AxisHeaderKey) -> Result<i64> {
        match key {
            AxisHeaderKey::TraceNumber => Ok((trace_index as i64).saturating_add(1)),
            AxisHeaderKey::SequenceLine => Ok(i64::from(self.read_i32(0)?)),
            AxisHeaderKey::SequenceFile => Ok(i64::from(self.read_i32(4)?)),
            AxisHeaderKey::FieldRecord => Ok(i64::from(self.read_i32(8)?)),
            AxisHeaderKey::Cdp => Ok(i64::from(self.read_i32(20)?)),
            AxisHeaderKey::Offset => Ok(i64::from(self.read_i32(36)?)),
            AxisHeaderKey::Inline => Ok(i64::from(self.read_i32(188)?)),
            AxisHeaderKey::Crossline => Ok(i64::from(self.read_i32(192)?)),
            AxisHeaderKey::CustomI16 { byte_offset } => Ok(i64::from(self.read_i16(byte_offset)?)),
            AxisHeaderKey::CustomI32 { byte_offset } => Ok(i64::from(self.read_i32(byte_offset)?)),
        }
    }

    /// Parses all drawing-relevant trace header fields into a [`TraceHeader`].
    pub fn to_trace_header(&self) -> Result<TraceHeader> {
        TraceHeader::parse(self.bytes)
    }
}

/// Parsed drawing-essential fields from a 240-byte SEG-Y trace header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraceHeader {
    /// Trace sequence number within line (bytes 1–4).
    pub trace_sequence_line: i32,
    /// Trace sequence number within SEG-Y file (bytes 5–8).
    pub trace_sequence_file: i32,
    /// Original field record number (bytes 9–12).
    pub field_record: i32,
    /// Trace number within the original field record (bytes 13–16).
    pub trace_number_field: i32,
    /// Energy source point number (bytes 17–20).
    pub energy_source_point: i32,
    /// Ensemble number (CDP/CMP, bytes 21–24).
    pub ensemble_number: i32,
    /// Trace number within the ensemble (bytes 25–28).
    pub trace_in_ensemble: i32,
    /// Trace identification code (bytes 29–30; `1` = seismic data).
    pub trace_id_code: i16,
    /// Distance from source point to receiver group (`offset`, bytes 37–40).
    pub offset: i32,
    /// Scalar applied to elevations and depths (bytes 69–70).
    pub elevation_scalar: i16,
    /// Scalar applied to coordinates (bytes 71–72).
    pub coordinate_scalar: i16,
    /// Delay recording time in milliseconds (bytes 109–110).
    pub delay_recording_time_ms: i16,
    /// Number of samples in this trace (`ns` override, bytes 115–116).
    pub ns_override: u16,
    /// Sample interval in microseconds (`dt` override, bytes 117–118).
    pub dt_override_us: u16,
    /// Ensemble X coordinate (bytes 181–184).
    pub cdp_x: i32,
    /// Ensemble Y coordinate (bytes 185–188).
    pub cdp_y: i32,
    /// 3-D poststack inline number (bytes 189–192).
    pub inline: i32,
    /// 3-D poststack crossline number (bytes 193–196).
    pub xline: i32,
}

impl TraceHeader {
    /// Parses a 240-byte SEG-Y trace header (big-endian).
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let view = TraceHeaderView::new(bytes)?;
        Ok(Self {
            trace_sequence_line: view.read_i32(0)?,
            trace_sequence_file: view.read_i32(4)?,
            field_record: view.read_i32(8)?,
            trace_number_field: view.read_i32(12)?,
            energy_source_point: view.read_i32(16)?,
            ensemble_number: view.read_i32(20)?,
            trace_in_ensemble: view.read_i32(24)?,
            trace_id_code: view.read_i16(28)?,
            offset: view.read_i32(36)?,
            elevation_scalar: view.read_i16(68)?,
            coordinate_scalar: view.read_i16(70)?,
            delay_recording_time_ms: view.read_i16(108)?,
            ns_override: view.read_u16(114)?,
            dt_override_us: view.read_u16(116)?,
            cdp_x: view.read_i32(180)?,
            cdp_y: view.read_i32(184)?,
            inline: view.read_i32(188)?,
            xline: view.read_i32(192)?,
        })
    }

    /// Returns `ns_override` when non-zero, otherwise falls back to the binary header's `default_ns`.
    #[inline]
    pub const fn effective_ns(&self, default_ns: u16) -> u16 {
        if self.ns_override > 0 {
            self.ns_override
        } else {
            default_ns
        }
    }

    /// Returns `dt_override_us` when non-zero, otherwise falls back to the binary header's `default_dt_us`.
    #[inline]
    pub const fn effective_dt_us(&self, default_dt_us: u16) -> u16 {
        if self.dt_override_us > 0 {
            self.dt_override_us
        } else {
            default_dt_us
        }
    }

    /// Returns `cdp_x` scaled by `coordinate_scalar`.
    #[inline]
    pub fn scaled_cdp_x(&self) -> f64 {
        apply_segy_scalar(self.cdp_x, self.coordinate_scalar)
    }

    /// Returns `cdp_y` scaled by `coordinate_scalar`.
    #[inline]
    pub fn scaled_cdp_y(&self) -> f64 {
        apply_segy_scalar(self.cdp_y, self.coordinate_scalar)
    }
}

#[inline]
pub(crate) fn read_u16_be(bytes: &[u8], offset: usize) -> Result<u16> {
    let end = offset
        .checked_add(2)
        .ok_or(SegyError::HeaderOffsetOutOfBounds { offset, width: 2 })?;
    let slice = bytes
        .get(offset..end)
        .ok_or(SegyError::HeaderOffsetOutOfBounds { offset, width: 2 })?;
    Ok(u16::from_be_bytes([slice[0], slice[1]]))
}

#[inline]
pub(crate) fn read_i16_be(bytes: &[u8], offset: usize) -> Result<i16> {
    Ok(read_u16_be(bytes, offset)? as i16)
}

#[inline]
pub(crate) fn read_i32_be(bytes: &[u8], offset: usize) -> Result<i32> {
    let end = offset
        .checked_add(4)
        .ok_or(SegyError::HeaderOffsetOutOfBounds { offset, width: 4 })?;
    let slice = bytes
        .get(offset..end)
        .ok_or(SegyError::HeaderOffsetOutOfBounds { offset, width: 4 })?;
    Ok(i32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}
