#!/usr/bin/env bash
# Runs desktop KOReader (Linux amd64 .deb, extracted) with the freshly built
# plugin, for manual / screenshot-driven end-to-end checks on a server.
#
#   fork/build.sh dev
#   KOREADER_DIR=/opt/kotest/root/usr/lib/koreader fork/e2e/koreader_run.sh [EVENTS]
#
# EVENTS (comma separated KOReader events, fired 4 s apart by the bundled
# kotestdriver plugin), default StartRakuyomiMangaLibLibraryView — opens the
# fork's library. Needs an X display (Xvfb :2 by default); interact with
# xdotool, take screenshots with
#   ffmpeg -loglevel error -f x11grab -video_size 600x800 -i "$DISPLAY" -frames:v 1 -y shot.png
# Logs: $KO_HOME/ko.log (KOReader), $KO_HOME/rakuyomi-mangalib/ (plugin data).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
: "${KOREADER_DIR:?set KOREADER_DIR to the extracted usr/lib/koreader}"
KO_HOME="${KO_HOME:-/tmp/kohome}"
export DISPLAY="${DISPLAY:-:2}"
PLUGIN="$ROOT/build/rakuyomi-mangalib.koplugin"
[ -d "$PLUGIN" ] || { echo "build the plugin first: fork/build.sh dev" >&2; exit 1; }

if ! xdpyinfo >/dev/null 2>&1; then
  Xvfb "$DISPLAY" -screen 0 600x800x24 >/dev/null 2>&1 &
  sleep 1
fi
pkill -f "reader.lua $KO_HOME" 2>/dev/null || true
pkill -f "rakuyomi-mangalib.koplugin/server" 2>/dev/null || true
sleep 1
mkdir -p "$KO_HOME"
rsync -a --delete "$PLUGIN/" "$KOREADER_DIR/plugins/rakuyomi-mangalib.koplugin/"
rsync -a --delete "$ROOT/fork/e2e/kotestdriver.koplugin/" "$KOREADER_DIR/plugins/kotestdriver.koplugin/"
cd "$KOREADER_DIR"
HOME="$KO_HOME" KO_TEST_EVENT="${1:-StartRakuyomiMangaLibLibraryView}" \
  nohup ./luajit ./reader.lua "$KO_HOME" >"$KO_HOME/ko.log" 2>&1 &
echo "KOReader started (pid $!), log: $KO_HOME/ko.log"
