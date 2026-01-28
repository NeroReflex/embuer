#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
DOCKERFILE="$ROOT_DIR/Dockerfile.amd64"
TAG="embuer-shim:amd64"
OUT_DIR="$ROOT_DIR/docker-build-out"

if ! command -v docker >/dev/null 2>&1; then
	echo "docker is not installed or not in PATH" >&2
	exit 1
fi

mkdir -p "$OUT_DIR"

echo "Building Docker image $TAG using $DOCKERFILE..."
# force amd64 platform to ensure shim is built for amd64
docker build --platform=linux/amd64 -f "$DOCKERFILE" -t "$TAG" "$ROOT_DIR"

echo "Using $OUT_DIR as a bind mount for output artifacts."
echo "Preparing to extract artifacts from image $TAG..."

# Create a temporary container from the built image and copy useful paths out.
cid=$(docker create --platform=linux/amd64 "$TAG")
echo "Created temporary container $cid"

# Copy shim install (EFI files produced by the shim Makefile)
docker cp "$cid":/workdir/shim_install "$OUT_DIR"/shim_install 2>/dev/null || true

# Also copy the workspace and any cargo release artifacts if present
docker cp "$cid":/workspace "$OUT_DIR"/workspace 2>/dev/null || true
docker cp "$cid":/target/release "$OUT_DIR"/release 2>/dev/null || true

# Remove the temporary container
docker rm "$cid" >/dev/null

echo "Artifacts copied to:"
echo " - $OUT_DIR/shim_install  (shim EFI outputs)"
echo " - $OUT_DIR/release       (if Rust target/release existed)"
echo " - $OUT_DIR/workspace     (full workspace from image, if present)"

cp "$OUT_DIR/shim_install/boot/efi/EFI/BOOT/BOOTX64.EFI" "BOOTX64.EFI"
cp "$OUT_DIR/shim_install/boot/efi/EFI/BOOT/mmx64.efi" "mmx64.efi"
