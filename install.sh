#!/bin/sh
# FoldDB Installer (fallback for non-brew paths — air-gapped, Linux arm64 builds-from-source)
#
# Prefer Homebrew on macOS and Linux x86_64:
#     brew install edgevector/folddb/folddb
#
# This script downloads the latest tarball from the EdgeVector/fold_db
# release mirror, verifies its sha256 against SHA256SUMS.txt, and installs
# folddb, folddb_server, and schema_service into /usr/local/bin (or
# $HOME/.local/bin as a fallback when /usr/local/bin is not writable).
#
# Usage: curl -fsSL https://raw.githubusercontent.com/EdgeVector/fold_db_node/main/install.sh | sh

set -e

REPO="EdgeVector/fold_db"
BINARIES="folddb folddb_server schema_service"

# Detect OS/arch → Rust target triple.
OS="$(uname -s)"
ARCH="$(uname -m)"
case "$OS-$ARCH" in
  Darwin-arm64|Darwin-aarch64) TARGET="aarch64-apple-darwin" ;;
  Darwin-x86_64|Darwin-amd64)  TARGET="x86_64-apple-darwin" ;;
  Linux-x86_64|Linux-amd64)    TARGET="x86_64-unknown-linux-gnu" ;;
  *)
    echo "Error: no pre-built tarball for ${OS}-${ARCH}."
    echo "Build from source:"
    echo "  cargo install --git https://github.com/EdgeVector/fold_db_node folddb folddb_server schema_service"
    exit 1
    ;;
esac

# Nudge Homebrew users toward the tap (but still proceed on explicit curl invocation).
case "$TARGET" in
  *-apple-darwin|x86_64-unknown-linux-gnu)
    if command -v brew >/dev/null 2>&1; then
      echo "Homebrew detected. The recommended install is:"
      echo "  brew install edgevector/folddb/folddb"
      echo ""
      echo "Continuing with direct tarball install in 3s... (Ctrl-C to abort)"
      sleep 3
    fi
    ;;
esac

echo "Detected target: ${TARGET}"

# Resolve latest release tag.
LATEST_URL="https://api.github.com/repos/${REPO}/releases/latest"
if command -v curl >/dev/null 2>&1; then
  RELEASE_JSON="$(curl -fsSL "$LATEST_URL")"
elif command -v wget >/dev/null 2>&1; then
  RELEASE_JSON="$(wget -qO- "$LATEST_URL")"
else
  echo "Error: curl or wget is required."
  exit 1
fi
TAG="$(echo "$RELEASE_JSON" | grep '"tag_name"' | head -1 | sed 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')"
if [ -z "$TAG" ]; then
  echo "Error: could not determine latest release tag."
  echo "Visit https://github.com/${REPO}/releases to download manually."
  exit 1
fi
VERSION="${TAG#v}"
echo "Latest version: ${VERSION}"

# Choose install dir.
INSTALL_DIR="/usr/local/bin"
NEED_SUDO=false
if [ ! -w "$INSTALL_DIR" ]; then
  if command -v sudo >/dev/null 2>&1; then
    NEED_SUDO=true
  else
    INSTALL_DIR="$HOME/.local/bin"
    mkdir -p "$INSTALL_DIR"
  fi
fi

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

TARBALL="folddb-${TARGET}.tar.gz"
TARBALL_URL="https://github.com/${REPO}/releases/download/${TAG}/${TARBALL}"
CHECKSUMS_URL="https://github.com/${REPO}/releases/download/${TAG}/SHA256SUMS.txt"

echo "Downloading ${TARBALL}..."
if command -v curl >/dev/null 2>&1; then
  curl -fSL --progress-bar -o "${TMP_DIR}/${TARBALL}" "$TARBALL_URL"
else
  wget -q --show-progress -O "${TMP_DIR}/${TARBALL}" "$TARBALL_URL"
fi

# SHA256 verification — releases cut before the checksum rollout may omit this file.
echo "Fetching SHA256SUMS.txt..."
HAS_CHECKSUMS=false
if command -v curl >/dev/null 2>&1; then
  curl -fsSL -o "${TMP_DIR}/SHA256SUMS.txt" "$CHECKSUMS_URL" 2>/dev/null && HAS_CHECKSUMS=true
elif command -v wget >/dev/null 2>&1; then
  wget -q -O "${TMP_DIR}/SHA256SUMS.txt" "$CHECKSUMS_URL" 2>/dev/null && HAS_CHECKSUMS=true
fi

if [ "$HAS_CHECKSUMS" = true ]; then
  EXPECTED="$(grep "  ${TARBALL}\$" "${TMP_DIR}/SHA256SUMS.txt" | awk '{print $1}')"
  if [ -z "$EXPECTED" ]; then
    echo "Error: ${TARBALL} not listed in SHA256SUMS.txt."
    exit 1
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    ACTUAL="$(sha256sum "${TMP_DIR}/${TARBALL}" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    ACTUAL="$(shasum -a 256 "${TMP_DIR}/${TARBALL}" | awk '{print $1}')"
  else
    echo "Error: no sha256sum/shasum available to verify download."
    exit 1
  fi
  if [ "$EXPECTED" != "$ACTUAL" ]; then
    echo "Error: checksum mismatch for ${TARBALL}"
    echo "  expected: $EXPECTED"
    echo "  actual:   $ACTUAL"
    exit 1
  fi
  echo "Checksum verified."
else
  echo "Warning: SHA256SUMS.txt not found for ${TAG} — continuing without verification."
fi

# Extract + install.
echo "Extracting ${TARBALL}..."
tar -xzf "${TMP_DIR}/${TARBALL}" -C "${TMP_DIR}"

for BINARY_NAME in $BINARIES; do
  if [ ! -f "${TMP_DIR}/${BINARY_NAME}" ]; then
    echo "Error: ${BINARY_NAME} not found in tarball."
    exit 1
  fi
  chmod +x "${TMP_DIR}/${BINARY_NAME}"
  if [ "$NEED_SUDO" = true ]; then
    sudo mv "${TMP_DIR}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
  else
    mv "${TMP_DIR}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
  fi
done

# PATH hint if non-default dir.
case ":$PATH:" in
  *":${INSTALL_DIR}:"*) ;;
  *)
    echo ""
    echo "NOTE: ${INSTALL_DIR} is not in your PATH."
    echo "Add it by running:  export PATH=\"${INSTALL_DIR}:\$PATH\""
    ;;
esac

echo ""
echo "FoldDB v${VERSION} installed!"
echo "  Run 'folddb daemon start' to launch the daemon, then visit http://localhost:9101"
echo "  Run 'folddb --help' for the CLI"
