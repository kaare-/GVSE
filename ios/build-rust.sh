#!/bin/sh
# Build libtank_ffi.a for the iPhone and the simulator.
# Run this on a Mac before opening ios/MakeAcreature.xcodeproj.
# The Xcode target links whichever of these products matches the SDK.
set -eu
cd "$(dirname "$0")/.."
# Device is arm64. Apple silicon simulators use aarch64-apple-ios-sim.
# Intel Mac simulators (this iMac) link x86_64-apple-ios.
rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
cargo build -p tank-ffi --release --target aarch64-apple-ios
cargo build -p tank-ffi --release --target aarch64-apple-ios-sim
cargo build -p tank-ffi --release --target x86_64-apple-ios
echo "static libs:"
echo "  target/aarch64-apple-ios/release/libtank_ffi.a"
echo "  target/aarch64-apple-ios-sim/release/libtank_ffi.a"
echo "  target/x86_64-apple-ios/release/libtank_ffi.a"
