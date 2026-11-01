#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

//! Read-only SEG-Y (rev 0/1) parser, lazy trace indexer, and sample decoder.

pub mod ebcdic;
pub mod error;
pub mod file;
pub mod header;
pub mod sample;

pub use ebcdic::{
    TEXTUAL_HEADER_COLS, TEXTUAL_HEADER_LEN, TEXTUAL_HEADER_LINES, TextEncoding, TextualHeader,
    ascii_to_ebcdic_byte, detect_text_encoding, ebcdic_to_ascii, ebcdic_to_ascii_byte,
};
pub use error::{Result, SegyError};
pub use file::{SegyFile, TraceIndex, TraceLocation};
pub use header::{
    AxisHeaderKey, BINARY_HEADER_LEN, BinaryHeader, FILE_HEADER_LEN, MeasurementSystem,
    SampleFormat, TRACE_HEADER_LEN, TraceHeader, TraceHeaderView, apply_segy_scalar,
};
pub use sample::{MAX_SAMPLES_PER_DECODE, decode_samples, ibm_to_ieee_f32, ieee_to_ibm_bits};

/// Placeholder readiness check used by downstream workspace crates.
pub fn crate_ready() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Helper to build a synthetic SEG-Y file in memory.
    struct SyntheticSegyBuilder {
        bytes: Vec<u8>,
    }

    impl SyntheticSegyBuilder {
        fn new(ns: u16, dt_us: u16, format_code: u16, rev: u16, fixed_flag: u16) -> Self {
            let mut bytes = vec![0u8; FILE_HEADER_LEN];
            // Fill textual header with EBCDIC spaces (0x40) and card labels "C01 ...".
            for line_idx in 0..TEXTUAL_HEADER_LINES {
                let start = line_idx * TEXTUAL_HEADER_COLS;
                let text = format!("C{:02} SYNTHETIC SEGY TEST HEADER LINE", line_idx + 1);
                for col in 0..TEXTUAL_HEADER_COLS {
                    let ascii_ch = text.as_bytes().get(col).copied().unwrap_or(b' ');
                    bytes[start + col] = ascii_to_ebcdic_byte(ascii_ch);
                }
            }
            // Populate binary header fields (big-endian at offset 3200).
            let bin = &mut bytes[TEXTUAL_HEADER_LEN..FILE_HEADER_LEN];
            bin[0..4].copy_from_slice(&1001_i32.to_be_bytes()); // job_id
            bin[4..8].copy_from_slice(&42_i32.to_be_bytes()); // line_number
            bin[8..12].copy_from_slice(&7_i32.to_be_bytes()); // reel_number
            bin[12..14].copy_from_slice(&24_u16.to_be_bytes()); // traces_per_ensemble
            bin[14..16].copy_from_slice(&2_u16.to_be_bytes()); // aux_traces_per_ensemble
            bin[16..18].copy_from_slice(&dt_us.to_be_bytes()); // sample_interval_us
            bin[20..22].copy_from_slice(&ns.to_be_bytes()); // samples_per_trace
            bin[24..26].copy_from_slice(&format_code.to_be_bytes()); // sample_format
            bin[54..56].copy_from_slice(&1_u16.to_be_bytes()); // meters
            bin[300..302].copy_from_slice(&rev.to_be_bytes()); // revision
            bin[302..304].copy_from_slice(&fixed_flag.to_be_bytes()); // fixed_length_flag
            Self { bytes }
        }

        #[allow(clippy::too_many_arguments)]
        fn push_trace_raw(
            &mut self,
            seq_line: i32,
            seq_file: i32,
            cdp: i32,
            offset: i32,
            inline: i32,
            xline: i32,
            coord_scalar: i16,
            ns_override: u16,
            sample_bytes: &[u8],
        ) {
            let mut th = [0u8; TRACE_HEADER_LEN];
            th[0..4].copy_from_slice(&seq_line.to_be_bytes());
            th[4..8].copy_from_slice(&seq_file.to_be_bytes());
            th[8..12].copy_from_slice(&(seq_file + 100).to_be_bytes());
            th[20..24].copy_from_slice(&cdp.to_be_bytes());
            th[28..30].copy_from_slice(&1_i16.to_be_bytes());
            th[36..40].copy_from_slice(&offset.to_be_bytes());
            th[68..70].copy_from_slice(&(-10_i16).to_be_bytes());
            th[70..72].copy_from_slice(&coord_scalar.to_be_bytes());
            th[114..116].copy_from_slice(&ns_override.to_be_bytes());
            th[116..118].copy_from_slice(&4000_u16.to_be_bytes());
            th[180..184].copy_from_slice(&500_000_i32.to_be_bytes());
            th[184..188].copy_from_slice(&9_000_000_i32.to_be_bytes());
            th[188..192].copy_from_slice(&inline.to_be_bytes());
            th[192..196].copy_from_slice(&xline.to_be_bytes());
            self.bytes.extend_from_slice(&th);
            self.bytes.extend_from_slice(sample_bytes);
        }

        fn build(self) -> Vec<u8> {
            self.bytes
        }
    }

    #[test]
    fn ebcdic_textual_header_decodes_cleanly() -> Result<()> {
        let raw = SyntheticSegyBuilder::new(4, 2000, 5, 0x0100, 1).build();
        let file = SegyFile::from_bytes(raw)?;
        assert_eq!(file.textual_header().encoding(), TextEncoding::Ebcdic);
        assert_eq!(file.textual_header().lines().len(), 40);
        let first_line = file.textual_header().lines()[0].trim_end();
        assert_eq!(first_line, "C01 SYNTHETIC SEGY TEST HEADER LINE");
        let last_line = file.textual_header().lines()[39].trim_end();
        assert_eq!(last_line, "C40 SYNTHETIC SEGY TEST HEADER LINE");
        Ok(())
    }

    #[test]
    fn binary_header_parses_expected_fields() -> Result<()> {
        let mut builder = SyntheticSegyBuilder::new(4, 4000, 5, 0x0100, 1);
        builder.push_trace_raw(10, 1, 501, 1250, 110, 220, -100, 4, &[0u8; 16]);
        let file = SegyFile::from_bytes(builder.build())?;

        let bh = file.binary_header();
        assert_eq!(bh.job_id, 1001);
        assert_eq!(bh.line_number, 42);
        assert_eq!(bh.reel_number, 7);
        assert_eq!(bh.sample_interval_us, 4000);
        assert_eq!(bh.samples_per_trace, 4);
        assert_eq!(bh.sample_format, SampleFormat::IeeeFloat32);
        assert_eq!(bh.measurement_system, MeasurementSystem::Meters);
        assert_eq!(bh.revision_major_minor(), (1, 0));
        assert!(bh.is_fixed_length_flag_set());
        assert!(file.is_fixed_stride());
        assert_eq!(file.trace_count(), 1);
        Ok(())
    }

    #[test]
    fn trace_header_and_axis_keys_parse_expected_fields() -> Result<()> {
        let mut builder = SyntheticSegyBuilder::new(4, 4000, 5, 0x0100, 1);
        builder.push_trace_raw(10, 1, 501, 1250, 110, 220, -100, 4, &[0u8; 16]);
        let file = SegyFile::from_bytes(builder.build())?;

        let th = file.trace_header(0)?;
        assert_eq!(th.trace_sequence_line, 10);
        assert_eq!(th.trace_sequence_file, 1);
        assert_eq!(th.field_record, 101);
        assert_eq!(th.ensemble_number, 501);
        assert_eq!(th.offset, 1250);
        assert_eq!(th.inline, 110);
        assert_eq!(th.xline, 220);
        assert_eq!(th.coordinate_scalar, -100);
        assert_eq!(th.elevation_scalar, -10);
        assert_eq!(th.ns_override, 4);
        assert_eq!(th.effective_ns(4), 4);
        assert!((th.scaled_cdp_x() - 5000.0).abs() < 1e-6);
        assert!((th.scaled_cdp_y() - 90000.0).abs() < 1e-6);

        assert_eq!(file.trace_axis_value(0, AxisHeaderKey::TraceNumber)?, 1);
        assert_eq!(file.trace_axis_value(0, AxisHeaderKey::SequenceLine)?, 10);
        assert_eq!(file.trace_axis_value(0, AxisHeaderKey::Cdp)?, 501);
        assert_eq!(file.trace_axis_value(0, AxisHeaderKey::Offset)?, 1250);
        assert_eq!(file.trace_axis_value(0, AxisHeaderKey::Inline)?, 110);
        assert_eq!(file.trace_axis_value(0, AxisHeaderKey::Crossline)?, 220);
        assert_eq!(
            file.trace_axis_value(0, AxisHeaderKey::CustomI32 { byte_offset: 188 })?,
            110
        );
        assert_eq!(
            file.trace_axis_value(0, AxisHeaderKey::CustomI16 { byte_offset: 70 })?,
            -100
        );
        Ok(())
    }

    #[test]
    fn sample_formats_1_2_3_5_8_decode_accurately() -> Result<()> {
        // Format 1: IBM 32-bit float
        {
            let expected = [0.0_f32, 1.0, -1.0, 0.5, -118.625, 1234.5];
            let mut sb = Vec::new();
            for &val in &expected {
                sb.extend_from_slice(&ieee_to_ibm_bits(val).to_be_bytes());
            }
            let mut builder = SyntheticSegyBuilder::new(expected.len() as u16, 2000, 1, 0, 0);
            builder.push_trace_raw(1, 1, 1, 0, 1, 1, 1, expected.len() as u16, &sb);
            let file = SegyFile::from_bytes(builder.build())?;
            let decoded = file.read_trace_samples(0)?;
            assert_eq!(decoded.len(), expected.len());
            for (got, want) in decoded.iter().zip(expected.iter()) {
                assert!((got - want).abs() < 1e-5, "got {got}, want {want}");
            }
        }

        // Format 2: i32
        {
            let expected: [i32; 4] = [-100_000, 0, 42, 250_000];
            let mut sb = Vec::new();
            for &val in &expected {
                sb.extend_from_slice(&val.to_be_bytes());
            }
            let mut builder = SyntheticSegyBuilder::new(4, 2000, 2, 0x0100, 1);
            builder.push_trace_raw(1, 1, 1, 0, 1, 1, 1, 4, &sb);
            let file = SegyFile::from_bytes(builder.build())?;
            let decoded = file.read_trace_samples(0)?;
            assert_eq!(decoded, vec![-100_000.0, 0.0, 42.0, 250_000.0]);
        }

        // Format 3: i16
        {
            let expected: [i16; 4] = [-32768, -1, 0, 32767];
            let mut sb = Vec::new();
            for &val in &expected {
                sb.extend_from_slice(&val.to_be_bytes());
            }
            let mut builder = SyntheticSegyBuilder::new(4, 2000, 3, 0x0100, 1);
            builder.push_trace_raw(1, 1, 1, 0, 1, 1, 1, 4, &sb);
            let file = SegyFile::from_bytes(builder.build())?;
            let decoded = file.read_trace_samples(0)?;
            assert_eq!(decoded, vec![-32768.0, -1.0, 0.0, 32767.0]);
        }

        // Format 5: IEEE f32 (including NaN sanitization)
        {
            let inputs: [f32; 4] = [-3.5, 0.25, f32::NAN, 100.0];
            let mut sb = Vec::new();
            for &val in &inputs {
                sb.extend_from_slice(&val.to_be_bytes());
            }
            let mut builder = SyntheticSegyBuilder::new(4, 2000, 5, 0x0100, 1);
            builder.push_trace_raw(1, 1, 1, 0, 1, 1, 1, 4, &sb);
            let file = SegyFile::from_bytes(builder.build())?;
            let decoded = file.read_trace_samples(0)?;
            assert_eq!(decoded, vec![-3.5, 0.25, 0.0, 100.0]);

            // Windowed read check
            let mut window = Vec::new();
            file.read_trace_sample_window_into(0, 1..3, &mut window)?;
            assert_eq!(window, vec![0.25, 0.0]);
        }

        // Format 8: i8
        {
            let inputs: [i8; 4] = [-128, -1, 0, 127];
            let sb: Vec<u8> = inputs.iter().map(|&v| v as u8).collect();
            let mut builder = SyntheticSegyBuilder::new(4, 2000, 8, 0x0100, 1);
            builder.push_trace_raw(1, 1, 1, 0, 1, 1, 1, 4, &sb);
            let file = SegyFile::from_bytes(builder.build())?;
            let decoded = file.read_trace_samples(0)?;
            assert_eq!(decoded, vec![-128.0, -1.0, 0.0, 127.0]);
        }

        Ok(())
    }

    #[test]
    fn variable_length_traces_build_offset_table() -> Result<()> {
        let mut builder = SyntheticSegyBuilder::new(4, 2000, 3, 0x0000, 0);
        // Trace 0 has 2 i16 samples; Trace 1 has 5 i16 samples.
        builder.push_trace_raw(1, 1, 10, 100, 1, 1, 1, 2, &[0, 10, 0, 20]);
        builder.push_trace_raw(2, 2, 11, 200, 1, 2, 1, 5, &[0, 1, 0, 2, 0, 3, 0, 4, 0, 5]);

        let file = SegyFile::from_bytes(builder.build())?;
        assert!(!file.is_fixed_stride());
        assert_eq!(file.trace_count(), 2);
        assert_eq!(file.trace_sample_count(0)?, 2);
        assert_eq!(file.trace_sample_count(1)?, 5);
        assert_eq!(file.read_trace_samples(0)?, vec![10.0, 20.0]);
        assert_eq!(file.read_trace_samples(1)?, vec![1.0, 2.0, 3.0, 4.0, 5.0]);
        Ok(())
    }

    #[test]
    fn hostile_inputs_return_errors_without_panics() {
        // 1. Empty and truncated file headers (< 3600 bytes)
        assert!(matches!(
            SegyFile::from_bytes(Vec::new()),
            Err(SegyError::FileTooSmall {
                actual_bytes: 0,
                min_bytes: 3600
            })
        ));
        assert!(matches!(
            SegyFile::from_bytes(vec![0u8; 3599]),
            Err(SegyError::FileTooSmall {
                actual_bytes: 3599,
                min_bytes: 3600
            })
        ));

        // 2. ns = 0 in binary header
        let ns_zero = SyntheticSegyBuilder::new(0, 2000, 1, 0, 0).build();
        assert!(matches!(
            SegyFile::from_bytes(ns_zero),
            Err(SegyError::InvalidSamplesPerTrace(0))
        ));

        // 3. Bogus sample format codes
        for bogus_fmt in [0_u16, 4, 6, 7, 9, 99, u16::MAX] {
            let raw = SyntheticSegyBuilder::new(10, 2000, bogus_fmt, 0, 0).build();
            assert!(matches!(
                SegyFile::from_bytes(raw),
                Err(SegyError::UnsupportedSampleFormat(code)) if code == bogus_fmt
            ));
        }

        // 4. Truncated trace header (3600 + 100 bytes)
        let mut trunc_th = SyntheticSegyBuilder::new(10, 2000, 1, 0, 1).build();
        trunc_th.extend_from_slice(&[0u8; 100]);
        assert!(matches!(
            SegyFile::from_bytes(trunc_th),
            Err(SegyError::TruncatedTrace { trace_index: 0, .. })
        ));

        // 5. Truncated trace samples (header says 10 x 4-byte samples, only 8 bytes present)
        let mut builder = SyntheticSegyBuilder::new(10, 2000, 1, 0, 0);
        builder.push_trace_raw(1, 1, 1, 0, 1, 1, 1, 10, &[0u8; 8]);
        assert!(matches!(
            SegyFile::from_bytes(builder.build()),
            Err(SegyError::TruncatedTrace { trace_index: 0, .. })
        ));

        // 6. Out-of-bounds trace index and sample range on valid file
        let valid_empty = SyntheticSegyBuilder::new(4, 2000, 5, 0x0100, 1).build();
        let file = SegyFile::from_bytes(valid_empty);
        assert!(file.is_ok());
        if let Ok(f) = file {
            assert_eq!(f.trace_count(), 0);
            assert!(matches!(
                f.trace_header(0),
                Err(SegyError::TraceOutOfBounds {
                    trace_index: 0,
                    trace_count: 0
                })
            ));
        }
    }

    #[test]
    fn sample_file_3x_75_pr_matches_expected_trace_count_and_headers() -> Result<()> {
        let sample_path = Path::new("/home/toto/Downloads/3X_75_PR.SGY");
        if !sample_path.exists() {
            return Ok(());
        }

        let file = SegyFile::open(sample_path)?;
        let size = file.file_size();
        assert_eq!(size, 5_168_048);

        let bh = file.binary_header();
        assert_eq!(bh.samples_per_trace, 751);
        assert_eq!(bh.sample_interval_us, 8000);
        assert_eq!(bh.sample_format, SampleFormat::IbmFloat32);

        let expected_trace_count = (size - 3600) / (240 + 751 * 4);
        assert_eq!(expected_trace_count, 1592);
        assert_eq!(file.trace_count(), expected_trace_count);
        assert!(file.is_fixed_stride());

        // Check textual header EBCDIC decoding
        let text = file.textual_header().to_trimmed_text();
        assert!(text.contains("CLIENT/JOB ID"));
        assert!(text.contains("LINE  L3X"));

        // Check first and last trace headers and decoded samples
        let first_th = file.trace_header(0)?;
        assert_eq!(first_th.effective_ns(bh.samples_per_trace), 751);
        assert_eq!(first_th.effective_dt_us(bh.sample_interval_us), 8000);
        assert_eq!(first_th.trace_sequence_file, 1);

        let first_samples = file.read_trace_samples(0)?;
        assert_eq!(first_samples.len(), 751);
        assert!(first_samples.iter().all(|v| v.is_finite()));
        assert!(first_samples.iter().any(|&v| v.abs() > 1.0));

        let last_samples = file.read_trace_samples(expected_trace_count - 1)?;
        assert_eq!(last_samples.len(), 751);
        assert!(last_samples.iter().all(|v| v.is_finite()));

        Ok(())
    }
}
