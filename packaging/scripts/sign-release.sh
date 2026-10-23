#!/usr/bin/env bash
# Release-workflow helper: signs <dir>/SHA256SUMS.txt with the Ed25519 key in $RELEASE_SIGNING_KEY
# (PEM text) and writes <dir>/SHA256SUMS.txt.sig (64 raw bytes). Does nothing (exit 0) without the
# secret, so unsigned releases keep working. With the secret set, any failure is fatal and the
# signature is verified again before it is published.
set -euo pipefail
DIR="${1:?release directory}"
if [ -z "${RELEASE_SIGNING_KEY:-}" ]; then
  echo "RELEASE_SIGNING_KEY not set - SHA256SUMS.txt stays unsigned"
  exit 0
fi
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
umask 077
printf '%s\n' "$RELEASE_SIGNING_KEY" > "$TMP/key.pem"
openssl pkeyutl -sign -inkey "$TMP/key.pem" -rawin -in "$DIR/SHA256SUMS.txt" -out "$DIR/SHA256SUMS.txt.sig"
openssl pkey -in "$TMP/key.pem" -pubout -out "$TMP/pub.pem"
openssl pkeyutl -verify -pubin -inkey "$TMP/pub.pem" -rawin -in "$DIR/SHA256SUMS.txt" -sigfile "$DIR/SHA256SUMS.txt.sig"
# Refuse to publish a signature the app would reject: the secret's public key must be the embedded one.
PUB="$(openssl pkey -in "$TMP/key.pem" -pubout -outform DER | tail -c 32 | base64 | tr -d '\n')"
EMBEDDED="$(grep -v '^[[:space:]]*#' "$(dirname "$0")/../../assets/release-signing-key.txt" | grep -v '^[[:space:]]*$' | head -1 | tr -d '[:space:]' || true)"
if [ "$PUB" != "$EMBEDDED" ]; then
  echo "::error::RELEASE_SIGNING_KEY does not match assets/release-signing-key.txt (embedded: '${EMBEDDED:-none}', secret: '$PUB')"
  exit 1
fi
chmod 644 "$DIR/SHA256SUMS.txt.sig"
echo "SHA256SUMS.txt signed ($(wc -c < "$DIR/SHA256SUMS.txt.sig") bytes, key $PUB)"
