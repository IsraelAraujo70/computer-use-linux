| variant | runs | success | success rate | agent min | **tasks/min** | mean wall s | MCP calls | screenshots | input tokens | MCP ms/call |
|---|---|---|---|---|---|---|---|---|---|---|
| baseline | 36 | 24 | 67% | 21.9 | **1.094** | 37 | 19.8 | 2.9 | 485k | 429 |
| v3 | 36 | 36 | 100% | 13.3 | **2.705** | 22 | 5.6 | 0.1 | 190k | 1010 |

| task | baseline ok | baseline median s | baseline calls | baseline shots | v3 ok | v3 median s | v3 calls | v3 shots |
|---|---|---|---|---|---|---|---|---|
| accents-form | 0/3 | 34 | 60.0 | 1.0 | 3/3 | 17 | 3.0 | 0.0 |
| async-autocomplete | 3/3 | 29 | 18.0 | 3.3 | 3/3 | 27 | 7.0 | 0.0 |
| drag-drop | 0/3 | 19 | 4.0 | 2.0 | 3/3 | 16 | 3.0 | 0.0 |
| file-upload | 0/3 | 51 | 14.3 | 6.7 | 3/3 | 13 | 3.0 | 0.0 |
| flaky-field | 3/3 | 40 | 24.7 | 3.3 | 3/3 | 19 | 5.0 | 0.0 |
| modal-focus | 3/3 | 27 | 15.0 | 4.0 | 3/3 | 19 | 6.0 | 0.0 |
| multi-tab | 3/3 | 37 | 11.7 | 1.7 | 3/3 | 18 | 5.0 | 0.0 |
| recover-error | 3/3 | 55 | 18.3 | 2.3 | 3/3 | 27 | 10.7 | 0.0 |
| rich-compose | 0/3 | 51 | 43.0 | 4.3 | 3/3 | 32 | 12.0 | 1.3 |
| slow-page | 3/3 | 22 | 6.0 | 3.0 | 3/3 | 22 | 5.0 | 0.0 |
| virtual-list | 3/3 | 24 | 7.0 | 0.0 | 3/3 | 30 | 3.0 | 0.0 |
| xorigin-iframe | 3/3 | 23 | 15.0 | 3.0 | 3/3 | 15 | 4.0 | 0.0 |

**v3 vs baseline: tasks/min ratio = 2.47x**; success rate 67% -> 100%; tasks regressed pass->fail: none; bar met: True
