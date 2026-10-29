| variant | runs | success | success rate | agent min | **tasks/min** | mean wall s | MCP calls | screenshots | input tokens | MCP ms/call |
|---|---|---|---|---|---|---|---|---|---|---|
| baseline | 12 | 8 | 67% | 6.8 | **1.170** | 34 | 18.1 | 2.3 | 444k | 364 |
| v2 | 12 | 11 | 92% | 5.4 | **2.030** | 27 | 6.0 | 0.2 | 204k | 1763 |

| task | baseline ok | baseline median s | baseline calls | baseline shots | v2 ok | v2 median s | v2 calls | v2 shots |
|---|---|---|---|---|---|---|---|---|
| accents-form | 0/1 | 33 | 22.0 | 1.0 | 1/1 | 17 | 3.0 | 0.0 |
| async-autocomplete | 1/1 | 25 | 17.0 | 3.0 | 1/1 | 21 | 6.0 | 0.0 |
| drag-drop | 0/1 | 27 | 4.0 | 2.0 | 1/1 | 17 | 3.0 | 0.0 |
| file-upload | 0/1 | 53 | 14.0 | 5.0 | 1/1 | 14 | 3.0 | 0.0 |
| flaky-field | 1/1 | 57 | 25.0 | 4.0 | 0/1 | 16 | 4.0 | 0.0 |
| modal-focus | 1/1 | 19 | 15.0 | 1.0 | 1/1 | 27 | 6.0 | 0.0 |
| multi-tab | 1/1 | 27 | 11.0 | 5.0 | 1/1 | 14 | 5.0 | 0.0 |
| recover-error | 1/1 | 26 | 13.0 | 1.0 | 1/1 | 70 | 14.0 | 0.0 |
| rich-compose | 0/1 | 67 | 68.0 | 2.0 | 1/1 | 31 | 14.0 | 2.0 |
| slow-page | 1/1 | 22 | 6.0 | 3.0 | 1/1 | 20 | 5.0 | 0.0 |
| virtual-list | 1/1 | 29 | 7.0 | 0.0 | 1/1 | 64 | 5.0 | 0.0 |
| xorigin-iframe | 1/1 | 27 | 15.0 | 1.0 | 1/1 | 15 | 4.0 | 0.0 |

**v2 vs baseline: tasks/min ratio = 1.73x**; success rate 67% -> 92%; tasks regressed pass->fail: ['flaky-field']; bar met: False
