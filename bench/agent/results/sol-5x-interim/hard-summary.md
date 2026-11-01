| variant | runs | success | success rate | agent min | **tasks/min** | mean wall s | MCP calls | screenshots | MCP KB | input tokens | MCP ms/call |
|---|---|---|---|---|---|---|---|---|---|---|---|
| baseline | 7 | 7 | 100% | 1.9 | **3.657** | 16 | 4.4 | 0.0 | 6.9 | 154k | 787 |
| final | 7 | 7 | 100% | 1.8 | **3.910** | 15 | 2.7 | 0.0 | 5.4 | 125k | 1846 |

| task | baseline ok | baseline median s | baseline calls | baseline shots | final ok | final median s | final calls | final shots |
|---|---|---|---|---|---|---|---|---|
| accents-form | 1/1 | 11 | 3.0 | 0.0 | 1/1 | 8 | 1.0 | 0.0 |
| drag-drop | 1/1 | 13 | 3.0 | 0.0 | 1/1 | 11 | 2.0 | 0.0 |
| flaky-field | 1/1 | 14 | 5.0 | 0.0 | 1/1 | 10 | 1.0 | 0.0 |
| modal-focus | 1/1 | 9 | 3.0 | 0.0 | 1/1 | 15 | 3.0 | 0.0 |
| rich-compose | 1/1 | 16 | 5.0 | 0.0 | 1/1 | 18 | 3.0 | 0.0 |
| slow-page | 1/1 | 28 | 8.0 | 0.0 | 1/1 | 17 | 5.0 | 0.0 |
| virtual-list | 1/1 | 23 | 4.0 | 0.0 | 1/1 | 28 | 4.0 | 0.0 |

**final vs baseline: tasks/min ratio = 1.07x**; success rate 100% -> 100%; tasks regressed pass->fail: none; bar met (>= 1.5x): False
