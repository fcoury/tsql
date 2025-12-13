# To Do List

## Pending

### Big tasks

### Minor tasks / fixes

- The dw should only delete the next word (or from where we are to the end of the current word) but it's deleting the whole line. Deleting the whole line should be dd.
- Add mouse support to the help popup and all the pickers

## Done

- The uuid fields look great but they should take less space when condensed by default (8 chars + the unicode ... instead of the literal one).

- Connection picker vs. connection management: when we open the app with no connection as param, instead of showing the connection manager, we want to show a connection picker, reusing the picker we already have for queries.

- Ctrl+Enter while on the INSERT mode on the query editor should return to normal mode and run the query, instead nothing happens.
- After adding the new connection I tried pressing a to create a new one, it didn't work
- Attempted to exit the dialog with <Esc> but had to hit it twice

- The order on the fields for the connection should be user > password > host > port > database > password.
- When the user attempts to exit the add new manager dialog with changes, also have the prompt to confirm.

- Connection management
- Scope Ctrl-R per connection by default, allow toggling

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
