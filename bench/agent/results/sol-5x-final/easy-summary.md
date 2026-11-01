| variant | runs | success | success rate | agent min | **tasks/min** | mean wall s | MCP calls | screenshots | MCP KB | input tokens | MCP ms/call |
|---|---|---|---|---|---|---|---|---|---|---|---|
| baseline | 30 | 30 | 100% | 8.4 | **3.577** | 17 | 5.5 | 0.1 | 18.2 | 188k | 658 |
| final | 30 | 30 | 100% | 7.7 | **3.909** | 15 | 3.2 | 0.0 | 11.4 | 145k | 1181 |

| task | baseline ok | baseline median s | baseline calls | baseline shots | final ok | final median s | final calls | final shots |
|---|---|---|---|---|---|---|---|---|
| calendar-event | 3/3 | 20 | 10.7 | 0.0 | 3/3 | 21 | 6.3 | 0.0 |
| contacts-infinite | 3/3 | 25 | 3.0 | 0.0 | 3/3 | 22 | 2.0 | 0.0 |
| form-signup | 3/3 | 12 | 3.0 | 0.0 | 3/3 | 11 | 1.0 | 0.0 |
| iframe-billing | 3/3 | 12 | 3.0 | 0.0 | 3/3 | 11 | 2.0 | 0.0 |
| invoices-bulk | 3/3 | 17 | 5.0 | 0.0 | 3/3 | 16 | 3.0 | 0.0 |
| mail-label | 3/3 | 19 | 10.3 | 0.0 | 3/3 | 21 | 5.0 | 0.0 |
| mail-reply | 3/3 | 15 | 6.0 | 0.0 | 3/3 | 11 | 3.0 | 0.0 |
| order-support | 3/3 | 15 | 5.0 | 0.0 | 3/3 | 16 | 4.0 | 0.0 |
| settings-dialog | 3/3 | 16 | 6.0 | 0.0 | 3/3 | 12 | 3.0 | 0.0 |
| terminal | 3/3 | 13 | 3.3 | 1.0 | 3/3 | 10 | 2.3 | 0.0 |

**final vs baseline: tasks/min ratio = 1.09x**; success rate 100% -> 100%; tasks regressed pass->fail: none; bar met (>= 1.5x): False
