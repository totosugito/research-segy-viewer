//! Unified seismic section renderer combining variable-density image, wiggle/variable-area,
//! and overlay displays with coarse/full level-of-detail (LOD) progressive rendering.

use crate::amplitude::AmplitudeParams;
use crate::colormap::ColormapConfig;
use crate::error::{RenderError, Result};
use crate::image::{
    DecimationMode, ImageRenderParams, ImageRenderer, ImageViewport, RenderedImage, TraceDataSource,
};
use crate::viewport::{DisplayMode, SeismicViewport};
use crate::wiggle::{WiggleFillMode, WiggleRenderParams, WiggleRenderer};

/// Default minimum horizontal pixel spacing between wiggle traces when viewing a dense section
/// without explicit trace decimation.
pub const DEFAULT_MIN_WIGGLE_SPACING_PX: f32 = 9.0;

/// Progressive rendering quality level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RenderLod {
    /// Fast low-latency preview (half-resolution rasterization upscaled to viewport size).
    Coarse,
    /// Full viewport resolution and full per-column/per-row sampling density.
    #[default]
    Full,
}

/// Complete rendering parameters for a seismic section across `Image`, `Wiggle`, and `Overlay` modes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SectionRenderParams {
    /// Amplitude normalization, clipping, gain, AGC, DC removal, and polarity settings.
    pub amplitude: AmplitudeParams,
    /// Variable-density colormap configuration.
    pub colormap: ColormapConfig,
    /// Envelope decimation mode when zoomed out in variable-density image rendering.
    pub decimation: DecimationMode,
    /// Variable-area lobe fill mode for wiggle traces.
    pub wiggle_fill: WiggleFillMode,
    /// Whether to draw the continuous wiggle trace line.
    pub wiggle_draw_line: bool,
    /// Wiggle excursion scale in trace spacings per unit normalized amplitude.
    pub wiggle_excursion_spacings: f32,
    /// Symmetric wiggle excursion clip limit in trace spacings (`±wiggle_clip_spacings`).
    pub wiggle_clip_spacings: f32,
    /// Minimum horizontal spacing in pixels between rendered wiggle traces when zoomed out.
    pub min_wiggle_spacing_px: f32,
    /// Standalone wiggle background RGBA8 color.
    pub background_rgba: [u8; 4],
    /// Wiggle trace line RGBA8 color.
    pub line_rgba: [u8; 4],
    /// Positive variable-area lobe fill RGBA8 color.
    pub positive_fill_rgba: [u8; 4],
    /// Negative variable-area lobe fill RGBA8 color.
    pub negative_fill_rgba: [u8; 4],
    /// Level of detail (`Coarse` preview vs `Full` quality).
    pub lod: RenderLod,
    /// Optional precomputed global reference amplitude.
    pub global_reference: Option<f32>,
}

impl Default for SectionRenderParams {
    fn default() -> Self {
        Self {
            amplitude: AmplitudeParams::default(),
            colormap: ColormapConfig::default(),
            decimation: DecimationMode::DominantExtremum,
            wiggle_fill: WiggleFillMode::Positive,
            wiggle_draw_line: true,
            wiggle_excursion_spacings: 1.45,
            wiggle_clip_spacings: 1.85,
            min_wiggle_spacing_px: DEFAULT_MIN_WIGGLE_SPACING_PX,
            background_rgba: [255, 255, 255, 255],
            line_rgba: [45, 45, 52, 255],
            positive_fill_rgba: [0, 0, 0, 255],
            negative_fill_rgba: [30, 100, 200, 255],
            lod: RenderLod::Full,
            global_reference: None,
        }
    }
}

impl SectionRenderParams {
    /// Sets the amplitude pipeline parameters.
    #[inline]
    pub const fn with_amplitude(mut self, amplitude: AmplitudeParams) -> Self {
        self.amplitude = amplitude;
        self
    }

    /// Sets the colormap configuration.
    #[inline]
    pub const fn with_colormap(mut self, colormap: ColormapConfig) -> Self {
        self.colormap = colormap;
        self
    }

    /// Sets the wiggle fill mode.
    #[inline]
    pub const fn with_wiggle_fill(mut self, wiggle_fill: WiggleFillMode) -> Self {
        self.wiggle_fill = wiggle_fill;
        self
    }

    /// Sets the progressive rendering LOD (`Coarse` or `Full`).
    #[inline]
    pub const fn with_lod(mut self, lod: RenderLod) -> Self {
        self.lod = lod;
        self
    }

    /// Builds [`ImageRenderParams`] for the given `lod`.
    #[inline]
    pub fn to_image_params(&self, lod: RenderLod) -> ImageRenderParams {
        let max_traces_per_col = match lod {
            RenderLod::Coarse => 1,
            RenderLod::Full => 4,
        };
        ImageRenderParams {
            amplitude: self.amplitude,
            colormap: self.colormap,
            decimation: self.decimation,
            max_traces_per_column: max_traces_per_col,
            global_reference: self.global_reference,
        }
    }

    /// Builds [`WiggleRenderParams`] for the given `viewport`, `render_width`, and `lod`.
    pub fn to_wiggle_params(
        &self,
        viewport: &SeismicViewport,
        render_width: u32,
        lod: RenderLod,
    ) -> WiggleRenderParams {
        let max_pts_per_px = match lod {
            RenderLod::Coarse => 1,
            RenderLod::Full => 2,
        };
        let min_spacing =
            if self.min_wiggle_spacing_px.is_finite() && self.min_wiggle_spacing_px >= 1.0 {
                self.min_wiggle_spacing_px
            } else {
                DEFAULT_MIN_WIGGLE_SPACING_PX
            };
        let spacing_cap = ((render_width as f32) / min_spacing).floor().max(1.0) as usize;
        let decimated_count = viewport.decimated_trace_count().max(1);
        let max_rendered = if viewport.trace_decimate_step() > 1 {
            decimated_count.min(render_width as usize)
        } else {
            decimated_count.min(spacing_cap)
        };

        WiggleRenderParams {
            amplitude: self.amplitude,
            fill_mode: self.wiggle_fill,
            draw_line: self.wiggle_draw_line,
            excursion_spacings: self.wiggle_excursion_spacings,
            clip_spacings: self.wiggle_clip_spacings,
            max_points_per_pixel: max_pts_per_px,
            max_rendered_traces: Some(max_rendered.max(1)),
            background_rgba: self.background_rgba,
            line_rgba: self.line_rgba,
            positive_fill_rgba: self.positive_fill_rgba,
            negative_fill_rgba: self.negative_fill_rgba,
            global_reference: self.global_reference,
        }
    }
}

/// Stateful section renderer that reuses viewport-sized image, wiggle, and LOD buffers across frames.
#[derive(Debug, Default, Clone)]
pub struct SectionRenderer {
    image_renderer: ImageRenderer,
    wiggle_renderer: WiggleRenderer,
    coarse_rgba: Vec<u8>,
}

impl SectionRenderer {
    /// Creates a new [`SectionRenderer`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Renders `viewport` from `source` into a viewport-sized [`RenderedImage`].
    pub fn render(
        &mut self,
        source: &dyn TraceDataSource,
        viewport: &SeismicViewport,
        params: &SectionRenderParams,
    ) -> Result<RenderedImage> {
        let img_vp = viewport.to_image_viewport();
        let byte_len = img_vp.rgba_byte_len()?;
        let mut rgba = vec![0u8; byte_len];
        self.render_into(source, viewport, params, &mut rgba)?;
        Ok(RenderedImage {
            width: img_vp.width,
            height: img_vp.height,
            rgba,
        })
    }

    /// Renders `viewport` from `source` directly into `out_rgba` (`width * height * 4` bytes).
    pub fn render_into(
        &mut self,
        source: &dyn TraceDataSource,
        viewport: &SeismicViewport,
        params: &SectionRenderParams,
        out_rgba: &mut [u8],
    ) -> Result<()> {
        let full_vp = viewport.to_image_viewport();
        let expected_bytes = full_vp.rgba_byte_len()?;
        if out_rgba.len() != expected_bytes {
            return Err(RenderError::BufferSizeMismatch {
                actual_bytes: out_rgba.len(),
                expected_bytes,
                width: full_vp.width,
                height: full_vp.height,
            });
        }

        if params.lod == RenderLod::Coarse && full_vp.width >= 4 && full_vp.height >= 4 {
            let coarse_w = (full_vp.width / 2).max(1);
            let coarse_h = (full_vp.height / 2).max(1);
            let coarse_vp = ImageViewport {
                width: coarse_w,
                height: coarse_h,
                trace_range: full_vp.trace_range.clone(),
                sample_range: full_vp.sample_range.clone(),
                reverse_traces: full_vp.reverse_traces,
            };
            let coarse_bytes = coarse_vp.rgba_byte_len()?;
            self.coarse_rgba.resize(coarse_bytes, 0);
            Self::render_pass_into(
                &mut self.image_renderer,
                &mut self.wiggle_renderer,
                source,
                viewport,
                &coarse_vp,
                params,
                RenderLod::Coarse,
                &mut self.coarse_rgba,
            )?;
            upscale_nearest_rgba(
                &self.coarse_rgba,
                coarse_w as usize,
                coarse_h as usize,
                out_rgba,
                full_vp.width as usize,
                full_vp.height as usize,
            );
            return Ok(());
        }

        Self::render_pass_into(
            &mut self.image_renderer,
            &mut self.wiggle_renderer,
            source,
            viewport,
            &full_vp,
            params,
            RenderLod::Full,
            out_rgba,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_pass_into(
        image_renderer: &mut ImageRenderer,
        wiggle_renderer: &mut WiggleRenderer,
        source: &dyn TraceDataSource,
        viewport: &SeismicViewport,
        target_vp: &ImageViewport,
        params: &SectionRenderParams,
        lod: RenderLod,
        out_rgba: &mut [u8],
    ) -> Result<()> {
        let mode = viewport.effective_display_mode();
        match mode {
            DisplayMode::Image => {
                let img_params = params.to_image_params(lod);
                image_renderer.render_into(source, target_vp, &img_params, out_rgba)?;
            }
            DisplayMode::Wiggle => {
                let wig_params = params.to_wiggle_params(viewport, target_vp.width, lod);
                wiggle_renderer.render_into(source, target_vp, &wig_params, true, out_rgba)?;
            }
            DisplayMode::Overlay => {
                let img_params = params.to_image_params(lod);
                image_renderer.render_into(source, target_vp, &img_params, out_rgba)?;
                let wig_params = params.to_wiggle_params(viewport, target_vp.width, lod);
                wiggle_renderer.render_into(source, target_vp, &wig_params, false, out_rgba)?;
            }
        }
        Ok(())
    }
}

/// Stateless helper that renders `viewport` from `source` into a [`RenderedImage`].
pub fn render_section_image(
    source: &dyn TraceDataSource,
    viewport: &SeismicViewport,
    params: &SectionRenderParams,
) -> Result<RenderedImage> {
    let mut renderer = SectionRenderer::new();
    renderer.render(source, viewport, params)
}

/// Stateless helper that renders `viewport` from `source` into `out_rgba`.
pub fn render_section_into(
    source: &dyn TraceDataSource,
    viewport: &SeismicViewport,
    params: &SectionRenderParams,
    out_rgba: &mut [u8],
) -> Result<()> {
    let mut renderer = SectionRenderer::new();
    renderer.render_into(source, viewport, params, out_rgba)
}

fn upscale_nearest_rgba(
    src: &[u8],
    src_w: usize,
    src_h: usize,
    dst: &mut [u8],
    dst_w: usize,
    dst_h: usize,
) {
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return;
    }
    for y in 0..dst_h {
        let sy = ((y * src_h) / dst_h).min(src_h - 1);
        let src_row = &src[sy * src_w * 4..(sy + 1) * src_w * 4];
        let dst_row = &mut dst[y * dst_w * 4..(y + 1) * dst_w * 4];
        for (x, px_out) in dst_row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let sx = ((x * src_w) / dst_w).min(src_w - 1);
            let idx = sx * 4;
            px_out.copy_from_slice(&src_row[idx..idx + 4]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colormap::ColormapKind;
    use crate::image::SliceTraceSource;
    use crate::viewport::SectionDataBounds;

    #[test]
    fn section_renderer_renders_wiggle_image_and_overlay_distinctly() -> Result<()> {
        let traces: Vec<Vec<f32>> = (0..16)
            .map(|t| {
                (0..32)
                    .map(|s| (((s as f32) * 0.3 + (t as f32) * 0.2).sin()) * 10.0)
                    .collect()
            })
            .collect();
        let src = SliceTraceSource::new(&traces);
        let bounds = SectionDataBounds::new(16, 32, 4000);
        let mut vp = SeismicViewport::new(128, 64, bounds);
        vp.set_auto_switch_wiggle_to_image(false);

        let params = SectionRenderParams::default()
            .with_colormap(ColormapConfig::new(ColormapKind::SeismicRwb));
        let mut renderer = SectionRenderer::new();

        vp.set_display_mode(DisplayMode::Image);
        let img_out = renderer.render(&src, &vp, &params)?;

        vp.set_display_mode(DisplayMode::Wiggle);
        let wig_out = renderer.render(&src, &vp, &params)?;

        vp.set_display_mode(DisplayMode::Overlay);
        let ovl_out = renderer.render(&src, &vp, &params)?;

        assert_eq!(img_out.width, 128);
        assert_eq!(img_out.height, 64);
        assert_ne!(img_out.rgba, wig_out.rgba);
        assert_ne!(img_out.rgba, ovl_out.rgba);
        assert_ne!(wig_out.rgba, ovl_out.rgba);

        // Coarse LOD produces the same viewport buffer size.
        let coarse_params = params.with_lod(RenderLod::Coarse);
        let coarse_out = renderer.render(&src, &vp, &coarse_params)?;
        assert_eq!(coarse_out.width, 128);
        assert_eq!(coarse_out.height, 64);
        assert_eq!(coarse_out.rgba.len(), 128 * 64 * 4);
        Ok(())
    }
}
