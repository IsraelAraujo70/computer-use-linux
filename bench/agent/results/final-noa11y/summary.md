| variant | runs | success | success rate | agent min | **tasks/min** | mean wall s | MCP calls | screenshots | input tokens | MCP ms/call |
|---|---|---|---|---|---|---|---|---|---|---|
| baseline | 20 | 20 | 100% | 14.1 | **1.419** | 42 | 19.1 | 6.2 | 524k | 275 |
| final | 20 | 20 | 100% | 13.7 | **1.464** | 41 | 16.2 | 6.5 | 598k | 205 |

| task | baseline ok | baseline median s | baseline calls | baseline shots | final ok | final median s | final calls | final shots |
|---|---|---|---|---|---|---|---|---|
| calendar-event | 2/2 | 44 | 26.5 | 9.0 | 2/2 | 77 | 26.0 | 10.0 |
| contacts-infinite | 2/2 | 53 | 13.5 | 5.5 | 2/2 | 47 | 14.0 | 6.5 |
| form-signup | 2/2 | 43 | 24.0 | 7.0 | 2/2 | 36 | 26.0 | 8.0 |
| iframe-billing | 2/2 | 22 | 16.0 | 3.0 | 2/2 | 33 | 16.0 | 4.0 |
| invoices-bulk | 2/2 | 31 | 12.0 | 5.0 | 2/2 | 31 | 14.0 | 6.0 |
| mail-label | 2/2 | 59 | 18.0 | 8.5 | 2/2 | 44 | 19.0 | 8.0 |
| mail-reply | 2/2 | 34 | 10.0 | 5.0 | 2/2 | 41 | 11.5 | 6.0 |
| order-support | 2/2 | 74 | 51.5 | 9.5 | 2/2 | 42 | 17.0 | 6.0 |
| settings-dialog | 2/2 | 41 | 14.0 | 7.0 | 2/2 | 41 | 16.0 | 8.0 |
| terminal | 2/2 | 23 | 6.0 | 2.0 | 2/2 | 18 | 3.0 | 2.0 |

**final vs baseline: tasks/min ratio = 1.03x**; success rate 100% -> 100%; tasks regressed pass->fail: none; bar met: False
