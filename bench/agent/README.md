# Agent computer-use benchmark

Measures how well an AI agent completes real tasks through this MCP server on the
X11 virtual display, and compares server builds. The metric and the bars are
pre-registered in [METRIC.md](METRIC.md); the run log is [results/log.tsv](results/log.tsv).

## One command

```sh
bench/agent/run_all.sh /tmp/cu-results <baseline-binary> <candidate-binary> [codex|claude] [reps]
```

Runs the easy (10 tasks) and hard (12 tasks) suites with the two builds interleaved,
then prints both summaries with `bar met: True/False`.

## Pieces

| File | Role |
|---|---|
| `env.sh` | Isolated desktop: Xvfb `:98` + i3 + a private D-Bus/AT-SPI bus. Refuses `:99`. |
| `server.py`, `apps/`, `apps/hard/` | Local web apps (Gmail/Calendar-like) that POST their state. |
| `tasks.py`, `tasks_hard.py` | Prompts and checkers. Checkers read posted state or files, never the agent's reply. |
| `selftest*.mjs`, `check_selftest*.py` | Prove each checker accepts the happy path and rejects wrong outcomes (headless Playwright). |
| `run.py` / `run_one.py` | Episode driver: fresh Chrome profile, fresh agent session (`codex exec` or `claude -p` with an isolated config dir), checker, metrics. Episodes run through `heavy.sh`. |
| `mcp_proxy.py` | Times every MCP call and records payload size and images. |
| `summarize.py` | Throughput, success rate, per-task table, bar check. |
| `interview.py` | Runs a task, then resumes the agent session and asks what slowed it down. |
| `probe.sh` | Opens one page in a throwaway Chrome and runs MCP calls by hand. |

Options worth knowing: `run.py --chrome-mode bots` reproduces `~/.local/bin/chrome-perfil` plus
`--force-renderer-accessibility`; `bots-noa11y` is chrome-perfil as it is today.

## Requirements

Xvfb, i3, xdotool, Chrome for Testing under `~/.cache/ms-playwright`, the `codex` and/or `claude`
CLIs logged in. Nothing touches `:99` or the real Chrome profiles.
