#!/usr/bin/env bash
set -euo pipefail

# Utilisation : build-deb.sh <version>
VERSION="$1"

ROOT_DIR=$(dirname "$0")/..
ROOT_DIR=$(cd "$ROOT_DIR" && pwd)

echo "Construction de simeis-server version $VERSION"

cd "$ROOT_DIR"

echo "Construction du binaire de release..."
cargo build --release -p simeis-server

BINARY=target/release/simeis-server
if [ ! -f "$BINARY" ]; then
  echo "Erreur : binaire non trouvé à $BINARY"
  exit 1
fi

PKGDIR=$(mktemp -d)
echo "Création de la racine du paquet à $PKGDIR"
mkdir -p "$PKGDIR"/usr/bin
mkdir -p "$PKGDIR"/usr/share/man/man1
mkdir -p "$PKGDIR"/etc/systemd/system
mkdir -p "$PKGDIR"/DEBIAN

echo "Copie du binaire..."
install -m 0755 "$BINARY" "$PKGDIR/usr/bin/simeis-server"

echo "Copie des fichiers de contrôle et des scripts de maintenance..."
cp -a debian/DEBIAN/* $PKGDIR/DEBIAN/ || true
# copier la page man si présente
if [ -d debian/usr/share/man ]; then
  cp -a debian/usr/share/man/* $PKGDIR/usr/share/man/ || true
fi
# installer l'unité systemd dans /etc/systemd/system
cp -a debian/lib/systemd/system/simeis-server.service $PKGDIR/etc/systemd/system/ || true

echo "Configuration des permissions pour les fichiers DEBIAN"
chmod 0755 "$PKGDIR/DEBIAN/preinst" || true
chmod 0755 "$PKGDIR/DEBIAN/postinst" || true
chmod 0755 "$PKGDIR/DEBIAN/prerm" || true
chmod 0755 "$PKGDIR/DEBIAN/postrm" || true

OUT=./simeis-server_${VERSION}_amd64.deb
echo "Construction du .deb -> $OUT"
dpkg-deb --build "$PKGDIR" "$OUT"

echo "Paquet construit : $OUT"
ls -lh "$OUT"

echo "Nettoyage"
rm -rf "$PKGDIR"

echo "Terminé"
