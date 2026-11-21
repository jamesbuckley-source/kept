#!/bin/sh
# Brings up Kept's headless desktop and the browser door onto it:
#
#   :99              Xvfb screen, 1920x1080 (override with KEPT_SCREEN)
#   localhost:5900   x11vnc
#   :6080/vnc.html   noVNC, the one you actually open
#
# The app itself is started by hand (`npm run tauri dev`), so this script only
# owns the desktop.
set -e

SCREEN="${KEPT_SCREEN:-1920x1080x24}"
DISPLAY_NUM="${DISPLAY:-:99}"

cleanup() {
  for pid in $PIDS; do
    kill "$pid" 2>/dev/null || true
  done
}
PIDS=""
trap cleanup EXIT INT TERM

echo "kept: starting Xvfb on ${DISPLAY_NUM} (${SCREEN})"
Xvfb "${DISPLAY_NUM}" -screen 0 "${SCREEN}" -nolisten tcp -dpi 96 &
PIDS="$PIDS $!"
sleep 1

xsetroot -solid "#0b0e14" || true

echo "kept: starting openbox"
openbox &
PIDS="$PIDS $!"
sleep 1

# The tray Kept's window icon expects; without it the icon has nowhere to sit.
stalonetray --geometry 1x1-0+0 --icon-size 24 --slot-size 28 &
PIDS="$PIDS $!"

echo "kept: starting x11vnc and noVNC on :6080"
x11vnc -display "${DISPLAY_NUM}" -nopw -forever -shared -rfbport 5900 -quiet -noxdamage \
  >/tmp/x11vnc.log 2>&1 &
PIDS="$PIDS $!"

websockify --web /usr/share/novnc 6080 localhost:5900 \
  >/tmp/websockify.log 2>&1 &
PIDS="$PIDS $!"

echo "kept: desktop ready - open http://localhost:6080/vnc.html"

# Stay in the foreground so the container's lifecycle owns the desktop.
wait
