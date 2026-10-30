# Rolling the X11-only build out to the bots (for the parent, after the user's OK)

Nothing here has been applied. All of it changes files outside this repository.

## 1. Install the binary (VPS only)

```sh
cd ~/Projects/computer-use-linux
git fetch origin feat/agent-cu-bench
git worktree add /tmp/cul-x11 origin/feat/agent-cu-bench
cd /tmp/cul-x11
/home/israel/.cache/t3-testdata/heavy.sh cargo install --path . --locked --force \
  --root ~/.local/share/computer-use-linux-fork
# The build no longer ships these helpers; remove the stale copies:
rm -f ~/.local/share/computer-use-linux-fork/bin/computer-use-linux-indicator \
      ~/.local/share/computer-use-linux-fork/bin/computer-use-linux-cosmic
cd ~/Projects/computer-use-linux && git worktree remove /tmp/cul-x11
```

Bots pick the new binary up when their MCP server restarts (next session / thread).

## 2. `~/.local/bin/computer-use-linux-mcp-x11` (new content)

The indicator variable is gone; everything else is unchanged.

```bash
#!/usr/bin/env bash
# VPS: drives the Xvfb :99 + i3 session with the X11-only fork.
set -euo pipefail
export DISPLAY="${DISPLAY:-:99}" XDG_SESSION_TYPE=x11 XDG_CURRENT_DESKTOP=i3
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
export DBUS_SESSION_BUS_ADDRESS="${DBUS_SESSION_BUS_ADDRESS:-unix:path=$XDG_RUNTIME_DIR/bus}"
export I3SOCK="${I3SOCK:-$(i3 --get-socketpath 2>/dev/null || true)}"
exec "$HOME/.local/share/computer-use-linux-fork/bin/computer-use-linux" "${@:-mcp}"
```

## 3. `~/.agents/scripts/computer-use-linux-mcp` (new content)

The fork no longer drives Wayland/Hyprland. On the VPS this launcher already fell through to the
X11 launcher; now it does only that. If `~/.agents` is synced to the Hyprland desktop, keep the old
binary/launcher there (this build will not work on Wayland).

```bash
#!/usr/bin/env bash
# Starts the X11-only computer-use-linux MCP server against the Xvfb :99 + i3 display.
set -euo pipefail
if [[ -S /tmp/.X11-unix/X99 && -x "$HOME/.local/bin/computer-use-linux-mcp-x11" ]]; then
  exec "$HOME/.local/bin/computer-use-linux-mcp-x11" mcp "$@"
fi
echo "computer-use-linux-mcp: no Xvfb :99 display here; start Xvfb :99 + i3 first" >&2
exit 1
```

No change is needed in `~/.claude.json` or `~/.codex/config.toml` (same launcher paths, same tool
server name).

## 4. `~/.local/bin/chrome-perfil`: expose the page to the accessibility tree

Without `--force-renderer-accessibility` Chrome exposes ~4 AT-SPI nodes and no page content
(measured); toggling AT-SPI flags at runtime does not turn it on. Change the launch line:

```diff
-nohup "$chrome" --user-data-dir="$dir" --no-first-run --no-default-browser-check "${url:-$default_url}" \
+nohup "$chrome" --user-data-dir="$dir" --no-first-run --no-default-browser-check --force-renderer-accessibility "${url:-$default_url}" \
   >"/tmp/chrome-$1.log" 2>&1 &
```

The second launch line (`nohup "$chrome" --user-data-dir="$dir" "$url"` for an already-open
profile) only opens a tab in the running browser, so the flag takes effect after the profile's
Chrome is closed and reopened once (do it when no bot is using that profile).

## 5. Host gaps found (optional, needs an install decision)

- There is no GTK and no xdg-desktop-portal, so Chrome cannot open a file chooser on the VPS at
  all. The new `drop_files` tool works around it (XDND drop onto the file input). Installing
  `libgtk-3-0` would also restore the native dialog.
- `tesseract` is not installed, so `ocr` reports itself unavailable (doctor `can_ocr=false`).
