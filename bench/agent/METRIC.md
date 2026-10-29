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
