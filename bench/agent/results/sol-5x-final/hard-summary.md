| variant | runs | success | success rate | agent min | **tasks/min** | mean wall s | MCP calls | screenshots | MCP KB | input tokens | MCP ms/call |
|---|---|---|---|---|---|---|---|---|---|---|---|
| baseline | 36 | 36 | 100% | 14.1 | **2.560** | 23 | 5.9 | 0.0 | 6.4 | 169k | 1109 |
| final | 36 | 36 | 100% | 12.8 | **2.809** | 21 | 3.1 | 0.0 | 8.1 | 138k | 2302 |

| task | baseline ok | baseline median s | baseline calls | baseline shots | final ok | final median s | final calls | final shots |
|---|---|---|---|---|---|---|---|---|
| accents-form | 3/3 | 16 | 3.0 | 0.0 | 3/3 | 10 | 1.0 | 0.0 |
| async-autocomplete | 3/3 | 25 | 7.0 | 0.0 | 3/3 | 35 | 6.0 | 0.0 |
| drag-drop | 3/3 | 16 | 3.0 | 0.0 | 3/3 | 13 | 2.0 | 0.0 |
| file-upload | 3/3 | 14 | 3.3 | 0.0 | 3/3 | 17 | 2.0 | 0.0 |
| flaky-field | 3/3 | 16 | 5.0 | 0.0 | 3/3 | 10 | 1.3 | 0.0 |
| modal-focus | 3/3 | 15 | 4.0 | 0.0 | 3/3 | 16 | 2.7 | 0.0 |
| multi-tab | 3/3 | 25 | 5.3 | 0.0 | 3/3 | 20 | 2.0 | 0.0 |
| recover-error | 3/3 | 49 | 21.7 | 0.0 | 3/3 | 33 | 8.0 | 0.3 |
| rich-compose | 3/3 | 19 | 5.0 | 0.0 | 3/3 | 22 | 3.0 | 0.0 |
| slow-page | 3/3 | 26 | 6.0 | 0.0 | 3/3 | 21 | 5.0 | 0.0 |
| virtual-list | 3/3 | 29 | 4.0 | 0.0 | 3/3 | 33 | 3.3 | 0.0 |
| xorigin-iframe | 3/3 | 21 | 4.0 | 0.0 | 3/3 | 15 | 1.0 | 0.0 |

**final vs baseline: tasks/min ratio = 1.10x**; success rate 100% -> 100%; tasks regressed pass->fail: none; bar met (>= 1.5x): False
