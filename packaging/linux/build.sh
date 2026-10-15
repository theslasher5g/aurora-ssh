#!/usr/bin/env bash
# Builds Linux release artifacts: a .deb and a portable .tar.gz.
# Usage: packaging/linux/build.sh <version> [out-dir]
set -euo pipefail

VERSION="${1:?version}"
OUT="${2:-dist}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
ARCH="$(dpkg --print-architecture 2>/dev/null || echo amd64)"
MAIN="$ROOT/target/release/aurora-term"
HELPER="$ROOT/crates/aurora-rdp/target/release/aurora-rdp"

[[ -x "$MAIN" && -x "$HELPER" ]] || { echo "Build first: cargo build --release (+ crates/aurora-rdp)"; exit 1; }
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"

# ---- .deb
PKG="$(mktemp -d)"
chmod 755 "$PKG"
install -Dm755 "$MAIN" "$PKG/usr/bin/aurora"
install -Dm755 "$HELPER" "$PKG/usr/lib/aurora-term/aurora-rdp"
install -Dm644 "$ROOT/packaging/linux/aurora.desktop" "$PKG/usr/share/applications/aurora.desktop"
install -Dm644 "$ROOT/assets/icon.png" "$PKG/usr/share/icons/hicolor/256x256/apps/aurora.png"
install -Dm644 "$ROOT/README.md" "$PKG/usr/share/doc/aurora/README.md"
install -Dm644 "$ROOT/SECURITY.md" "$PKG/usr/share/doc/aurora/SECURITY.md"
mkdir -p "$PKG/DEBIAN"
SIZE="$(du -sk "$PKG/usr" | cut -f1)"
cat > "$PKG/DEBIAN/control" <<CONTROL
Package: aurora
Version: $VERSION
Section: net
Priority: optional
Architecture: $ARCH
Installed-Size: $SIZE
Depends: libc6 (>= 2.35), libxkbcommon-x11-0, libgl1 | libegl1
Suggests: freerdp3-x11 | freerdp2-x11
Maintainer: Aurora <noreply@users.noreply.github.com>
Description: Modern SSH, SFTP, RDP and serial client
 Aurora combines an SSH terminal, SFTP file browser, embedded RDP and serial
 consoles with an encrypted keychain, snippets, tunnels and host groups.
CONTROL
dpkg-deb --root-owner-group --build "$PKG" "$OUT/aurora_${VERSION}_${ARCH}.deb" >/dev/null
rm -rf "$PKG"

# ---- portable .tar.gz
NAME="aurora-${VERSION}-linux-x86_64"
TAR="$(mktemp -d)/$NAME"
mkdir -p "$TAR"
install -m755 "$MAIN" "$TAR/aurora"
install -m755 "$HELPER" "$TAR/aurora-rdp"
install -m644 "$ROOT/packaging/linux/aurora.desktop" "$ROOT/assets/icon.png" "$ROOT/README.md" "$TAR/"
cat > "$TAR/install.sh" <<'INSTALL'
#!/usr/bin/env bash
# Installs Aurora for the current user (~/.local).
set -euo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"
install -Dm755 "$DIR/aurora" "$HOME/.local/bin/aurora"
install -Dm755 "$DIR/aurora-rdp" "$HOME/.local/bin/aurora-rdp"
install -Dm644 "$DIR/icon.png" "$HOME/.local/share/icons/hicolor/256x256/apps/aurora.png"
mkdir -p "$HOME/.local/share/applications"
sed "s|^Exec=.*|Exec=$HOME/.local/bin/aurora|" "$DIR/aurora.desktop" > "$HOME/.local/share/applications/aurora.desktop"
echo "Aurora installed: $HOME/.local/bin/aurora"
INSTALL
chmod 755 "$TAR/install.sh"
tar -C "$(dirname "$TAR")" -czf "$OUT/$NAME.tar.gz" "$NAME"
rm -rf "$(dirname "$TAR")"

(cd "$OUT" && sha256sum aurora_*.deb "$NAME.tar.gz" > SHA256SUMS-linux.txt)
ls -la "$OUT"
