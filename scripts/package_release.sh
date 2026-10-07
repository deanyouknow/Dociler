#!/usr/bin/env bash
# Dociler Release Packaging Script
# Generates release tarball/zip bundles and SHA256SUMS manifest.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$REPO_ROOT"

# Extract version from Cargo.toml
VERSION=$(python3 -c '
with open("Cargo.toml") as f:
    for line in f:
        if line.strip().startswith("version ="):
            print(line.split("=")[1].strip().strip("\"").strip("\x27"))
            break
')

if [ -z "$VERSION" ]; then
  echo "Error: Failed to determine version from Cargo.toml" >&2
  exit 1
fi

# Detect host target
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64|amd64) TARGET="linux-x86_64" ;;
      aarch64|arm64) TARGET="linux-aarch64" ;;
      *) echo "Unsupported Linux architecture $ARCH" >&2; exit 1 ;;
    esac
    ;;
  Darwin)
    case "$ARCH" in
      arm64|aarch64) TARGET="macos-arm64" ;;
      x86_64|amd64) TARGET="macos-x86_64" ;;
      *) echo "Unsupported macOS architecture $ARCH" >&2; exit 1 ;;
    esac
    ;;
  MINGW*|MSYS*|CYGWIN*)
    TARGET="windows-x86_64"
    ;;
  *)
    TARGET="unknown-target"
    ;;
esac

# Allow overriding target via argument
TARGET="${1:-$TARGET}"

echo "Packaging Dociler v${VERSION} for ${TARGET}..."

# Build release binary if not present
if [ ! -f "target/release/dociler" ] && [ ! -f "target/release/dociler.exe" ]; then
  echo "Building release binary..."
  cargo build --workspace --release --locked
fi

DIST_DIR="$REPO_ROOT/target/dist"
STAGING_DIR="$DIST_DIR/staging"
rm -rf "$STAGING_DIR"
mkdir -p "$STAGING_DIR" "$DIST_DIR"

if [[ "$TARGET" == *"windows"* ]]; then
  EXE_NAME="dociler.exe"
  BUNDLE_NAME="dociler-v${VERSION}-${TARGET}.zip"
else
  EXE_NAME="dociler"
  BUNDLE_NAME="dociler-v${VERSION}-${TARGET}.tar.gz"
fi

BINARY_PATH="$REPO_ROOT/target/release/$EXE_NAME"
if [ ! -f "$BINARY_PATH" ]; then
  echo "Error: Binary not found at $BINARY_PATH" >&2
  exit 1
fi

# Copy release files
cp "$BINARY_PATH" "$STAGING_DIR/"
cp "$REPO_ROOT/LICENSE" "$STAGING_DIR/"
cp "$REPO_ROOT/NOTICE" "$STAGING_DIR/"
cp "$REPO_ROOT/README.md" "$STAGING_DIR/"

if [[ "$TARGET" == *"windows"* ]]; then
  (cd "$STAGING_DIR" && zip -q -9 -r "$DIST_DIR/$BUNDLE_NAME" .)
else
  tar -czf "$DIST_DIR/$BUNDLE_NAME" -C "$STAGING_DIR" .
fi

# Generate or update SHA256SUMS
(
  cd "$DIST_DIR"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum dociler-v*.* > SHA256SUMS
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 dociler-v*.* > SHA256SUMS
  fi
)

echo "Release package created: $DIST_DIR/$BUNDLE_NAME"
echo "Checksum manifest: $DIST_DIR/SHA256SUMS"
cat "$DIST_DIR/SHA256SUMS"
