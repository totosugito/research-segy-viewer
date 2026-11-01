#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

//! Pure headless seismic trace rendering (amplitude pipeline, variable-density image, wiggle paths).

pub mod amplitude;
pub mod colormap;
pub mod error;
pub mod image;
pub mod viewport;
pub mod wiggle;

pub use amplitude::{
    AgcConfig, AmplitudeParams, AmplitudeScratch, ClipStrategy, MIN_AMP_EPSILON, NormScope,
    Polarity, TraceBalance, abs_percentile, abs_percentile_with_scratch, apply_agc_in_place,
    apply_agc_with_scratch, apply_scale_and_clamp_in_place, apply_trace_balance_in_place,
    compute_clip_reference, compute_clip_reference_with_scratch, compute_global_reference,
    gain_db_to_linear, preprocess_trace_with_scratch, process_section, process_trace,
    process_trace_in_place, process_trace_with_scratch, remove_dc_in_place, sanitize_sample,
    trace_max_abs, trace_mean, trace_mean_abs, trace_rms,
};
pub use colormap::{ColormapConfig, ColormapKind, ColormapLut, LUT_SIZE_256, LUT_SIZE_4096};
pub use error::{RenderError, Result};
pub use image::{
    DecimationMode, ImageRenderParams, ImageRenderer, ImageViewport, MAX_VIEWPORT_DIMENSION,
    RenderedImage, SliceTraceSource, TraceDataSource, render_image, render_image_into,
    resample_trace_vertical_into, sample_bin_minmax, select_decimated_amplitude,
};
pub use viewport::{
    AxisTick, CursorReadout, DEFAULT_AUTO_SWITCH_TRACE_SPACING_PX, DisplayMode, SectionDataBounds,
    SeismicViewport, compute_linear_ticks, nice_step_size,
};
pub use wiggle::{
    LobePolarity, Point2f, TraceWiggleGeometry, WiggleFillMode, WigglePolygon, WiggleRenderParams,
    WiggleRenderer, WiggleSectionGeometry, build_trace_wiggle, build_trace_wiggle_with_scratch,
    decimate_trace_vertical_into, extract_lobe_polygons, interpolate_zero_crossing_y,
    rasterize_wiggle_geometry_into, render_wiggle_geometry, render_wiggle_image,
    render_wiggle_into, sample_index_to_pixel_y,
};

/// Readiness check for the headless render crate.
pub fn render_ready() -> bool {
    segy_core::crate_ready()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffold_smoke() {
        assert!(render_ready());
    }
}
