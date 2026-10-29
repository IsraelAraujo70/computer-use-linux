| variant | runs | success | success rate | agent min | **tasks/min** | mean wall s | MCP calls | screenshots | input tokens | MCP ms/call |
|---|---|---|---|---|---|---|---|---|---|---|
| baseline | 30 | 30 | 100% | 24.0 | **1.249** | 48 | 25.6 | 4.9 | 640k | 365 |
| v1 | 30 | 30 | 100% | 10.6 | **2.842** | 21 | 6.1 | 0.2 | 210k | 639 |

| task | baseline ok | baseline median s | baseline calls | baseline shots | v1 ok | v1 median s | v1 calls | v1 shots |
|---|---|---|---|---|---|---|---|---|
| calendar-event | 3/3 | 44 | 26.0 | 6.3 | 3/3 | 26 | 9.3 | 0.0 |
| contacts-infinite | 3/3 | 45 | 18.3 | 4.7 | 3/3 | 27 | 7.0 | 0.0 |
| form-signup | 3/3 | 68 | 42.0 | 8.0 | 3/3 | 17 | 3.0 | 0.0 |
| iframe-billing | 3/3 | 37 | 32.7 | 4.7 | 3/3 | 17 | 3.0 | 0.0 |
| invoices-bulk | 3/3 | 45 | 18.0 | 3.0 | 3/3 | 21 | 7.0 | 0.0 |
| mail-label | 3/3 | 60 | 21.3 | 3.7 | 3/3 | 23 | 12.3 | 0.0 |
| mail-reply | 3/3 | 38 | 11.7 | 5.0 | 3/3 | 22 | 5.0 | 0.0 |
| order-support | 3/3 | 73 | 66.3 | 8.7 | 3/3 | 21 | 5.0 | 0.0 |
| settings-dialog | 3/3 | 36 | 14.0 | 3.3 | 3/3 | 18 | 6.7 | 0.0 |
| terminal | 3/3 | 19 | 5.3 | 1.7 | 3/3 | 17 | 2.7 | 1.7 |

**v1 vs baseline: tasks/min ratio = 2.27x**; success rate 100% -> 100%; tasks regressed pass->fail: none; bar met: True
