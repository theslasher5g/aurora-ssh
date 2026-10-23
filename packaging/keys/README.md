# Release signing keys

Aurora releases can carry a signature over `SHA256SUMS.txt`. The checksum file lists the hash of
every package, so one signature protects all downloads **and** the in-app updater.

Everything here is optional: without the secrets the release workflow publishes unsigned
releases, exactly as before. **Never commit a private key.**

## What is signed and how

| Layer | Needs a secret? | Files in the release | Checked by |
|---|---|---|---|
| **Ed25519 key of the project** (the one the app checks) | `RELEASE_SIGNING_KEY` | `SHA256SUMS.txt.sig` (64 raw bytes) | the app's updater, `packaging/keys/verify.sh`, plain `openssl` |
| **Sigstore / cosign keyless** (extra proof) | none (GitHub OIDC, job permission `id-token: write`) | `SHA256SUMS.txt.cosign.bundle` | `cosign verify-blob` |
| GPG signature of the `.deb` | `DEB_SIGNING_KEY` (+ `DEB_SIGNING_PASSPHRASE`) | `aurora_*.deb.asc`, `aurora-release-gpg.asc` | `gpg --verify` |

Why two schemes for the checksums:

* The **Ed25519 key** can be checked *inside the app* with a few lines of code and no network
  access (the public key is compiled in: `assets/release-signing-key.txt`). It is a long-lived
  secret you must guard; if it leaks, rotate it (see the end of this file).
* **cosign keyless** needs no secret at all. A short-lived certificate from Sigstore's Fulcio CA binds the
  signature to the identity of *this repository's release workflow*, and the signature is logged in the
  public Rekor transparency log. It proves "built by this workflow of this repo", which an attacker who
  merely uploads files to a hijacked release cannot fake. The drawback: verifying it needs the Sigstore
  libraries and network access (trust root, Rekor), too heavy for the app, so it is a proof for people
  and scripts using `cosign`, not enforced by the updater. A Sigstore outage must not stop a release, so that
  workflow step is `continue-on-error`.

## 1. Generate the Ed25519 key pair (once)

```bash
packaging/keys/make-keypair.sh ~/aurora-keys     # needs openssl
```

The script writes `aurora-release.key` (private, mode 600) and prints the public key as one base64 line.

1. Put the printed line into `assets/release-signing-key.txt` (below the comments) and commit it. This key
   is compiled into the next Aurora build.
2. GitHub → *Settings → Secrets and variables → Actions → New repository secret*:
   name **`RELEASE_SIGNING_KEY`**, value = the complete content of `aurora-release.key`
   (including the `-----BEGIN PRIVATE KEY-----` lines).
3. Back up the private key (password manager), then delete it from the machine.

The workflow step `packaging/scripts/sign-release.sh` signs the checksum file, verifies the signature
again and **refuses to publish** if the secret's public key differs from `assets/release-signing-key.txt`
(that would make the app reject the release). Order of events for the first signed release:
commit the public key → add the secret → tag the release. Releases made *before* the app contains the key
still verify only from the version that embeds it onwards; older apps show "not signed".

## 2. Verify a download by hand

```bash
# download SHA256SUMS.txt, SHA256SUMS.txt.sig and your package into one folder, then
packaging/keys/verify.sh ~/Downloads/aurora-release
```

That runs the three plain commands, which you can also type yourself (`pub.pem` is the public key as PEM;
the script builds it from `assets/release-signing-key.txt`):

```bash
openssl pkeyutl -verify -pubin -inkey pub.pem -rawin -in SHA256SUMS.txt -sigfile SHA256SUMS.txt.sig
sha256sum --ignore-missing -c SHA256SUMS.txt          # Linux
shasum -a 256 -c SHA256SUMS.txt                       # macOS (prints errors for files you did not download)
```

Windows (PowerShell): `Get-FileHash .\Aurora-<version>-windows-x64-setup.exe -Algorithm SHA256` and compare with the
line in `SHA256SUMS.txt`.

Take the public key from a trusted place (the repository on GitHub, `assets/release-signing-key.txt`), not from
the release you are checking.

### cosign (keyless)

```bash
cosign verify-blob SHA256SUMS.txt \
  --bundle SHA256SUMS.txt.cosign.bundle \
  --certificate-identity-regexp '^https://github.com/theslasher5g/ssh-client/\.github/workflows/release\.yml@refs/.+' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
```

### .deb (GPG)

```bash
gpg --import aurora-release-gpg.asc
gpg --verify aurora_<version>_amd64.deb.asc aurora_<version>_amd64.deb
```

Compare the printed fingerprint with the one published in the repository / release notes.

## 3. Optional: GPG signature for the .deb

```bash
gpg --quick-gen-key "Aurora Release <you@example.org>" ed25519 sign 3y
gpg --armor --export-secret-keys <KEYID>      # -> secret DEB_SIGNING_KEY
# passphrase (if any)                          # -> secret DEB_SIGNING_PASSPHRASE
```

`packaging/linux/sign-deb.sh` (called by the workflow) then writes a detached `.deb.asc` per package and publishes
the public key as `aurora-release-gpg.asc`. `apt` does not check these detached signatures; they are for manual
verification. A real **apt repository** (a `Release`/`InRelease` file signed with the same GPG key, e.g. with
`reprepro` or `aptly`, hosted on GitHub Pages) would let `apt` verify and update Aurora itself. That is not
implemented.

## Rotating or losing the key

Generate a new pair, replace `assets/release-signing-key.txt`, update the secret, release. Apps that still contain the
old key reject releases signed with the new one (hard fail by design); users then install the new version once from
the download page (and can check it with `verify.sh` against the key in the repository).
