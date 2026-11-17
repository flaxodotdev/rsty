# 🦀 rsty

A terminal UI that finds the junk eating your disk and deletes it. It was built for the Rust leftovers that pile up quietly.

[![CI](https://github.com/flaxodotdev/rsty/actions/workflows/ci.yml/badge.svg)](https://github.com/flaxodotdev/rsty/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/flaxodotdev/rsty)](https://github.com/flaxodotdev/rsty/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Works on **macOS, Linux and Windows**.

## Install

**macOS / Linux**

```sh
curl -fsSL https://raw.githubusercontent.com/flaxodotdev/rsty/main/install.sh | sh
```

**Windows** (PowerShell)

```powershell
irm https://raw.githubusercontent.com/flaxodotdev/rsty/main/install.ps1 | iex
```

**From source** (any platform with Rust)

```sh
cargo install --git https://github.com/flaxodotdev/rsty
```

Prebuilt binaries for x86_64 and ARM64 on every OS are on the [releases page](https://github.com/flaxodotdev/rsty/releases).
The installers verify SHA-256 checksums. Pin a version with `RSTY_VERSION=v0.1.0` and change the location with `RSTY_INSTALL_DIR`.

## Usage

```
rsty                     # scan your home dir
rsty ~/code /Volumes/X   # scan specific places
rsty --dry-run           # try it without touching anything
rsty --report            # plain-text summary, no TUI
rsty -m 1G               # only call files ≥ 1 GiB "large"
```

## What it finds

| Tab | What |
|---|---|
| **Rust** | every `target/` dir (with project name, profiles, last build time), `~/.cargo/registry/{cache,src,index}`, `~/.cargo/git`, rustup toolchains (the default one is flagged), sccache |
| **Dev junk** | `node_modules`, Python venvs/`__pycache__`/tool caches, `.next`/`.turbo`/`.svelte-kit`, Gradle, Swift `.build`, Zig, Elixir, Haskell |
| **Caches** | `~/Library/Caches/*`, `~/.cache/*`, Xcode DerivedData, npm/bun/pnpm/yarn, Go modcache, Maven, Trash; on Windows: `%TEMP%`, npm/pip/yarn/NuGet/Go caches, INetCache, crash dumps |
| **Large files** | any single file above `--min-size` (default 100 MiB), labelled (disk image, video, model…) |
| **Big folders** | two levels deep under each scan root |

## Keys

`←→`/`1-5` tabs · `↑↓` move · `space` mark · `a` mark all · `m` mark stale (30+ days) · `u` unmark ·
`d` delete (asks first: `enter` = permanent, `t` = Trash) · `o` reveal in Finder/Explorer · `s` sort · `r` rescan · `?` help · `q` quit

## Safety

- Never follows symlinks; refuses to delete `/`, your home, its parents, or a scan root.
- The confirm dialog warns you when anything selected isn't regenerable (large files, folders, Trash) or is your default toolchain.
- Skips iCloud/CloudStorage and other apps' containers, so the scan doesn't trigger downloads or privacy prompts.
- Handles read-only trees like the Go module cache.

## Building a release

Push a tag like `v0.1.0`. GitHub Actions builds binaries for 6 targets and attaches them to the release.

## License

MIT
