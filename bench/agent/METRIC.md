# Pre-registered metric for "computer use is at least 2x better"

Written and committed before any change to the MCP server. It does not move afterwards;
if the task set changes, the baseline is rerun on the new set.

## Setup

- Agent under test: GPT-6.1 Terra (`codex exec`, model `gpt-6.1-terra`, reasoning effort `low`),
  a fresh ephemeral session per episode, only the `cu` MCP server enabled (shell, codex's own
  computer use, browser and apps tools disabled).
- Display: isolated Xvfb `:98` + i3 + private D-Bus/AT-SPI bus (`env.sh`), 1920x1080.
- Apps: Chrome for Testing (fresh throwaway profile per episode, `--force-renderer-accessibility`)
  on local pages served by `server.py`, plus Alacritty for the terminal task.
- Suite: the 10 tasks in `tasks.py`. Each episode is judged by its checker, which reads the
  state the page posted (or the file the terminal wrote), never the agent's reply.
- Timeout: 360 s per episode. A timed-out episode is judged by the checker like any other
  and its full 360 s count as agent time.
- "Before" = the binary the bots run today, `~/.local/share/computer-use-linux-fork/bin/computer-use-linux`
  (built from f669057), unchanged.

## Primary metric

**Throughput = successful episodes / total agent wall-clock minutes**, summed over every episode
of a variant (all tasks, all reps). Agent wall time is measured from launching the agent CLI until
it exits (app setup and checking are excluded).

This rewards both reliability and speed: a variant that fails more wastes minutes without
adding successes; a variant that is faster with the same success rate scores higher.

## The bar

The final variant is "2x better" when, in one interleaved run with at least 3 reps per task
per variant (baseline and final alternating episode by episode):

1. throughput(final) >= 2.0 x throughput(baseline), and
2. success rate(final) >= success rate(baseline), and
3. no task regresses from passing to failing, where a task "passes" for a variant when a strict
   majority of its episodes succeed.

`summarize.py --base baseline --cand <final>` computes all three and prints `bar met: True/False`.

## Secondary numbers (reported, not part of the bar)

Per episode: MCP calls, screenshots returned, input tokens, MCP-side latency per call
(`mcp_proxy.py`), tool errors. Confirmatory pass with `claude-sonnet-5-5` on the final variant
only, to show no regression for Claude-based bots.

---

# Hard suite and final comparison (pre-registered 2026-10-29, before any run on the hard suite)

The easy suite above is passed 100% by the baseline, so it only measures speed. The owner raised
the bar: 2x on the easy suite is the floor; a hard suite measures what breaks bots in real life.

## Hard suite

- 12 tasks in `tasks_hard.py` (pages under `apps/hard/`), each with a checker that reads posted
  page state; proven by `selftest_hard.mjs` + `check_selftest_hard.py` to accept the happy path
  and reject `{}` and realistic wrong outcomes. Committed before any agent episode on it.
- Timeout 300 s per episode (`--timeout 300`).
- **Headline metric: success rate** = successful episodes / episodes. Second: throughput
  (successful episodes per agent-minute), computed as for the easy suite.
- **Bar:** success rate(final) >= success rate(baseline) + 30 percentage points, and no hard task
  that passes for the baseline (strict majority of its episodes) fails for the final.

## Final comparison conditions (both suites)

- Chrome started exactly as the bots' `~/.local/bin/chrome-perfil` starts it (`--user-data-dir`
  throwaway profile, `--no-first-run`, `--no-default-browser-check`, the URL) plus the fix
  `--force-renderer-accessibility` (`run.py --chrome-mode bots`), on `:98`.
- Terra (`gpt-6.1-terra`, effort low): baseline vs final interleaved, 3 reps per task, both suites.
  Easy-suite bar unchanged from above (>= 2x throughput, success not lower, no pass->fail);
  hard-suite bar as above.
- Claude Sonnet 5.5 (`claude-sonnet-5-5`, effort medium, isolated config dir): baseline vs final
  interleaved, 2 reps per task, both suites. Reported, not part of the bar.
- Flag dependence (reported, not part of the bar): baseline and final on the easy suite with the
  bots' current flags (`--chrome-mode bots-noa11y`, no accessibility flag), 2 reps.

---

# Sol 5x (2026-10-31)

Pre-registered and committed before any change to the MCP server on branch `feat/cu-sol-5x`.
The agent the bots run for computer use is now GPT-6.1 Sol; this section measures for it.

## Setup

- Agent under test: `gpt-6.1-sol`, reasoning effort `low`, via `codex exec` (same isolated
  CODEX_HOME and disabled features as above; only the `cu` MCP server).
- Baseline: the binary installed today in `~/.local/share/computer-use-linux-fork/bin/computer-use-linux`
  (built from 45bb968, v9 + per-display input lock; sha256 `77bf7316…92b8cc`), frozen as a copy
  before the first run.
- Chrome: `run.py --chrome-mode bots` (chrome-perfil flags + `--force-renderer-accessibility`), display `:98`.
- Suites: easy (10 tasks, timeout 360 s) and hard (12 tasks, timeout 300 s), unchanged.
- Final comparison: baseline and final interleaved episode by episode, 3 reps per task per variant,
  both suites (`MODEL=gpt-6.1-sol EFFORT=low MIN_RATIO=1.5 run_all.sh <out> <baseline> <final> codex 3`).

## Primary metric

Throughput = successful episodes / agent wall-clock minutes, per suite, as defined above.

## The bar to install

Per suite (easy and hard each):

1. throughput(final) >= 1.5 x throughput(baseline), and
2. success rate(final) >= success rate(baseline), and
3. no task passes for the baseline (strict majority) and fails for the final.

Both suites must meet all three. `summarize.py --min-ratio 1.5` prints `bar met: True/False`.
Stretch goal (reported, not required): 5x throughput.

## Secondary (reported, not part of the bar)

- Turns per episode (model turns = MCP calls + 1 final answer), MCP ms per call, response bytes,
  input tokens.
- Claude confirmation: `claude-sonnet-5-5` (effort medium), baseline vs final interleaved, 2 reps,
  both suites; the check is "no regression" (success not lower, no pass->fail).

## Long suite (secondary, pre-registered 2026-11-01 before any agent episode on it)

Added at the dispatcher's request: the easy and hard tasks take 6-8 model turns, so the ~7 s of fixed
cost per episode (first and final turn) dominates their ratio; real bot work is longer.

- 5 tasks in `tasks_long.py` (`long_tasks/<name>.py`, pages under `apps/long/`), each 22-36 UI actions on
  the happy path: a 4-step onboarding wizard, a 5-line expense report, an inbox triage with distractors,
  CRM edits across two pages plus a delete, and a shop checkout. Checkers read posted state; proven by
  `selftest_long.mjs` + `check_selftest_long.py` (happy path accepted, `{}` and every WRONG case rejected).
- Agent `gpt-6.1-sol` low via `codex exec`, `--chrome-mode bots`, timeout 600 s, baseline (45bb968) vs final
  interleaved, 2 reps per task.
- Reported separately: throughput ratio, success rate, turns and wall time per task. Not part of the
  install bar above and not mixed into it.

---

# argo35: 35 general desktop tasks, hill climb to 100% with Terra (pre-registered 2026-11-01)

Written and committed before any agent episode on the suite and before any change to the MCP
server on branch `feat/cu-argo35-terra` (from `feat/cu-sol-5x` at `d494628`).

## Why

@argofowl ran 35 desktop tasks on a Mac (https://x.com/argofowl/status/2116818055607025905):
agent-desktop (Opus 5.5) 26/35, his tuned cua 33/35, Codex computer use with `gpt-6.1-terra` 34/35.
agent-desktop failed, without the real mouse, on three categories: drag, canvas clicks and scrolling
a huge list. His task list is not public, so `argo35` reconstructs its spirit on this host's Xvfb
`:98` (Chrome pages served by `server.py`, plus Alacritty), with deterministic checkers.

## Suite

- 35 tasks in `tasks_argo35.py` (modules `argo35_tasks/<name>.py`, pages `apps/argo35/`), specified in
  `argo35_tasks/SPECS.md`. Categories (tasks): drag 6, canvas 6, virtual-list 5, context-menu 2,
  shortcut 2, multi-window 3 (popup, tabs, Chrome + terminal), dialog 2, iframe 2, upload-download 2,
  table 1, rich-text 1, date-picker 1, hover 1, keyboard-only 1.
- Every checker reads the state the page posted (and, for the download and terminal tasks, files under
  the episode root), never the agent's reply. `check_selftest_argo35.py` (through `heavy.sh`) proves
  each one accepts its Playwright happy path and rejects `{}` and at least 3 realistic wrong states.
- Timeout 300 s per episode (`--timeout 300`), `--chrome-mode bots`.

## Train / holdout split

Drawn once by `random.Random(20261101).sample(sorted(MODULES), 10)` (`tasks_argo35.py`), before any
iteration:

- holdout (10): canvas_connect, canvas_map_pin, canvas_signature, ctx_submenu, datepicker_range,
  kbd_gmail, kbd_palette, rte_format, upload_hidden_input, vlist_listbox
- train (25): every other module.

Hill-climb iterations run only `--suite argo35-train`. The holdout runs only in the final measurement.
If a task has to be fixed after this commit (a broken page or checker, not difficulty), the fix and
its reason go to `results/argo35/log.tsv`, and the task's earlier runs are discarded.

## Agent

`--agent codex --model gpt-6.1-terra`, reasoning effort `low` (the catalog default), isolated
CODEX_HOME, only the `cu` MCP server, as in the sections above.

## The bar

1. **argo35:** the final binary passes 35/35 tasks in each of 3 reps (105/105 episodes), Terra low,
   one run of the full suite (`--suite argo35 --reps 3`).
2. **No regression with Sol:** on the easy (3 reps), hard (3 reps) and long (1 rep, timeout 600 s)
   suites with `gpt-6.1-sol` low, no task passes (strict majority) for `d494628` and fails for the
   final. The `d494628` side is the pre-registered Sol 5x final run of that code
   (`results/sol-5x-final/{easy,hard,long}-runs.jsonl`, variant `final`, binary `4334fc2` = same
   server source as `d494628`); the final binary runs the same suites, reps and flags.

Throughput (successful episodes per agent-minute) is reported, not part of the bar.

## Reported, not part of the bar

- Baselines on all 35 tasks with Terra low, 1 rep each, in the final run (interleaved with the final
  binary): the installed binary `45bb968` (sha256 `77bf7316…92b8cc`) and `d494628`
  (sha256 `d7e879d2…1615be`, the `4334fc2` build).
- Per category: success, median wall time, MCP calls.
- Optional: vercel-labs/agent-desktop driven by Terra on argo35, if it installs into a local prefix.
