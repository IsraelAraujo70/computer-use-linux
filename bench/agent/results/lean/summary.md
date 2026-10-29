| variant | runs | success | success rate | agent min | **tasks/min** | mean wall s | MCP calls | screenshots | input tokens | MCP ms/call |
|---|---|---|---|---|---|---|---|---|---|---|
| baseline | 30 | 30 | 100% | 24.0 | **1.249** | 48 | 25.6 | 4.9 | 640k | 365 |
| lean | 30 | 30 | 100% | 20.2 | **1.489** | 40 | 24.2 | 2.5 | 536k | 251 |

| task | baseline ok | baseline median s | baseline calls | baseline shots | lean ok | lean median s | lean calls | lean shots |
|---|---|---|---|---|---|---|---|---|
| calendar-event | 3/3 | 44 | 26.0 | 6.3 | 3/3 | 47 | 35.0 | 6.0 |
| contacts-infinite | 3/3 | 45 | 18.3 | 4.7 | 3/3 | 47 | 24.7 | 2.3 |
| form-signup | 3/3 | 68 | 42.0 | 8.0 | 3/3 | 56 | 33.0 | 4.0 |
| iframe-billing | 3/3 | 37 | 32.7 | 4.7 | 3/3 | 39 | 24.7 | 2.3 |
| invoices-bulk | 3/3 | 45 | 18.0 | 3.0 | 3/3 | 40 | 18.0 | 1.0 |
| mail-label | 3/3 | 60 | 21.3 | 3.7 | 3/3 | 36 | 21.7 | 1.0 |
| mail-reply | 3/3 | 38 | 11.7 | 5.0 | 3/3 | 33 | 14.0 | 1.3 |
| order-support | 3/3 | 73 | 66.3 | 8.7 | 3/3 | 71 | 52.7 | 4.0 |
| settings-dialog | 3/3 | 36 | 14.0 | 3.3 | 3/3 | 26 | 15.7 | 1.3 |
| terminal | 3/3 | 19 | 5.3 | 1.7 | 3/3 | 15 | 3.0 | 2.0 |

**lean vs baseline: tasks/min ratio = 1.19x**; success rate 100% -> 100%; tasks regressed pass->fail: none; bar met: False
