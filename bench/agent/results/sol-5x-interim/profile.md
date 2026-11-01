| run | ok | wall s | boot s | model s (turns) | s/turn | final s | MCP s | KB | slowest calls | tools |
|---|---|---|---|---|---|---|---|---|---|---|
| baseline-calendar-event-r0 | ok | 19 | 0.1 | 16.0 (11) | 1.5 | 2.1 | 3.1 | 19.6 | fill_form 0.9, click 0.3 | get_app_state click click click fill_form click click click click find_elements |
| baseline-calendar-event-r1 | ok | 20 | 0.1 | 16.4 (12) | 1.4 | 1.9 | 3.8 | 20.2 | fill_form 1.4, click 0.3 | get_app_state click click click fill_form click click click click find_elements find_elements |
| baseline-calendar-event-r2 | ok | 22 | 0.1 | 18.1 (12) | 1.5 | 2.3 | 4.0 | 20.1 | fill_form 1.4, click 0.4 | get_app_state click click click click fill_form click click click click find_elements |
| baseline-contacts-infinite-r0 | ok | 25 | 0.1 | 10.0 (4) | 2.5 | 2.0 | 15.3 | 5.9 | scroll 14.3, click 0.8 | get_app_state scroll click |
| baseline-contacts-infinite-r1 | ok | 25 | 0.1 | 11.0 (4) | 2.8 | 2.1 | 14.2 | 5.9 | scroll 13.2, click 0.8 | get_app_state scroll click |
| baseline-contacts-infinite-r2 | ok | 25 | 0.1 | 10.5 (4) | 2.6 | 2.3 | 14.1 | 5.9 | scroll 13.0, click 0.8 | get_app_state scroll click |
| baseline-form-signup-r0 | ok | 12 | 0.1 | 9.4 (4) | 2.4 | 2.0 | 2.1 | 3.6 | fill_form 1.6, click 0.3 | get_app_state fill_form click |
| baseline-form-signup-r1 | ok | 13 | 0.1 | 10.8 (4) | 2.7 | 2.2 | 2.1 | 3.6 | fill_form 1.7, click 0.3 | get_app_state fill_form click |
| baseline-form-signup-r2 | ok | 12 | 0.1 | 9.8 (4) | 2.4 | 2.0 | 2.1 | 3.6 | fill_form 1.7, click 0.3 | get_app_state fill_form click |
| baseline-iframe-billing-r0 | ok | 12 | 0.1 | 9.7 (4) | 2.4 | 1.8 | 2.0 | 6.0 | fill_form 1.4, click 0.3 | get_app_state fill_form click |
| baseline-iframe-billing-r1 | ok | 13 | 0.1 | 10.7 (4) | 2.7 | 2.1 | 2.0 | 6.0 | fill_form 1.4, click 0.3 | get_app_state fill_form click |
| baseline-iframe-billing-r2 | ok | 11 | 0.1 | 9.1 (4) | 2.3 | 1.9 | 2.0 | 6.0 | fill_form 1.5, click 0.3 | get_app_state fill_form click |
| baseline-invoices-bulk-r0 | ok | 21 | 0.1 | 17.4 (6) | 2.9 | 2.1 | 3.3 | 28.9 | click 1.7, fill_form 0.6 | get_app_state fill_form click click get_app_state |
| baseline-invoices-bulk-r1 | ok | 17 | 0.1 | 13.7 (6) | 2.3 | 2.5 | 3.4 | 28.9 | click 1.8, fill_form 0.7 | get_app_state fill_form click click get_app_state |
| baseline-invoices-bulk-r2 | ok | 16 | 0.1 | 12.7 (6) | 2.1 | 2.4 | 3.3 | 28.9 | click 1.8, fill_form 0.6 | get_app_state fill_form click click get_app_state |
| baseline-mail-label-r0 | ok | 19 | 0.2 | 14.7 (12) | 1.2 | 2.0 | 3.7 | 19.9 | set_value 0.6, click 0.4 | get_app_state set_value press_key set_value set_value set_value click click click find_elements find_elements |
| baseline-mail-label-r1 | ok | 19 | 0.2 | 15.4 (11) | 1.4 | 1.8 | 3.1 | 31.2 | set_value 0.6, fill_form 0.5 | get_app_state set_value press_key get_app_state fill_form click click click find_elements find_elements |
| baseline-mail-label-r2 | ok | 19 | 0.1 | 15.3 (11) | 1.4 | 2.0 | 3.2 | 30.9 | set_value 0.6, fill_form 0.5 | get_app_state set_value press_key get_app_state fill_form click click click find_elements find_elements |
| baseline-mail-reply-r0 | ok | 15 | 0.1 | 13.2 (7) | 1.9 | 2.3 | 1.7 | 15.4 | set_value 0.5, click 0.4 | get_app_state click click set_value click find_elements |
| baseline-mail-reply-r1 | ok | 16 | 0.2 | 14.1 (7) | 2.0 | 2.3 | 1.4 | 15.4 | set_value 0.4, get_app_state 0.3 | get_app_state click click set_value click find_elements |
| baseline-mail-reply-r2 | ok | 15 | 0.1 | 13.7 (7) | 2.0 | 2.2 | 1.5 | 15.4 | set_value 0.4, click 0.4 | get_app_state click click set_value click find_elements |
| baseline-order-support-r0 | ok | 15 | 0.1 | 13.1 (6) | 2.2 | 2.9 | 2.1 | 10.4 | fill_form 0.9, click 0.4 | get_app_state click click fill_form click |
| baseline-order-support-r1 | ok | 15 | 0.2 | 12.6 (6) | 2.1 | 2.5 | 2.3 | 10.4 | fill_form 1.0, click 0.4 | get_app_state click click fill_form click |
| baseline-order-support-r2 | ok | 18 | 0.3 | 15.2 (6) | 2.5 | 3.9 | 2.1 | 10.4 | fill_form 0.9, click 0.4 | get_app_state click click fill_form click |
| baseline-settings-dialog-r0 | ok | 16 | 0.1 | 14.2 (7) | 2.0 | 2.1 | 1.6 | 6.2 | fill_form 0.5, click 0.3 | get_app_state click click click fill_form click |
| baseline-settings-dialog-r1 | ok | 17 | 0.1 | 15.1 (7) | 2.2 | 2.3 | 1.6 | 6.2 | fill_form 0.5, click 0.3 | get_app_state click click click fill_form click |
| baseline-settings-dialog-r2 | ok | 16 | 0.2 | 14.0 (7) | 2.0 | 2.2 | 1.6 | 6.2 | fill_form 0.5, click 0.3 | get_app_state click click click fill_form click |
| baseline-terminal-r0 | ok | 13 | 0.1 | 11.0 (4) | 2.8 | 1.9 | 1.7 | 58.0 | type_text 0.9, ocr 0.7 | get_app_state type_text ocr |
| baseline-terminal-r1 | ok | 14 | 0.1 | 12.4 (5) | 2.5 | 2.2 | 1.9 | 58.4 | ocr 0.7, type_text 0.6 | get_app_state type_text type_text ocr |
| baseline-terminal-r2 | ok | 13 | 0.1 | 10.3 (4) | 2.6 | 2.1 | 2.8 | 58.0 | ocr 1.7, type_text 0.9 | get_app_state type_text ocr |
| final-calendar-event-r0 | ok | 20 | 0.2 | 16.0 (7) | 2.3 | 2.1 | 4.1 | 20.9 | act 2.0, act 1.0 | get_app_state act get_app_state act act act |
| final-calendar-event-r1 | ok | 21 | 0.1 | 16.4 (8) | 2.1 | 2.0 | 4.2 | 21.3 | act 1.8, act 0.7 | get_app_state act get_app_state act act act act |
| final-calendar-event-r2 | ok | 21 | 0.1 | 16.9 (7) | 2.4 | 2.4 | 3.9 | 20.9 | act 1.9, act 0.7 | get_app_state act get_app_state act act act |
| final-contacts-infinite-r0 | ok | 22 | 0.1 | 8.9 (3) | 3.0 | 2.4 | 13.4 | 8.8 | act 13.2, get_app_state 0.3 | get_app_state act |
| final-contacts-infinite-r1 | ok | 22 | 0.1 | 8.0 (3) | 2.7 | 2.4 | 13.7 | 8.8 | act 13.4, get_app_state 0.2 | get_app_state act |
| final-contacts-infinite-r2 | ok | 23 | 0.1 | 9.6 (3) | 3.2 | 2.7 | 13.4 | 8.8 | act 13.1, get_app_state 0.2 | get_app_state act |
| final-form-signup-r0 | ok | 11 | 0.1 | 8.0 (2) | 4.0 | 2.2 | 2.8 | 2.6 | act 2.8 | act |
| final-form-signup-r1 | ok | 11 | 0.1 | 8.4 (2) | 4.2 | 2.1 | 2.8 | 2.6 | act 2.8 | act |
| final-form-signup-r2 | ok | 14 | 0.1 | 11.5 (2) | 5.7 | 4.1 | 2.7 | 2.6 | act 2.7 | act |
| final-iframe-billing-r0 | ok | 11 | 0.1 | 8.6 (3) | 2.9 | 2.0 | 2.4 | 5.8 | act 1.5, act 0.9 | act act |
| final-iframe-billing-r1 | ok | 11 | 0.1 | 8.6 (3) | 2.9 | 1.7 | 2.5 | 5.8 | act 1.6, act 0.9 | act act |
| final-iframe-billing-r2 | ok | 13 | 0.8 | 9.3 (3) | 3.1 | 2.4 | 2.5 | 5.8 | act 1.6, act 0.9 | act act |
| final-invoices-bulk-r0 | ok | 16 | 0.1 | 11.6 (4) | 2.9 | 2.4 | 3.9 | 17.0 | act 1.8, click 1.7 | get_app_state act click |
| final-invoices-bulk-r1 | ok | 16 | 0.1 | 11.9 (4) | 3.0 | 2.2 | 3.8 | 17.0 | act 1.8, click 1.7 | get_app_state act click |
| final-invoices-bulk-r2 | ok | 14 | 0.1 | 10.5 (4) | 2.6 | 2.1 | 3.8 | 17.0 | act 1.8, click 1.7 | get_app_state act click |
| final-mail-label-r0 | ok | 21 | 0.2 | 18.0 (6) | 3.0 | 4.0 | 3.2 | 28.6 | act 1.2, act 0.9 | get_app_state act get_app_state act act |
| final-mail-label-r1 | ok | 23 | 0.1 | 19.6 (6) | 3.3 | 2.2 | 3.4 | 28.7 | act 1.4, act 0.8 | get_app_state act get_app_state act act |
| final-mail-label-r2 | ok | 18 | 0.1 | 15.0 (6) | 2.5 | 2.7 | 3.2 | 28.7 | act 1.2, act 0.8 | get_app_state act get_app_state act act |
| final-mail-reply-r0 | ok | 10 | 0.1 | 8.6 (4) | 2.2 | 1.8 | 1.3 | 14.1 | act 0.7, act 0.4 | get_app_state act act |
| final-mail-reply-r1 | ok | 11 | 0.1 | 9.3 (4) | 2.3 | 2.2 | 1.3 | 14.1 | act 0.7, act 0.4 | get_app_state act act |
| final-mail-reply-r2 | ok | 12 | 0.1 | 10.7 (4) | 2.7 | 2.8 | 1.3 | 14.1 | act 0.7, act 0.4 | get_app_state act act |
| final-order-support-r0 | ok | 16 | 0.1 | 13.2 (5) | 2.6 | 2.5 | 2.4 | 10.2 | act 1.5, act 0.4 | get_app_state act act act |
| final-order-support-r1 | ok | 16 | 0.1 | 13.9 (5) | 2.8 | 2.8 | 2.4 | 10.2 | act 1.4, act 0.4 | get_app_state act act act |
| final-order-support-r2 | ok | 15 | 0.2 | 12.2 (5) | 2.4 | 2.6 | 2.3 | 10.2 | act 1.4, act 0.4 | get_app_state act act act |
| final-settings-dialog-r0 | ok | 12 | 0.1 | 10.1 (4) | 2.5 | 2.3 | 2.1 | 4.6 | act 1.1, act 0.8 | act act act |
| final-settings-dialog-r1 | ok | 12 | 0.1 | 10.2 (4) | 2.5 | 2.1 | 2.1 | 4.6 | act 1.1, act 0.8 | act act act |
| final-settings-dialog-r2 | ok | 13 | 0.1 | 10.6 (4) | 2.7 | 2.4 | 2.3 | 4.6 | act 1.3, act 0.8 | act act act |
| final-terminal-r0 | ok | 10 | 0.1 | 8.4 (3) | 2.8 | 2.1 | 1.8 | 1.3 | type_text 1.2, ocr 0.6 | type_text ocr |
| final-terminal-r1 | ok | 13 | 0.1 | 11.2 (4) | 2.8 | 3.0 | 1.6 | 1.7 | type_text 1.1, ocr 0.5 | type_text ocr ocr |
| final-terminal-r2 | ok | 10 | 0.1 | 8.2 (3) | 2.7 | 2.0 | 1.7 | 1.3 | type_text 1.1, ocr 0.5 | type_text ocr |
| baseline-accents-form-r0 | ok | 11 | 0.1 | 9.4 (4) | 2.4 | 2.3 | 1.4 | 2.8 | fill_form 1.0, click 0.3 | get_app_state fill_form click |
| baseline-drag-drop-r0 | ok | 13 | 0.1 | 11.7 (4) | 2.9 | 2.9 | 1.6 | 4.3 | drag 1.3, get_app_state 0.2 | get_app_state drag get_app_state |
| baseline-flaky-field-r0 | ok | 14 | 0.1 | 11.8 (6) | 2.0 | 2.0 | 2.3 | 4.3 | fill_form 1.1, set_value 0.5 | get_app_state fill_form click set_value click |
| baseline-modal-focus-r0 | ok | 9 | 0.1 | 7.8 (4) | 1.9 | 1.9 | 1.5 | 2.6 | fill_form 1.0, click 0.3 | get_app_state fill_form click |
| baseline-rich-compose-r0 | ok | 16 | 0.1 | 12.0 (6) | 2.0 | 2.0 | 3.8 | 5.0 | fill_form 2.7, type_text 0.5 | get_app_state fill_form type_text press_key click |
| baseline-slow-page-r0 | ok | 28 | 0.3 | 25.5 (9) | 2.8 | 3.0 | 1.9 | 12.3 | wait_for 0.7, click 0.4 | get_app_state click get_app_state get_app_state wait_for wait_for get_app_state click |
| baseline-virtual-list-r0 | ok | 23 | 0.1 | 11.2 (5) | 2.2 | 2.1 | 11.9 | 17.0 | scroll 11.0, click 0.5 | get_app_state scroll click get_app_state |
| final-accents-form-r0 | ok | 8 | 0.1 | 6.6 (2) | 3.3 | 1.9 | 1.6 | 1.9 | act 1.6 | act |
| final-drag-drop-r0 | ok | 11 | 0.1 | 9.4 (3) | 3.1 | 2.4 | 1.5 | 2.5 | drag 1.3, get_app_state 0.2 | get_app_state drag |
| final-flaky-field-r0 | ok | 10 | 0.1 | 7.9 (2) | 4.0 | 2.1 | 1.9 | 2.3 | act 1.9 | act |
| final-modal-focus-r0 | ok | 15 | 0.7 | 11.7 (4) | 2.9 | 3.2 | 2.2 | 5.1 | act 1.6, act 0.4 | act act act |
| final-rich-compose-r0 | ok | 18 | 0.1 | 11.7 (4) | 2.9 | 2.3 | 6.6 | 5.7 | act 3.0, act 2.0 | act act act |
| final-slow-page-r0 | ok | 17 | 0.1 | 12.8 (6) | 2.1 | 2.1 | 3.9 | 9.9 | wait_for 3.1, act 0.4 | get_app_state act wait_for get_app_state act |
| final-virtual-list-r0 | ok | 28 | 0.2 | 10.7 (5) | 2.1 | 1.4 | 17.4 | 10.2 | scroll 13.7, act 3.0 | act find_elements scroll click |

| variant | eps | mean wall | boot | model | MCP | turns | s per model turn | KB | share boot/model/MCP |
|---|---|---|---|---|---|---|---|---|---|
| baseline | 37 | 16.7 | 0.1 | 12.9 | 3.6 | 6.3 | 2.0 | 16.1 | 1%/78%/22% |
| final | 37 | 15.3 | 0.2 | 11.2 | 4.0 | 4.1 | 2.7 | 10.3 | 1%/73%/26% |
