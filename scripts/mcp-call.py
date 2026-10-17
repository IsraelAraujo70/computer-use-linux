#!/usr/bin/env python3
"""Call tools on a computer-use-linux MCP binary over stdio and print each result.

Usage: mcp-call.py [--bin PATH] [--stats] TOOL '{"json": "args"}' [TOOL ARGS ...]
Calls run in order in one server process, so element indices from a snapshot
carry over to later calls. --stats prints only per-content-block sizes.
"""
import argparse
import json
import subprocess
import sys

parser = argparse.ArgumentParser()
parser.add_argument("--bin", default="target/release/computer-use-linux")
parser.add_argument("--stats", action="store_true")
parser.add_argument("calls", nargs="+", help="TOOL [ARGS_JSON] pairs")
opts = parser.parse_args()
calls = []
for token in opts.calls:
    if token.lstrip().startswith("{") and calls:
        calls[-1][1] = json.loads(token)
    else:
        calls.append([token, {}])

proc = subprocess.Popen(
    [opts.bin, "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
    env={**__import__("os").environ, "COMPUTER_USE_LINUX_INDICATOR": "0"},
)


def send(message):
    proc.stdin.write(json.dumps(message) + "\n")
    proc.stdin.flush()


def receive(request_id):
    for line in proc.stdout:
        message = json.loads(line)
        if message.get("id") == request_id:
            return message
    sys.exit("server closed stdout")


send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
    "protocolVersion": "2025-06-18", "capabilities": {},
    "clientInfo": {"name": "mcp-call", "version": "0"}}})
receive(1)
send({"jsonrpc": "2.0", "method": "notifications/initialized"})
for request_id, (tool, arguments) in enumerate(calls, start=2):
    send({"jsonrpc": "2.0", "id": request_id, "method": "tools/call",
          "params": {"name": tool, "arguments": arguments}})
    response = receive(request_id)
    if len(calls) > 1:
        print(f"### {tool}")
    if "error" in response:
        print(json.dumps(response["error"]))
        continue
    blocks = response["result"].get("content", [])
    for block in blocks:
        if opts.stats:
            print(f"{block['type']}: {len(block.get('text') or block.get('data') or '')} chars")
        elif block["type"] == "text":
            print(block["text"])
        else:
            print(f"<{block['type']} {block.get('mimeType')} {len(block.get('data', ''))} b64 chars>")
proc.stdin.close()
proc.wait(timeout=10)
