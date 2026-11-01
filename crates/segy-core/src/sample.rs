use crate::error::{Result, SegyError};
use crate::header::SampleFormat;

/// Maximum number of samples allowed in a single trace decode call (16M samples = 64 MiB of `f32`).
pub const MAX_SAMPLES_PER_DECODE: usize = 16 * 1024 * 1024;

/// Converts a big-endian 32-bit IBM System/360 hexadecimal floating-point word into an IEEE 754 `f32`.
///
/// Format:
/// - Bit 31: sign (`0` = `+`, `1` = `-`)
/// - Bits 30..24: base-16 exponent biased by 64 (`exp16`)
/// - Bits 23..0: 24-bit unsigned fraction (`frac / 2^24`)
///
/// Value = `(-1)^sign * (frac / 2^24) * 16^(exp16 - 64)`.
#[inline]
pub fn ibm_to_ieee_f32(bits: u32) -> f32 {
    let frac = bits & 0x00ff_ffff;
    if frac == 0 {
        return 0.0;
    }
    let sign = bits & 0x8000_0000;
    let exp16 = ((bits >> 24) & 0x7f) as i32;
    // `frac` is non-zero in bits 23..0, so `leading_zeros()` is in `8..=31`.
    let lz = frac.leading_zeros() as i32;
    let shift = lz - 8;
    // Shift `frac` so the highest set bit lands at bit 23 (the implicit IEEE 754 leading 1).
    let norm = frac << shift;
    // Unbiased base-2 exponent: 4 * (exp16 - 64) - 24 + (23 - shift) = 4 * exp16 - 249 - lz.
    // Add IEEE 754 bias (127): 4 * exp16 - 122 - lz.
    let ieee_exp = 4 * exp16 - 122 - lz;

    if ieee_exp >= 255 {
        return if sign != 0 { -f32::MAX } else { f32::MAX };
    }
    if ieee_exp <= 0 {
        if ieee_exp < -23 {
            return 0.0;
        }
        let sub_shift = (1 - ieee_exp) as u32;
        let sub_mant = (norm >> sub_shift) & 0x007f_ffff;
        return f32::from_bits(sign | sub_mant);
    }

    let ieee_bits = sign | ((ieee_exp as u32) << 23) | (norm & 0x007f_ffff);
    f32::from_bits(ieee_bits)
}

/// Encodes an IEEE 754 `f32` into a 32-bit IBM hexadecimal floating-point word (for synthetic tests).
pub fn ieee_to_ibm_bits(value: f32) -> u32 {
    if !value.is_finite() || value == 0.0 {
        return 0;
    }
    let bits = value.to_bits();
    let sign = bits & 0x8000_0000;
    let ieee_exp = ((bits >> 23) & 0xff) as i32;
    let mant24 = if ieee_exp == 0 {
        bits & 0x007f_ffff
    } else {
        (bits & 0x007f_ffff) | 0x0080_0000
    };
    if mant24 == 0 {
        return 0;
    }
    // value = (-1)^s * (mant24 / 2^23) * 2^(exp2)
    //       = (-1)^s * (mant24 / 2^24) * 2^(exp2 + 1)
    let exp2_plus_1 = if ieee_exp == 0 {
        -126 + 1
    } else {
        (ieee_exp - 127) + 1
    };

    // Find smallest `4 * (exp16 - 64) >= exp2_plus_1` so right-shift is in `0..=3`.
    let rem = exp2_plus_1.rem_euclid(4);
    let align_shift = if rem == 0 { 0 } else { 4 - rem };
    let exp16 = (exp2_plus_1 + align_shift) / 4 + 64;
    if exp16 <= 0 {
        return 0;
    }
    if exp16 >= 128 {
        return sign | 0x7fff_ffff;
    }
    let ibm_frac = (mant24 >> align_shift) & 0x00ff_ffff;
    if ibm_frac == 0 {
        return 0;
    }
    sign | ((exp16 as u32) << 24) | ibm_frac
}

/// Decodes `sample_count` big-endian samples of `format` from `raw` into `out` (replacing its contents).
pub fn decode_samples(
    raw: &[u8],
    format: SampleFormat,
    sample_count: usize,
    out: &mut Vec<f32>,
) -> Result<()> {
    if sample_count > MAX_SAMPLES_PER_DECODE {
        return Err(SegyError::AllocationLimitExceeded {
            requested_samples: sample_count,
            max_samples: MAX_SAMPLES_PER_DECODE,
        });
    }
    let bps = format.bytes_per_sample();
    let needed = sample_count
        .checked_mul(bps)
        .ok_or(SegyError::AllocationLimitExceeded {
            requested_samples: sample_count,
            max_samples: MAX_SAMPLES_PER_DECODE,
        })?;
    let payload = raw.get(..needed).ok_or(SegyError::FileTooSmall {
        actual_bytes: raw.len(),
        min_bytes: needed,
    })?;

    out.clear();
    out.reserve(sample_count);

    match format {
        SampleFormat::IbmFloat32 => {
            for &chunk in payload.as_chunks::<4>().0 {
                let bits = u32::from_be_bytes(chunk);
                out.push(ibm_to_ieee_f32(bits));
            }
        }
        SampleFormat::Int32 => {
            for &chunk in payload.as_chunks::<4>().0 {
                let val = i32::from_be_bytes(chunk);
                out.push(val as f32);
            }
        }
        SampleFormat::Int16 => {
            for &chunk in payload.as_chunks::<2>().0 {
                let val = i16::from_be_bytes(chunk);
                out.push(f32::from(val));
            }
        }
        SampleFormat::IeeeFloat32 => {
            for &chunk in payload.as_chunks::<4>().0 {
                let val = f32::from_be_bytes(chunk);
                let clean = if val.is_finite() {
                    val
                } else if val.is_nan() {
                    0.0
                } else if val.is_sign_negative() {
                    -f32::MAX
                } else {
                    f32::MAX
                };
                out.push(clean);
            }
        }
        SampleFormat::Int8 => {
            for &b in payload {
                out.push(f32::from(b as i8));
            }
        }
    }

    Ok(())
}
