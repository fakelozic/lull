#!/bin/sh
# The one command that checks everything. Makes no sound and cannot reach the speakers.
set -eu
cd "$(dirname "$0")/.."
export PATH="$HOME/.local/share/mise/shims:$PATH"
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --release --locked
