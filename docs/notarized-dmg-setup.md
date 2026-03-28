# Notarized macOS DMG Build Pipeline

## Overview

The `release-dmg.yml` workflow builds signed, Apple-notarized DMG installers for both Intel (x86_64) and Apple Silicon (aarch64) Macs. It runs on tag push (`v*`) or manual dispatch.

## Required GitHub Secrets

Configure these in **Settings > Secrets and variables > Actions**:

### Code Signing

| Secret | Description |
|--------|-------------|
| `APPLE_CERTIFICATE` | Base64-encoded `.p12` Developer ID Application certificate |
| `APPLE_CERTIFICATE_PASSWORD` | Password for the `.p12` file |
| `APPLE_SIGNING_IDENTITY` | Full identity string, e.g. `Developer ID Application: Your Name (TEAMID)` |

### Notarization

| Secret | Description |
|--------|-------------|
| `APPLE_ID` | Apple ID email used for notarization |
| `APPLE_PASSWORD` | App-specific password (NOT your Apple ID password) |
| `APPLE_TEAM_ID` | 10-character Apple Developer Team ID |

### Repository Access

| Secret | Description |
|--------|-------------|
| `PRIVATE_DEPS_TOKEN` | GitHub PAT with read access to `fold_db` repo (already configured) |

## Apple Developer Setup

### 1. Export Developer ID Certificate

You need a **Developer ID Application** certificate (not Mac App Store).

```bash
# In Keychain Access:
# 1. Find "Developer ID Application: ..." in login keychain
# 2. Right-click > Export Items... > Save as .p12 with a password
# 3. Base64-encode it:
base64 -i DeveloperIDApplication.p12 | pbcopy
# Paste as APPLE_CERTIFICATE secret
```

If you don't have one yet:
1. Go to https://developer.apple.com/account/resources/certificates/list
2. Click "+" > "Developer ID Application"
3. Follow the CSR creation steps
4. Download and install in Keychain Access
5. Export as .p12

### 2. Create App-Specific Password

1. Go to https://appleid.apple.com/account/manage
2. Sign In & Security > App-Specific Passwords
3. Generate a new password, label it "GitHub Actions Notarization"
4. Use this as the `APPLE_PASSWORD` secret

### 3. Find Your Team ID

```bash
# If you have Xcode installed:
xcrun altool --list-providers -u "your@apple.id" -p "app-specific-password"
```

Or find it at https://developer.apple.com/account > Membership Details.

## How It Works

1. **Certificate import** — Decodes the .p12 from secrets, creates a temporary keychain, imports the cert
2. **Frontend build** — `npm ci && npm run build` in the React app
3. **Tauri build** — `npx tauri build --target <arch> --bundles dmg` with signing env vars
4. **Notarization** — Tauri 2.x automatically submits to Apple's notarization service and staples the ticket when `APPLE_ID`, `APPLE_PASSWORD`, and `APPLE_TEAM_ID` are set
5. **Verification** — Runs `spctl`, `stapler validate`, and `codesign --verify` to confirm
6. **Release** — Uploads both DMGs + SHA256 checksums to a GitHub Release

## Triggering a Build

### On tag push (automatic)
```bash
git tag v0.2.4
git push origin v0.2.4
```

### Manual dispatch
Go to Actions > "Release Notarized DMG" > Run workflow. Optionally specify a tag.

## Verifying on a Clean Mac

After downloading the DMG from the GitHub Release:

```bash
# Should show "accepted" with "notarized" source
spctl --assess --type open --context context:primary-signature -vvv FoldDB_0.2.3_arm64.dmg

# Should show "The validate action worked!"
stapler validate FoldDB_0.2.3_arm64.dmg
```

Double-clicking the DMG on a clean macOS install should open without Gatekeeper warnings.

## Tauri Configuration

The signing config lives in `src/server/static-react/src-tauri/tauri.conf.json`:
- `signingIdentity: "-"` — uses ad-hoc signing locally; CI overrides via `APPLE_SIGNING_IDENTITY` env var
- `entitlements: "Entitlements.plist"` — hardened runtime entitlements (network, file access, unsigned memory)

## Troubleshooting

**"No identity found"** — Certificate wasn't imported correctly. Check `APPLE_CERTIFICATE` is valid base64 of a .p12 file.

**Notarization fails with "invalid credentials"** — Verify `APPLE_ID` and `APPLE_PASSWORD` (must be an app-specific password, not your account password).

**Notarization fails with "not signed with Developer ID"** — `APPLE_SIGNING_IDENTITY` must match exactly. Run `security find-identity -v -p codesigning` to see available identities.

**DMG opens with Gatekeeper warning** — Notarization or stapling failed. Check the "Verify notarization staple" step in the workflow logs.
