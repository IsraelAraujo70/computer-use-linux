| variant | runs | success | success rate | agent min | **tasks/min** | mean wall s | MCP calls | screenshots | MCP KB | input tokens | MCP ms/call |
|---|---|---|---|---|---|---|---|---|---|---|---|
| baseline | 5 | 5 | 100% | 3.4 | **1.482** | 40 | 22.2 | 0.0 | 44.4 | 380k | 484 |
| final | 5 | 5 | 100% | 3.3 | **1.512** | 40 | 8.8 | 0.0 | 30.0 | 328k | 1148 |

| task | baseline ok | baseline median s | baseline calls | baseline shots | final ok | final median s | final calls | final shots |
|---|---|---|---|---|---|---|---|---|
| long-checkout | 1/1 | 26 | 15.0 | 0.0 | 1/1 | 37 | 7.0 | 0.0 |
| long-crm | 1/1 | 42 | 23.0 | 0.0 | 1/1 | 40 | 8.0 | 0.0 |
| long-expenses | 1/1 | 52 | 29.0 | 0.0 | 1/1 | 37 | 9.0 | 0.0 |
| long-onboarding | 1/1 | 37 | 13.0 | 0.0 | 1/1 | 41 | 8.0 | 0.0 |
| long-triage | 1/1 | 45 | 31.0 | 0.0 | 1/1 | 44 | 12.0 | 0.0 |

**final vs baseline: tasks/min ratio = 1.02x**; success rate 100% -> 100%; tasks regressed pass->fail: none; bar met (>= 1.5x): False
