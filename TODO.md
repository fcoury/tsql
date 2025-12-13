# To Do List

## Pending

### Big tasks

- Connection management
- Scope Ctrl-R per connection by default, allow toggling

### Minor tasks

- Hitting Ctrl+Enter on the results panel should rerun the last query
- The uuid fields look great but they should take less space when condensed. Maybe have a special key (suggest one) to expand/collapse them.
- Add mouse support
- Allow scrolling on tab completion and history (Ctrl-R) with Ctrl+J/Ctrl+K
- Add scrollbars everywhere (editors, completions, history, results)

## Done

- Fix the coloring of the # column on the selected line: the foreground is the same color as the background, making it invisible.
- Use Ctrl+Enter to run the current query regardless of which state vim editor is on and which panel is focused.
- When we use y (yank) in the editor, we should copy the selected text to the system clipboard so that it can be pasted outside the application.
- Right now it seems like if we open the JSON editor we need to hit <Esc> twice to dismiss it. Create a test to try and replicate this issue, fix it and retest.
