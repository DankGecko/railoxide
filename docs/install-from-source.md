# Install RailOxide From Source

RailOxide is alpha software. The source installer builds from GitHub instead of installing prebuilt signed binaries.

## Quick Install

macOS/Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/triamazikamno/railoxide/main/scripts/install-wallet | bash
```

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/triamazikamno/railoxide/main/scripts/install-wallet.ps1 | iex
```

## Inspect First

macOS/Linux:

```bash
curl -fsSLO https://raw.githubusercontent.com/triamazikamno/railoxide/main/scripts/install-wallet
less install-wallet
bash install-wallet
```

Windows PowerShell:

```powershell
iwr https://raw.githubusercontent.com/triamazikamno/railoxide/main/scripts/install-wallet.ps1 -OutFile install-wallet.ps1
notepad .\install-wallet.ps1
powershell -ExecutionPolicy Bypass -File .\install-wallet.ps1
```

## What It Does

- Prompts for the source to build: latest release, latest `main`, or a specific tag/branch/commit.
- Uses the latest published GitHub release by default, including pre-releases.
- Clones or updates `https://github.com/triamazikamno/railoxide.git`.
- Stores the managed checkout at `~/.local/src/railoxide` on macOS/Linux and `%LOCALAPPDATA%\RailOxide\src\railoxide` on Windows by default.
- Installs Rust `1.94.0` through `rustup` if needed.
- Builds the wallet with hardware-wallet support enabled by default.
- Prints the exact source commit before building.
- Refuses to run as root on macOS/Linux.
- Uses `sudo` only for Ubuntu/Debian system package installation.

## macOS

The installer checks for Apple Command Line Tools, Rust, and Git.

If full Xcode and the Metal tools are available, the installer builds with build-time Metal shader compilation. If full Xcode is installed but the Metal tools are missing, it asks before running:

```bash
xcodebuild -downloadComponent MetalToolchain
```

If Metal is unavailable or the download is skipped, it falls back to `gpui/runtime_shaders`, which works with Command Line Tools-only installs.

The installed app is written to:

```text
~/Applications/RailOxide.app
```

The installer also writes a command wrapper to:

```text
~/.local/bin/railoxide-wallet
```

Useful macOS options:

```bash
bash install-wallet --metal
bash install-wallet --runtime-shaders
bash install-wallet --main
bash install-wallet --ref v0.1.0-alpha.1
```

## Ubuntu/Debian Linux

The installer uses `apt-get` to install the build dependencies listed in [`build-wallet-ubuntu.md`](build-wallet-ubuntu.md), then builds the release binary.

The installed command is written to:

```text
~/.local/bin/railoxide-wallet
```

It also installs a desktop entry and icon under `~/.local/share`.

## Windows

The Windows installer uses `winget` to install missing dependencies from [`build-wallet-windows.md`](build-wallet-windows.md), including Git, Rustup, CMake, LLVM, and Visual Studio Build Tools.

It builds the x64 MSVC target:

```text
x86_64-pc-windows-msvc
```

It also downloads the official SQLite DLL package, generates `sqlite3.lib` with Visual Studio's `lib` tool, and copies `sqlite3.dll` next to the wallet executable.

The installed files are written to:

```text
%LOCALAPPDATA%\RailOxide\bin\wallet.exe
%LOCALAPPDATA%\RailOxide\bin\sqlite3.dll
```

The installer creates a Start Menu shortcut unless `-NoShortcut` is passed.

## Options

macOS/Linux options:

```text
--prefix PATH          Install under PATH instead of ~/.local
--source-dir PATH      Use PATH for the managed source checkout
--main                 Build the latest main branch without prompting
--ref REF              Build a specific tag, branch, or commit without prompting
--no-deps              Do not install missing system dependencies
--no-hardware          Build without hardware-wallet support
--metal                On macOS, require build-time Metal shader compilation
--runtime-shaders      On macOS, force runtime shaders instead of Metal
-y, --yes              Do not prompt before dependency installs/downloads
--dry-run              Print what would happen without changing anything
--verbose              Print commands as they run
-h, --help             Show help
```

Windows PowerShell options:

```text
-InstallDir PATH       Install wallet.exe and sqlite3.dll under PATH
-SourceDir PATH        Use PATH for the managed source checkout
-Main                  Build the latest main branch without prompting
-Ref REF               Build a specific tag, branch, or commit without prompting
-NoDeps                Do not install missing system dependencies
-NoHardware            Build without hardware-wallet support
-Yes                   Do not prompt before dependency installs/downloads
-DryRun                Print what would happen without changing anything
-NoShortcut            Do not create a Start Menu shortcut
-Help                  Show help
```

## Updating

Rerun the installer. It updates the managed checkout to the selected source and rebuilds.

## Troubleshooting

If `railoxide-wallet` is not found after installation on macOS/Linux, add `~/.local/bin` to your shell `PATH`.

If the installer reports that `~/.local/src/railoxide` is not installer-managed, either move that existing checkout or pass a different `--source-dir`.

If the Windows installer reports that `%LOCALAPPDATA%\RailOxide\src\railoxide` is not installer-managed, either move that existing checkout or pass a different `-SourceDir`.

For manual platform-specific build steps, see:

- [`Ubuntu`](build-wallet-ubuntu.md)
- [`macOS`](build-wallet-macos.md)
- [`Windows`](build-wallet-windows.md)
