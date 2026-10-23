#!/usr/bin/env bash
# Builds the side-by-side plugin (see fork/side-by-side.json).
#
#   fork/build.sh <target>            full build via upstream scripts/build-all.sh
#                                      (kindle, kindlehf, kindlea9, desktop, aarch64)
#   fork/build.sh <target> --reuse    reuse backend/target/<rust-target>/release binaries
#   fork/build.sh dev                 debug host binaries from backend/target/debug
#
# Result: build/<plugin_dir>/ and build/<plugin_dir without .koplugin>-<target>.zip
# The upstream build scripts are used unchanged; the renaming happens in
# fork/side_by_side.py on the packaged copy.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

NAME="${1:?usage: fork/build.sh <target>|dev [--reuse]}"
MODE="${2:-}"

declare -A TARGETS=(
  ["desktop"]="x86_64-unknown-linux-musl"
  ["aarch64"]="aarch64-unknown-linux-musl"
  ["kindle"]="arm-unknown-linux-musleabi"
  ["kindlehf"]="arm-unknown-linux-musleabihf"
  ["kindlea9"]="arm-unknown-linux-musleabi"
)

PLUGIN_DIR="$(python3 -c 'import json; print(json.load(open("fork/side-by-side.json"))["plugin_dir"])')"
UPSTREAM_OUT="build/rakuyomi.koplugin"
OUT="build/$PLUGIN_DIR"
ZIP="build/${PLUGIN_DIR%.koplugin}-$NAME.zip"

# scripts/build-plugin.sh compiles the translations in place and deletes the
# translation sources from the working tree: put them back afterwards.
restore_l10n() {
  git checkout -q -- frontend/rakuyomi.koplugin/l10n 2>/dev/null || true
}
trap restore_l10n EXIT

rm -rf "$UPSTREAM_OUT"
if [[ "$NAME" == "dev" ]]; then
  (cd backend && cargo build -p server -p uds_http_request -p cbz_metadata_reader)
  bash ./scripts/build-plugin.sh none rakuyomi.koplugin dev
  cp backend/target/debug/{server,uds_http_request,cbz_metadata_reader} "$UPSTREAM_OUT/"
elif [[ -n "${TARGETS[$NAME]:-}" ]]; then
  if [[ "$MODE" == "--reuse" ]]; then
    bash ./scripts/build-plugin.sh "${TARGETS[$NAME]}" rakuyomi.koplugin "$NAME"
  else
    bash ./scripts/build-all.sh "$NAME"
  fi
else
  echo "unknown target: $NAME" >&2
  exit 1
fi
restore_l10n

rm -rf "$OUT"
python3 fork/side_by_side.py "$UPSTREAM_OUT" "$OUT"
bash fork/build-sources.sh "$OUT/bundled-sources"

rm -f "$ZIP"
(cd build && zip -q -r -X "$(basename "$ZIP")" "$PLUGIN_DIR")
echo "DONE → $OUT, $ZIP"
