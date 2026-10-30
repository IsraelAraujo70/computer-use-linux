| variant | runs | success | success rate | agent min | **tasks/min** | mean wall s | MCP calls | screenshots | input tokens | MCP ms/call |
|---|---|---|---|---|---|---|---|---|---|---|
| baseline | 30 | 30 | 100% | 19.7 | **1.523** | 39 | 21.1 | 4.4 | 588k | 388 |
| final | 30 | 30 | 100% | 10.9 | **2.745** | 22 | 5.6 | 0.2 | 211k | 670 |

| task | baseline ok | baseline median s | baseline calls | baseline shots | final ok | final median s | final calls | final shots |
|---|---|---|---|---|---|---|---|---|
| calendar-event | 3/3 | 39 | 25.7 | 6.3 | 3/3 | 22 | 9.3 | 0.0 |
| contacts-infinite | 3/3 | 47 | 13.3 | 5.3 | 3/3 | 24 | 4.3 | 0.0 |
| form-signup | 3/3 | 54 | 37.7 | 7.7 | 3/3 | 14 | 3.0 | 0.0 |
| iframe-billing | 3/3 | 32 | 21.0 | 3.7 | 3/3 | 18 | 3.0 | 0.0 |
| invoices-bulk | 3/3 | 39 | 16.7 | 3.0 | 3/3 | 20 | 5.0 | 0.0 |
| mail-label | 3/3 | 47 | 20.3 | 4.0 | 3/3 | 28 | 12.0 | 0.0 |
| mail-reply | 3/3 | 36 | 11.7 | 3.3 | 3/3 | 18 | 5.0 | 0.0 |
| order-support | 3/3 | 57 | 46.3 | 7.7 | 3/3 | 22 | 5.0 | 0.0 |
| settings-dialog | 3/3 | 25 | 13.7 | 1.0 | 3/3 | 20 | 6.0 | 0.0 |
| terminal | 3/3 | 18 | 5.0 | 2.0 | 3/3 | 18 | 3.3 | 2.0 |

**final vs baseline: tasks/min ratio = 1.80x**; success rate 100% -> 100%; tasks regressed pass->fail: none; bar met: False
