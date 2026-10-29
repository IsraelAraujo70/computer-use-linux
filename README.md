<div align="center">
  <h1>computer-use-linux</h1>
  <p><strong>Drive apps on a virtual X11 display from any MCP host.</strong></p>
  <p>
    <a href="https://github.com/agent-sh/computer-use-linux/actions/workflows/ci.yml"><img src="https://github.com/agent-sh/computer-use-linux/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
    <a href="https://crates.io/crates/computer-use-linux"><img src="https://img.shields.io/crates/v/computer-use-linux.svg" alt="crates.io"></a>
    <a href="https://www.npmjs.com/package/@agent-sh/computer-use-linux"><img src="https://img.shields.io/npm/v/@agent-sh/computer-use-linux.svg" alt="npm"></a>
    <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-yellow.svg" alt="License: MIT"></a>
  </p>
</div>

`computer-use-linux` is a Rust MCP server and CLI that lets an agent operate GUI apps (Chrome, terminals) on a virtual X11 display: Xvfb with the i3 window manager. It reads AT-SPI accessibility trees, captures X11 screenshots, lists and focuses i3 windows, and sends clicks, scrolls, drags, keys and text through `xdotool`.

```bash
npm install -g @agent-sh/computer-use-linux
computer-use-linux doctor | jq .readiness
```

The crate is published as [`computer-use-linux`](https://crates.io/crates/computer-use-linux), the npm wrapper as [`@agent-sh/computer-use-linux`](https://www.npmjs.com/package/@agent-sh/computer-use-linux), and prebuilt binaries ship with the [latest release](https://github.com/agent-sh/computer-use-linux/releases/latest).

## Session requirements

The server drives the display named by its environment:

| Need | How |
| --- | --- |
| X display | `DISPLAY=:N` and `XDG_SESSION_TYPE=x11` for a running Xvfb |
| Window manager | i3 on that display, with `I3SOCK` pointing at its IPC socket (`wmctrl` and `xprop` cover other X11 window managers) |
| Accessibility | `DBUS_SESSION_BUS_ADDRESS` for the session bus that starts the AT-SPI bus (`at-spi2-core`) |
| Input | `xdotool` |
| OCR (optional) | `tesseract` |

Chromium and Electron apps need `--force-renderer-accessibility` for a full tree. `computer-use-linux doctor` checks each row and every blocker it reports names the fix.

## Tools

- `doctor`: readiness report for the X display, i3 IPC, AT-SPI bus, `xdotool` and `tesseract`.
- `list_apps`, `list_windows`, `focused_window`: running apps and AT-SPI roots; i3 windows with ids, titles, classes, pids and bounds.
- `get_app_state`: screenshot plus accessibility outline (`[index] role "name" @x,y wxh {states} actions=...`) for one app or window. The index is what the element tools take.
- `find_elements`, `wait_for`: search the tree, or poll until an element appears, disappears or the tree changes.
- `screenshot`: size-bounded PNG or JPEG of the screen or one window (focused first, then cropped).
- `ocr`: tesseract text lines with coordinates, for surfaces without a tree.
- `click`, `scroll`, `drag`: by element index, semantic selector (`role`/`name`/`text`/`states`) or coordinates. A plain left click on an element invokes its AT-SPI click/press/toggle action; coordinates and other buttons go through xdotool.
- `drop_files`: upload files by dropping them (XDND, `text/uri-list`) onto a file input or drop zone; the display has no file dialog.
- `press_key`, `type_text`: xdotool keys and literal text, optionally after focusing a window or terminal (`tty`, `terminal_pid`, `terminal_command`, `terminal_cwd`). Results report the focused AT-SPI element and warn when it is not editable.
- `perform_action`, `set_value`: invoke an AT-SPI action or set a value or editable text directly.
- `activate_window`: focus a window, switching i3 workspace if needed.
- `run_shell`: same-user `/bin/sh -c`, registered only with `COMPUTER_USE_LINUX_ENABLE_SHELL=1`. It is not a sandbox.

Screenshots default to at most 1920 px per side and 2 MiB. Pass `max_width`, `max_height`, `max_bytes`, `scale`, `format: "jpeg"` or `quality` to change that. Results carry `coordinate_width`, `coordinate_height` and `scale` for mapping image pixels back to screen coordinates. For `click` or `scroll` with `relative: true`, measure from the target window's screenshot crop and divide preview pixels by `scale`.

### MCP safety contract

| Class | Tools | Contract |
| --- | --- | --- |
| Read-only observation | `doctor`, `list_apps`, `list_windows`, `focused_window`, `get_app_state`, `find_elements`, `wait_for`, `ocr` | `readOnlyHint=true`; may reveal window, accessibility and screen contents. |
| UI state mutators | `activate_window`, `scroll`, `screenshot` | `readOnlyHint=false`, `destructiveHint=false`; change focus or scroll position, or focus a window to capture it. |
| Desktop action mutators | `click`, `drag`, `drop_files`, `press_key`, `type_text`, `perform_action`, `set_value` | `readOnlyHint=false`, `destructiveHint=true`; can trigger any action in the targeted app. |
| Conditional host-code execution | `run_shell` | Absent unless `COMPUTER_USE_LINUX_ENABLE_SHELL=1`; runs with the server user's permissions. |

Annotations are hints, not authorization. Hosts should still ask the user before calls that submit, delete, send, purchase or overwrite data. `run_shell` clears the ambient environment down to a small allowlist, uses a non-login `/bin/sh`, a 30 s default and 120 s maximum timeout, and writes a stderr audit record keyed by the command's SHA-256.

The CLI exposes the same pieces for scripting:

```
computer-use-linux mcp                 # stdio MCP server
computer-use-linux doctor              # JSON readiness report
computer-use-linux apps
computer-use-linux state [APP_NAME]
computer-use-linux screenshot          # JSON screenshot summary
computer-use-linux windows
```

## Install

### `./install.sh` from a clone

Installs the system packages (`at-spi2-core`, `xdotool`, build tools) on Debian/Ubuntu, Fedora, or Arch-like systems, installs Rust if needed, builds the release binary into `~/.local/bin`, and runs `doctor`.

```bash
git clone https://github.com/agent-sh/computer-use-linux
cd computer-use-linux
./install.sh
```

For an unrecognized distro, pass `--package-manager apt|dnf|pacman`.

### `cargo install`

```bash
cargo install computer-use-linux
sudo apt install at-spi2-core xdotool   # tesseract-ocr for ocr
```

### npm wrapper

Downloads the matching Linux x86_64 or aarch64 binary for the package version and verifies its `.sha256`.

```bash
npm install -g @agent-sh/computer-use-linux
```

### Prebuilt binary

```bash
asset=computer-use-linux-x86_64-unknown-linux-gnu
base=https://github.com/agent-sh/computer-use-linux/releases/latest/download
curl -L -O "$base/$asset" -O "$base/$asset.sha256"
sha256sum -c "$asset.sha256"
install -m 0755 "$asset" "$HOME/.local/bin/computer-use-linux"
```

## Wire it into your MCP host

Spawn the binary with `mcp` as its only argument, in an environment that carries the session variables above.

**Claude Code and Codex plugin.** This repo is a plugin marketplace. The plugin registers the server and the `computer-use-linux` skill, and on first start downloads the pinned, sha256-verified release binary into `~/.cache/computer-use-linux/plugin/`. Set `COMPUTER_USE_LINUX_BIN` to run your own build.

```bash
claude plugin marketplace add agent-sh/computer-use-linux
claude plugin install computer-use-linux@computer-use-linux
# Codex:
codex plugin marketplace add agent-sh/computer-use-linux
codex plugin add computer-use-linux@computer-use-linux
```

**Claude Code CLI.** `claude mcp add --scope user computer-use-linux -- computer-use-linux mcp`

**Claude Desktop and generic clients.**

```json
{
  "mcpServers": {
    "computer-use-linux": { "command": "computer-use-linux", "args": ["mcp"] }
  }
}
```

**Pi.** `pi install npm:@agent-sh/computer-use-linux` adds native `computer_use_linux_*` tools behind one loader (`computer_use_linux_tools`); see the [Pi setup guide](skills/computer-use-linux/references/pi-setup.md).

**Hermes.** See the [Hermes setup guide](skills/computer-use-linux/references/hermes-setup.md).

## First run

```bash
computer-use-linux doctor | jq .readiness
```

Ready means `ready: true` and an empty `blockers` array. Doctor connects to the X display but does not capture, so `screenshot_capture_status` stays `unverified` until `screenshot` or `get_app_state` runs. `can_ocr: false` only disables `ocr`.

## Environment variables

| Variable | Effect |
| --- | --- |
| `COMPUTER_USE_LINUX_XDOTOOL_TYPE_DELAY_MS` | Per-character delay for `xdotool type` (default `12`). `0` is faster but can reorder characters on some X servers. |
| `COMPUTER_USE_LINUX_ENABLE_SHELL` | Set exactly to `1` to register `run_shell`. Do not enable for untrusted or unattended hosts. |
| `COMPUTER_USE_LINUX_BIN` | npm wrapper and plugin: run this absolute binary path instead of the bundled one. |
| `COMPUTER_USE_LINUX_DOWNLOAD_BASE` | npm wrapper and plugin: release base URL to download from. |
| `COMPUTER_USE_LINUX_SKIP_DOWNLOAD=1` / `COMPUTER_USE_LINUX_LOCAL_BINARY` | npm wrapper: skip the download, or install a local build. |

## Architecture

- **Accessibility**: the [`atspi`](https://crates.io/crates/atspi) crate reads the AT-SPI registry on the session bus. Nodes are flattened and indexed per `get_app_state` snapshot; element tools resolve indices under an input lock so a checked index cannot turn into another app's node mid-action.
- **Screenshots**: one `GetImage` on the root window through [`x11rb`](https://crates.io/crates/x11rb), in device pixels (the space xdotool and window origins use). X11 queries share a transport-enforced deadline, so a stalled X server cannot hang a call.
- **Windows**: i3 IPC (`i3-msg`, with `xprop` for pids) first, generic X11/EWMH (`wmctrl`, `xprop`) second. Terminal windows are enriched with their tty and foreground process so input can target "the terminal running `pytest`".
- **Input**: `xdotool` (XTEST) for pointer and keyboard. Keysyms resolve against the live XKB layout; wheel scrolling uses buttons 4-7 so GTK 3 does not drop the event (#184).
- **MCP transport**: [`rmcp`](https://crates.io/crates/rmcp) over stdio; no network listener.

## Security

- Anything that can reach the X display can read the screen and inject input; keep the Xvfb display and its session bus private to the agent's user.
- AT-SPI exposes window contents to any client on the session bus.
- The binary opens no network listener and sends no telemetry.
- The tool list annotates read-only versus mutating tools, and CI fails if they drift from the table above.

## Troubleshooting

`computer-use-linux doctor` is the source of truth; each blocker names its fix.

- **X display unreachable**: start Xvfb on that display or export `DISPLAY` for the running one.
- **AT-SPI bus unreachable or disabled**: export the right `DBUS_SESSION_BUS_ADDRESS`; enable with `busctl --user set-property org.a11y.Bus /org/a11y/bus org.a11y.Status IsEnabled b true` and restart the app.
- **Window listing failed**: start i3 on the display and export `I3SOCK`.
- **Empty tree in Chrome or Electron**: relaunch with `--force-renderer-accessibility`, or use `ocr`.
- **`type_text` went to the wrong window**: pass a target (`window_id`, `title`, or a terminal selector).

For coordinate calibration, run [the GTK4 probe](examples/coordinate_probe.py) on the display, take a targeted screenshot, and click the green square's center relative to the same window; `hit: true` is the acceptance condition. [The semantic-click regression](scripts/semantic_click_test.py) checks GTK3 button activation through MCP in an isolated display.

## Related

- [agent-workspace-linux](https://github.com/agent-sh/agent-workspace-linux) gives an agent its own hidden Xvfb desktop.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Report security vulnerabilities through [SECURITY.md](SECURITY.md), not public issues.

## Credits

Extracted from [`codex-desktop-linux`](https://github.com/avifenesh/codex-desktop-linux). Maintained by [Avi Fenesh](https://github.com/avifenesh). Built on [`atspi`](https://crates.io/crates/atspi), [`zbus`](https://crates.io/crates/zbus), [`x11rb`](https://crates.io/crates/x11rb) and [`rmcp`](https://crates.io/crates/rmcp).

## Publishing

Publishing is tag-driven from GitHub Actions and needs the `CARGO_REGISTRY_TOKEN` and `NPM_TOKEN` secrets. Bump `Cargo.toml` and `package.json` together, update `CHANGELOG.md`, and push a `vX.Y.Z` tag.

## License

MIT — see [LICENSE](LICENSE).
