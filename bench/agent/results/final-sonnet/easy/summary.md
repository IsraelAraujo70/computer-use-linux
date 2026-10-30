| variant | runs | success | success rate | agent min | **tasks/min** | mean wall s | MCP calls | screenshots | input tokens | MCP ms/call |
|---|---|---|---|---|---|---|---|---|---|---|
| baseline | 20 | 20 | 100% | 9.8 | **2.040** | 29 | 22.4 | 7.6 | 402k | 269 |
| final | 20 | 20 | 100% | 4.3 | **4.638** | 13 | 6.0 | 0.3 | 96k | 490 |

| task | baseline ok | baseline median s | baseline calls | baseline shots | final ok | final median s | final calls | final shots |
|---|---|---|---|---|---|---|---|---|
| calendar-event | 2/2 | 48 | 36.0 | 16.5 | 2/2 | 20 | 13.5 | 2.0 |
| contacts-infinite | 2/2 | 25 | 10.0 | 2.5 | 2/2 | 18 | 4.5 | 0.0 |
| form-signup | 2/2 | 55 | 58.0 | 20.0 | 2/2 | 10 | 3.5 | 0.5 |
| iframe-billing | 2/2 | 16 | 13.0 | 4.5 | 2/2 | 11 | 4.5 | 0.0 |
| invoices-bulk | 2/2 | 20 | 13.0 | 5.0 | 2/2 | 13 | 7.0 | 0.0 |
| mail-label | 2/2 | 24 | 15.5 | 5.0 | 2/2 | 14 | 8.0 | 0.0 |
| mail-reply | 2/2 | 19 | 10.0 | 4.0 | 2/2 | 10 | 5.0 | 0.0 |
| order-support | 2/2 | 49 | 50.5 | 10.5 | 2/2 | 11 | 5.0 | 0.0 |
| settings-dialog | 2/2 | 29 | 15.5 | 7.0 | 2/2 | 12 | 6.0 | 0.0 |
| terminal | 2/2 | 8 | 3.0 | 1.0 | 2/2 | 9 | 3.0 | 1.0 |

**final vs baseline: tasks/min ratio = 2.27x**; success rate 100% -> 100%; tasks regressed pass->fail: none; bar met: True
