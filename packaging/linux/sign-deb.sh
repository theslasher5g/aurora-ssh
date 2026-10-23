#!/usr/bin/env bash
# Optional: detached, ASCII-armored GPG signature of every .deb in <dir> (aurora_*.deb.asc)
# plus the matching public key (aurora-release-gpg.asc).
#   DEB_SIGNING_KEY         armored private key (gpg --armor --export-secret-keys <id>)
#   DEB_SIGNING_PASSPHRASE  its passphrase (optional, empty = key without passphrase)
# Without DEB_SIGNING_KEY this does nothing (exit 0). Verify:
#   gpg --import aurora-release-gpg.asc && gpg --verify aurora_X_amd64.deb.asc aurora_X_amd64.deb
set -euo pipefail
DIR="${1:?directory with the .deb}"
if [ -z "${DEB_SIGNING_KEY:-}" ]; then
  echo "DEB_SIGNING_KEY not set - .deb stays unsigned"
  exit 0
fi
export GNUPGHOME
GNUPGHOME="$(mktemp -d)"
chmod 700 "$GNUPGHOME"
trap 'gpgconf --kill gpg-agent >/dev/null 2>&1 || true; rm -rf "$GNUPGHOME"' EXIT
printf '%s\n' "$DEB_SIGNING_KEY" | gpg --batch --quiet --import
FPR="$(gpg --batch --list-secret-keys --with-colons | awk -F: '/^fpr:/ {print $10; exit}')"
[ -n "$FPR" ] || { echo "no secret key found in DEB_SIGNING_KEY" >&2; exit 1; }
shopt -s nullglob
debs=("$DIR"/*.deb)
[ ${#debs[@]} -gt 0 ] || { echo "no .deb in $DIR" >&2; exit 1; }
for deb in "${debs[@]}"; do
  printf '%s' "${DEB_SIGNING_PASSPHRASE:-}" | gpg --batch --yes --pinentry-mode loopback --passphrase-fd 0 \
    --local-user "$FPR" --armor --detach-sign --output "$deb.asc" "$deb"
  gpg --batch --verify "$deb.asc" "$deb"
done
gpg --batch --armor --export "$FPR" > "$DIR/aurora-release-gpg.asc"
echo "signed ${#debs[@]} package(s) with key $FPR"
