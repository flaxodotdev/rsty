#!/bin/sh
# rsty installer for macOS and Linux.
#   curl -fsSL https://raw.githubusercontent.com/flaxodotdev/rsty/main/install.sh | sh
# Env: RSTY_VERSION (e.g. v0.1.0, default latest), RSTY_INSTALL_DIR (default ~/.local/bin)
set -eu

REPO="flaxodotdev/rsty"
VERSION="${RSTY_VERSION:-latest}"
INSTALL_DIR="${RSTY_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '\033[1;38;5;208m🦀 rsty\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

os=$(uname -s)
arch=$(uname -m)
case "$arch" in
  x86_64 | amd64) arch=x86_64 ;;
  arm64 | aarch64) arch=aarch64 ;;
  *) die "unsupported architecture: $arch (try: cargo install --git https://github.com/$REPO)" ;;
esac
case "$os" in
  Darwin) target="$arch-apple-darwin" ;;
  Linux) target="$arch-unknown-linux-musl" ;;
  MINGW* | MSYS* | CYGWIN*) die "on Windows run: irm https://raw.githubusercontent.com/$REPO/main/install.ps1 | iex" ;;
  *) die "unsupported OS: $os" ;;
esac

if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -q "$1" -O "$2"; }
else
  die "need curl or wget"
fi

if [ "$VERSION" = latest ]; then
  base="https://github.com/$REPO/releases/latest/download"
else
  base="https://github.com/$REPO/releases/download/$VERSION"
fi
file="rsty-$target.tar.gz"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

say "downloading $file ($VERSION)…"
fetch "$base/$file" "$tmp/$file" || die "download failed: $base/$file"

if fetch "$base/$file.sha256" "$tmp/$file.sha256" 2>/dev/null; then
  expected=$(cut -d' ' -f1 <"$tmp/$file.sha256")
  if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$tmp/$file" | cut -d' ' -f1)
  elif command -v shasum >/dev/null 2>&1; then
    actual=$(shasum -a 256 "$tmp/$file" | cut -d' ' -f1)
  else
    actual="$expected"
  fi
  [ "$expected" = "$actual" ] || die "checksum mismatch for $file"
  say "checksum ok"
fi

tar -xzf "$tmp/$file" -C "$tmp"
mkdir -p "$INSTALL_DIR"
install -m 755 "$tmp/rsty-$target/rsty" "$INSTALL_DIR/rsty" 2>/dev/null ||
  { cp "$tmp/rsty-$target/rsty" "$INSTALL_DIR/rsty" && chmod 755 "$INSTALL_DIR/rsty"; }

say "installed to $INSTALL_DIR/rsty"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) say "run: rsty" ;;
  *)
    say "$INSTALL_DIR is not on your PATH. Add this to your shell profile:"
    printf '\n    export PATH="%s:$PATH"\n\n' "$INSTALL_DIR"
    ;;
esac
