#!/usr/bin/env python3
"""Run one benchmark episode: fresh app, fresh agent session, checker, metrics.

Called by run.py (inside the heavy.sh wrapper). Reads a JSON spec and writes a
JSON result next to it. Starts and stops its own Chrome/Alacritty; never
touches any display other than the bench one.
"""
import json
import os
import shutil
import signal
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import strip_images  # noqa: E402
import tasks  # noqa: E402

CHROME = sorted(Path.home().glob(".cache/ms-playwright/chromium-*/chrome-linux64/chrome"))[-1]
PREAMBLE = (
    "You are operating a Linux desktop through the `cu` computer-use MCP tools; use only those tools. "
    "{context} Task: {prompt} "
    "Do not ask questions; finish the task completely, then reply with one short line saying what you did."
)


def session_env(root: Path) -> dict:
    out = subprocess.run([str(HERE / "env.sh"), "vars"], capture_output=True, text=True, check=True,
                         env={**os.environ, "BENCH_ROOT": str(root)}).stdout
    env = {}
    for line in out.splitlines():
        if line.startswith("export "):
            for part in line[len("export "):].split(" "):
                key, _, value = part.partition("=")
                env[key] = value.strip("'")
    return env


def http(url: str, data: bytes | None = None) -> bytes:
    req = urllib.request.Request(url, data=data, method="POST" if data is not None else "GET")
    with urllib.request.urlopen(req, timeout=5) as resp:
        return resp.read()


def window_count(env: dict) -> int:
    out = subprocess.run(["i3-msg", "-t", "get_tree"], capture_output=True, text=True, env={**os.environ, **env})
    return out.stdout.count('"window_type":"normal"') if out.returncode == 0 else 0


def chrome_args(profile: Path, mode: str) -> list[str]:
    """Chrome command line for an episode.

    "bots" reproduces ~/.local/bin/chrome-perfil exactly (plus the accessibility
    flag the fix adds); "bots-noa11y" is chrome-perfil as it is today; "bench"
    adds a few quieting flags used by the first runs.
    """
    base = [str(CHROME), f"--user-data-dir={profile}", "--no-first-run", "--no-default-browser-check"]
    if mode == "bots":
        return base + ["--force-renderer-accessibility"]
    if mode == "bots-noa11y":
        return base
    if mode == "bench":
        return base + ["--password-store=basic", "--disable-sync", "--disable-search-engine-choice-screen",
                       "--disable-features=Translate,MediaRouter", "--force-renderer-accessibility"]
    raise ValueError(f"unknown chrome mode {mode!r}")


def seed_profile(profile: Path, root: Path) -> None:
    """Point the throwaway profile's downloads at root/downloads without a save dialog."""
    (profile / "Default").mkdir(parents=True, exist_ok=True)
    prefs = {"download": {"default_directory": str(root / "downloads"), "prompt_for_download": False,
                          "directory_upgrade": True}, "savefile": {"default_directory": str(root / "downloads")}}
    (profile / "Default" / "Preferences").write_text(json.dumps(prefs))


def launch_app(task: tasks.Task, spec: dict, env: dict, root: Path) -> list[subprocess.Popen]:
    """Open the task's app(s): Chrome on its page, Alacritty in root/term, or both ("web+terminal")."""
    full_env = {**os.environ, **env}
    procs = []
    if task.kind in ("terminal", "web+terminal"):
        procs.append(subprocess.Popen(["alacritty", "--working-directory", str(root / "term"), "--title", "Terminal"],
                                      env=full_env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                      start_new_session=True))
        deadline = time.time() + 20
        while time.time() < deadline and window_count(env) < 1:
            time.sleep(0.2)
    if task.kind in ("web", "web+terminal"):
        profile = root / "profiles" / spec["run_id"]
        shutil.rmtree(profile, ignore_errors=True)
        profile.mkdir(parents=True)
        seed_profile(profile, root)
        url = f"http://127.0.0.1:{spec['port']}/apps/{task.page}"
        args = chrome_args(profile, spec.get("chrome_mode", "bench"))
        procs.append(subprocess.Popen(args + [url], env=full_env, stdout=subprocess.DEVNULL,
                                      stderr=subprocess.DEVNULL, start_new_session=True))
    deadline = time.time() + 20
    while time.time() < deadline and window_count(env) < len(procs):
        time.sleep(0.2)
    time.sleep(2.5 if task.kind != "terminal" else 1.0)  # first paint + AT-SPI registration
    return procs


def stop_app(procs: list[subprocess.Popen], spec: dict, root: Path) -> None:
    for proc in procs:
        try:
            os.killpg(proc.pid, signal.SIGTERM)
            proc.wait(timeout=5)
        except (ProcessLookupError, subprocess.TimeoutExpired):
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
    shutil.rmtree(root / "profiles" / spec["run_id"], ignore_errors=True)


def mcp_env_toml(env: dict) -> str:
    keys = ["DISPLAY", "XDG_SESSION_TYPE", "XDG_CURRENT_DESKTOP", "XDG_RUNTIME_DIR",
            "DBUS_SESSION_BUS_ADDRESS", "I3SOCK"]
    return "{" + ",".join(f'{k}="{env[k]}"' for k in keys if k in env) + "}"


def codex_cmd(spec: dict, env: dict, prompt: str, mcp_log: Path) -> list[str]:
    server = ["python3", str(HERE / "mcp_proxy.py"), str(mcp_log), "--", spec["binary"], "mcp"]
    disable = ["shell_tool", "unified_exec", "computer_use", "browser_use", "in_app_browser",
               "image_generation", "apps", "plugins", "memories"]
    cmd = ["codex", "exec", "--json", "--ephemeral", "--skip-git-repo-check", "-C", spec["workdir"],
           "-m", spec["model"], "-c", f'model_reasoning_effort="{spec["effort"]}"',
           "-c", 'approval_policy="never"', "-s", "read-only"]
    for feature in disable:
        cmd += ["--disable", feature]
    cmd += ["-c", "mcp_servers.cu.command=" + json.dumps(server[0]),
            "-c", "mcp_servers.cu.args=" + json.dumps(server[1:]),
            "-c", 'mcp_servers.cu.default_tools_approval_mode="approve"',
            "-c", "mcp_servers.cu.tool_timeout_sec=120",
            "-c", "mcp_servers.cu.env=" + mcp_env_toml(env), prompt]
    return cmd


def claude_cmd(spec: dict, env: dict, prompt: str, mcp_log: Path, cfg_path: Path) -> list[str]:
    cfg = {"mcpServers": {"cu": {"command": "python3",
                                 "args": [str(HERE / "mcp_proxy.py"), str(mcp_log), "--", spec["binary"], "mcp"],
                                 "env": env}}}
    cfg_path.write_text(json.dumps(cfg))
    return ["claude", "-p", "--model", spec["model"], "--output-format", "stream-json", "--verbose",
            "--mcp-config", str(cfg_path), "--strict-mcp-config", "--setting-sources", "",
            "--tools", "", "--allowedTools", "mcp__cu", "--permission-mode", "bypassPermissions",
            "--no-session-persistence", "--effort", spec.get("effort", "medium")]  # prompt goes on stdin


def parse_codex(events: list[dict]) -> dict:
    tool_calls, tokens, final = [], {"input": 0, "cached": 0, "output": 0}, ""
    for e in events:
        item = e.get("item") or {}
        if e.get("type") == "item.completed" and item.get("type") == "mcp_tool_call":
            tool_calls.append({"tool": item.get("tool"), "error": bool(item.get("error"))})
        if e.get("type") == "item.completed" and item.get("type") == "agent_message":
            final = item.get("text", "")
        if e.get("type") == "turn.completed":
            u = e.get("usage") or {}
            tokens["input"] += u.get("input_tokens", 0)
            tokens["cached"] += u.get("cached_input_tokens", 0)
            tokens["output"] += u.get("output_tokens", 0) + u.get("reasoning_output_tokens", 0)
    return {"tool_calls": tool_calls, "tokens": tokens, "final": final}


def parse_claude(events: list[dict]) -> dict:
    tool_calls, tokens, final = [], {"input": 0, "cached": 0, "output": 0}, ""
    for e in events:
        if e.get("type") == "assistant":
            for block in (e.get("message") or {}).get("content") or []:
                if block.get("type") == "tool_use":
                    tool_calls.append({"tool": block.get("name", "").removeprefix("mcp__cu__"), "error": False})
        if e.get("type") == "result":
            final = e.get("result", "")
            u = e.get("usage") or {}
            tokens["input"] = u.get("input_tokens", 0) + u.get("cache_read_input_tokens", 0) + u.get("cache_creation_input_tokens", 0)
            tokens["cached"] = u.get("cache_read_input_tokens", 0)
            tokens["output"] = u.get("output_tokens", 0)
    return {"tool_calls": tool_calls, "tokens": tokens, "final": final}


def main() -> None:
    spec_path = Path(sys.argv[1])
    spec = json.loads(spec_path.read_text())
    root = Path(spec["root"])
    task = tasks.by_id(spec["task"])
    run_dir = spec_path.parent
    env = session_env(root)
    http(f"http://127.0.0.1:{spec['port']}/api/reset", b"{}")
    # Fresh terminal folder and download folder per episode; setup() may then fill them.
    for fresh in ("term", "downloads"):
        shutil.rmtree(root / fresh, ignore_errors=True)
        (root / fresh).mkdir(parents=True)
    if task.setup:
        task.setup(root)
    app = launch_app(task, spec, env, root)
    context = {"terminal": "An Alacritty terminal window is already open.",
               "web": "A Chrome window is already open on the app for this task.",
               "web+terminal": "A Chrome window is already open on the app for this task, and an Alacritty "
                               "terminal window is open next to it."}[task.kind]
    prompt = spec.get("prompt_override") or PREAMBLE.format(context=context, prompt=task.prompt)
    mcp_log = run_dir / "mcp.jsonl"
    agent_env = {**os.environ, "CODEX_HOME": spec["codex_home"],
                 "CLAUDE_CONFIG_DIR": spec.get("claude_home", str(root / "claude-home"))}
    if spec["agent"] == "codex":
        cmd = codex_cmd(spec, env, prompt, mcp_log)
    else:
        cmd = claude_cmd(spec, env, prompt, mcp_log, run_dir / "mcp-config.json")
    started = time.monotonic()
    started_at = time.time()
    timed_out = False
    with open(run_dir / "agent.jsonl", "w") as out, open(run_dir / "agent.err", "w") as err:
        stdin = subprocess.PIPE if spec["agent"] == "claude" else subprocess.DEVNULL
        proc = subprocess.Popen(cmd, stdout=out, stderr=err, stdin=stdin, env=agent_env,
                                cwd=spec["workdir"], start_new_session=True, text=True)
        if spec["agent"] == "claude":
            proc.stdin.write(prompt)
            proc.stdin.close()
        try:
            proc.wait(timeout=spec["timeout"])
        except subprocess.TimeoutExpired:
            timed_out = True
            os.killpg(proc.pid, signal.SIGTERM)
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(proc.pid, signal.SIGKILL)
                proc.wait()
    wall = time.monotonic() - started
    ended_at = time.time()
    time.sleep(0.5)  # let a final state POST land
    state = tasks.read_state(root / "state", task.id)
    try:
        ok, detail = task.check(state, root)
    except Exception as error:  # a malformed state is a failed run, not a crashed bench
        ok, detail = False, f"checker error: {error!r}"
    stop_app(app, spec, root)
    events = []
    for line in (run_dir / "agent.jsonl").read_text(errors="replace").splitlines():
        try:
            events.append(json.loads(line))
        except ValueError:
            pass
    parsed = parse_codex(events) if spec["agent"] == "codex" else parse_claude(events)
    calls = []
    if mcp_log.exists():
        for line in mcp_log.read_text().splitlines():
            entry = json.loads(line)
            if entry.get("method") == "tools/call":
                calls.append(entry)
    tools_list_bytes = next((json.loads(l)["bytes"] for l in mcp_log.read_text().splitlines()
                             if json.loads(l).get("method") == "tools/list"), None) if mcp_log.exists() else None
    result = {
        **{k: spec[k] for k in ("run_id", "variant", "task", "rep", "agent", "model", "effort")},
        "ok": ok,
        "detail": detail,
        "timed_out": timed_out,
        "exit_code": proc.returncode,
        "wall_s": round(wall, 2),
        "started_at": started_at,  # epoch seconds, to line up with mcp.jsonl "t"
        "ended_at": ended_at,
        "agent_tool_calls": len(parsed["tool_calls"]),
        "agent_tool_errors": sum(c["error"] for c in parsed["tool_calls"]),
        "mcp_calls": len(calls),
        "mcp_ms_total": round(sum(c["ms"] for c in calls), 1),
        "mcp_bytes_total": sum(c["bytes"] for c in calls),
        "screenshots": sum(1 for c in calls if c["images"] > 0),
        "tools_used": [c["tool"] for c in calls],
        "tools_list_bytes": tools_list_bytes,
        "tokens": parsed["tokens"],
        "final": parsed["final"][:500],
        "state": state,
    }
    (run_dir / "result.json").write_text(json.dumps(result, ensure_ascii=False, indent=1))
    strip_images.strip(run_dir / "agent.jsonl")


if __name__ == "__main__":
    main()
