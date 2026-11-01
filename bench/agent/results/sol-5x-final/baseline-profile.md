# Where a Sol episode spends its time (baseline 45bb968, gpt-6.1-sol low, 1 rep, 22 episodes)

Source: `bench/agent/profile.py /tmp/cu-sol-5x/base1/easy /tmp/cu-sol-5x/base1/hard` (per-episode table in profile-table.md).
Timeline per episode from mcp.jsonl timestamps + run_one started_at/ended_at.

| suite | eps | success | mean wall s | boot s | model s | MCP s | model turns | s per turn | MCP KB |
|---|---|---|---|---|---|---|---|---|---|
| easy | 10 | 10/10 | 18.2 | 0.3 | 14.2 (78%) | 3.7 (20%) | 6.6 | 2.2 | 17.6 |
| hard | 12 | 12/12 | 19.9 | 0.2 | 15.2 (77%) | 4.5 (22%) | 7.6 | 2.0 | 6.9 |

Turn costs (medians over all 22 episodes): first turn (CLI start + first sampling, ends with the first tool call) 4.8 s;
each later turn 1.5 s; final reply 2.6 s. So ~7.4 s per episode is fixed (first + final), whatever the server does.

## Where the time is

1. Model turns: 77-78%. Every MCP call is one Sol turn (Sol low emits no reasoning items and never batches
   tool calls). 6.6-7.6 turns per task.
2. MCP: 20-22%, dominated by a few calls: scroll-until on long lists (contacts-infinite 12.9 s, virtual-list 15.2 s),
   fill_form ~0.5 s per field, settle ~0.2-0.3 s per action.
3. Bytes are small already (7-18 KB per episode); not the limiter.

## Avoidable turns (from the traces)

- Verification turns after the last action: mail-reply, calendar-event, mail-label (x2), modal-focus,
  drag-drop, async-autocomplete, slow-page, recover-error (many). Root cause found for the mail/calendar ones:
  Chrome names ARIA role=status `statusbar`, the change report only ranked `status bar` as a notice, so toasts
  ("Mensagem enviada", "Evento salvo") were cut from long reports and Sol went looking for them.
- Sequential single actions the agent already knew how to do: open mail -> Responder -> type -> Enviar (4 turns),
  Criar -> fill -> date picker -> month -> day -> Salvar (8 turns), settings menu -> dialog -> fields -> save.
  With a batch tool that accepts role/name for not-yet-visible elements these are 1 turn.
- Unavoidable: the first observation (get_app_state) and the final reply.

## Ceiling

Best case per task = first turn (4.8) + one act turn (~2) + final (2.6) + MCP (~2-3) ≈ 11-12 s vs 18-20 s:
about 1.6-1.8x. 5x would need ~3.6-4 s per episode, below the fixed cost of the first and final turns
(7.4 s) that the server cannot touch. The stretch goal is out of reach with this agent loop; the bar (1.5x) is not.
