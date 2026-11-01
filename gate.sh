#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT_DIR"

if [ -d "$ROOT_DIR/.toolchain/cargo/bin" ]; then
    export RUSTUP_HOME="$ROOT_DIR/.toolchain/rustup"
    export CARGO_HOME="$ROOT_DIR/.toolchain/cargo"
    export PATH="$ROOT_DIR/.toolchain/cargo/bin:$PATH"
fi

echo "==> cargo fmt --all -- --check"
cargo fmt --all -- --check

echo "==> cargo clippy --workspace --all-targets -- -D warnings"
cargo clippy --workspace --all-targets -- -D warnings

echo "==> cargo test --workspace --all-targets"
cargo test --workspace --all-targets
