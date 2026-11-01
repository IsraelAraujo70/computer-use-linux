## argo35 overall
| variant | episodes | success | train | holdout | agent min | succ/min | median s | calls |
|---|---|---|---|---|---|---|---|---|
| b45bb968 | 35 | 33/35 | 24/25 | 9/10 | 18.5 | 1.79 | 16 | 7.4 |
| d494628 | 35 | 34/35 | 24/25 | 10/10 | 19.4 | 1.75 | 15 | 5.7 |
| final | 105 | 105/105 | 75/75 | 30/30 | 40.3 | 2.61 | 17 | 5.1 |

## per category (successful episodes)
| category | tasks | b45bb968 | d494628 | final |
|---|---|---|---|---|
| drag | 6 | 6/6 | 6/6 | 18/18 |
| canvas | 6 | 6/6 | 6/6 | 18/18 |
| virtual-list | 5 | 4/5 | 4/5 | 15/15 |
| context-menu | 2 | 2/2 | 2/2 | 6/6 |
| shortcut | 2 | 2/2 | 2/2 | 6/6 |
| multi-window | 3 | 3/3 | 3/3 | 9/9 |
| dialog | 2 | 2/2 | 2/2 | 6/6 |
| iframe | 2 | 2/2 | 2/2 | 6/6 |
| upload-download | 2 | 2/2 | 2/2 | 6/6 |
| table | 1 | 1/1 | 1/1 | 3/3 |
| rich-text | 1 | 0/1 | 1/1 | 3/3 |
| date-picker | 1 | 1/1 | 1/1 | 3/3 |
| hover | 1 | 1/1 | 1/1 | 3/3 |
| keyboard-only | 1 | 1/1 | 1/1 | 3/3 |

## per task
| task | set | b45bb968 ok / median s / calls | d494628 ok / median s / calls | final ok / median s / calls |
|---|---|---|---|---|
| a35-canvas-chart | train | 1/1 / 12 / 4 | 1/1 / 13 / 4 | 3/3 / 12 / 4 |
| a35-canvas-connect | holdout | 1/1 / 44 / 9 | 1/1 / 23 / 8 | 3/3 / 18 / 7 |
| a35-canvas-draw-rect | train | 1/1 / 14 / 5 | 1/1 / 14 / 4 | 3/3 / 22 / 4 |
| a35-canvas-map-pin | holdout | 1/1 / 14 / 4 | 1/1 / 15 / 5 | 3/3 / 17 / 5 |
| a35-canvas-seats | train | 1/1 / 12 / 5 | 1/1 / 12 / 5 | 3/3 / 13 / 5 |
| a35-canvas-signature | holdout | 1/1 / 48 / 34 | 1/1 / 35 / 23 | 3/3 / 33 / 16 |
| a35-ctx-rename | train | 1/1 / 13 / 6 | 1/1 / 13 / 4 | 3/3 / 11 / 4 |
| a35-ctx-submenu | holdout | 1/1 / 14 / 5 | 1/1 / 13 / 6 | 3/3 / 14 / 5 |
| a35-datepicker-range | holdout | 1/1 / 28 / 10 | 1/1 / 18 / 3 | 3/3 / 16 / 3 |
| a35-dialog-nested | train | 1/1 / 16 / 6 | 1/1 / 14 / 2 | 3/3 / 13 / 2 |
| a35-native-dialogs | train | 1/1 / 17 / 7 | 1/1 / 15 / 6 | 3/3 / 17 / 6 |
| a35-dnd-file-folder | train | 1/1 / 20 / 6 | 1/1 / 36 / 10 | 3/3 / 42 / 12 |
| a35-dnd-kanban-pointer | train | 1/1 / 16 / 3 | 1/1 / 13 / 3 | 3/3 / 12 / 3 |
| a35-dnd-reorder | train | 1/1 / 19 / 6 | 1/1 / 13 / 4 | 3/3 / 17 / 5 |
| a35-dnd-resize-panel | train | 1/1 / 11 / 3 | 1/1 / 12 / 3 | 3/3 / 11 / 3 |
| a35-dnd-slider | train | 1/1 / 15 / 5 | 1/1 / 18 / 5 | 3/3 / 20 / 5 |
| a35-dnd-upload-zone | train | 1/1 / 8 / 3 | 1/1 / 10 / 3 | 3/3 / 16 / 3 |
| a35-hover-tooltip | train | 1/1 / 13 / 4 | 1/1 / 14 / 3 | 3/3 / 14 / 4 |
| a35-iframe-nested | train | 1/1 / 14 / 3 | 1/1 / 9 / 2 | 3/3 / 10 / 2 |
| a35-iframe-scroll-terms | train | 1/1 / 12 / 4 | 1/1 / 11 / 3 | 3/3 / 11 / 3 |
| a35-kbd-only-otp | train | 1/1 / 11 / 4 | 1/1 / 9 / 2 | 3/3 / 8 / 2 |
| a35-popup-oauth | train | 1/1 / 26 / 12 | 1/1 / 26 / 12 | 3/3 / 27 / 13 |
| a35-tabs-compare | train | 1/1 / 22 / 9 | 1/1 / 17 / 5 | 3/3 / 17 / 5 |
| a35-term-code | train | 1/1 / 20 / 6 | 1/1 / 12 / 3 | 3/3 / 12 / 4 |
| a35-rte-format | holdout | 0/1 / 18 / 12 | 1/1 / 16 / 4 | 3/3 / 27 / 4 |
| a35-kbd-gmail | holdout | 1/1 / 29 / 13 | 1/1 / 44 / 9 | 3/3 / 33 / 11 |
| a35-kbd-palette | holdout | 1/1 / 23 / 13 | 1/1 / 26 / 9 | 3/3 / 29 / 9 |
| a35-table-sort-edit | train | 1/1 / 15 / 7 | 1/1 / 20 / 6 | 3/3 / 15 / 6 |
| a35-download-csv | train | 1/1 / 12 / 4 | 1/1 / 9 / 2 | 3/3 / 8 / 2 |
| a35-upload-hidden-input | holdout | 1/1 / 8 / 3 | 1/1 / 13 / 3 | 3/3 / 10 / 3 |
| a35-vlist-10k | train | 1/1 / 87 / 8 | 1/1 / 89 / 8 | 3/3 / 30 / 6 |
| a35-vlist-grid | train | 1/1 / 43 / 16 | 1/1 / 150 / 11 | 3/3 / 75 / 5 |
| a35-vlist-infinite | train | 1/1 / 77 / 3 | 1/1 / 78 / 3 | 3/3 / 44 / 3 |
| a35-vlist-listbox | holdout | 1/1 / 55 / 6 | 1/1 / 39 / 7 | 3/3 / 22 / 4 |
| a35-vlist-logs | train | 0/1 / 300 / 11 | 0/1 / 300 / 11 | 3/3 / 80 / 3 |

## failures
- b45bb968-a35-rte-format-r0: Exact formatting and unchanged text required
- b45bb968-a35-vlist-logs-r0: Requested state does not match (timeout)
- d494628-a35-vlist-logs-r0: Requested state does not match (timeout)

argo35 bar (35 tasks x 3 reps, every episode passes) for final: 105/105 episodes, 35 tasks -> met: True
