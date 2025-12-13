# To Do List

## Pending

### Big tasks

- Connection management
- Scope Ctrl-R per connection by default, allow toggling

### Minor tasks

- I attempted to save by hitting Ctrl+S on the form like this:
┌ Connections (0) ─────────────────────────────────────────────────────────────┐
│                                                                              │
│                             No connections saved                             │
│                                                                              │
│         ┌ New Connection ──────────────────────────────────────────┐         │
│         │Name:     Paypol                                          │         │
│         │──────────────────────────────────────────────────────────│         │
│         │Host:     localhost                                       │         │
│         │Port:     5432                                            │         │
│         │Database: paypol                                          │         │
│         │User:     postgres                                        │         │
│         │Password:                                                 │         │
│         │          [ ] Save to keychain                            │         │
│         │Color:    ◀ green    ▶                                    │         │
│         │──────────────────────────────────────────────────────────│         │
│         │Paste URL:postgres://user:pass@host/db                    │         │
│         │──────────────────────────────────────────────────────────│         │
│         │      Tab next  Ctrl+S save  Ctrl+T test  Esc cancel      │         │
│         │                                                          │         │
│         │                                                          │         │
│         │                                                          │         │
│         └──────────────────────────────────────────────────────────┘         │
│──────────────────────────────────────────────────────────────────────────────│
│          [a]dd [e]dit [d]elete [f]avorite [Enter] connect [q] close          │
└──────────────────────────────────────────────────────────────────────────────┘
And nothing happend.
- Attempted to exit the dialog with <Esc> but had to hit it twice

## Done

- The order on the fields for the connection should be user > password > host > port > database > password. 
- When the user attempts to exit the add new manager dialog with changes, also have the prompt to confirm.

- Hitting Ctrl+Enter on the results panel should rerun the last query
- Quit should have a prompt regardless of unsaved changes, but the prompt should be different if there are unsaved changes
- The uuid fields look great but they should take less space when condensed. Maybe have a special key (suggest one) to expand/collapse them.
- Add mouse support
- Allow scrolling on tab completion and history (Ctrl-R) with Ctrl+J/Ctrl+K
- Add scrollbars everywhere (editors, completions, history, results)

- Fix the coloring of the # column on the selected line: the foreground is the same color as the background, making it invisible.
- Use Ctrl+Enter to run the current query regardless of which state vim editor is on and which panel is focused.
- When we use y (yank) in the editor, we should copy the selected text to the system clipboard so that it can be pasted outside the application.
- Right now it seems like if we open the JSON editor we need to hit <Esc> twice to dismiss it. Create a test to try and replicate this issue, fix it and retest.

