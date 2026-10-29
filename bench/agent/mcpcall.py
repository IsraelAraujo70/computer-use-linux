#!/usr/bin/env python3
"""Minimal MCP stdio client for probing a server by hand.

Usage: mcpcall.py <server-cmd> [tool json-args]...  (pairs; prints each result's text)
"""
import json
import subprocess
import sys
import time

server = sys.argv[1].split()
pairs = list(zip(sys.argv[2::2], sys.argv[3::2]))
p = subprocess.Popen(server + ["mcp"] if len(server) == 1 else server, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
n = 0


def rpc(method, params, notify=False):
    global n
    msg = {"jsonrpc": "2.0", "method": method, "params": params}
    if not notify:
        n += 1
        msg["id"] = n
    p.stdin.write(json.dumps(msg) + "\n")
    p.stdin.flush()
    if notify:
        return None
    while True:
        r = json.loads(p.stdout.readline())
        if r.get("id") == n:
            return r


t0 = time.monotonic()
rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "probe", "version": "0"}})
rpc("notifications/initialized", {}, notify=True)
print(f"# init {1000*(time.monotonic()-t0):.0f} ms", file=sys.stderr)
for tool, args in pairs:
    t0 = time.monotonic()
    if tool == "LIST":
        r = rpc("tools/list", {})
        for t in r["result"]["tools"]:
            print(t["name"], len(json.dumps(t)))
        continue
    r = rpc("tools/call", {"name": tool, "arguments": json.loads(args)})
    print(f"# {tool} {1000*(time.monotonic()-t0):.0f} ms", file=sys.stderr)
    for c in r.get("result", {}).get("content", []):
        if c["type"] == "text":
            print(c["text"])
        else:
            print(f"<{c['type']} {len(c.get('data',''))} b64 bytes>")
    if "error" in r:
        print("ERROR", r["error"])
p.stdin.close()
p.wait(timeout=10)
