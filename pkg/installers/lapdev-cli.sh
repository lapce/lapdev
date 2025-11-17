#!/usr/bin/env bash
set -euo pipefail

BASE_URL="${BASE_URL:-}"
PRODUCT="lapdev-cli"
BINARY_NAME="lapdev"
DEFAULT_INSTALL_DIR="/usr/local/bin"
FALLBACK_INSTALL_DIR="${HOME}/.local/bin"

VERSION="latest"
REQUESTED_INSTALL_DIR=""

usage() {
  cat <<'USAGE'
Lapdev CLI installer

Options:
  --version <x.y.z>   Install a specific version (defaults to latest release)
  --install-dir <dir> Install into a custom directory (defaults to /usr/local/bin or ~/.local/bin)
  -h, --help          Show this message

Environment overrides:
  BASE_URL            Override release base URL (defaults to GitHub Releases)
  INSTALL_DIR         Same as --install-dir
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      shift
      VERSION="${1:-}"
      if [[ -z "$VERSION" ]]; then
        echo "Missing value for --version" >&2
        exit 1
      fi
      ;;
    --version=*)
      VERSION="${1#*=}"
      ;;
    --install-dir)
      shift
      REQUESTED_INSTALL_DIR="${1:-}"
      ;;
    --install-dir=*)
      REQUESTED_INSTALL_DIR="${1#*=}"
      ;;
    INSTALL_DIR=*)
      # Allow INSTALL_DIR=value positional style for curl | bash convenience
      REQUESTED_INSTALL_DIR="${1#*=}"
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage
      exit 1
      ;;
  esac
  shift
done

if [[ -n "${INSTALL_DIR:-}" && -z "$REQUESTED_INSTALL_DIR" ]]; then
  REQUESTED_INSTALL_DIR="$INSTALL_DIR"
fi

ensure_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

http_download() {
  local url=$1
  local dest=$2
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" -o "$dest"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$dest" "$url"
  else
    echo "Need curl or wget to download releases" >&2
    exit 1
  fi
}

detect_platform() {
  local kernel machine
  kernel="$(uname -s)"
  machine="$(uname -m)"

  case "$kernel" in
    Linux)
      PLATFORM_OS="linux"
      ;;
    Darwin)
      PLATFORM_OS="macos"
      ;;
    MINGW*|MSYS*|CYGWIN*|Windows*)
      echo "Please use the PowerShell installer: irm https://get.lap.dev/lapdev-cli.ps1 | iex" >&2
      exit 1
      ;;
    *)
      echo "Unsupported operating system: $kernel" >&2
      exit 1
      ;;
  esac
  case "$machine" in
    x86_64|amd64)
      PLATFORM_ARCH="x86_64"
      ;;
    arm64|aarch64)
      PLATFORM_ARCH="aarch64"
      ;;
    *)
      echo "Unsupported architecture: $machine" >&2
      exit 1
      ;;
  esac

  if [[ "${PLATFORM_OS}" == "linux" && -f /proc/version ]]; then
    if grep -qi microsoft /proc/version >/dev/null 2>&1; then
      echo "Detected WSL - installing Linux binary" >&2
    fi
  fi
}

build_target() {
  case "${PLATFORM_OS}:${PLATFORM_ARCH}" in
    linux:x86_64)
      TARGET_TRIPLE="x86_64-unknown-linux-gnu"
      ;;
    linux:aarch64)
      TARGET_TRIPLE="aarch64-unknown-linux-gnu"
      ;;
    macos:x86_64)
      TARGET_TRIPLE="x86_64-apple-darwin"
      ;;
    macos:aarch64)
      TARGET_TRIPLE="aarch64-apple-darwin"
      ;;
    *)
      echo "No supported build for ${PLATFORM_OS}/${PLATFORM_ARCH}" >&2
      exit 1
      ;;
  esac
}

compute_download_url() {
  local asset_name="${PRODUCT}-${TARGET_TRIPLE}.tar.gz"
  if [[ -n "$BASE_URL" ]]; then
    if [[ "$VERSION" == "latest" ]]; then
      DOWNLOAD_URL="${BASE_URL}/releases/latest/${asset_name}"
    else
      local normalized="v${VERSION#v}"
      DOWNLOAD_URL="${BASE_URL}/releases/${normalized}/${asset_name}"
    fi
    return
  fi

  if [[ "$VERSION" == "latest" ]]; then
    DOWNLOAD_URL="https://github.com/lapce/lapdev/releases/latest/download/${asset_name}"
  else
    local normalized="v${VERSION#v}"
    DOWNLOAD_URL="https://github.com/lapce/lapdev/releases/download/${normalized}/${asset_name}"
  fi
}

ensure_install_dir() {
  if [[ -n "$REQUESTED_INSTALL_DIR" ]]; then
    INSTALL_DIR="$REQUESTED_INSTALL_DIR"
  else
    if [[ -d "$DEFAULT_INSTALL_DIR" && -w "$DEFAULT_INSTALL_DIR" ]] || [[ ! -e "$DEFAULT_INSTALL_DIR" && -w "$(dirname "$DEFAULT_INSTALL_DIR")" ]]; then
      INSTALL_DIR="$DEFAULT_INSTALL_DIR"
    else
      INSTALL_DIR="$FALLBACK_INSTALL_DIR"
      mkdir -p "$INSTALL_DIR"
    fi
  fi

  if [[ -d "$INSTALL_DIR" ]]; then
    return
  fi

  if mkdir -p "$INSTALL_DIR" 2>/dev/null; then
    return
  fi

  if command -v sudo >/dev/null 2>&1; then
    sudo mkdir -p "$INSTALL_DIR"
  else
    echo "Cannot create $INSTALL_DIR; re-run with sudo or pick INSTALL_DIR you can write to." >&2
    exit 1
  fi
}

install_binary() {
  local src=$1
  local dst="$INSTALL_DIR/$BINARY_NAME"
  if [[ -w "$INSTALL_DIR" ]]; then
    install -m 0755 "$src" "$dst"
  elif command -v sudo >/dev/null 2>&1; then
    sudo install -m 0755 "$src" "$dst"
  else
    echo "Cannot write to $INSTALL_DIR and sudo is unavailable. Re-run with INSTALL_DIR or sudo." >&2
    exit 1
  fi
}

post_install_message() {
  echo "Lapdev CLI installed to $INSTALL_DIR/$BINARY_NAME"
  if ! command -v lapdev >/dev/null 2>&1; then
    echo "Add $INSTALL_DIR to your PATH to use the 'lapdev' command."
  fi
  "$INSTALL_DIR/$BINARY_NAME" --version || true
}

ensure_cmd uname
ensure_cmd mktemp
ensure_cmd tar
ensure_cmd install

detect_platform
build_target
compute_download_url
ensure_install_dir

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

ARCHIVE_PATH="${TMPDIR}/lapdev-cli.tar.gz"

echo "Downloading Lapdev CLI (${TARGET_TRIPLE}) from ${DOWNLOAD_URL}"
http_download "$DOWNLOAD_URL" "$ARCHIVE_PATH"

tar -xzf "$ARCHIVE_PATH" -C "$TMPDIR"
BIN_PATH="$(find "$TMPDIR" -type f -name "$BINARY_NAME" -perm -u+x | head -n 1 || true)"

if [[ -z "$BIN_PATH" ]]; then
  echo "Failed to locate Lapdev binary in archive" >&2
  exit 1
fi

install_binary "$BIN_PATH"
post_install_message
