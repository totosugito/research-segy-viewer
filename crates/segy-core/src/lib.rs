#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

//! Read-only SEG-Y (rev 0/1) parser and lazy trace indexer.

/// Placeholder crate version check for workspace scaffolding.
pub fn crate_ready() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffold_smoke() {
        assert!(crate_ready());
    }
}
