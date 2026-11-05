#!/usr/bin/env bash
set -e
cd contract && rustup show active-toolchain && cd ..
rustup target add wasm32-unknown-unknown
# Prebuilt binaries: no long compile
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/near/cargo-near/releases/latest/download/cargo-near-installer.sh | sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/near/near-cli-rs/releases/latest/download/near-cli-rs-installer.sh | sh
echo "Setup done. Try: cargo near --version && near --version"
