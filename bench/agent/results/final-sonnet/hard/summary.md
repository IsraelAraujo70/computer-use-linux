| variant | runs | success | success rate | agent min | **tasks/min** | mean wall s | MCP calls | screenshots | input tokens | MCP ms/call |
|---|---|---|---|---|---|---|---|---|---|---|
| baseline | 24 | 18 | 75% | 9.7 | **1.853** | 24 | 18.5 | 5.0 | 238k | 315 |
| final | 24 | 24 | 100% | 6.4 | **3.769** | 16 | 4.7 | 0.1 | 65k | 1367 |

| task | baseline ok | baseline median s | baseline calls | baseline shots | final ok | final median s | final calls | final shots |
|---|---|---|---|---|---|---|---|---|
| accents-form | 2/2 | 46 | 32.5 | 8.5 | 2/2 | 9 | 3.0 | 0.0 |
| async-autocomplete | 2/2 | 15 | 15.0 | 4.5 | 2/2 | 24 | 7.0 | 1.0 |
| drag-drop | 0/2 | 19 | 10.0 | 3.5 | 2/2 | 9 | 2.0 | 0.0 |
| file-upload | 0/2 | 40 | 30.0 | 9.5 | 2/2 | 9 | 3.0 | 0.0 |
| flaky-field | 2/2 | 23 | 26.0 | 6.0 | 2/2 | 10 | 3.0 | 0.0 |
| modal-focus | 2/2 | 15 | 14.0 | 3.5 | 2/2 | 8 | 3.0 | 0.0 |
| multi-tab | 2/2 | 20 | 10.0 | 3.0 | 2/2 | 11 | 5.0 | 0.0 |
| recover-error | 2/2 | 14 | 7.0 | 3.0 | 2/2 | 11 | 5.0 | 0.0 |
| rich-compose | 0/2 | 50 | 50.0 | 10.5 | 2/2 | 13 | 7.0 | 0.0 |
| slow-page | 2/2 | 15 | 6.5 | 3.0 | 2/2 | 14 | 5.0 | 0.5 |
| virtual-list | 2/2 | 21 | 10.0 | 2.5 | 2/2 | 64 | 9.0 | 0.0 |
| xorigin-iframe | 2/2 | 13 | 11.0 | 3.0 | 2/2 | 9 | 4.0 | 0.0 |

**final vs baseline: tasks/min ratio = 2.03x**; success rate 75% -> 100%; tasks regressed pass->fail: none; bar met: True
