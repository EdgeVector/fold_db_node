# macOS Code Signing & Notarization

This document covers Apple Developer enrollment, certificate generation, local signing,
and CI configuration for distributing the FoldDB macOS app (.dmg).

## Why This Is Required

macOS Gatekeeper blocks unsigned apps downloaded from the internet. Users see
"FoldDB.app cannot be opened because the developer cannot be verified." To distribute
the DMG without this friction, the app must be:

1. **Signed** with a Developer ID Application certificate
2. **Notarized** by Apple (an automated malware scan)
3. **Stapled** so offline verification works

## Step 1: Apple Developer Program Enrollment

1. Go to https://developer.apple.com/programs/
2. Sign in with the Apple ID that will own the certificate
3. Enroll as **Individual / Sole Proprietor** — $99/year
   - No D-U-N-S number required for sole proprietor enrollment
4. Once approved, the Team ID (10-character alphanumeric) appears at
   https://developer.apple.com/account → Membership Details

## Step 2: Generate a Developer ID Application Certificate

### Option A: Via Xcode (recommended for local setup)

1. Open Xcode → Settings → Accounts → select your Apple ID
2. Click "Manage Certificates" → click "+" → "Developer ID Application"
3. Xcode generates the CSR, submits it to Apple, and installs the certificate + private key
   in your login keychain automatically

### Option B: Via Apple Developer Portal (manual)

1. On your Mac, open Keychain Access → Certificate Assistant → Request a Certificate
   From a Certificate Authority
   - User email: your Apple ID email
   - Common Name: "Edge Vector Foundation"
   - CA Email: leave blank
   - Request is: Saved to disk
2. Go to https://developer.apple.com/account/resources/certificates/add
3. Select "Developer ID Application"
4. Upload the CSR file
5. Download the resulting `.cer` file
6. Double-click to install in Keychain Access

### Verify the certificate

```bash
security find-identity -v -p codesigning
```

You should see a line like:
```
1) ABCDEF1234... "Developer ID Application: Edge Vector Foundation (TEAMID)"
```

The full quoted string is your **signing identity**.

## Step 3: Export a .p12 for CI

CI runners don't have your keychain, so the certificate + private key must be exported:

1. Open Keychain Access → My Certificates
2. Find "Developer ID Application: Edge Vector Foundation (TEAMID)"
3. Right-click → Export → save as `developer-id.p12`
4. Set a strong password (you'll need it as a CI secret)
5. Base64-encode it:
   ```bash
   base64 -i developer-id.p12 -o developer-id.p12.b64
   ```

## Step 4: Create an App-Specific Password

Notarization requires an app-specific password (not your regular Apple ID password):

1. Go to https://appleid.apple.com → Sign-In and Security → App-Specific Passwords
2. Generate one, label it "FoldDB Notarization"
3. Save the generated password — you'll add it as a CI secret

## Step 5: Configure GitHub Secrets

Add these secrets to the `fold_db_node` repository at
Settings → Secrets and variables → Actions:

| Secret                      | Value                                                                   |
| --------------------------- | ----------------------------------------------------------------------- |
| `APPLE_CERTIFICATE`         | Contents of `developer-id.p12.b64` (base64-encoded .p12)               |
| `APPLE_CERTIFICATE_PASSWORD`| Password used when exporting the .p12                                   |
| `APPLE_SIGNING_IDENTITY`    | `Developer ID Application: Edge Vector Foundation (TEAMID)`             |
| `APPLE_ID`                  | Apple ID email used for notarization                                    |
| `APPLE_PASSWORD`            | App-specific password from Step 4                                       |
| `APPLE_TEAM_ID`             | 10-character Team ID from developer.apple.com membership                |

## Step 6: Local Signing

### Ad-hoc (default, no Apple Developer account needed)

```bash
./build_macos_app.sh
```

This produces a locally-runnable app. Users must right-click → Open to bypass Gatekeeper.

### Developer ID signed + notarized

```bash
export APPLE_SIGNING_IDENTITY="Developer ID Application: Edge Vector Foundation (TEAMID)"
export APPLE_ID="your@email.com"
export APPLE_PASSWORD="xxxx-xxxx-xxxx-xxxx"
export APPLE_TEAM_ID="XXXXXXXXXX"

./build_macos_app.sh --sign
```

### Verify the result

```bash
# Check signature
codesign --verify --deep --strict --verbose=2 \
  src/server/static-react/src-tauri/target/release/bundle/macos/FoldDB.app

# Check Gatekeeper (only passes after notarization)
spctl --assess --type exec --verbose=2 \
  src/server/static-react/src-tauri/target/release/bundle/macos/FoldDB.app

# Check notarization staple
stapler validate \
  src/server/static-react/src-tauri/target/release/bundle/dmg/*.dmg
```

## Step 7: CI Workflow

The `.github/workflows/tauri-release.yml` workflow runs on tag push (`v*`) or manual dispatch:

1. Imports the .p12 certificate into a temporary keychain on the runner
2. Builds the React frontend + Tauri app for both `aarch64` and `x86_64`
3. Signs with the Developer ID Application certificate
4. Notarizes and staples the DMG
5. Uploads DMGs as release artifacts
6. Cleans up the temporary keychain

## Entitlements

`src-tauri/Entitlements.plist` grants the app:

- **Hardened Runtime** with `allow-unsigned-executable-memory` (required by Tauri's WebView)
- **Network client + server** (embedded HTTP server on localhost:9001)
- **User-selected file access** (for `~/.folddb/` data directory)

## Troubleshooting

### "errSecInternalComponent" during CI signing
The keychain is locked. Ensure `security unlock-keychain` runs before `codesign`.

### "The signature of the binary is invalid" after notarization
The app was modified after signing. Ensure nothing touches the .app bundle between
`codesign` and `notarytool submit`.

### Notarization fails with "The binary uses an SDK older than the 10.9 SDK"
Update `minimumSystemVersion` in `tauri.conf.json`. Currently set to `10.13`.

### "Developer ID Application" certificate not showing up
It can take a few minutes after creation. Restart Keychain Access or run:
```bash
security find-identity -v -p codesigning
```

### Certificate expires
Developer ID Application certificates are valid for 5 years. Set a calendar reminder.
Regenerate following Step 2, re-export the .p12, and update CI secrets.
