#!/bin/bash

# Script to build the FoldDB macOS native app
# Supports both ad-hoc (local dev) and Developer ID (release) signing.
#
# Usage:
#   ./build_macos_app.sh                  # Ad-hoc signed (local dev)
#   ./build_macos_app.sh --sign           # Developer ID signed + notarized
#
# Environment variables for --sign mode:
#   APPLE_SIGNING_IDENTITY  - e.g. "Developer ID Application: Edge Vector Foundation (TEAMID)"
#   APPLE_ID                - Apple ID email for notarization
#   APPLE_PASSWORD          - App-specific password (or @keychain:notarytool)
#   APPLE_TEAM_ID           - 10-char Apple Developer Team ID

set -e

SIGN_MODE="adhoc"
if [[ "$1" == "--sign" ]]; then
    SIGN_MODE="release"
fi

echo "Building FoldDB macOS Native App"
echo "======================================"
echo "Mode: $SIGN_MODE"

# --- Prerequisites ---

if ! command -v cargo &> /dev/null; then
    echo "Error: Rust toolchain not found. Install from https://rustup.rs/"
    exit 1
fi

if ! command -v node &> /dev/null; then
    echo "Error: Node.js not found. Install from https://nodejs.org/"
    exit 1
fi

if [[ "$SIGN_MODE" == "release" ]]; then
    if [[ -z "$APPLE_SIGNING_IDENTITY" ]]; then
        echo "Error: APPLE_SIGNING_IDENTITY is required for --sign mode"
        echo "  Example: \"Developer ID Application: Edge Vector Foundation (TEAMID)\""
        exit 1
    fi

    # Verify the signing identity exists in the keychain
    if ! security find-identity -v -p codesigning | grep -q "$APPLE_SIGNING_IDENTITY"; then
        echo "Error: Signing identity not found in keychain: $APPLE_SIGNING_IDENTITY"
        echo "  Run: security find-identity -v -p codesigning"
        exit 1
    fi
    echo "Signing identity: $APPLE_SIGNING_IDENTITY"
fi

# --- Build ---

cd src/server/static-react

rm -rf dist
rm -rf src-tauri/target/release/bundle

echo "Installing dependencies..."
npm ci

echo "Building frontend..."
npm run build

echo "Building Tauri app (this may take a while)..."

if [[ "$SIGN_MODE" == "release" ]]; then
    # Tauri reads APPLE_SIGNING_IDENTITY to override tauri.conf.json signingIdentity
    export APPLE_SIGNING_IDENTITY
    npm run tauri:build
else
    # Ad-hoc signing (default in tauri.conf.json: signingIdentity = "-")
    npm run tauri:build
fi

BUNDLE_DIR="src-tauri/target/release/bundle"
APP_PATH="$BUNDLE_DIR/macos/FoldDB.app"
DMG_PATH="$BUNDLE_DIR/dmg/FoldDB_$(grep '"version"' src-tauri/tauri.conf.json | head -1 | sed 's/.*: "//;s/".*//')_aarch64.dmg"

# --- Verify signing ---

echo ""
echo "Verifying code signature..."
codesign --verify --deep --strict --verbose=2 "$APP_PATH" 2>&1 || true

if [[ "$SIGN_MODE" == "release" ]]; then
    echo ""
    echo "Checking Gatekeeper assessment..."
    spctl --assess --type exec --verbose=2 "$APP_PATH" 2>&1 || echo "(Gatekeeper assessment may fail until notarized)"
fi

# --- Notarization ---

if [[ "$SIGN_MODE" == "release" ]]; then
    if [[ -n "$APPLE_ID" && -n "$APPLE_PASSWORD" && -n "$APPLE_TEAM_ID" ]]; then
        echo ""
        echo "Submitting for notarization..."

        # Notarize the DMG (contains the signed .app)
        if [[ -f "$DMG_PATH" ]]; then
            xcrun notarytool submit "$DMG_PATH" \
                --apple-id "$APPLE_ID" \
                --password "$APPLE_PASSWORD" \
                --team-id "$APPLE_TEAM_ID" \
                --wait

            echo "Stapling notarization ticket..."
            xcrun stapler staple "$DMG_PATH"
        else
            echo "Warning: DMG not found at $DMG_PATH, notarizing .app directly"
            # Create a zip for notarization
            ZIP_PATH="$BUNDLE_DIR/FoldDB-notarize.zip"
            ditto -c -k --keepParent "$APP_PATH" "$ZIP_PATH"
            xcrun notarytool submit "$ZIP_PATH" \
                --apple-id "$APPLE_ID" \
                --password "$APPLE_PASSWORD" \
                --team-id "$APPLE_TEAM_ID" \
                --wait
            xcrun stapler staple "$APP_PATH"
            rm "$ZIP_PATH"
        fi

        echo ""
        echo "Notarization complete."
    else
        echo ""
        echo "Skipping notarization (set APPLE_ID, APPLE_PASSWORD, APPLE_TEAM_ID to enable)"
    fi
fi

# --- Done ---

echo ""
echo "Build complete!"
echo ""
echo "App:  $APP_PATH"
if [[ -f "$DMG_PATH" ]]; then
    echo "DMG:  $DMG_PATH"
fi
echo ""
if [[ "$SIGN_MODE" == "adhoc" ]]; then
    echo "Note: This build is ad-hoc signed (local dev only)."
    echo "      Users will need to right-click > Open to bypass Gatekeeper."
    echo "      For distribution, rebuild with: ./build_macos_app.sh --sign"
fi
