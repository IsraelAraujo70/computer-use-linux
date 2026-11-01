| run | ok | wall s | boot s | model s (turns) | s/turn | final s | MCP s | KB | slowest calls | tools |
|---|---|---|---|---|---|---|---|---|---|---|
| baseline-accents-form-r0 | ok | 11 | 0.1 | 9.4 (4) | 2.4 | 2.3 | 1.4 | 2.8 | fill_form 1.0, click 0.3 | get_app_state fill_form click |
| baseline-accents-form-r1 | ok | 18 | 1.5 | 12.0 (4) | 3.0 | 2.3 | 4.3 | 2.8 | fill_form 2.6, get_app_state 1.3 | get_app_state fill_form click |
| baseline-accents-form-r2 | ok | 16 | 0.1 | 14.4 (4) | 3.6 | 3.7 | 1.5 | 2.8 | fill_form 0.9, click 0.3 | get_app_state fill_form click |
| baseline-async-autocomplete-r0 | ok | 40 | 1.2 | 22.3 (9) | 2.5 | 2.2 | 16.4 | 6.8 | fill_form 6.3, wait_for 5.1 | get_app_state fill_form type_text wait_for get_app_state click click get_app_state |
| baseline-async-autocomplete-r1 | ok | 23 | 0.3 | 18.2 (7) | 2.6 | 3.0 | 4.5 | 4.6 | fill_form 2.8, click 0.6 | get_app_state fill_form type_text wait_for click click |
| baseline-async-autocomplete-r2 | ok | 25 | 0.2 | 16.4 (8) | 2.1 | 2.7 | 8.7 | 7.3 | wait_for 4.6, fill_form 2.7 | get_app_state fill_form type_text wait_for get_app_state click click |
| baseline-drag-drop-r0 | ok | 13 | 0.1 | 11.7 (4) | 2.9 | 2.9 | 1.6 | 4.3 | drag 1.3, get_app_state 0.2 | get_app_state drag get_app_state |
| baseline-drag-drop-r1 | ok | 16 | 1.0 | 11.1 (4) | 2.8 | 2.6 | 4.1 | 4.3 | drag 2.1, get_app_state 1.2 | get_app_state drag get_app_state |
| baseline-drag-drop-r2 | ok | 18 | 0.2 | 16.5 (4) | 4.1 | 2.6 | 1.7 | 4.3 | drag 1.3, get_app_state 0.3 | get_app_state drag get_app_state |
| baseline-file-upload-r0 | ok | 23 | 2.7 | 17.2 (5) | 3.4 | 4.5 | 3.2 | 2.9 | get_app_state 1.3, get_app_state 0.9 | get_app_state drop_files click get_app_state |
| baseline-file-upload-r1 | ok | 13 | 1.2 | 9.1 (4) | 2.3 | 2.4 | 2.6 | 2.0 | get_app_state 1.3, drop_files 0.7 | get_app_state drop_files click |
| baseline-file-upload-r2 | ok | 14 | 0.1 | 13.2 (4) | 3.3 | 3.0 | 0.8 | 2.0 | drop_files 0.3, click 0.3 | get_app_state drop_files click |
| baseline-flaky-field-r0 | ok | 14 | 0.1 | 11.8 (6) | 2.0 | 2.0 | 2.3 | 4.3 | fill_form 1.1, set_value 0.5 | get_app_state fill_form click set_value click |
| baseline-flaky-field-r1 | ok | 25 | 0.4 | 16.1 (6) | 2.7 | 2.8 | 8.2 | 4.1 | fill_form 3.4, get_app_state 1.9 | get_app_state fill_form click set_value click |
| baseline-flaky-field-r2 | ok | 16 | 0.1 | 13.1 (6) | 2.2 | 3.3 | 2.4 | 4.3 | fill_form 1.2, set_value 0.5 | get_app_state fill_form click set_value click |
| baseline-modal-focus-r0 | ok | 9 | 0.1 | 7.8 (4) | 1.9 | 1.9 | 1.5 | 2.6 | fill_form 1.0, click 0.3 | get_app_state fill_form click |
| baseline-modal-focus-r1 | ok | 28 | 1.1 | 17.0 (7) | 2.4 | 3.2 | 9.5 | 5.5 | fill_form 3.9, click 2.1 | get_app_state fill_form click click set_value click |
| baseline-modal-focus-r2 | ok | 15 | 0.2 | 13.3 (4) | 3.3 | 4.0 | 1.6 | 2.6 | fill_form 1.1, click 0.3 | get_app_state fill_form click |
| baseline-multi-tab-r0 | ok | 29 | 1.5 | 21.1 (6) | 3.5 | 3.1 | 6.9 | 5.2 | set_value 2.1, click 1.6 | get_app_state click click set_value click |
| baseline-multi-tab-r1 | ok | 25 | 1.3 | 16.1 (7) | 2.3 | 3.0 | 7.6 | 6.4 | set_value 2.4, click 1.7 | get_app_state click click set_value click get_app_state |
| baseline-multi-tab-r2 | ok | 17 | 0.1 | 14.8 (6) | 2.5 | 2.9 | 2.5 | 5.2 | click 0.7, set_value 0.7 | get_app_state click click set_value click |
| baseline-recover-error-r0 | ok | 55 | 1.1 | 32.4 (21) | 1.5 | 5.0 | 21.3 | 13.6 | click 1.7, press_key 1.7 | get_app_state set_value click click press_key get_app_state set_value click click click get_app_state press_key get_app_state click press_key type_text press_key click click get_app_state |
| baseline-recover-error-r1 | ok | 34 | 0.2 | 24.6 (20) | 1.2 | 2.6 | 9.6 | 12.4 | click 1.2, click 1.1 | get_app_state set_value click click press_key get_app_state set_value press_key click click get_app_state press_key click press_key type_text press_key click click get_app_state |
| baseline-recover-error-r2 | ok | 49 | 0.4 | 35.1 (27) | 1.3 | 5.0 | 14.0 | 17.6 | click 1.2, click 1.1 | get_app_state set_value click click press_key get_app_state click press_key type_text press_key click click press_key get_app_state set_value click click wait_for click get_app_state press_key get_app_state set_value click click get_app_state |
| baseline-rich-compose-r0 | ok | 16 | 0.1 | 12.0 (6) | 2.0 | 2.0 | 3.8 | 5.0 | fill_form 2.7, type_text 0.5 | get_app_state fill_form type_text press_key click |
| baseline-rich-compose-r1 | ok | 25 | 1.7 | 13.0 (6) | 2.2 | 3.1 | 10.2 | 5.0 | fill_form 5.4, type_text 1.6 | get_app_state fill_form type_text press_key click |
| baseline-rich-compose-r2 | ok | 19 | 0.2 | 14.7 (6) | 2.5 | 3.3 | 3.9 | 5.1 | fill_form 2.7, type_text 0.5 | get_app_state fill_form type_text press_key click |
| baseline-slow-page-r0 | ok | 28 | 0.3 | 25.5 (9) | 2.8 | 3.0 | 1.9 | 12.3 | wait_for 0.7, click 0.4 | get_app_state click get_app_state get_app_state wait_for wait_for get_app_state click |
| baseline-slow-page-r1 | ok | 26 | 1.0 | 18.3 (6) | 3.1 | 2.4 | 6.5 | 7.4 | wait_for 2.5, click 1.3 | get_app_state click wait_for get_app_state click |
| baseline-slow-page-r2 | ok | 17 | 0.5 | 12.9 (6) | 2.2 | 1.9 | 3.5 | 9.6 | wait_for 2.4, click 0.4 | get_app_state click wait_for get_app_state click |
| baseline-virtual-list-r0 | ok | 23 | 0.1 | 11.2 (5) | 2.2 | 2.1 | 11.9 | 17.0 | scroll 11.0, click 0.5 | get_app_state scroll click get_app_state |
| baseline-virtual-list-r1 | ok | 46 | 1.1 | 15.2 (5) | 3.0 | 2.8 | 29.9 | 9.8 | scroll 24.4, click 2.2 | get_app_state scroll click find_elements |
| baseline-virtual-list-r2 | ok | 29 | 0.2 | 13.9 (5) | 2.8 | 2.8 | 14.6 | 17.0 | scroll 13.3, click 0.6 | get_app_state scroll click get_app_state |
| baseline-xorigin-iframe-r0 | ok | 21 | 0.8 | 14.8 (5) | 3.0 | 2.9 | 5.8 | 4.2 | fill_form 2.7, get_app_state 1.4 | get_app_state fill_form click click |
| baseline-xorigin-iframe-r1 | ok | 20 | 1.1 | 13.7 (5) | 2.7 | 3.3 | 5.3 | 4.2 | fill_form 2.7, get_app_state 1.3 | get_app_state fill_form click click |
| baseline-xorigin-iframe-r2 | ok | 26 | 0.7 | 23.5 (5) | 4.7 | 8.4 | 1.6 | 4.2 | fill_form 0.9, get_app_state 0.3 | get_app_state fill_form click click |
| final-accents-form-r0 | ok | 8 | 0.1 | 6.6 (2) | 3.3 | 1.9 | 1.6 | 1.9 | act 1.6 | act |
| final-accents-form-r1 | ok | 17 | 1.3 | 11.0 (2) | 5.5 | 2.5 | 4.5 | 1.9 | act 4.5 | act |
| final-accents-form-r2 | ok | 10 | 0.2 | 8.5 (2) | 4.3 | 2.2 | 1.6 | 1.9 | act 1.6 | act |
| final-async-autocomplete-r0 | ok | 44 | 1.0 | 24.2 (8) | 3.0 | 3.4 | 19.1 | 7.7 | act 7.1, wait_for 6.0 | act act act wait_for get_app_state act get_app_state |
| final-async-autocomplete-r1 | ok | 35 | 0.2 | 22.0 (7) | 3.1 | 3.0 | 12.6 | 5.7 | wait_for 5.1, act 4.1 | act act act wait_for find_elements act |
| final-async-autocomplete-r2 | ok | 27 | 0.3 | 17.7 (6) | 2.9 | 3.1 | 8.9 | 6.7 | wait_for 4.8, act 3.2 | act act wait_for get_app_state act |
| final-drag-drop-r0 | ok | 11 | 0.1 | 9.4 (3) | 3.1 | 2.4 | 1.5 | 2.5 | drag 1.3, get_app_state 0.2 | get_app_state drag |
| final-drag-drop-r1 | ok | 13 | 1.0 | 9.2 (3) | 3.1 | 2.9 | 3.1 | 2.5 | drag 2.1, get_app_state 1.0 | get_app_state drag |
| final-drag-drop-r2 | ok | 15 | 0.2 | 12.9 (3) | 4.3 | 2.3 | 1.6 | 2.5 | drag 1.3, get_app_state 0.3 | get_app_state drag |
| final-file-upload-r0 | ok | 20 | 3.9 | 10.9 (3) | 3.6 | 2.6 | 5.3 | 1.2 | drop_files 4.8, act 0.5 | drop_files act |
| final-file-upload-r1 | ok | 17 | 1.1 | 10.2 (3) | 3.4 | 2.3 | 5.4 | 1.2 | drop_files 4.5, act 0.9 | drop_files act |
| final-file-upload-r2 | ok | 10 | 0.1 | 8.4 (3) | 2.8 | 1.7 | 1.2 | 1.2 | drop_files 0.9, act 0.3 | drop_files act |
| final-flaky-field-r0 | ok | 10 | 0.1 | 7.9 (2) | 4.0 | 2.1 | 1.9 | 2.3 | act 1.9 | act |
| final-flaky-field-r1 | ok | 20 | 1.4 | 12.5 (3) | 4.2 | 2.9 | 5.8 | 3.1 | act 5.1, act 0.7 | act act |
| final-flaky-field-r2 | ok | 10 | 0.1 | 8.2 (2) | 4.1 | 2.4 | 2.1 | 2.2 | act 2.1 | act |
| final-modal-focus-r0 | ok | 15 | 0.7 | 11.7 (4) | 2.9 | 3.2 | 2.2 | 5.1 | act 1.6, act 0.4 | act act act |
| final-modal-focus-r1 | ok | 22 | 1.1 | 10.8 (3) | 3.6 | 2.9 | 9.6 | 3.0 | act 6.6, act 3.0 | act act |
| final-modal-focus-r2 | ok | 16 | 0.2 | 13.3 (4) | 3.3 | 2.4 | 2.2 | 5.1 | act 1.5, act 0.5 | act act act |
| final-multi-tab-r0 | ok | 22 | 1.1 | 12.9 (3) | 4.3 | 4.4 | 7.9 | 3.4 | act 4.1, act 3.8 | act act |
| final-multi-tab-r1 | ok | 20 | 1.1 | 11.7 (3) | 3.9 | 2.4 | 7.1 | 3.4 | act 4.3, act 2.8 | act act |
| final-multi-tab-r2 | ok | 13 | 0.1 | 10.7 (3) | 3.6 | 2.1 | 2.4 | 3.4 | act 1.5, act 0.9 | act act |
| final-recover-error-r0 | ok | 17 | 1.3 | 10.3 (3) | 3.4 | 2.5 | 5.6 | 2.1 | act 4.1, act 1.5 | act act |
| final-recover-error-r1 | ok | 33 | 0.3 | 23.5 (8) | 2.9 | 2.4 | 9.2 | 7.6 | wait_for 3.3, act 2.1 | act act wait_for act act act get_app_state |
| final-recover-error-r2 | ok | 71 | 0.2 | 50.4 (16) | 3.2 | 3.6 | 20.2 | 136.2 | wait_for 3.4, act 2.5 | act act act act act wait_for act act act act act screenshot act act wait_for |
| final-rich-compose-r0 | ok | 18 | 0.1 | 11.7 (4) | 2.9 | 2.3 | 6.6 | 5.7 | act 3.0, act 2.0 | act act act |
| final-rich-compose-r1 | ok | 27 | 1.6 | 12.9 (4) | 3.2 | 3.2 | 12.1 | 5.7 | act 4.7, act 4.0 | act act act |
| final-rich-compose-r2 | ok | 22 | 0.2 | 15.2 (4) | 3.8 | 4.7 | 6.7 | 5.7 | act 2.8, act 2.3 | act act act |
| final-slow-page-r0 | ok | 17 | 0.1 | 12.8 (6) | 2.1 | 2.1 | 3.9 | 9.9 | wait_for 3.1, act 0.4 | get_app_state act wait_for get_app_state act |
| final-slow-page-r1 | ok | 22 | 1.2 | 13.7 (6) | 2.3 | 2.5 | 7.0 | 7.9 | wait_for 2.3, act 2.2 | get_app_state act wait_for get_app_state act |
| final-slow-page-r2 | ok | 21 | 0.2 | 18.6 (6) | 3.1 | 3.0 | 2.5 | 9.9 | wait_for 1.4, act 0.4 | get_app_state act wait_for get_app_state act |
| final-virtual-list-r0 | ok | 28 | 0.2 | 10.7 (5) | 2.1 | 1.4 | 17.4 | 10.2 | scroll 13.7, act 3.0 | act find_elements scroll click |
| final-virtual-list-r1 | ok | 43 | 1.2 | 11.9 (4) | 3.0 | 2.4 | 30.0 | 9.3 | scroll 23.0, act 4.5 | act scroll act |
| final-virtual-list-r2 | ok | 33 | 0.2 | 12.2 (4) | 3.0 | 3.0 | 20.4 | 9.3 | scroll 16.2, act 3.2 | act scroll act |
| final-xorigin-iframe-r0 | ok | 17 | 1.2 | 10.5 (2) | 5.2 | 2.8 | 5.4 | 1.4 | act 5.4 | act |
| final-xorigin-iframe-r1 | ok | 11 | 0.5 | 8.4 (2) | 4.2 | 2.4 | 1.7 | 1.4 | act 1.7 | act |
| final-xorigin-iframe-r2 | ok | 15 | 0.2 | 13.3 (2) | 6.6 | 7.5 | 1.7 | 1.4 | act 1.7 | act |

| variant | eps | mean wall | boot | model | MCP | turns | s per model turn | KB | share boot/model/MCP |
|---|---|---|---|---|---|---|---|---|---|
| baseline | 36 | 23.4 | 0.7 | 16.2 | 6.6 | 6.9 | 2.3 | 6.4 | 3%/69%/28% |
| final | 36 | 21.4 | 0.7 | 13.5 | 7.2 | 4.1 | 3.3 | 8.1 | 3%/63%/34% |
