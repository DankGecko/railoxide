This guide builds the RailOxide desktop wallet binary from source on macOS with Ledger and Trezor support enabled.

The commands below were verified on macOS 26.5.1 on Apple Silicon with Xcode Command Line Tools 26.5 and Rust 1.94.0.

## Install Command Line Tools

Install Apple's Command Line Tools first.

```bash
xcode-select --install
```

Verify the tools are active:

```bash
xcode-select -p
git --version
clang --version
```

`xcode-select -p` should report `/Library/Developer/CommandLineTools` or a full Xcode developer directory.

## Install Rust 1.94

If Rust 1.94+ is not installed, install it with `rustup`:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
  sh -s -- -y --default-toolchain 1.94.0
. "$HOME/.cargo/env"
rustc --version
cargo --version
```

Both version commands should report `1.94.0` or higher.

## Clone The Repository

```bash
git clone https://github.com/triamazikamno/railoxide.git
cd railoxide
```

## Build With Hardware Wallet Support

Build the optimized release binary:

### With Metal GPU acceleration (Xcode installation required)

```bash
cargo build --release -p wallet --features hardware
```

### Without Metal

```bash
cargo build --release -p wallet --features hardware,gpui/runtime_shaders
```

The wallet binary is written to:

```bash
target/release/wallet
```

Verify the binary starts far enough to parse command-line options:

```bash
./target/release/wallet --help
```

Run the wallet:

```bash
./target/release/wallet
```

To store wallet data in a custom location:

```bash
./target/release/wallet --db-path "$HOME/RailOxideData"
```

## Profile Rust Heap Memory

The optional `heap-profiling` feature replaces the Rust global allocator with a
profiling-enabled jemalloc build. It is intended only for local profiling and is
not enabled by default in release or packaged wallet builds.

Build the unstripped profiling binary with frame pointers:

```bash
RUSTFLAGS="-C force-frame-pointers=yes" \
  cargo build --profile profiling -p wallet --features heap-profiling
```

Run `target/profiling/wallet`, wait until the wallet reaches the state to
inspect, then press `Command-Option-Shift-H`. The wallet writes a sampled heap
profile to the macOS per-user temporary directory and logs the exact path.
Profiling builds also log each GPUI asset cache miss with its source path,
encoded size, format, and SVG intrinsic dimensions when present. Each heap dump
log also reports jemalloc's live allocated, active, resident, and retained byte
counts for comparison with the process footprint.

Install `jeprof` and the Rust stack-processing tools if needed:

```bash
brew install jemalloc
cargo install rustfilt inferno
```

Generate a Rust-demangled heap flamegraph using the same unstripped executable.
The script rebases wallet addresses from the runtime Mach-O mapping so `jeprof`
does not assign symbols using unadjusted macOS ASLR addresses:

```bash
PROFILE_PATH="dump.heap"
scripts/render-wallet-heap-profile \
  --profile "$PROFILE_PATH" \
  --binary target/profiling/wallet \
  --output /tmp/railoxide-heap.svg
```

## Package A macOS App

The repository includes a packaging script that builds the hardware-enabled wallet, creates `RailOxide.app`, ad-hoc signs it by default, and creates a DMG. By default, the script builds with `hardware,gpui/runtime_shaders` so it works on Command Line Tools-only installs:

```bash
scripts/package-wallet-macos
```

The packaged app and DMG are written to:

```bash
target/macos/RailOxide.app
target/macos/RailOxide.dmg
```

To package with build-time Metal shader compilation after enabling full Xcode, override the Cargo features:

```bash
CARGO_FEATURES=hardware scripts/package-wallet-macos
```

## Enable Build-Time Metal Shader Compilation

Build-time Metal shader compilation embeds a compiled GPUI Metal shader library in the wallet binary. It requires full Xcode because Apple's `metal` and `metallib` tools are not included with Command Line Tools.

Install full Xcode from the App Store or Apple Developer downloads, then select it as the active developer directory:

```bash
sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer
sudo xcodebuild -license accept
xcodebuild -runFirstLaunch
xcodebuild -downloadComponent MetalToolchain
```

Verify the Metal compiler tools are available:

```bash
xcrun --find metal
xcrun --find metallib
xcodebuild -version
```

Build the hardware-enabled wallet without `gpui/runtime_shaders`:

```bash
cargo check -p wallet --features hardware
cargo build --release -p wallet --features hardware
```

Package the app with build-time Metal shader compilation:

```bash
CARGO_FEATURES=hardware scripts/package-wallet-macos
```

To return to the Command Line Tools developer directory later:

```bash
sudo xcode-select --switch /Library/Developer/CommandLineTools
```

## Troubleshooting

If `cargo` is not found after installing Rust, load rustup's environment:

```bash
. "$HOME/.cargo/env"
```

If the build fails because `metal` or `metallib` is missing, build with `--features hardware,gpui/runtime_shaders` or install full Xcode and select it as the active developer directory.
