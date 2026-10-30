# What the agents asked for

Sources: interviews after benchmark episodes (`interview.py`: the agent finishes a task, then the
same session is resumed and asked six fixed questions; transcripts in `results/interviews-*/`),
traces of benchmark episodes (`results/*/runs/*/mcp.jsonl`), and real bot threads read on
2026-10-29 (Secretário and LinkedIn bots; read-only).

## Ranked by evidence, with status

| # | Wish (agents' words, condensed) | Evidence | Status |
|---|---|---|---|
| 1 | "A set-field / form-fill tool that works on web fields and returns the value actually in the field" | 4/4 Terra baseline interviews; 37 failed `set_value` calls in 30 baseline episodes ("element does not expose AT-SPI Value or EditableText") | Done: `set_value`/`fill_form` focus + select-all + type/paste, read back through AT-SPI Text; masks tolerated, dropped characters rejected |
| 2 | "Reliable Unicode text entry" ("não" arrived as "no") | Terra interview (order-support); baseline 0/3 on accents-form and rich-compose in every run | Done: CLIPBOARD owner + Ctrl+V; multi-line values pasted as one payload (no Enter) |
| 3 | "Know what an action changed without re-observing" | 226 `get_app_state` calls in 30 baseline episodes, nearly one after every action | Done: every action returns `+/-/~` changes, focus and title; busy-aware settle waits for "Salvando…" to finish |
| 4 | "Less output: no browser UI noise, no screenshot when the tree suffices" | get_app_state averaged 186 KB (tree + screenshot) at baseline; all interviews | Done: page-scoped text outline (1.4–6.4 KB), screenshot only when the tree is empty |
| 5 | "Actions on off-screen list items that do not silently miss" | Terra: off-screen option click returned `ok: true` and landed nowhere | Done: element action or ScrollTo, never ok for a click on no pixel |
| 6 | "Element ids that stay valid" | Agents re-read the page to re-find indices | Done for living elements (ids follow the AT-SPI object); an element the page re-renders gets a new id (Sonnet noticed) |
| 7 | "Upload a file" / "drag a card" | Baseline 0/3 on file-upload and drag-drop; no file dialog exists on the VPS (no GTK, no portal) | Done: `drop_files` (XDND), stepped `drag` by element ids |
| 8 | "Find an item in a virtualized list directly" | Terra and Sonnet (virtual-list) | Done: `scroll(until=…)`; Sonnet asks that `find_elements` point to it when nothing matches |
| 9 | "Tell me a dialog is in the way" | Terra (modal-focus): "Cached AT-SPI action name is no longer available" | Done: errors name the dialog and its buttons |
| 10 | "Show off-screen rows" / "echo the clicked element's new state" | Final-binary interviews: Terra (mail-label, invoices) and Sonnet (mail-label) | In progress (n11/n12) |
| 11 | "A table/list reader returning rows with their action ids" | Terra (invoices, mail-label) | Partly covered by #10; not built |
| 12 | "Token/chip inputs: type and commit with Enter" | Sonnet (rich-compose): fill_form said "no option named …" | Hint in the error (n12) |
| 13 | "Diagnostics that do not send me to the wrong machine" | Secretário thread: "Could not find wayland compositor", wmctrl/hyprctl errors → a delegate stopped as BLOCKED | Done: X11-only doctor; blockers name the fix |
| 14 | "A way to open an app/URL" | Bots launched Chrome from Bash (Secretário) | Not built (chrome-perfil covers it) |
| 15 | "Smaller tool catalog" | Every Terra interview mentions long tool discovery | Reduced 76 KB → 21.6 KB; mostly Codex's own tool-search display now |
