| variant | runs | success | success rate | agent min | **tasks/min** | mean wall s | MCP calls | screenshots | input tokens | MCP ms/call |
|---|---|---|---|---|---|---|---|---|---|---|
| baseline | 36 | 25 | 69% | 22.7 | **1.100** | 38 | 18.1 | 2.6 | 470k | 338 |
| final | 36 | 36 | 100% | 14.3 | **2.515** | 24 | 5.2 | 0.0 | 196k | 1094 |

| task | baseline ok | baseline median s | baseline calls | baseline shots | final ok | final median s | final calls | final shots |
|---|---|---|---|---|---|---|---|---|
| accents-form | 1/3 | 41 | 32.7 | 0.3 | 3/3 | 18 | 3.0 | 0.0 |
| async-autocomplete | 3/3 | 29 | 17.3 | 3.0 | 3/3 | 29 | 7.0 | 0.0 |
| drag-drop | 0/3 | 24 | 4.0 | 2.0 | 3/3 | 15 | 2.0 | 0.0 |
| file-upload | 0/3 | 57 | 14.3 | 5.7 | 3/3 | 15 | 3.0 | 0.0 |
| flaky-field | 3/3 | 47 | 27.3 | 5.0 | 3/3 | 20 | 5.0 | 0.0 |
| modal-focus | 3/3 | 29 | 15.3 | 1.0 | 3/3 | 19 | 5.0 | 0.0 |
| multi-tab | 3/3 | 36 | 11.7 | 3.0 | 3/3 | 17 | 5.0 | 0.0 |
| recover-error | 3/3 | 42 | 13.0 | 1.3 | 3/3 | 28 | 10.7 | 0.0 |
| rich-compose | 0/3 | 59 | 53.0 | 3.3 | 3/3 | 23 | 5.0 | 0.0 |
| slow-page | 3/3 | 23 | 6.0 | 3.0 | 3/3 | 19 | 5.0 | 0.0 |
| virtual-list | 3/3 | 31 | 7.0 | 0.0 | 3/3 | 61 | 7.3 | 0.0 |
| xorigin-iframe | 3/3 | 32 | 15.3 | 4.0 | 3/3 | 16 | 4.0 | 0.0 |

**final vs baseline: tasks/min ratio = 2.29x**; success rate 69% -> 100%; tasks regressed pass->fail: none; bar met: True
