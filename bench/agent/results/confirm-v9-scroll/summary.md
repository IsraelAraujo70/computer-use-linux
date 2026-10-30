| variant | runs | success | success rate | agent min | **tasks/min** | mean wall s | MCP calls | screenshots | input tokens | MCP ms/call |
|---|---|---|---|---|---|---|---|---|---|---|
| v8 | 12 | 12 | 100% | 7.3 | **1.642** | 37 | 5.6 | 0.0 | 202k | 2642 |
| v9 | 12 | 12 | 100% | 6.7 | **1.795** | 33 | 5.2 | 0.0 | 174k | 1570 |

| task | v8 ok | v8 median s | v8 calls | v8 shots | v9 ok | v9 median s | v9 calls | v9 shots |
|---|---|---|---|---|---|---|---|---|
| contacts-infinite | 3/3 | 24 | 3.3 | 0.0 | 3/3 | 33 | 3.0 | 0.0 |
| invoices-bulk | 3/3 | 27 | 4.0 | 0.0 | 3/3 | 29 | 5.0 | 0.0 |
| mail-label | 3/3 | 28 | 9.3 | 0.0 | 3/3 | 35 | 9.7 | 0.0 |
| virtual-list | 3/3 | 59 | 5.7 | 0.0 | 3/3 | 28 | 3.0 | 0.0 |

**v9 vs v8: tasks/min ratio = 1.09x**; success rate 100% -> 100%; tasks regressed pass->fail: none; bar met: False
