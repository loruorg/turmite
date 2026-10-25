#!/usr/bin/env bash
# Builds dist/turmite-woocommerce.zip.
set -euo pipefail

cd "$(dirname "$0")"
name=turmite-woocommerce
out="$PWD/dist/$name.zip"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

mkdir -p "$stage/$name" dist
tar --exclude=./tests --exclude=./dist --exclude=./build.sh --exclude=./README.md --exclude=.DS_Store -cf - . |
	tar -xf - -C "$stage/$name"

rm -f "$out"
if command -v zip >/dev/null 2>&1; then
	(cd "$stage" && zip -qrX "$out" "$name")
else
	(cd "$stage" && python3 -m zipfile -c "$out" "$name")
fi
echo "$out"
