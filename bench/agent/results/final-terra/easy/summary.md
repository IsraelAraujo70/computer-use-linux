| variant | runs | success | success rate | agent min | **tasks/min** | mean wall s | MCP calls | screenshots | input tokens | MCP ms/call |
|---|---|---|---|---|---|---|---|---|---|---|
| baseline | 30 | 30 | 100% | 23.7 | **1.266** | 47 | 22.4 | 4.6 | 674k | 376 |
| final | 30 | 30 | 100% | 11.5 | **2.619** | 23 | 5.3 | 0.2 | 204k | 589 |

| task | baseline ok | baseline median s | baseline calls | baseline shots | final ok | final median s | final calls | final shots |
|---|---|---|---|---|---|---|---|---|
| calendar-event | 3/3 | 54 | 32.3 | 8.3 | 3/3 | 37 | 11.3 | 0.0 |
| contacts-infinite | 3/3 | 60 | 12.7 | 5.0 | 3/3 | 26 | 3.3 | 0.0 |
| form-signup | 3/3 | 54 | 30.0 | 7.3 | 3/3 | 20 | 3.3 | 0.0 |
| iframe-billing | 3/3 | 36 | 21.7 | 3.7 | 3/3 | 19 | 3.0 | 0.0 |
| invoices-bulk | 3/3 | 40 | 16.7 | 4.3 | 3/3 | 18 | 4.0 | 0.0 |
| mail-label | 3/3 | 51 | 24.0 | 4.7 | 3/3 | 22 | 8.7 | 0.0 |
| mail-reply | 3/3 | 38 | 10.7 | 4.0 | 3/3 | 21 | 5.0 | 0.0 |
| order-support | 3/3 | 84 | 57.0 | 3.3 | 3/3 | 21 | 5.0 | 0.0 |
| settings-dialog | 3/3 | 34 | 14.0 | 3.0 | 3/3 | 23 | 6.0 | 0.0 |
| terminal | 3/3 | 24 | 5.3 | 2.0 | 3/3 | 19 | 3.0 | 2.0 |

**final vs baseline: tasks/min ratio = 2.07x**; success rate 100% -> 100%; tasks regressed pass->fail: none; bar met: True
