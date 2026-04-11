#!/bin/bash
set -e

# Install Rust toolchain if not present
if ! command -v cargo &>/dev/null; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  source "$HOME/.cargo/env"
fi

# Build the project
cargo build --release

echo "Hermip build complete. Binary at ./target/release/hermip"
