//! Precomputed 256-entry and 4096-entry RGBA colormap lookup tables (LUTs) for variable-density
//! seismic image rendering.

/// Number of entries in the high-precision 12-bit colormap LUT.
pub const LUT_SIZE_4096: usize = 4096;
/// Number of entries in the compact 8-bit colormap LUT.
pub const LUT_SIZE_256: usize = 256;

/// Built-in seismic variable-density colormaps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColormapKind {
    /// Standard seismic grayscale: `-1.0` = white, `0.0` = mid-gray (`128`), `+1.0` = black.
    #[default]
    Grayscale,
    /// Diverging seismic Blue–White–Red (`-1.0` = blue, `0.0` = white, `+1.0` = red; flip with `reversed`).
    SeismicRwb,
    /// Perceptually uniform scientific Viridis colormap (`-1.0` = purple, `0.0` = teal, `+1.0` = yellow).
    Viridis,
    /// Multi-stop spectral Rainbow colormap (`-1.0` = blue, `0.0` = green, `+1.0` = red).
    Rainbow,
    /// Classic seismic diverging Brown–White–Black (`-1.0` = warm brown, `0.0` = white, `+1.0` = charcoal).
    BrownWhiteBlack,
}

impl ColormapKind {
    /// Returns all available built-in colormap variants.
    pub const ALL: [Self; 5] = [
        Self::Grayscale,
        Self::SeismicRwb,
        Self::Viridis,
        Self::Rainbow,
        Self::BrownWhiteBlack,
    ];

    /// Human-readable display name for UI menus.
    #[inline]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Grayscale => "Grayscale",
            Self::SeismicRwb => "Seismic (Blue-White-Red)",
            Self::Viridis => "Viridis",
            Self::Rainbow => "Rainbow",
            Self::BrownWhiteBlack => "Brown-White-Black",
        }
    }
}

/// Configuration for building a [`ColormapLut`], including reversal, center bias, contrast, and clip colors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColormapConfig {
    /// Base colormap family.
    pub kind: ColormapKind,
    /// Reverse the colormap direction (`-1.0` ↔ `+1.0`).
    pub reversed: bool,
    /// Bias / center shift in `[-1.0, 1.0]` (`0.0` = centered at zero amplitude).
    pub bias: f32,
    /// Contrast / slope multiplier around the center (`1.0` = nominal).
    pub contrast: f32,
    /// Optional explicit RGBA color applied when normalized amplitude is clipped at `<= -1.0`.
    pub low_clip_color: Option<[u8; 4]>,
    /// Optional explicit RGBA color applied when normalized amplitude is clipped at `>= +1.0`.
    pub high_clip_color: Option<[u8; 4]>,
}

impl Default for ColormapConfig {
    fn default() -> Self {
        Self {
            kind: ColormapKind::Grayscale,
            reversed: false,
            bias: 0.0,
            contrast: 1.0,
            low_clip_color: None,
            high_clip_color: None,
        }
    }
}

impl ColormapConfig {
    /// Creates a default [`ColormapConfig`] for `kind`.
    #[inline]
    pub const fn new(kind: ColormapKind) -> Self {
        Self {
            kind,
            reversed: false,
            bias: 0.0,
            contrast: 1.0,
            low_clip_color: None,
            high_clip_color: None,
        }
    }

    /// Sets whether the colormap is reversed.
    #[inline]
    pub const fn with_reversed(mut self, reversed: bool) -> Self {
        self.reversed = reversed;
        self
    }

    /// Sets the center bias in `[-1.0, 1.0]`.
    #[inline]
    pub const fn with_bias(mut self, bias: f32) -> Self {
        self.bias = bias;
        self
    }

    /// Sets the contrast factor (`> 0.0`).
    #[inline]
    pub const fn with_contrast(mut self, contrast: f32) -> Self {
        self.contrast = contrast;
        self
    }

    /// Sets optional low/high saturation clip colors.
    #[inline]
    pub const fn with_clip_colors(
        mut self,
        low_clip_color: Option<[u8; 4]>,
        high_clip_color: Option<[u8; 4]>,
    ) -> Self {
        self.low_clip_color = low_clip_color;
        self.high_clip_color = high_clip_color;
        self
    }
}

/// Precomputed 4096-entry and 256-entry RGBA lookup tables.
#[derive(Debug, Clone)]
pub struct ColormapLut {
    config: ColormapConfig,
    entries_4096: Box<[[u8; 4]; LUT_SIZE_4096]>,
    entries_256: [[u8; 4]; LUT_SIZE_256],
}

impl Default for ColormapLut {
    fn default() -> Self {
        Self::new(ColormapKind::Grayscale)
    }
}

impl ColormapLut {
    /// Builds a [`ColormapLut`] for `kind` with default settings.
    pub fn new(kind: ColormapKind) -> Self {
        Self::from_config(ColormapConfig::new(kind))
    }

    /// Builds a [`ColormapLut`] from a full [`ColormapConfig`].
    pub fn from_config(config: ColormapConfig) -> Self {
        let mut entries_4096 = Box::new([[0u8; 4]; LUT_SIZE_4096]);
        let mut entries_256 = [[0u8; 4]; LUT_SIZE_256];

        for (i, slot) in entries_4096.iter_mut().enumerate() {
            // Map i in 0..4095 to normalized amplitude in [-1.0, 1.0];
            // force the exact midpoint to 0.0 so diverging colormaps hit pure white at zero.
            let norm = if i == LUT_SIZE_4096 / 2 {
                0.0
            } else {
                (i as f32 / (LUT_SIZE_4096 - 1) as f32) * 2.0 - 1.0
            };
            *slot = evaluate_color(&config, norm);
        }
        if let Some(low) = config.low_clip_color {
            entries_4096[0] = low;
        }
        if let Some(high) = config.high_clip_color {
            entries_4096[LUT_SIZE_4096 - 1] = high;
        }

        for (i, slot) in entries_256.iter_mut().enumerate() {
            let norm = if i == 128 {
                0.0
            } else {
                (i as f32 / (LUT_SIZE_256 - 1) as f32) * 2.0 - 1.0
            };
            *slot = evaluate_color(&config, norm);
        }
        if let Some(low) = config.low_clip_color {
            entries_256[0] = low;
        }
        if let Some(high) = config.high_clip_color {
            entries_256[LUT_SIZE_256 - 1] = high;
        }

        Self {
            config,
            entries_4096,
            entries_256,
        }
    }

    /// Returns the configuration used to build this LUT.
    #[inline]
    pub const fn config(&self) -> &ColormapConfig {
        &self.config
    }

    /// Returns the underlying 4096-entry RGBA lookup table.
    #[inline]
    pub fn entries_4096(&self) -> &[[u8; 4]; LUT_SIZE_4096] {
        &self.entries_4096
    }

    /// Returns the underlying 256-entry RGBA lookup table.
    #[inline]
    pub const fn entries_256(&self) -> &[[u8; 4]; LUT_SIZE_256] {
        &self.entries_256
    }

    /// Quantizes a normalized amplitude in `[-1.0, 1.0]` into a 12-bit LUT index (`0..=4095`).
    /// Non-finite inputs map to the midpoint (`2048`, corresponding to `0.0`).
    #[inline]
    pub fn quantize_4096(normalized: f32) -> u16 {
        if !normalized.is_finite() {
            return (LUT_SIZE_4096 / 2) as u16;
        }
        if normalized == 0.0 {
            return (LUT_SIZE_4096 / 2) as u16;
        }
        let clamped = normalized.clamp(-1.0, 1.0);
        let scaled = (clamped + 1.0) * (0.5 * (LUT_SIZE_4096 - 1) as f32) + 0.5;
        (scaled as u16).min((LUT_SIZE_4096 - 1) as u16)
    }

    /// Quantizes a normalized amplitude in `[-1.0, 1.0]` into an 8-bit LUT index (`0..=255`).
    /// Non-finite inputs map to `128` (`0.0`).
    #[inline]
    pub fn quantize_256(normalized: f32) -> u8 {
        if !normalized.is_finite() || normalized == 0.0 {
            return 128;
        }
        let clamped = normalized.clamp(-1.0, 1.0);
        let scaled = (clamped + 1.0) * (0.5 * (LUT_SIZE_256 - 1) as f32) + 0.5;
        (scaled as u16).min((LUT_SIZE_256 - 1) as u16) as u8
    }

    /// Looks up the RGBA color for a normalized amplitude in `[-1.0, 1.0]` via the 4096-entry LUT.
    #[inline]
    pub fn sample(&self, normalized: f32) -> [u8; 4] {
        let idx = Self::quantize_4096(normalized) as usize;
        self.entries_4096[idx & (LUT_SIZE_4096 - 1)]
    }

    /// Looks up the RGBA color for a pre-quantized 12-bit LUT index (`0..4096`).
    #[inline]
    pub fn sample_index_4096(&self, index: u16) -> [u8; 4] {
        self.entries_4096[(index as usize) & (LUT_SIZE_4096 - 1)]
    }

    /// Looks up the RGBA color for a pre-quantized 8-bit LUT index (`0..256`).
    #[inline]
    pub const fn sample_index_256(&self, index: u8) -> [u8; 4] {
        self.entries_256[index as usize]
    }
}

fn evaluate_color(config: &ColormapConfig, normalized: f32) -> [u8; 4] {
    let bias = if config.bias.is_finite() {
        config.bias.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let contrast = if config.contrast.is_finite() && config.contrast > 0.0 {
        config.contrast.clamp(0.05, 50.0)
    } else {
        1.0
    };

    let mut v = ((normalized - bias) * contrast).clamp(-1.0, 1.0);
    if config.reversed {
        v = -v;
    }

    // t in [0.0, 1.0], with exact 0.5 when v == 0.0.
    let t = if v == 0.0 {
        0.5
    } else {
        ((v + 1.0) * 0.5).clamp(0.0, 1.0)
    };

    match config.kind {
        ColormapKind::Grayscale => {
            // Standard seismic VD: -1.0 -> 255 (white), 0.0 -> 128 (mid-gray), +1.0 -> 0 (black).
            let lum = if v == 0.0 {
                128
            } else {
                ((1.0 - t) * 255.0 + 0.5).clamp(0.0, 255.0) as u8
            };
            [lum, lum, lum, 255]
        }
        ColormapKind::SeismicRwb => {
            const STOPS: [(f32, [u8; 3]); 5] = [
                (0.0, [0, 0, 255]),
                (0.25, [120, 165, 255]),
                (0.5, [255, 255, 255]),
                (0.75, [255, 135, 120]),
                (1.0, [255, 0, 0]),
            ];
            interpolate_stops(&STOPS, t)
        }
        ColormapKind::Viridis => {
            const STOPS: [(f32, [u8; 3]); 5] = [
                (0.0, [68, 1, 84]),
                (0.25, [59, 82, 139]),
                (0.5, [33, 145, 140]),
                (0.75, [94, 201, 98]),
                (1.0, [253, 231, 37]),
            ];
            interpolate_stops(&STOPS, t)
        }
        ColormapKind::Rainbow => {
            const STOPS: [(f32, [u8; 3]); 5] = [
                (0.0, [0, 32, 255]),
                (0.25, [0, 220, 255]),
                (0.5, [0, 210, 60]),
                (0.75, [255, 225, 0]),
                (1.0, [255, 20, 0]),
            ];
            interpolate_stops(&STOPS, t)
        }
        ColormapKind::BrownWhiteBlack => {
            const STOPS: [(f32, [u8; 3]); 5] = [
                (0.0, [176, 96, 24]),
                (0.25, [224, 180, 132]),
                (0.5, [255, 255, 255]),
                (0.75, [128, 128, 128]),
                (1.0, [12, 12, 12]),
            ];
            interpolate_stops(&STOPS, t)
        }
    }
}

fn interpolate_stops(stops: &[(f32, [u8; 3])], t: f32) -> [u8; 4] {
    let clamped = t.clamp(0.0, 1.0);
    for window in stops.windows(2) {
        let (t0, c0) = window[0];
        let (t1, c1) = window[1];
        if clamped <= t1 {
            let span = (t1 - t0).max(1e-6);
            let alpha = ((clamped - t0) / span).clamp(0.0, 1.0);
            let r = (f32::from(c0[0]) + (f32::from(c1[0]) - f32::from(c0[0])) * alpha + 0.5) as u8;
            let g = (f32::from(c0[1]) + (f32::from(c1[1]) - f32::from(c0[1])) * alpha + 0.5) as u8;
            let b = (f32::from(c0[2]) + (f32::from(c1[2]) - f32::from(c0[2])) * alpha + 0.5) as u8;
            return [r, g, b, 255];
        }
    }
    let last = stops[stops.len() - 1].1;
    [last[0], last[1], last[2], 255]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grayscale_lut_endpoints_and_midpoint_are_exact() {
        let lut = ColormapLut::new(ColormapKind::Grayscale);
        assert_eq!(lut.sample(-1.0), [255, 255, 255, 255]);
        assert_eq!(lut.sample(0.0), [128, 128, 128, 255]);
        assert_eq!(lut.sample(1.0), [0, 0, 0, 255]);

        let rev = ColormapLut::from_config(
            ColormapConfig::new(ColormapKind::Grayscale).with_reversed(true),
        );
        assert_eq!(rev.sample(-1.0), [0, 0, 0, 255]);
        assert_eq!(rev.sample(0.0), [128, 128, 128, 255]);
        assert_eq!(rev.sample(1.0), [255, 255, 255, 255]);
    }

    #[test]
    fn seismic_rwb_lut_has_blue_white_red_stops() {
        let lut = ColormapLut::new(ColormapKind::SeismicRwb);
        assert_eq!(lut.sample(-1.0), [0, 0, 255, 255]);
        assert_eq!(lut.sample(0.0), [255, 255, 255, 255]);
        assert_eq!(lut.sample(1.0), [255, 0, 0, 255]);
    }

    #[test]
    fn all_colormaps_produce_opaque_rgba_and_handle_non_finite() {
        for kind in ColormapKind::ALL {
            let lut = ColormapLut::new(kind);
            for &v in &[
                -10.0,
                -1.0,
                -0.5,
                0.0,
                0.5,
                1.0,
                10.0,
                f32::NAN,
                f32::INFINITY,
            ] {
                let c = lut.sample(v);
                assert_eq!(c[3], 255);
            }
            assert_eq!(lut.sample(f32::NAN), lut.sample(0.0));
        }
    }
}
