#!/usr/bin/env bash
# Builds the bundled Aidoku sources (sources/*/res/source.json) into .aix
# packages named after their source id.
# Usage: fork/build-sources.sh [output-dir]   (default: build/sources)
# Requires: rustup target wasm32-unknown-unknown, zip, python3.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${1:-$ROOT/build/sources}"
mkdir -p "$OUT_DIR"
OUT_DIR="$(cd "$OUT_DIR" && pwd)"

for SRC in "$ROOT"/sources/*/; do
  SRC="${SRC%/}"
  [ -f "$SRC/res/source.json" ] || continue
  ID="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["info"]["id"])' "$SRC/res/source.json")"
  CRATE="$(sed -n 's/^name = "\(.*\)"/\1/p' "$SRC/Cargo.toml" | head -n1)"

  (cd "$SRC" && cargo build --release --target wasm32-unknown-unknown)

  STAGE="$(mktemp -d)"
  mkdir -p "$STAGE/Payload"
  cp "$SRC/target/wasm32-unknown-unknown/release/${CRATE//-/_}.wasm" "$STAGE/Payload/main.wasm"
  for f in source.json settings.json filters.json icon.png; do
    [ -f "$SRC/res/$f" ] && cp "$SRC/res/$f" "$STAGE/Payload/"
  done
  OUT="$OUT_DIR/$ID.aix"
  rm -f "$OUT"
  (cd "$STAGE" && zip -q -X -r "$OUT" Payload)
  rm -rf "$STAGE"
  echo "Built $OUT"
done
