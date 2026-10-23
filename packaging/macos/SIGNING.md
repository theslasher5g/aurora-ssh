# macOS: Developer ID signing and notarization

Without any secret `packaging/macos/build.sh` ad-hoc signs the app (it runs, but Gatekeeper warns on first
start: right-click → *Open*). With the secrets below the workflow produces a **Developer ID signed, notarized
and stapled** `.dmg` that opens without any warning, also offline.

## Requirements and cost

* **Apple Developer Program**, 99 USD per year (individual or organization). There is no free way to get a
  Developer ID certificate or to notarize.
* A Mac (or any Keychain Access) once, to create and export the certificate.

## 1. Create and export the certificate

1. developer.apple.com → *Certificates, Identifiers & Profiles* → *Certificates* → **+** →
   **Developer ID Application**. Upload a certificate signing request (Keychain Access → *Certificate
   Assistant → Request a Certificate from a Certificate Authority*, saved to disk). Download the `.cer` and
   double-click it to add it to the login keychain.
2. Keychain Access → *My Certificates* → right-click "Developer ID Application: Your Name (TEAMID)" →
   **Export…** → format **Personal Information Exchange (.p12)**, choose a strong password. (Export the
   *certificate with its private key*; it must show the small arrow with the key below it.)
3. Base64 encode it for the secret: `base64 -i DeveloperID.p12 | pbcopy`.

## 2. Notarization credentials (pick one)

**A. App Store Connect API key (recommended, no personal password in CI)**
appstoreconnect.apple.com → *Users and Access → Integrations → Keys* → generate a key with role *Developer*.
Download `AuthKey_XXXXXX.p8` (only once), note *Key ID* and *Issuer ID*.

**B. Apple ID with app-specific password**
appleid.apple.com → *Sign-In and Security → App-Specific Passwords*. Needs the Apple ID e-mail and the 10-character
Team ID (developer.apple.com → *Membership details*).

## 3. Repository secrets (Settings → Secrets and variables → Actions)

| Secret | Value | Needed for |
|---|---|---|
| `MACOS_CERT_P12` | base64 of the `.p12` | signing |
| `MACOS_CERT_PASSWORD` | the `.p12` password | signing |
| `MACOS_SIGN_IDENTITY` | optional: certificate name or SHA-1 if the .p12 holds several | signing |
| `MACOS_NOTARY_KEY_ID` | API key id | notarization (A) |
| `MACOS_NOTARY_ISSUER` | issuer UUID | notarization (A) |
| `MACOS_NOTARY_KEY` | full text of `AuthKey_XXXXXX.p8` | notarization (A) |
| `MACOS_NOTARY_APPLE_ID` | Apple ID e-mail | notarization (B) |
| `MACOS_NOTARY_TEAM_ID` | Team ID | notarization (B) |
| `MACOS_NOTARY_PASSWORD` | app-specific password | notarization (B) |

* Only `MACOS_CERT_P12` (+ password) → signed with Developer ID, **not** notarized.
* Plus one complete notarization set (A or B) → signed, notarized and stapled.
* None → ad-hoc signed as before.

If signing or notarization is configured and fails, the macOS job fails (the Windows/Linux release is still
published; the macOS job is non-blocking). The rejection log of the notary service is printed in the job log.

## What the script does

1. Builds the universal bundle (`aurora`, `aurora-rdp`, `aurora-web` in `Contents/MacOS`).
2. Imports the certificate into a temporary keychain (deleted at the end of the script).
3. Signs **inside-out** with `codesign --force --options runtime --timestamp --entitlements
   packaging/macos/entitlements.plist`: first the helpers `aurora-rdp` and `aurora-web`, then `aurora`, then
   the bundle. No `--deep`. Hardened Runtime and a secure timestamp are required for notarization. The
   entitlements file is deliberately empty (no JIT, no sandbox); add keys only if a notarized build proves
   it needs them.
4. `xcrun notarytool submit … --wait` for a zip of the app, `xcrun stapler staple Aurora.app`.
5. Builds the `.dmg`, signs it, notarizes it and staples the ticket (`stapler staple` + `validate`).

## Verify on a Mac

```bash
codesign --verify --deep --strict --verbose=2 /Applications/Aurora.app
spctl --assess --type execute -vv /Applications/Aurora.app     # accepted, source=Notarized Developer ID
xcrun stapler validate Aurora-<version>-macos-universal.dmg
```

## Not tested

This was written without an Apple certificate or Apple account, so the signing and notarization branch of
`build.sh` has only been syntax-checked (`bash -n`); the ad-hoc path is unchanged. Expect to fix small
details (identity selection, entitlements) on the first real run.
