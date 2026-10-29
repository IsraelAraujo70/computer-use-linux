---
name: computer-use-linux
description: "Observe and control apps on a virtual X11 display (Xvfb + i3) via native Pi tools or the computer-use-linux MCP server: accessibility trees, screenshots, window targeting, and xdotool input (click, type, scroll)."
author: agent-sh
license: MIT
platforms: [linux]
compatibility: "Native Pi tools require Pi 0.84.4+ and Node.js 22.19+; the standalone CLI/MCP server supports Node.js 18+."
---

# computer-use-linux

Use `computer-use-linux` when an agent needs to observe or operate apps on a virtual X11 display (Xvfb with the i3 window manager): inspect the accessibility tree, list/focus windows, take screenshots, click, scroll, type, press keys, or invoke AT-SPI actions.

## When to Use

Use this skill when:

- The user wants the agent to control a GUI app (Chrome, a terminal) on the X11 display.
- You need app state from AT-SPI, screenshots, or i3 window metadata.
- You are configuring the `computer-use-linux` MCP server for your agent.
- A desktop action needs target-aware input instead of blind shell commands.

Do not use this for remote browsers, websites, or headless automation when a browser-specific tool is available. Do not assume desktop actions are safe just because the MCP connection works.

## Session requirements

The server talks to the display named by its environment:

- `DISPLAY=:N` and `XDG_SESSION_TYPE=x11` for the Xvfb display.
- `I3SOCK` for the i3 IPC socket (or `wmctrl` and `xprop` on another X11 window manager).
- `DBUS_SESSION_BUS_ADDRESS` for the session bus that runs the AT-SPI bus.
- `xdotool` for input; `tesseract` (optional) for `ocr`.
- Chromium and Electron apps need `--force-renderer-accessibility` for a full tree.

## Install

Pick the install that matches how you will run this skill. You can use both.

### Claude Code or Codex plugin

```bash
claude plugin marketplace add agent-sh/computer-use-linux
claude plugin install computer-use-linux@computer-use-linux
# or, in Codex:
codex plugin marketplace add agent-sh/computer-use-linux
codex plugin add computer-use-linux@computer-use-linux
```

The plugin registers the MCP server and downloads the pinned, sha256-verified
release binary on first start. It does not put `computer-use-linux` on
`PATH`; shell commands in this skill need the CLI install below. Set
`COMPUTER_USE_LINUX_BIN` to run a local build instead.

### Pi native tools

```bash
pi install npm:@agent-sh/computer-use-linux
```

This enables Pi's `computer_use_linux_*` tools. It does not put
`computer-use-linux` on `PATH`. Shell commands in this skill (`doctor`, the
MCP `command` config, and Verification) need the CLI install below.

### Shell CLI / MCP server

Use this when you need `computer-use-linux` on `PATH` for the commands in this
skill.

```bash
npm install -g @agent-sh/computer-use-linux
computer-use-linux doctor | jq .readiness
```

Rust users can install from crates.io:

```bash
cargo install computer-use-linux
computer-use-linux doctor | jq .readiness
```

Each entry in `readiness.blockers` names its fix.

## Configure Your Agent

The `computer-use-linux` binary is an MCP server. Configure it as a stdio MCP server in your agent of choice:

```json
{
  "command": "computer-use-linux",
  "args": ["mcp"]
}
```

If the binary is not on `PATH`, use the absolute path (typically `~/.local/bin/computer-use-linux` or the npm global bin directory).
Pi native tools skip this MCP `command` config; see [Pi setup](references/pi-setup.md).

### Host-specific guides

- [Hermes setup](references/hermes-setup.md)
- [Pi coding agent setup](references/pi-setup.md)

## Procedure

1. In Pi, call `computer_use_linux_tools` with the exact tools or capability you need. Enabled tools use the `computer_use_linux_<name>` prefix, appear starting on the next model turn, and remain active for the session.
2. Begin every desktop-control turn with `get_app_state`, scoped to the app you are working in: pass `app_name_or_bundle_identifier` or a window target (`window_id`, `pid`, `app_id`, `wm_class`, `title`). Without a target the result is the whole desktop AT-SPI tree, `tree_scoped` is `false`, and `message` warns; that can flood context. Use `include_screenshot: false` when the accessibility tree is sufficient. If `accessibility_tree_truncated` is `true`, the tree is incomplete: scope to a narrower target and raise `max_nodes` or `max_depth` (hard caps 2000 and 64) rather than lowering them.
3. To locate one control, `find_elements` is cheaper than a full `get_app_state`. Actions already wait for the page to settle (including "Salvando…" buttons and spinners) and report what changed. If a result is still pending, call `wait_for` with no query (`condition=changed`) rather than guessing what will appear, and never take repeated screenshots.
4. If a tool fails, call `doctor`: each `readiness.blockers` entry names its fix.
5. Before targeted input, call `list_windows` or `focused_window` and verify the intended window by title, app id, pid, or wm class.
6. Prefer semantic targeting: use element indices or role/name/text/states selectors.
7. Use coordinates only when the UI surface has no useful accessibility tree; `ocr` reads text where the tree is empty.
8. For text input, prefer `type_text` with a target selector (`window_id`, `pid`, `app_id`, `wm_class`, `title`, `tty`, `terminal_pid`, `terminal_command`, or `terminal_cwd`) rather than relying on current focus. For long text in an editable field, `set_value` is faster.
9. After mutating actions, re-check state with `get_app_state`, `focused_window`, or an app-specific readback.

`type_text` and `press_key` use xdotool (XTEST), which resolves keysyms against
the live keyboard layout. `ok: true` confirms dispatch only; field contents are
not verified, so re-observe before retrying. With a target, the result reports
the AT-SPI focused element and warns when it is not editable. Exact window focus
alone does not suppress missing-focus warnings: only a complete scoped tree
containing window/menu chrome without content makes the feedback informational.

Plain left element/index/selector `click` prefers native AT-SPI `click`,
`press`, or `toggle` over toolkit bounds, avoiding coordinate
conversion when available. This preference does not replace a coordinate click
with an arbitrary action name. Explicit `x`/`y`, right clicks, and double/multiple
clicks retain pointer semantics.
Use `perform_action` explicitly for entry `activate` or slider `jump`; `click`
never substitutes those actions, including when bounds are unavailable.

### Screenshot-relative coordinates

Skip unless: a coordinate `click` or `scroll` uses `relative: true`.

Select a target window and use its clipped screenshot crop origin. Divide
preview `x`/`y` by screenshot `scale` first. Widget-local and raw GDK surface
coordinates are not interchangeable with that origin; missing window targets
are rejected. For calibration, use the repository's
`examples/coordinate_probe.py`: select the green square from the screenshot
and require a delivered-event `hit: true`. Do not pass widget-local `(85, 85)`
directly to a window-relative click.

## Pitfalls

- Already-running GTK, Qt, and Electron apps may need a restart after AT-SPI is enabled.
- i3 may consume its own key bindings (for example `Super+...`) before the app sees them.
- A window-targeted `get_app_state` screenshot needs that window already focused; the `screenshot` tool focuses it first.
- Desktop input is stateful. Avoid concurrent tool calls against this MCP server.
- Pi serializes the native Computer Use tools and keeps one process for the session. If that process exits, do not replay an ambiguous mutating call; obtain a fresh `get_app_state` before another element-based action.
- `click`, `drag`, `press_key`, `type_text`, `perform_action`, and `set_value` can change real application state.

## Verification

Pi-only installs: enable and call `computer_use_linux_doctor` as in
[Pi setup](references/pi-setup.md). Shell `computer-use-linux doctor` needs
the CLI on `PATH`.

Run:

```bash
computer-use-linux doctor | jq .readiness
```

Ready output should have:

- `ready: true`
- `can_build_accessibility_tree: true`
- `can_query_windows: true`
- `can_focus_windows: true`
- `can_send_input: true`
- `screenshot_capture_status: "unverified"`
- `blockers: []`

Doctor checks that the X display is reachable but does not capture. Call `get_app_state` with `include_screenshot: true` to verify capture: success reports `screenshot_capture_status: "verified"` and `can_capture_screenshots: true`; a failure reports `failed`, and `screenshot_error` has the cause. `can_ocr: false` only means `ocr` needs `tesseract`.

Then test with your agent by calling the `doctor` tool or asking the agent to list windows.
