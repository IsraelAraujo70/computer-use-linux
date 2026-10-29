# computer-use-linux

NPM wrapper for the `computer-use-linux` MCP server, published as
[`@agent-sh/computer-use-linux`](https://www.npmjs.com/package/@agent-sh/computer-use-linux).

The server drives apps on a virtual X11 display (Xvfb + i3): it needs `DISPLAY`,
`XDG_SESSION_TYPE=x11`, `I3SOCK`, a session D-Bus with the AT-SPI bus, and
`xdotool` (plus `tesseract` for `ocr`). `computer-use-linux doctor` names the
fix for anything missing.

Security note: this server can control the apps on that display. Tools such as
`click`, `type_text`, `press_key`, `perform_action`, and `set_value` are
mutating and can change real application state. The MCP tool list includes
`ToolAnnotations` so hosts can distinguish read-only observation from mutating
desktop actions.

Plain left `click` by element index or selector prefers a native AT-SPI
`click`, `press`, or `toggle` action over toolkit bounds,
avoiding pointer-coordinate conversion when that action is available. Explicit
`activate`/`jump` requests belong in `perform_action`, not `click`, even when
bounds are missing. Explicit
`x`/`y`, right clicks, and multi-clicks retain pointer semantics. For coordinate
`click` or `scroll` with `relative: true`, use the clipped target-window
screenshot crop origin and divide preview coordinates by screenshot `scale`.
Do not use widget-local or raw GDK surface coordinates. A window target is
required.

```bash
npm install -g @agent-sh/computer-use-linux
computer-use-linux doctor
pi install npm:@agent-sh/computer-use-linux
hermes skills tap add agent-sh/computer-use-linux
hermes skills install agent-sh/computer-use-linux/computer-use-linux
hermes mcp add computer-use-linux --command computer-use-linux --args mcp
hermes mcp test computer-use-linux
hermes mcp configure computer-use-linux
```

The generated Hermes config should look like this:

```yaml
mcp_servers:
  computer-use-linux:
    command: computer-use-linux
    args: ["mcp"]
    timeout: 120
    connect_timeout: 30
```

The package downloads the matching Linux x86_64 or aarch64 binary from the
GitHub release for this package version and verifies the `.sha256` asset before
installing it.

When installed through Pi, the package supplies native, dynamically loaded
`computer_use_linux_*` tools. No separate MCP adapter or manual MCP
configuration is required. Native tools require Pi 0.84.4 or newer; the
standalone CLI wrapper retains Node.js 18 support.

In Pi codemode, native tools return `{ text, images, isError }`. Pass image
blocks directly to `image(result.images[0])` and check `isError` before using
a result.

If you already built or installed the binary yourself, set
`COMPUTER_USE_LINUX_BIN=/path/to/computer-use-linux` to make the wrapper use
that executable instead.

X11 text input sends LF and CRLF line endings as Return. Focused terminals
accept keyboard input even when AT-SPI does not expose EditableText. Focus
feedback describes the target; re-observe its contents to confirm input.

`type_text` uses xdotool, which resolves keysyms against the live keyboard
layout and rejects NUL. `ok: true` confirms dispatch only; field contents are
not verified; re-observe before retrying. Exact window focus alone does not suppress missing-focus
warnings: only a complete scoped tree containing window/menu chrome without
content makes the feedback informational. Such trees get a conditional
Electron/Chromium hint computed before compaction; this is not positive Electron
detection.
