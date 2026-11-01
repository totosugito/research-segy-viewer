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
pub mod error;

pub use amplitude::{
    AgcConfig, AmplitudeParams, AmplitudeScratch, ClipStrategy, MIN_AMP_EPSILON, NormScope,
    Polarity, TraceBalance, abs_percentile, abs_percentile_with_scratch, apply_agc_in_place,
    apply_agc_with_scratch, apply_scale_and_clamp_in_place, apply_trace_balance_in_place,
    compute_clip_reference, compute_clip_reference_with_scratch, compute_global_reference,
    gain_db_to_linear, preprocess_trace_with_scratch, process_section, process_trace,
    process_trace_in_place, process_trace_with_scratch, remove_dc_in_place, sanitize_sample,
    trace_max_abs, trace_mean, trace_mean_abs, trace_rms,
};
pub use error::{RenderError, Result};

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
