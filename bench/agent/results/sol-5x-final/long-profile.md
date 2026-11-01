| run | ok | wall s | boot s | model s (turns) | s/turn | final s | MCP s | KB | slowest calls | tools |
|---|---|---|---|---|---|---|---|---|---|---|
| baseline-long-checkout-r0 | ok | 26 | 0.2 | 17.3 (16) | 1.1 | 2.7 | 8.8 | 39.7 | fill_form 2.1, click 0.7 | get_app_state fill_form click click click click set_value click click fill_form get_app_state click click click click |
| baseline-long-crm-r0 | ok | 42 | 0.2 | 33.6 (24) | 1.4 | 4.1 | 8.6 | 40.1 | fill_form 0.7, fill_form 0.7 | get_app_state click fill_form click click fill_form click click fill_form click set_value click fill_form click set_value click fill_form click set_value click click set_value set_value |
| baseline-long-expenses-r0 | ok | 52 | 0.2 | 37.5 (30) | 1.3 | 3.6 | 14.2 | 39.3 | fill_form 3.5, fill_form 1.6 | get_app_state fill_form click fill_form click click click press_key press_key click click fill_form click click fill_form click click fill_form click press_key press_key click click fill_form click get_app_state click click find_elements |
| baseline-long-onboarding-r0 | ok | 37 | 0.2 | 26.1 (14) | 1.9 | 2.4 | 10.5 | 22.0 | fill_form 2.0, fill_form 1.8 | get_app_state fill_form click fill_form click fill_form click click fill_form click fill_form click click |
| baseline-long-triage-r0 | ok | 45 | 0.2 | 33.2 (32) | 1.0 | 5.0 | 11.6 | 81.1 | set_value 1.7, set_value 0.5 | get_app_state set_value set_value set_value set_value click click get_app_state set_value set_value set_value set_value click click get_app_state set_value set_value set_value set_value set_value click get_app_state set_value find_elements find_elements get_app_state click get_app_state click find_elements find_elements |
| final-long-checkout-r0 | ok | 37 | 0.2 | 24.6 (8) | 3.1 | 2.5 | 11.7 | 39.1 | act 5.9, act 2.3 | get_app_state act act act get_app_state act act |
| final-long-crm-r0 | ok | 40 | 0.2 | 31.2 (9) | 3.5 | 2.3 | 8.4 | 17.6 | act 1.8, act 1.7 | get_app_state act act act act act act act |
| final-long-expenses-r0 | ok | 37 | 0.2 | 27.3 (10) | 2.7 | 2.1 | 9.8 | 13.9 | act 1.7, act 1.6 | get_app_state act act act act act act act act |
| final-long-onboarding-r0 | ok | 41 | 0.2 | 29.6 (9) | 3.3 | 2.9 | 11.0 | 16.2 | act 2.3, act 2.1 | get_app_state act act act act act act act |
| final-long-triage-r0 | ok | 44 | 0.2 | 34.3 (13) | 2.6 | 2.8 | 9.7 | 63.2 | act 2.1, act 2.1 | get_app_state act act act act get_app_state act get_app_state click get_app_state perform_action find_elements |

| variant | eps | mean wall | boot | model | MCP | turns | s per model turn | KB | share boot/model/MCP |
|---|---|---|---|---|---|---|---|---|---|
| baseline | 5 | 40.5 | 0.2 | 29.5 | 10.7 | 23.2 | 1.3 | 44.4 | 0%/73%/27% |
| final | 5 | 39.7 | 0.2 | 29.4 | 10.1 | 9.8 | 3.0 | 30.0 | 0%/74%/25% |
