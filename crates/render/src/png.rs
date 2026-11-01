//! Pure-Rust, panic-free RGBA8 PNG encoder and decoder for offscreen seismic snapshots.

use std::path::Path;

use crate::error::{RenderError, Result};
use crate::image::{MAX_VIEWPORT_DIMENSION, RenderedImage};

/// Standard 8-byte PNG file signature (`\x89PNG\r\n\x1a\n`).
pub const PNG_SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

/// Maximum payload size of a single uncompressed DEFLATE block (`BTYPE = 00`).
const MAX_STORED_BLOCK_LEN: usize = 65_535;

impl RenderedImage {
    /// Encodes this [`RenderedImage`] into a standard-compliant RGBA8 PNG byte vector.
    pub fn to_png_bytes(&self) -> Result<Vec<u8>> {
        encode_rgba_png(self.width, self.height, &self.rgba)
    }

    /// Saves this [`RenderedImage`] to `path` as an RGBA8 PNG file, creating parent directories if needed.
    pub fn save_png(&self, path: impl AsRef<Path>) -> Result<()> {
        save_rgba_png(path, self.width, self.height, &self.rgba)
    }
}

/// Encodes a row-major `width × height` RGBA8 pixel buffer into a standard PNG byte vector.
pub fn encode_rgba_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    validate_png_buffer(width, height, rgba)?;

    let w = width as usize;
    let h = height as usize;
    let row_bytes = w * 4;
    let raw_scanline_bytes = h * (1 + row_bytes);

    let mut filtered = Vec::with_capacity(raw_scanline_bytes);
    for y in 0..h {
        filtered.push(0u8); // Filter type 0 (None)
        let start = y * row_bytes;
        filtered.extend_from_slice(&rgba[start..start + row_bytes]);
    }

    let idat_payload = zlib_encode_stored(&filtered);

    let mut out = Vec::with_capacity(64 + idat_payload.len());
    out.extend_from_slice(&PNG_SIGNATURE);

    let mut ihdr = [0u8; 13];
    ihdr[0..4].copy_from_slice(&width.to_be_bytes());
    ihdr[4..8].copy_from_slice(&height.to_be_bytes());
    ihdr[8] = 8; // 8 bits per channel
    ihdr[9] = 6; // Color type 6: Truecolor with alpha (RGBA)
    ihdr[10] = 0; // Deflate compression
    ihdr[11] = 0; // Adaptive filtering
    ihdr[12] = 0; // No interlace
    write_png_chunk(&mut out, *b"IHDR", &ihdr);
    write_png_chunk(&mut out, *b"IDAT", &idat_payload);
    write_png_chunk(&mut out, *b"IEND", &[]);

    Ok(out)
}

/// Saves a row-major `width × height` RGBA8 buffer to `path` as a PNG file.
pub fn save_rgba_png(path: impl AsRef<Path>, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    let path_ref = path.as_ref();
    let bytes = encode_rgba_png(width, height, rgba)?;
    if let Some(parent) = path_ref.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path_ref, bytes)?;
    Ok(())
}

/// Decodes an RGBA8 PNG byte slice written by [`encode_rgba_png`] back into a [`RenderedImage`].
pub fn decode_rgba_png(bytes: &[u8]) -> Result<RenderedImage> {
    if bytes.len() < PNG_SIGNATURE.len() || bytes[..8] != PNG_SIGNATURE {
        return Err(RenderError::Png("invalid PNG signature".to_string()));
    }

    let mut pos = 8_usize;
    let mut dimensions: Option<(u32, u32)> = None;
    let mut idat_stream = Vec::new();
    let mut saw_iend = false;

    while pos < bytes.len() {
        let header = bytes
            .get(pos..pos + 8)
            .ok_or_else(|| RenderError::Png("truncated PNG chunk header".to_string()))?;
        let chunk_len = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let chunk_type = [header[4], header[5], header[6], header[7]];
        pos += 8;

        let data_end = pos
            .checked_add(chunk_len)
            .ok_or_else(|| RenderError::Png("PNG chunk length overflow".to_string()))?;
        let crc_end = data_end
            .checked_add(4)
            .ok_or_else(|| RenderError::Png("truncated PNG chunk CRC".to_string()))?;
        let chunk_data = bytes
            .get(pos..data_end)
            .ok_or_else(|| RenderError::Png("truncated PNG chunk payload".to_string()))?;
        let crc_slice = bytes
            .get(data_end..crc_end)
            .ok_or_else(|| RenderError::Png("truncated PNG chunk CRC".to_string()))?;
        let expected_crc =
            u32::from_be_bytes([crc_slice[0], crc_slice[1], crc_slice[2], crc_slice[3]]);
        let actual_crc = crc32_chunk(chunk_type, chunk_data);
        if actual_crc != expected_crc {
            return Err(RenderError::Png("PNG chunk CRC mismatch".to_string()));
        }
        pos = crc_end;

        match &chunk_type {
            b"IHDR" => {
                dimensions = Some(parse_ihdr(chunk_data)?);
            }
            b"IDAT" => {
                idat_stream.extend_from_slice(chunk_data);
            }
            b"IEND" => {
                saw_iend = true;
                break;
            }
            _ => {}
        }
    }

    if !saw_iend {
        return Err(RenderError::Png("missing IEND chunk".to_string()));
    }
    let (width, height) =
        dimensions.ok_or_else(|| RenderError::Png("missing IHDR chunk".to_string()))?;

    let expected_raw_len = (height as usize) * (1 + (width as usize) * 4);
    let filtered = zlib_decode_stored(&idat_stream, expected_raw_len)?;
    let rgba = unfilter_rgba_scanlines(width as usize, height as usize, &filtered)?;

    Ok(RenderedImage {
        width,
        height,
        rgba,
    })
}

/// Reads and decodes an RGBA8 PNG file from disk.
pub fn load_rgba_png(path: impl AsRef<Path>) -> Result<RenderedImage> {
    let bytes = std::fs::read(path)?;
    decode_rgba_png(&bytes)
}

fn validate_png_buffer(width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    if width == 0
        || height == 0
        || width > MAX_VIEWPORT_DIMENSION
        || height > MAX_VIEWPORT_DIMENSION
    {
        return Err(RenderError::InvalidViewport {
            width,
            height,
            max_dimension: MAX_VIEWPORT_DIMENSION,
        });
    }
    let expected_bytes = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(4);
    if rgba.len() != expected_bytes {
        return Err(RenderError::BufferSizeMismatch {
            actual_bytes: rgba.len(),
            expected_bytes,
            width,
            height,
        });
    }
    Ok(())
}

fn parse_ihdr(data: &[u8]) -> Result<(u32, u32)> {
    if data.len() != 13 {
        return Err(RenderError::Png("invalid IHDR length".to_string()));
    }
    let width = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
    let height = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    if width == 0
        || height == 0
        || width > MAX_VIEWPORT_DIMENSION
        || height > MAX_VIEWPORT_DIMENSION
    {
        return Err(RenderError::InvalidViewport {
            width,
            height,
            max_dimension: MAX_VIEWPORT_DIMENSION,
        });
    }
    if data[8] != 8 || data[9] != 6 {
        return Err(RenderError::Png(
            "unsupported PNG bit depth or color type (expected RGBA8)".to_string(),
        ));
    }
    Ok((width, height))
}

fn write_png_chunk(out: &mut Vec<u8>, chunk_type: [u8; 4], data: &[u8]) {
    let len = data.len() as u32;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&chunk_type);
    out.extend_from_slice(data);
    let crc = crc32_chunk(chunk_type, data);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn zlib_encode_stored(raw: &[u8]) -> Vec<u8> {
    let num_blocks = if raw.is_empty() {
        1
    } else {
        raw.len().div_ceil(MAX_STORED_BLOCK_LEN)
    };
    let mut out = Vec::with_capacity(2 + num_blocks * 5 + raw.len() + 4);
    // Zlib header: CMF = 0x78 (deflate, 32K window), FLG = 0x01 (no dict, FCHECK valid).
    out.push(0x78);
    out.push(0x01);

    if raw.is_empty() {
        out.push(0x01); // BFINAL = 1, BTYPE = 00
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(!0u16).to_le_bytes());
    } else {
        let chunks = raw.chunks(MAX_STORED_BLOCK_LEN);
        let total_chunks = chunks.len();
        for (idx, block) in chunks.enumerate() {
            let is_final = idx + 1 == total_chunks;
            out.push(u8::from(is_final));
            let len = block.len() as u16;
            let nlen = !len;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&nlen.to_le_bytes());
            out.extend_from_slice(block);
        }
    }

    let checksum = adler32(raw);
    out.extend_from_slice(&checksum.to_be_bytes());
    out
}

fn zlib_decode_stored(stream: &[u8], expected_len: usize) -> Result<Vec<u8>> {
    if stream.len() < 6 {
        return Err(RenderError::Png("truncated zlib stream".to_string()));
    }
    let cmf = u16::from(stream[0]);
    let flg = u16::from(stream[1]);
    if (cmf & 0x0F) != 8 || ((cmf << 8) | flg) % 31 != 0 || (flg & 0x20) != 0 {
        return Err(RenderError::Png("invalid zlib header".to_string()));
    }

    let mut pos = 2_usize;
    let mut out = Vec::with_capacity(expected_len);

    loop {
        let header = stream
            .get(pos..pos + 5)
            .ok_or_else(|| RenderError::Png("truncated deflate stored block header".to_string()))?;
        let bheader = header[0];
        let bfinal = (bheader & 0x01) != 0;
        let btype = (bheader >> 1) & 0x03;
        if btype != 0 {
            return Err(RenderError::Png(
                "unsupported deflate block type (expected stored block)".to_string(),
            ));
        }
        let len = u16::from_le_bytes([header[1], header[2]]);
        let nlen = u16::from_le_bytes([header[3], header[4]]);
        if len != !nlen {
            return Err(RenderError::Png(
                "deflate stored block LEN/NLEN mismatch".to_string(),
            ));
        }
        pos += 5;

        let block_len = usize::from(len);
        if out.len().saturating_add(block_len) > expected_len {
            return Err(RenderError::Png(
                "inflated PNG data exceeds expected image size".to_string(),
            ));
        }
        let block_slice = stream
            .get(pos..pos + block_len)
            .ok_or_else(|| RenderError::Png("truncated deflate stored block data".to_string()))?;
        out.extend_from_slice(block_slice);
        pos += block_len;

        if bfinal {
            break;
        }
    }

    let adler_slice = stream
        .get(pos..pos + 4)
        .ok_or_else(|| RenderError::Png("missing zlib Adler-32 checksum".to_string()))?;
    let expected_adler = u32::from_be_bytes([
        adler_slice[0],
        adler_slice[1],
        adler_slice[2],
        adler_slice[3],
    ]);
    if adler32(&out) != expected_adler {
        return Err(RenderError::Png(
            "zlib Adler-32 checksum mismatch".to_string(),
        ));
    }
    if out.len() != expected_len {
        return Err(RenderError::Png(
            "unexpected inflated scanline byte length".to_string(),
        ));
    }

    Ok(out)
}

fn unfilter_rgba_scanlines(width: usize, height: usize, filtered: &[u8]) -> Result<Vec<u8>> {
    let row_bytes = width * 4;
    let stride = 1 + row_bytes;
    if filtered.len() != height * stride {
        return Err(RenderError::Png("invalid scanline buffer size".to_string()));
    }

    let mut rgba = vec![0u8; height * row_bytes];
    for y in 0..height {
        let src_row = &filtered[y * stride..(y + 1) * stride];
        let filter_type = src_row[0];
        let scanline = &src_row[1..];
        let dst_start = y * row_bytes;
        if filter_type != 0 {
            return Err(RenderError::Png(format!(
                "unsupported PNG scanline filter type {filter_type}"
            )));
        }
        rgba[dst_start..dst_start + row_bytes].copy_from_slice(scanline);
    }
    Ok(rgba)
}

fn adler32(data: &[u8]) -> u32 {
    const MOD_ADLER: u32 = 65_521;
    let mut s1: u32 = 1;
    let mut s2: u32 = 0;
    for chunk in data.chunks(5552) {
        for &b in chunk {
            s1 += u32::from(b);
            s2 += s1;
        }
        s1 %= MOD_ADLER;
        s2 %= MOD_ADLER;
    }
    (s2 << 16) | s1
}

const CRC32_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut bit = 0;
        while bit < 8 {
            if (c & 1) != 0 {
                c = (c >> 1) ^ 0xEDB8_8320;
            } else {
                c >>= 1;
            }
            bit += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

fn crc32_chunk(chunk_type: [u8; 4], data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in &chunk_type {
        crc = (crc >> 8) ^ CRC32_TABLE[((crc as u8) ^ b) as usize];
    }
    for &b in data {
        crc = (crc >> 8) ^ CRC32_TABLE[((crc as u8) ^ b) as usize];
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_roundtrip_preserves_dimensions_and_rgba_pixels() -> Result<()> {
        let width = 32_u32;
        let height = 24_u32;
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                rgba.push(((x * 7) & 0xFF) as u8);
                rgba.push(((y * 11) & 0xFF) as u8);
                rgba.push((((x + y) * 5) & 0xFF) as u8);
                rgba.push(255);
            }
        }

        let img = RenderedImage {
            width,
            height,
            rgba: rgba.clone(),
        };
        let png_bytes = img.to_png_bytes()?;
        assert!(png_bytes.starts_with(&PNG_SIGNATURE));

        let decoded = decode_rgba_png(&png_bytes)?;
        assert_eq!(decoded.width, width);
        assert_eq!(decoded.height, height);
        assert_eq!(decoded.rgba, rgba);
        Ok(())
    }

    #[test]
    fn png_rejects_corrupted_signature_and_bad_dimensions() {
        assert!(matches!(
            encode_rgba_png(0, 10, &[]),
            Err(RenderError::InvalidViewport { .. })
        ));
        assert!(matches!(
            encode_rgba_png(2, 2, &[0u8; 8]),
            Err(RenderError::BufferSizeMismatch { .. })
        ));
        assert!(matches!(
            decode_rgba_png(b"not a png"),
            Err(RenderError::Png(_))
        ));
    }
}
