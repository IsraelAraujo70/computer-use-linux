#!/usr/bin/env python3
"""Stdio MCP proxy that times every JSON-RPC request answered by the wrapped server.

Usage: mcp_proxy.py <log.jsonl> -- <server command...>
Each tools/call gets one JSON line: tool name, argument keys, latency in ms,
response bytes, number of image blocks, and whether the result was an error.
"""
import json
import subprocess
import sys
import threading
import time

log_path = sys.argv[1]
cmd = sys.argv[sys.argv.index("--") + 1 :]
proc = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE)
pending = {}
lock = threading.Lock()
log = open(log_path, "a", buffering=1)


def client_to_server():
    for line in sys.stdin.buffer:
        try:
            msg = json.loads(line)
            if isinstance(msg, dict) and "id" in msg and "method" in msg:
                params = msg.get("params") or {}
                with lock:
                    pending[msg["id"]] = (
                        time.monotonic(),
                        msg["method"],
                        params.get("name"),
                        params.get("arguments"),
                    )
        except ValueError:
            pass
        proc.stdin.write(line)
        proc.stdin.flush()
    proc.stdin.close()


def server_to_client():
    for line in proc.stdout:
        sys.stdout.buffer.write(line)
        sys.stdout.buffer.flush()
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        if not isinstance(msg, dict) or "id" not in msg:
            continue
        with lock:
            entry = pending.pop(msg["id"], None)
        if not entry:
            continue
        started, method, tool, args = entry
        result = msg.get("result") or {}
        content = result.get("content") or [] if isinstance(result, dict) else []
        log.write(
            json.dumps(
                {
                    "t": time.time(),
                    "method": method,
                    "tool": tool,
                    "args": args,
                    "ms": round((time.monotonic() - started) * 1000, 1),
                    "bytes": len(line),
                    "images": sum(1 for c in content if c.get("type") == "image"),
                    "is_error": bool(result.get("isError")) or "error" in msg,
                }
            )
            + "\n"
        )


threading.Thread(target=client_to_server, daemon=True).start()
server_to_client()
sys.exit(proc.wait())
