#!/usr/bin/env sh
# Dociler Installer for macOS and Linux
# https://github.com/deanyouknow/Dociler
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/deanyouknow/Dociler/main/install.sh | sh
#
# Options (via flags or environment variables):
#   -v, --version <VER>    Specify version to install (default: latest release or DOCILER_VERSION)
#   -d, --dir <DIR>        Installation directory (default: ~/.local/bin or DOCILER_INSTALL_DIR)
#   -t, --target <TARGET>  Platform target override (e.g. linux-x86_64, macos-arm64)
#   -f, --file <PATH>      Install from local archive or binary directly
#   -c, --checksum <HASH>  Verify expected SHA-256 hash of the archive
#   --dry-run              Display installation plan without downloading or modifying files
#   -h, --help             Show this help message

set -eu

DEFAULT_REPO="deanyouknow/Dociler"
REPO="${DOCILER_REPO:-$DEFAULT_REPO}"
VERSION="${DOCILER_VERSION:-}"
INSTALL_DIR="${DOCILER_INSTALL_DIR:-}"
TARGET="${DOCILER_TARGET:-}"
LOCAL_FILE="${DOCILER_FILE:-}"
EXPECTED_CHECKSUM="${DOCILER_CHECKSUM:-}"
DRY_RUN=0

print_help() {
  cat <<'EOF'
Dociler Installer for macOS and Linux

Usage:
  install.sh [options]

Options:
  -v, --version <VER>     Version tag to install (e.g., v0.1.0)
  -d, --dir <DIR>         Installation directory (default: ~/.local/bin)
  -t, --target <TARGET>   Target platform (linux-x86_64, linux-aarch64, macos-arm64, macos-x86_64)
  -f, --file <PATH>       Install directly from local archive or binary
  -c, --checksum <HASH>   Verify SHA-256 checksum of archive
  --dry-run               Show detected platform and target path without making changes
  -h, --help              Show this help message

Environment variables:
  DOCILER_VERSION         Version to install
  DOCILER_INSTALL_DIR     Destination directory for the dociler binary
  DOCILER_REPO            GitHub repository (default: deanyouknow/Dociler)
  DOCILER_TARGET          Target platform override
  DOCILER_FILE            Local archive path override
  DOCILER_CHECKSUM        Expected SHA-256 checksum

Safety note:
  Dociler is a standalone, local-first application. This installer copies a single
  binary to your user directory. It does NOT install Docker, Python, Ollama, model
  weights, background daemons, or system-wide services.
EOF
}

# Parse command-line flags
while [ $# -gt 0 ]; do
  case "$1" in
    -v|--version)
      VERSION="$2"
      shift 2
      ;;
    -d|--dir)
      INSTALL_DIR="$2"
      shift 2
      ;;
    -t|--target)
      TARGET="$2"
      shift 2
      ;;
    -f|--file)
      LOCAL_FILE="$2"
      shift 2
      ;;
    -c|--checksum)
      EXPECTED_CHECKSUM="$2"
      shift 2
      ;;
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    -h|--help)
      print_help
      exit 0
      ;;
    *)
      echo "Error: Unknown argument: $1" >&2
      echo "Run with --help for usage." >&2
      exit 1
      ;;
  esac
done

# Detect operating system and architecture if target not explicitly provided
if [ -z "$TARGET" ]; then
  OS="$(uname -s)"
  ARCH="$(uname -m)"

  case "$OS" in
    Linux)
      case "$ARCH" in
        x86_64|amd64)
          TARGET="linux-x86_64"
          ;;
        aarch64|arm64)
          TARGET="linux-aarch64"
          ;;
        *)
          echo "Error: Unsupported architecture '$ARCH' on Linux." >&2
          echo "Dociler v1 supports Linux x86_64 and aarch64." >&2
          exit 1
          ;;
      esac
      ;;
    Darwin)
      case "$ARCH" in
        arm64|aarch64)
          TARGET="macos-arm64"
          ;;
        x86_64|amd64)
          TARGET="macos-x86_64"
          ;;
        *)
          echo "Error: Unsupported architecture '$ARCH' on macOS." >&2
          echo "Dociler v1 supports Apple Silicon (arm64) and Intel (x86_64)." >&2
          exit 1
          ;;
      esac
      ;;
    CYGWIN*|MINGW*|MSYS*)
      echo "Error: Windows shell detected. Please use install.ps1 in PowerShell instead:" >&2
      echo "  irm https://raw.githubusercontent.com/$REPO/main/install.ps1 | iex" >&2
      exit 1
      ;;
    *)
      echo "Error: Unsupported operating system '$OS'." >&2
      echo "Dociler v1 supports Linux, macOS, and Windows." >&2
      exit 1
      ;;
  esac
fi

# Determine default user-local install directory
if [ -z "$INSTALL_DIR" ]; then
  if [ -n "${XDG_BIN_HOME:-}" ]; then
    INSTALL_DIR="$XDG_BIN_HOME"
  else
    INSTALL_DIR="$HOME/.local/bin"
  fi
fi

# Default version fallback if unset
if [ -z "$VERSION" ]; then
  VERSION="v0.1.0-dev"
fi

# Ensure version has 'v' prefix for release tag URL formatting
case "$VERSION" in
  v*) ;;
  *) VERSION="v$VERSION" ;;
esac

echo "Dociler Installer"
echo "================="
echo "Target Platform : $TARGET"
echo "Version         : $VERSION"
echo "Install Dir     : $INSTALL_DIR"

if [ "$DRY_RUN" -eq 1 ]; then
  echo ""
  echo "Dry run enabled. No files will be downloaded or modified."
  exit 0
fi

# Hash calculation helper
calculate_sha256() {
  file_to_hash="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$file_to_hash" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$file_to_hash" | awk '{print $1}'
  elif command -v openssl >/dev/null 2>&1; then
    openssl dgst -sha256 "$file_to_hash" | awk '{print $NF}'
  else
    echo "Error: No SHA-256 utility found (sha256sum, shasum, or openssl required)." >&2
    exit 1
  fi
}

# Download helper
download() {
  url="$1"
  dest="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --proto '=https' --tlsv1.2 -o "$dest" "$url"
  elif command -v wget >/dev/null 2>&1; then
    wget -q --https-only -O "$dest" "$url"
  else
    echo "Error: Neither curl nor wget was found. Please install curl or wget." >&2
    exit 1
  fi
}

# Create staging temporary directory with restricted permissions
TMP_DIR="$(mktemp -d 2>/dev/null || mktemp -d -t 'dociler-install')"
chmod 0700 "$TMP_DIR"
cleanup() {
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT INT TERM

# Ensure user-local install directory exists
mkdir -p "$INSTALL_DIR"

ARCHIVE_NAME="dociler-${VERSION}-${TARGET}.tar.gz"
STAGING_ARCHIVE="$TMP_DIR/$ARCHIVE_NAME"

if [ -n "$LOCAL_FILE" ]; then
  echo "Using local file: $LOCAL_FILE"
  if [ -d "$LOCAL_FILE" ]; then
    echo "Error: Local file path is a directory: $LOCAL_FILE" >&2
    exit 1
  fi
  case "$LOCAL_FILE" in
    *.tar.gz|*.tgz)
      cp "$LOCAL_FILE" "$STAGING_ARCHIVE"
      ;;
    *)
      # Assume raw executable binary
      cp "$LOCAL_FILE" "$TMP_DIR/dociler"
      chmod 0755 "$TMP_DIR/dociler"
      ;;
  esac
else
  RELEASE_BASE_URL="https://github.com/$REPO/releases/download/$VERSION"
  ARCHIVE_URL="$RELEASE_BASE_URL/$ARCHIVE_NAME"
  CHECKSUM_URL="$RELEASE_BASE_URL/SHA256SUMS"

  echo "Downloading $ARCHIVE_NAME..."
  download "$ARCHIVE_URL" "$STAGING_ARCHIVE"

  # If explicit checksum was not provided via flag/env, attempt downloading SHA256SUMS
  if [ -z "$EXPECTED_CHECKSUM" ]; then
    CHECKSUM_FILE="$TMP_DIR/SHA256SUMS"
    if download "$CHECKSUM_URL" "$CHECKSUM_FILE" 2>/dev/null; then
      EXPECTED_CHECKSUM="$(grep "$ARCHIVE_NAME" "$CHECKSUM_FILE" 2>/dev/null | awk '{print $1}' || true)"
    fi
  fi
fi

# Verify checksum if archive is present and checksum is known
if [ -f "$STAGING_ARCHIVE" ]; then
  ACTUAL_CHECKSUM="$(calculate_sha256 "$STAGING_ARCHIVE")"
  if [ -n "$EXPECTED_CHECKSUM" ]; then
    echo "Verifying SHA-256 checksum..."
    # Normalize to lowercase
    EXP_LOWER="$(echo "$EXPECTED_CHECKSUM" | tr '[:upper:]' '[:lower:]')"
    ACT_LOWER="$(echo "$ACTUAL_CHECKSUM" | tr '[:upper:]' '[:lower:]')"
    if [ "$EXP_LOWER" != "$ACT_LOWER" ]; then
      echo "Error: Checksum verification failed!" >&2
      echo "  Expected: $EXP_LOWER" >&2
      echo "  Actual  : $ACT_LOWER" >&2
      exit 1
    fi
    echo "Checksum verified: $ACT_LOWER"
  fi

  # Extract binary from archive
  tar -xzf "$STAGING_ARCHIVE" -C "$TMP_DIR"
fi

if [ ! -f "$TMP_DIR/dociler" ]; then
  echo "Error: Failed to find 'dociler' executable in extracted archive." >&2
  exit 1
fi

chmod 0755 "$TMP_DIR/dociler"

# Atomically install to target destination
DEST_BINARY="$INSTALL_DIR/dociler"
mv "$TMP_DIR/dociler" "$DEST_BINARY"

echo ""
echo "Installation complete!"
echo "Installed executable: $DEST_BINARY"
echo ""
echo "Note: Dociler is a standalone, local-first tool."
echo "No Docker, Python, Ollama, model weights, or system services were installed."
echo ""

# Check whether INSTALL_DIR is in user's PATH
PATH_FOUND=0
case ":$PATH:" in
  *":$INSTALL_DIR:"*) PATH_FOUND=1 ;;
  *) PATH_FOUND=0 ;;
esac

if [ "$PATH_FOUND" -eq 0 ]; then
  echo "Notice: '$INSTALL_DIR' is not currently in your PATH."
  echo "To use 'dociler' from any terminal, add the directory to your shell profile:"
  echo ""
  echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
  echo ""
  echo "For example, append it to ~/.bashrc or ~/.zshrc:"
  echo "  echo 'export PATH=\"$INSTALL_DIR:\$PATH\"' >> ~/.bashrc"
else
  echo "Run 'dociler --version' or 'dociler doctor' to verify your installation."
fi
