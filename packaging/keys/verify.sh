#!/usr/bin/env bash
# Manually verifies a downloaded Aurora release.
#   packaging/keys/verify.sh <dir-with-release-files> [public-key-file]
# <dir> must contain SHA256SUMS.txt, SHA256SUMS.txt.sig and the packages you downloaded.
# The public key defaults to assets/release-signing-key.txt of this checkout; to be sure it
# is the real one, compare it with the key in the repository on GitHub.
set -euo pipefail
DIR="${1:?directory with the downloaded release files}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
KEYFILE="${2:-$ROOT/assets/release-signing-key.txt}"
PUB="$(grep -v '^[[:space:]]*#' "$KEYFILE" | grep -v '^[[:space:]]*$' | head -1 | tr -d '[:space:]' || true)"
[ -n "$PUB" ] || { echo "no public key in $KEYFILE" >&2; exit 2; }
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
# SubjectPublicKeyInfo header for Ed25519 + the 32 raw key bytes
{ printf '\x30\x2a\x30\x05\x06\x03\x2b\x65\x70\x03\x21\x00'; printf '%s' "$PUB" | base64 -d; } > "$TMP/pub.der"
openssl pkey -pubin -inform DER -in "$TMP/pub.der" -out "$TMP/pub.pem"
if [ "$(wc -c < "$DIR/SHA256SUMS.txt.sig")" = 64 ]; then
  cp "$DIR/SHA256SUMS.txt.sig" "$TMP/sig"
else
  base64 -d "$DIR/SHA256SUMS.txt.sig" > "$TMP/sig"
fi
openssl pkeyutl -verify -pubin -inkey "$TMP/pub.pem" -rawin -in "$DIR/SHA256SUMS.txt" -sigfile "$TMP/sig"
echo "Signature of SHA256SUMS.txt: OK"
cd "$DIR"
sha256sum --ignore-missing -c SHA256SUMS.txt
