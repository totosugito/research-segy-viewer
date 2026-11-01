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

/// Placeholder readiness check for the headless render crate.
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
