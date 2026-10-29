#!/usr/bin/env bash
# Isolated benchmark desktop: Xvfb :98 + i3 + a private D-Bus session bus (so the
# AT-SPI registry only sees apps started here, never the bots' :99 desktop).
# Usage: env.sh start|stop|status ; `source <(env.sh vars)` exports the session env.
set -euo pipefail
DISPLAY_NUM="${BENCH_DISPLAY:-98}"
ROOT="${BENCH_ROOT:-/tmp/agent-cu-bench}"
RUN="$ROOT/runtime"
[[ "$DISPLAY_NUM" == 99 ]] && { echo "refusing to touch :99" >&2; exit 2; }

vars() {
  echo "export DISPLAY=:$DISPLAY_NUM XDG_SESSION_TYPE=x11 XDG_CURRENT_DESKTOP=i3"
  echo "export XDG_RUNTIME_DIR=$RUN"
  [[ -f "$ROOT/dbus.addr" ]] && echo "export DBUS_SESSION_BUS_ADDRESS='$(cat "$ROOT/dbus.addr")'"
  echo "export I3SOCK=$RUN/i3-ipc.sock"
  echo "unset WAYLAND_DISPLAY HYPRLAND_INSTANCE_SIGNATURE"
}

start() {
  mkdir -p "$ROOT" "$RUN"; chmod 700 "$RUN"
  if [[ ! -S /tmp/.X11-unix/X$DISPLAY_NUM ]]; then
    nohup Xvfb ":$DISPLAY_NUM" -screen 0 1920x1080x24 -nolisten tcp >"$ROOT/xvfb.log" 2>&1 &
    echo $! >"$ROOT/xvfb.pid"
    for _ in $(seq 50); do [[ -S /tmp/.X11-unix/X$DISPLAY_NUM ]] && break; sleep 0.1; done
  fi
  if [[ ! -f "$ROOT/dbus.pid" ]] || ! kill -0 "$(cat "$ROOT/dbus.pid")" 2>/dev/null; then
    out=$(XDG_RUNTIME_DIR=$RUN dbus-daemon --session --fork --print-address=1 --print-pid=1 --address="unix:path=$RUN/bus")
    echo "$out" | sed -n 1p >"$ROOT/dbus.addr"
    echo "$out" | sed -n 2p >"$ROOT/dbus.pid"
  fi
  eval "$(vars)"
  # Make the a11y bus report enabled, like a desktop with assistive tech on.
  if [[ ! -f "$ROOT/i3.pid" ]] || ! kill -0 "$(cat "$ROOT/i3.pid")" 2>/dev/null; then
    printf 'font pango:monospace 9\nbar { mode invisible }\nipc-socket %s\nfocus_follows_mouse no\n' "$I3SOCK" >"$ROOT/i3.conf"
    nohup i3 -c "$ROOT/i3.conf" >"$ROOT/i3.log" 2>&1 &
    echo $! >"$ROOT/i3.pid"
    for _ in $(seq 50); do [[ -S "$I3SOCK" ]] && break; sleep 0.1; done
  fi
  echo "bench desktop :$DISPLAY_NUM up (root $ROOT)"
}

stop() {
  for f in i3 dbus xvfb; do
    if [[ -f "$ROOT/$f.pid" ]]; then kill "$(cat "$ROOT/$f.pid")" 2>/dev/null || true; rm -f "$ROOT/$f.pid"; fi
  done
  # at-spi launcher/registry started on our private bus
  pkill -f -- "--address=unix:path=$RUN/" 2>/dev/null || true
  pkill -u "$(id -u)" -f "at-spi.*$RUN" 2>/dev/null || true
  rm -f "$ROOT/dbus.addr"
  echo "bench desktop :$DISPLAY_NUM stopped"
}

case "${1:-}" in
  start) start ;; stop) stop ;; vars) vars ;;
  status) [[ -S /tmp/.X11-unix/X$DISPLAY_NUM ]] && echo up || echo down ;;
  *) echo "usage: $0 start|stop|vars|status" >&2; exit 2 ;;
esac
