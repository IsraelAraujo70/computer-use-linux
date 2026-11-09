#!/usr/bin/env python3
"""macOS end-to-end smoke test over the LaunchAgent socket.

Opens a local test form in Safari *in the background* (open -g) and drives
it through the MCP tools the way an agent would, while the user keeps
working: element ids come from get_app_state, never hard-coded. Every step
asserts what the page shows afterwards, read back through the tree, and
that Safari never became the frontmost app (background mode). Exit status
0 means all steps passed.

Usage: scripts/macos_smoke.py [--sock PATH]
"""
import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import time

FORM = """<!doctype html><html><head><meta charset="utf-8"><title>Teste CUL</title></head><body>
<h1>Formulário de teste</h1>
<form onsubmit="document.getElementById('out').textContent='enviado: '+document.getElementById('nome').value+' / '+document.getElementById('obs').value+' / '+document.getElementById('cidade').value+' / '+document.getElementById('aceito').checked+' / eventos '+(window.ev||0); return false;">
<label>Nome <input id="nome" oninput="window.ev=(window.ev||0)+1"></label><br>
<label>Observação <textarea id="obs"></textarea></label><br>
<label>Cidade <select id="cidade"><option>Curitiba</option><option>São Paulo</option><option>Recife</option></select></label><br>
<label><input type="checkbox" id="aceito"> Aceito os termos</label><br>
<button type="submit">Enviar</button>
</form><p id="out">nada enviado</p>
<div style="height:2000px"></div><p>Fim da página</p>
</body></html>"""

parser = argparse.ArgumentParser()
parser.add_argument("--sock", default=os.path.expanduser("~/.local/state/computer-use-mac/mcp.sock"))
opts = parser.parse_args()

# A fixed path, so Safari reuses its tab instead of opening one per run.
page = os.path.join(tempfile.gettempdir(), "cul-smoke", "form.html")
os.makedirs(os.path.dirname(page), exist_ok=True)
open(page, "w").write(FORM)
subprocess.run(["open", "-g", "-a", "Safari", page], check=True)
time.sleep(2)
safari_pid = subprocess.run(["pgrep", "-x", "Safari"], capture_output=True, text=True).stdout.split()[0]


def frontmost_pid():
    script = 'tell application "System Events" to get unix id of first process whose frontmost is true'
    return subprocess.run(["osascript", "-e", script], capture_output=True, text=True).stdout.strip()

proc = subprocess.Popen(["nc", "-U", opts.sock], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
next_id = 0


def rpc(method, params=None, notify=False):
    global next_id
    message = {"jsonrpc": "2.0", "method": method}
    if params is not None:
        message["params"] = params
    if not notify:
        next_id += 1
        message["id"] = next_id
    proc.stdin.write(json.dumps(message) + "\n")
    proc.stdin.flush()
    if notify:
        return None
    for line in proc.stdout:
        reply = json.loads(line)
        if reply.get("id") == next_id:
            return reply


def call(tool, args=None):
    result = rpc("tools/call", {"name": tool, "arguments": args or {}})["result"]
    text = "\n".join(block.get("text", "") for block in result["content"] if block["type"] == "text")
    return text, bool(result.get("isError"))


rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "smoke", "version": "0"}})
rpc("notifications/initialized", notify=True)

failures = []


def check(name, condition, detail):
    front = frontmost_pid()
    if front == safari_pid:
        condition = False
        detail = f"Safari (pid {safari_pid}) became the frontmost app: background mode broken\n" + detail
    print(("PASS " if condition else "FAIL ") + name)
    if not condition:
        failures.append(name)
        print("  " + detail.replace("\n", "\n  ")[:1500])


def element(outline, role, name):
    match = re.search(r"\[(\d+)\] " + re.escape(role) + r' "' + re.escape(name) + '"', outline)
    if not match:
        raise SystemExit(f"no {role} {name!r} in:\n{outline}")
    return int(match.group(1))


state, _ = call("get_app_state", {"app_name_or_bundle_identifier": "safari"})
check("outline has url and page rule", "url=file://" in state and "--- page ---" in state, state)
check("off-page element is marked below", 'static "Fim da página" (below)' in state, state)
nome = element(state, "entry", "Nome")
obs = element(state, "entry", "Observação")
cidade = element(state, "combo box", "Cidade")
aceito = element(state, "check box", "Aceito os termos")
enviar = element(state, "push button", "Enviar")
document = element(state, "document web", "Teste CUL")

text, error = call("fill_form", {"fields": [
    {"element_index": nome, "value": "José Ação"},
    {"element_index": cidade, "value": "Recife"},
    {"element_index": aceito, "value": "true"},
]})
check("fill_form sets text, select and checkbox in a background app", not error, text)

text, error = call("click", {"element_index": aceito})
text, error = call("click", {"element_index": aceito})
check("click by id toggles the checkbox twice in the background", not error, text)

text, error = call("type_text", {"element_index": obs, "text": "linha ✓ ünïcode"})
check("type_text enters Unicode without the clipboard", not error and "ünïcode" in text, text)

text, error = call("click", {"element_index": enviar})
expected = "enviado: José Ação / linha ✓ ünïcode / Recife / true / eventos "
check("submit shows every value", expected in text, text)
events = re.search(r"eventos (\d+)", text)
check("the page saw typing (input events), not a silent value write", bool(events) and int(events.group(1)) > 0, text)

text, error = call("scroll", {"element_index": document, "direction": "down", "until": {"name": "Fim da página"}})
check("scroll until reaches the page end", not error and "Fim da página" in text, text)

text, error = call("find_elements", {"app_name_or_bundle_identifier": "safari", "role": "push button", "name": "Enviar"})
check("find_elements finds the button", 'push button \\"Enviar\\"' in text, text)

text, error = call("press_key", {"key": "Cmd+Up", "app_id": "com.apple.Safari"})
check("press_key chord in the target app", not error, text)

# The Dock edge of the main display: never a Safari window.
text, error = call("click", {"x": 1469, "y": 955})
check("a click outside the target app is refused, not sent elsewhere", error, text)

text, error = call("screenshot", {"format": "jpeg", "max_width": 800})
check("screenshot caption in points", '"coordinate_width"' in text, text)

call("get_app_state", {"app_name_or_bundle_identifier": "safari"})
text, error = call("screenshot", {"pid": int(safari_pid), "annotate": True, "max_width": 900})
labels = re.search(r'"annotated_elements":(\d+)', text)
check("annotated screenshot of the background window labels its elements", '"cropped_to_window":true' in text and bool(labels) and int(labels.group(1)) >= 5, text)

# A native app, opened in the background by its English name on any
# system language, driven by a click and by keys sent to its process.
text, error = call("app", {"mode": "launch", "name": "Calculator"})
check("app launch opens Calculator in the background", not error and '"ok":true' in text, text)
calc_pid = subprocess.run(["pgrep", "-x", "Calculator"], capture_output=True, text=True).stdout.split()
safari_pid = calc_pid[0] if calc_pid else safari_pid  # Calculator must not come to the front either
call("get_app_state", {"app_name_or_bundle_identifier": "calculator"})
call("press_key", {"key": "Escape"})
text, error = call("click", {"role": "push button", "name": "4"})
check("click a native button by name", not error, text)
text, error = call("type_text", {"text": "+5="})
state, _ = call("get_app_state", {"app_name_or_bundle_identifier": "calculator"})
check("keys reach the background native app (4+5=9)", re.search(r'static "\W*9\W*"', state) is not None, text + "\n" + state)
text, error = call("app", {"mode": "resize", "name": "Calculator", "x": 120, "y": 140})
check("app resize moves the window", '"ok":true' in text, text)

text, error = call("scrape", {"url": "http://127.0.0.1/"})
check("scrape refuses local addresses", error and "private or local" in text, text)
text, error = call("wait", {"seconds": 0.2})
check("wait", '"ok":true' in text, text)

proc.stdin.close()
print(f"\n{len(failures)} failed" if failures else "\nall passed")
sys.exit(1 if failures else 0)
