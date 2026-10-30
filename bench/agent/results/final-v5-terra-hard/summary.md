| variant | runs | success | success rate | agent min | **tasks/min** | mean wall s | MCP calls | screenshots | input tokens | MCP ms/call |
|---|---|---|---|---|---|---|---|---|---|---|
| baseline | 36 | 24 | 67% | 21.7 | **1.105** | 36 | 18.9 | 2.6 | 473k | 310 |
| final | 36 | 36 | 100% | 12.3 | **2.934** | 20 | 4.9 | 0.0 | 176k | 764 |

| task | baseline ok | baseline median s | baseline calls | baseline shots | final ok | final median s | final calls | final shots |
|---|---|---|---|---|---|---|---|---|
| accents-form | 0/3 | 39 | 24.0 | 1.3 | 3/3 | 19 | 3.0 | 0.0 |
| async-autocomplete | 3/3 | 29 | 17.0 | 3.0 | 3/3 | 25 | 7.0 | 0.0 |
| drag-drop | 0/3 | 25 | 4.0 | 2.0 | 3/3 | 15 | 3.0 | 0.0 |
| file-upload | 0/3 | 50 | 14.3 | 6.3 | 3/3 | 15 | 3.0 | 0.0 |
| flaky-field | 3/3 | 43 | 26.7 | 5.0 | 3/3 | 18 | 5.0 | 0.0 |
| modal-focus | 3/3 | 24 | 15.7 | 2.0 | 3/3 | 17 | 5.0 | 0.0 |
| multi-tab | 3/3 | 41 | 12.0 | 1.7 | 3/3 | 19 | 5.0 | 0.0 |
| recover-error | 3/3 | 36 | 15.3 | 1.7 | 3/3 | 29 | 11.0 | 0.0 |
| rich-compose | 0/3 | 58 | 69.3 | 1.7 | 3/3 | 18 | 4.0 | 0.0 |
| slow-page | 3/3 | 21 | 6.0 | 3.0 | 3/3 | 19 | 5.0 | 0.0 |
| virtual-list | 3/3 | 39 | 7.7 | 0.0 | 3/3 | 23 | 4.3 | 0.0 |
| xorigin-iframe | 3/3 | 31 | 15.0 | 4.0 | 3/3 | 20 | 4.0 | 0.0 |

**final vs baseline: tasks/min ratio = 2.66x**; success rate 67% -> 100%; tasks regressed pass->fail: none; bar met: True
