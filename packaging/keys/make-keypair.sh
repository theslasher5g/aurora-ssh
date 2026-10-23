#!/usr/bin/env bash
# Creates the Ed25519 release-signing key pair (needs OpenSSL 1.1.1+ / 3.x).
#   packaging/keys/make-keypair.sh [out-dir]
# Writes <out-dir>/aurora-release.key (PRIVATE - never commit, never share) and prints
#   * the line for assets/release-signing-key.txt (public key, base64 of 32 raw bytes)
#   * what to paste into the GitHub secret RELEASE_SIGNING_KEY.
set -euo pipefail
OUT="${1:-.}"
mkdir -p "$OUT"
KEY="$OUT/aurora-release.key"
[ -e "$KEY" ] && { echo "$KEY exists - refusing to overwrite" >&2; exit 1; }
umask 077
openssl genpkey -algorithm ed25519 -out "$KEY"
PUB="$(openssl pkey -in "$KEY" -pubout -outform DER | tail -c 32 | base64 | tr -d '\n')"
echo "Private key written to: $KEY  (mode 600)"
echo
echo "1) Public key - put this line into assets/release-signing-key.txt and commit it:"
echo "$PUB"
echo
echo "2) GitHub -> Settings -> Secrets and variables -> Actions -> New repository secret"
echo "   Name: RELEASE_SIGNING_KEY   Value: the whole content of $KEY (including the BEGIN/END lines)"
echo
echo "3) Keep a backup of $KEY in your password manager, then delete it from this machine."
