| variant | runs | success | success rate | agent min | **tasks/min** | mean wall s | MCP calls | screenshots | input tokens | MCP ms/call |
|---|---|---|---|---|---|---|---|---|---|---|
| baseline | 30 | 29 | 97% | 23.4 | **1.241** | 47 | 21.2 | 4.1 | 621k | 388 |
| v6 | 30 | 30 | 100% | 11.6 | **2.577** | 23 | 6.7 | 0.2 | 195k | 560 |

| task | baseline ok | baseline median s | baseline calls | baseline shots | v6 ok | v6 median s | v6 calls | v6 shots |
|---|---|---|---|---|---|---|---|---|
| calendar-event | 3/3 | 50 | 28.7 | 9.3 | 3/3 | 26 | 9.0 | 0.0 |
| contacts-infinite | 3/3 | 49 | 14.0 | 6.7 | 3/3 | 29 | 17.3 | 0.0 |
| form-signup | 3/3 | 63 | 35.0 | 6.0 | 3/3 | 18 | 3.7 | 0.0 |
| iframe-billing | 3/3 | 42 | 23.3 | 3.7 | 3/3 | 16 | 3.0 | 0.0 |
| invoices-bulk | 3/3 | 41 | 15.7 | 3.3 | 3/3 | 19 | 4.0 | 0.0 |
| mail-label | 3/3 | 52 | 22.7 | 5.0 | 3/3 | 24 | 9.0 | 0.0 |
| mail-reply | 3/3 | 33 | 10.7 | 2.7 | 3/3 | 16 | 5.0 | 0.0 |
| order-support | 2/3 | 64 | 41.3 | 1.3 | 3/3 | 24 | 6.3 | 0.0 |
| settings-dialog | 3/3 | 35 | 14.0 | 1.0 | 3/3 | 27 | 6.0 | 0.0 |
| terminal | 3/3 | 26 | 6.3 | 2.0 | 3/3 | 23 | 3.3 | 2.0 |

**v6 vs baseline: tasks/min ratio = 2.08x**; success rate 97% -> 100%; tasks regressed pass->fail: none; bar met: True
