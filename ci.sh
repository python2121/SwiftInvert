#!/usr/bin/env bash
# The quality gate for the Rust port: formatting, clippy with warnings denied, the whole test suite.
# GPU tests skip (and say so) when no adapter is available; NEG_GPU=off forces the CPU renderer.
set -euo pipefail
cd "$(dirname "$0")"
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace "$@"
