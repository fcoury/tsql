# feat: Add Ctrl+HJKL Panel Navigation

## Overview

Add vim-style directional panel navigation using Ctrl+HJKL as global keybindings. This allows users to move focus between panels (Sidebar, Query Editor, Results Grid) using intuitive directional keys, except when pickers are active (which already use Ctrl+J/K for item navigation).

## Problem Statement

Currently, panel navigation uses Tab/Shift+Tab for sequential cycling through panels. This doesn't match the spatial mental model of the layout:

```
┌─────────────┬──────────────────┐
│             │  Query Editor    │ ← Focus::Query
│  Sidebar    ├──────────────────┤
│   (Left)    │  Results Grid    │ ← Focus::Grid
│             │                  │
└─────────────┴──────────────────┘
     ↑
Focus::Sidebar(Connections|Schema)
```

Users expect to press "left" to go to sidebar, "down" to go to grid, etc.

## Proposed Solution

Add Ctrl+HJKL keybindings that navigate between panels based on their spatial position:
- **Ctrl+H**: Move focus left (to Sidebar)
- **Ctrl+J**: Move focus down
- **Ctrl+K**: Move focus up
- **Ctrl+L**: Move focus right (from Sidebar)

### Navigation Matrix

The navigation is **spatially precise** based on vertical alignment:

```
┌─────────────────┬──────────────────┐
│  Connections    │  Query Editor    │  ← Top row
├─────────────────┼──────────────────┤
│  Schema         │  Results Grid    │  ← Bottom row
└─────────────────┴──────────────────┘
```

| Current Focus | Ctrl+H | Ctrl+J | Ctrl+K | Ctrl+L |
|---------------|--------|--------|--------|--------|
| Query | Sidebar(Connections) | Grid | no-op | no-op |
| Grid | Sidebar(Schema) | no-op | Query | no-op |
| Sidebar(Connections) | no-op | Sidebar(Schema) | no-op | Query |
| Sidebar(Schema) | no-op | no-op | Sidebar(Connections) | Grid |

**Key insight:**
- Query ↔ Connections (same top row)
- Grid ↔ Schema (same bottom row)
- Ctrl+J/K move vertically within each column

**When sidebar is hidden:** Ctrl+H and Ctrl+L are no-ops.

### Exception: Pickers

FuzzyPicker (used for history picker, connection picker) already uses:
- **Ctrl+J**: Move selection down
- **Ctrl+K**: Move selection up

When any picker is active, Ctrl+J/K should continue to work for picker navigation (not panel navigation). Ctrl+H/L will be no-ops when picker is active.

## Technical Approach

### Files to Modify

1. **`crates/tsql/src/config/keymap.rs`**
   - Add new `Action` variants for panel navigation

2. **`crates/tsql/src/app/app.rs`**
   - Add global key handling for Ctrl+HJKL before focus-specific handling
   - Add navigation logic method

3. **`crates/tsql/src/app/state.rs`** (optional)
   - Could add `Direction` enum and `Focus::move_direction()` method

### Implementation Details

#### Step 1: Add Action Variants

```rust
// crates/tsql/src/config/keymap.rs - Add to Action enum
pub enum Action {
    // ... existing actions ...

    /// Navigate focus to panel on the left
    FocusPanelLeft,
    /// Navigate focus to panel below
    FocusPanelDown,
    /// Navigate focus to panel above
    FocusPanelUp,
    /// Navigate focus to panel on the right
    FocusPanelRight,
}
```

Update `Action::from_str` to parse these new actions.

#### Step 2: Add Global Navigation Handler

In `app.rs`, add a method to handle directional navigation:

```rust
// crates/tsql/src/app/app.rs
impl App {
    /// Handle directional panel navigation (Ctrl+HJKL)
    /// Returns true if a navigation key was handled
    fn handle_panel_navigation(&mut self, key: &KeyEvent) -> bool {
        // Only handle Ctrl+HJKL
        if key.modifiers != KeyModifiers::CONTROL {
            return false;
        }

        let direction = match key.code {
            KeyCode::Char('h') => Direction::Left,
            KeyCode::Char('j') => Direction::Down,
            KeyCode::Char('k') => Direction::Up,
            KeyCode::Char('l') => Direction::Right,
            _ => return false,
        };

        // Calculate new focus based on direction and current state
        let new_focus = self.calculate_focus_for_direction(direction);

        if let Some(focus) = new_focus {
            self.focus = focus;
            // Update sidebar focus if moving to sidebar
            if let Focus::Sidebar(section) = focus {
                self.sidebar_focus = section;
            }
        }

        true // Key was handled (even if no-op)
    }

    fn calculate_focus_for_direction(&self, direction: Direction) -> Option<Focus> {
        // If sidebar hidden, Ctrl+H/L do nothing
        if !self.sidebar_visible && matches!(direction, Direction::Left | Direction::Right) {
            return None;
        }

        // Navigation is spatially precise based on vertical alignment:
        // ┌─────────────────┬──────────────────┐
        // │  Connections    │  Query Editor    │  ← Top row
        // ├─────────────────┼──────────────────┤
        // │  Schema         │  Results Grid    │  ← Bottom row
        // └─────────────────┴──────────────────┘

        match (&self.focus, direction) {
            // From Query (top-right) - aligned with Connections
            (Focus::Query, Direction::Left) => Some(Focus::Sidebar(SidebarSection::Connections)),
            (Focus::Query, Direction::Down) => Some(Focus::Grid),

            // From Grid (bottom-right) - aligned with Schema
            (Focus::Grid, Direction::Left) => Some(Focus::Sidebar(SidebarSection::Schema)),
            (Focus::Grid, Direction::Up) => Some(Focus::Query),

            // From Sidebar(Connections) (top-left) - aligned with Query
            (Focus::Sidebar(SidebarSection::Connections), Direction::Down) => {
                Some(Focus::Sidebar(SidebarSection::Schema))
            }
            (Focus::Sidebar(SidebarSection::Connections), Direction::Right) => Some(Focus::Query),

            // From Sidebar(Schema) (bottom-left) - aligned with Grid
            (Focus::Sidebar(SidebarSection::Schema), Direction::Up) => {
                Some(Focus::Sidebar(SidebarSection::Connections))
            }
            (Focus::Sidebar(SidebarSection::Schema), Direction::Right) => Some(Focus::Grid),

            // All other combinations are no-ops (at boundary)
            _ => None,
        }
    }
}

enum Direction {
    Left,
    Down,
    Up,
    Right,
}
```

#### Step 3: Integrate into Key Handler

In `on_key()` method, add panel navigation handling **after** picker checks but **before** focus-specific handling:

```rust
// crates/tsql/src/app/app.rs - in on_key() method

// ... existing picker handling at lines 1667-1676 ...

// Handle history picker
if self.history_picker.is_some() {
    self.handle_history_picker_key(key);
    return true;
}

// Handle connection picker
if self.connection_picker.is_some() {
    self.handle_connection_picker_key(key);
    return true;
}

// NEW: Global panel navigation (Ctrl+HJKL) - only when no picker active
if self.mode == Mode::Normal && self.handle_panel_navigation(&key) {
    return true;
}

// ... rest of existing handling ...
```

#### Step 4: Update Help/Documentation

Add the new keybindings to the help popup and any documentation.

## Acceptance Criteria

- [ ] Ctrl+H moves focus left to Sidebar (when visible)
- [ ] Ctrl+J moves focus down (Query→Grid, Sidebar Connections→Schema)
- [ ] Ctrl+K moves focus up (Grid→Query, Sidebar Schema→Connections)
- [ ] Ctrl+L moves focus right from Sidebar to Query/Grid
- [ ] Navigation is no-op when direction has no target panel
- [ ] Navigation is no-op for Ctrl+H/L when sidebar is hidden
- [ ] Ctrl+J/K continue to work in pickers for item navigation
- [ ] Ctrl+H/L are no-ops when picker is active
- [ ] Works in Normal mode (not in Insert mode in editor)
- [ ] Visual focus indicator updates immediately on navigation

## Testing Requirements

### Unit Tests

```rust
// crates/tsql/src/app/app.rs - tests module

#[test]
fn test_ctrl_h_from_query_moves_to_sidebar() {
    let mut app = create_test_app();
    app.focus = Focus::Query;
    app.sidebar_visible = true;

    let key = KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL);
    app.handle_panel_navigation(&key);

    assert_eq!(app.focus, Focus::Sidebar(SidebarSection::Connections));
}

#[test]
fn test_ctrl_h_from_query_noop_when_sidebar_hidden() {
    let mut app = create_test_app();
    app.focus = Focus::Query;
    app.sidebar_visible = false;

    let key = KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL);
    app.handle_panel_navigation(&key);

    assert_eq!(app.focus, Focus::Query); // Unchanged
}

#[test]
fn test_ctrl_j_moves_down_from_query_to_grid() {
    let mut app = create_test_app();
    app.focus = Focus::Query;

    let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL);
    app.handle_panel_navigation(&key);

    assert_eq!(app.focus, Focus::Grid);
}

#[test]
fn test_ctrl_j_blocked_when_picker_active() {
    // This tests that picker handling comes BEFORE panel navigation
    let mut app = create_test_app();
    app.focus = Focus::Query;
    app.history_picker = Some(create_test_picker());

    // Ctrl+J should be handled by picker, not panel navigation
    let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL);
    let handled = app.on_key(key);

    assert!(handled);
    assert_eq!(app.focus, Focus::Query); // Focus unchanged
}
```

## Dependencies & Risks

### Dependencies
- None - this is a self-contained feature

### Risks
- **Ctrl+J/K conflict**: Mitigated by checking picker state first
- **Ctrl+H terminal conflict**: Some terminals use Ctrl+H for backspace. Testing needed.
- **Ctrl+L terminal conflict**: Some terminals use Ctrl+L for clear. Testing needed.

### Terminal Compatibility Notes
- **iTerm2**: Ctrl+HJKL should work
- **Alacritty**: Ctrl+HJKL should work
- **Windows Terminal**: Ctrl+HJKL should work
- **macOS Terminal.app**: May need to disable "Use Option as Meta key"

## References

### Internal
- Focus enum: `crates/tsql/src/app/state.rs:10-14`
- Current Tab navigation: `crates/tsql/src/app/app.rs:1855-1891`
- Key handler: `crates/tsql/src/app/app.rs:1563`
- Picker key handling: `crates/tsql/src/app/app.rs:1667-1676`
- FuzzyPicker Ctrl+J/K: `crates/tsql/src/ui/fuzzy_picker.rs:126-243`
- Action enum: `crates/tsql/src/config/keymap.rs:11-108`

### External
- [Vim Keybindings Everywhere](https://github.com/erikw/vim-keybindings-everywhere-the-ultimate-list)
- [Ratatui Event Handling](https://ratatui.rs/concepts/event-handling/)
- [Managing Keybindings in Rust TUI Apps](https://dystroy.org/blog/keybindings/)
