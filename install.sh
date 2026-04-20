#!/bin/sh
# FoldDB installer shim.
#
# The curl|sh install path is deprecated in favour of Homebrew on macOS + Linux
# x86_64 and `cargo install` everywhere else. This script no longer downloads
# or installs anything — it prints the supported install command for the
# current platform and exits.
#
# Usage: curl -fsSL https://raw.githubusercontent.com/EdgeVector/fold_db_node/main/install.sh | sh

OS="$(uname -s 2>/dev/null || echo unknown)"
ARCH="$(uname -m 2>/dev/null || echo unknown)"

case "$OS-$ARCH" in
  Darwin-arm64|Darwin-aarch64|Darwin-x86_64|Darwin-amd64|Linux-x86_64|Linux-amd64)
    cat <<'EOF'
FoldDB is distributed via Homebrew on this platform.

    brew install edgevector/folddb/folddb

After installing:

    folddb daemon start         # start the daemon
    open http://localhost:9101  # open the dashboard

Documentation: https://github.com/EdgeVector/fold_db_node#readme
EOF
    exit 0
    ;;
  Linux-aarch64|Linux-arm64)
    cat <<'EOF'
No pre-built tarball is published for Linux arm64 yet. Build from source:

    cargo install --git https://github.com/EdgeVector/fold_db_node --bins

Or track progress on a native arm64 tap at:

    https://github.com/EdgeVector/homebrew-folddb/issues
EOF
    exit 0
    ;;
  *)
    cat <<EOF
FoldDB has no pre-built binary for ${OS}-${ARCH}. Build from source:

    cargo install --git https://github.com/EdgeVector/fold_db_node --bins

Supported targets: macOS (arm64/x86_64), Linux x86_64 via Homebrew.
EOF
    exit 0
    ;;
esac
