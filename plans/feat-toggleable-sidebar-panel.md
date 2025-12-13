# feat: Toggleable Left Sidebar with Connections & Schema Tree

**Created**: 2025-12-13
**Type**: Enhancement
**Complexity**: High

## Overview

Add a toggleable left sidebar panel with two sections:
- **Top**: Connections list (quick switch between saved connections)
- **Bottom**: Database schema tree (tables, views, columns in expandable hierarchy)

```
┌──────────────────┬─────────────────────────────────────────────┐
│ Connections      │  Query Editor                               │
│ ● Production DB  │  SELECT * FROM customers WHERE...           │
│   Dev DB         │                                             │
│   Test DB        ├─────────────────────────────────────────────┤
│──────────────────│  Results Grid                               │
│ Schema           │  id | name      | email                     │
│ ▼ public         │  1  | John Doe  | john@example.com          │
│   ▼ Tables       │  2  | Jane Doe  | jane@example.com          │
│       customers  │                                             │
│       orders     ├─────────────────────────────────────────────┤
│   ▶ Views        │  Connected: Production DB | 2 rows | 0.05s  │
└──────────────────┴─────────────────────────────────────────────┘
```

## Technical Approach

### Dependencies

Add to `Cargo.toml`:
```toml
[dependencies]
tui-tree-widget = "0.22"
```

### New Files

| File | Purpose |
|------|---------|
| `src/ui/sidebar.rs` | Main sidebar component with state and rendering |
| `src/ui/schema_tree.rs` | Schema tree widget using tui-tree-widget |
| `src/schema/mod.rs` | Schema metadata fetching and caching |

### Modified Files

| File | Changes |
|------|---------|
| `src/app/app.rs` | Add sidebar state, layout split, focus handling |
| `src/app/state.rs` | Add `Focus::Sidebar` variant |
| `src/config/keymap.rs` | Add sidebar toggle action |
| `src/ui/mod.rs` | Export new modules |

## Implementation

### Phase 1: Sidebar Shell

**Goal**: Toggleable empty sidebar with layout integration

#### 1.1 Add Sidebar State to App (`src/app/app.rs`)

```rust
pub struct App {
    // ... existing fields ...
    pub sidebar_visible: bool,
    pub sidebar_width: u16,
    pub sidebar_focus: SidebarSection,
}

#[derive(Default, Clone, Copy, PartialEq)]
pub enum SidebarSection {
    #[default]
    Connections,
    Schema,
}
```

#### 1.2 Modify Layout (`src/app/app.rs:~821`)

```rust
// Before: vertical-only layout
// After: horizontal split, then vertical for main content

let horizontal = Layout::horizontal([
    if self.sidebar_visible {
        Constraint::Length(self.sidebar_width)
    } else {
        Constraint::Length(0)
    },
    Constraint::Min(60), // Main content minimum width
]).split(frame.size());

let sidebar_area = horizontal[0];
let main_area = horizontal[1];

// Existing vertical layout for main_area
let vertical = Layout::vertical([
    Constraint::Length(7),           // Query editor
    Constraint::Length(error_height),
    Constraint::Min(3),              // Grid
    Constraint::Length(1),           // Status
]).split(main_area);
```

#### 1.3 Add Toggle Shortcut (`src/config/keymap.rs`)

```rust
pub enum Action {
    // ... existing actions ...
    ToggleSidebar,
}
```

Default binding: `Ctrl+B` (configurable)

#### 1.4 Create Sidebar Component (`src/ui/sidebar.rs`)

```rust
pub struct Sidebar {
    pub connections_scroll: usize,
    pub schema_scroll: usize,
}

impl Sidebar {
    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        connections: &ConnectionsFile,
        current_connection: Option<&str>,
        schema: Option<&SchemaTree>,
        focused_section: SidebarSection,
        has_focus: bool,
    ) {
        // Split into top (30%) and bottom (70%)
        let chunks = Layout::vertical([
            Constraint::Percentage(30),
            Constraint::Percentage(70),
        ]).split(area);

        self.render_connections(frame, chunks[0], connections, current_connection,
            has_focus && focused_section == SidebarSection::Connections);
        self.render_schema(frame, chunks[1], schema,
            has_focus && focused_section == SidebarSection::Schema);
    }
}
```

### Phase 2: Connections List

**Goal**: Display and interact with saved connections

#### 2.1 Connections Rendering

```rust
fn render_connections(
    &mut self,
    frame: &mut Frame,
    area: Rect,
    connections: &ConnectionsFile,
    current: Option<&str>,
    focused: bool,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Connections ")
        .border_style(if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default()
        });

    let items: Vec<ListItem> = connections.sorted()
        .iter()
        .map(|conn| {
            let marker = if Some(conn.name.as_str()) == current { "● " } else { "  " };
            let style = if Some(conn.name.as_str()) == current {
                Style::default().bold()
            } else {
                Style::default()
            };
            ListItem::new(format!("{}{}", marker, conn.name)).style(style)
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().reversed());

    frame.render_stateful_widget(list, area, &mut self.connections_state);
}
```

#### 2.2 Connection Actions

| Key | Action |
|-----|--------|
| `Enter` | Connect to selected connection |
| `↑/↓` | Navigate list |
| `a` | Open add connection modal |
| `e` | Open edit connection modal |
| `d` | Delete connection (with confirmation) |

### Phase 3: Schema Tree

**Goal**: Display database schema in expandable tree

#### 3.1 Schema Data Structure (`src/schema/mod.rs`)

```rust
use tui_tree_widget::{TreeItem, TreeState};

pub struct SchemaCache {
    pub tree_items: Vec<TreeItem<'static, String>>,
    pub tree_state: TreeState<String>,
    pub last_refreshed: Option<Instant>,
    pub loading: bool,
    pub error: Option<String>,
}

impl SchemaCache {
    pub async fn fetch(&mut self, client: &Client) -> Result<()> {
        self.loading = true;

        // Fetch schemas
        let schemas = client.query(
            "SELECT schema_name FROM information_schema.schemata
             WHERE schema_name NOT IN ('pg_catalog', 'information_schema')
             ORDER BY schema_name", &[]
        ).await?;

        // Fetch tables per schema
        let tables = client.query(
            "SELECT table_schema, table_name, table_type
             FROM information_schema.tables
             WHERE table_schema NOT IN ('pg_catalog', 'information_schema')
             ORDER BY table_schema, table_name", &[]
        ).await?;

        // Build tree structure
        self.tree_items = self.build_tree(schemas, tables);
        self.loading = false;
        self.last_refreshed = Some(Instant::now());
        Ok(())
    }

    fn build_tree(&self, schemas: Vec<Row>, tables: Vec<Row>) -> Vec<TreeItem<'static, String>> {
        // Group tables by schema, then by type (Tables, Views)
        // Return hierarchical TreeItem structure
    }
}
```

#### 3.2 Schema Tree Rendering (`src/ui/schema_tree.rs`)

```rust
pub fn render_schema_tree(
    frame: &mut Frame,
    area: Rect,
    cache: &mut SchemaCache,
    focused: bool,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Schema ")
        .border_style(if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default()
        });

    if cache.loading {
        let loading = Paragraph::new("Loading schema...")
            .block(block)
            .alignment(Alignment::Center);
        frame.render_widget(loading, area);
        return;
    }

    if let Some(error) = &cache.error {
        let error_widget = Paragraph::new(format!("Error: {}\nPress 'r' to retry", error))
            .block(block)
            .style(Style::default().fg(Color::Red));
        frame.render_widget(error_widget, area);
        return;
    }

    let tree = Tree::new(&cache.tree_items)
        .expect("valid tree")
        .block(block)
        .highlight_style(Style::default().reversed())
        .highlight_symbol("▶ ");

    frame.render_stateful_widget(tree, area, &mut cache.tree_state);
}
```

#### 3.3 Tree Actions

| Key | Action |
|-----|--------|
| `↑/↓` | Navigate tree |
| `→` or `Enter` | Expand node / Insert name at cursor |
| `←` | Collapse node / Go to parent |
| `r` | Refresh schema |
| `Enter` on leaf | Insert object name into query editor |

### Phase 4: Focus Management

**Goal**: Seamless keyboard navigation between sidebar and main content

#### 4.1 Extend Focus Enum (`src/app/state.rs`)

```rust
#[derive(Default, Clone, Copy, PartialEq)]
pub enum Focus {
    #[default]
    QueryEditor,
    Grid,
    Sidebar(SidebarSection),
}
```

#### 4.2 Focus Transitions

| Current Focus | Key | New Focus |
|---------------|-----|-----------|
| QueryEditor | `Ctrl+B` | Toggle sidebar visibility |
| QueryEditor | `Tab` (sidebar open) | Sidebar(Connections) |
| Grid | `Tab` (sidebar open) | Sidebar(Connections) |
| Sidebar(Connections) | `↓` at bottom | Sidebar(Schema) |
| Sidebar(Schema) | `↑` at top | Sidebar(Connections) |
| Sidebar(*) | `Tab` | QueryEditor |
| Sidebar(*) | `Esc` | QueryEditor |

#### 4.3 Input Priority Update (`src/app/app.rs`)

Add sidebar handling early in the key event cascade:
```rust
// After modal handling, before grid/editor handling
if self.sidebar_visible && matches!(self.focus, Focus::Sidebar(_)) {
    if let Some(action) = self.sidebar.handle_key(key, &mut self.sidebar_focus) {
        match action {
            SidebarAction::Connect(name) => self.connect_to(&name),
            SidebarAction::InsertText(text) => self.editor.insert(&text),
            SidebarAction::OpenAddConnection => self.open_connection_form(None),
            SidebarAction::OpenEditConnection(name) => self.open_connection_form(Some(name)),
            SidebarAction::RefreshSchema => self.refresh_schema(),
            SidebarAction::FocusEditor => self.focus = Focus::QueryEditor,
        }
        return;
    }
}
```

## Acceptance Criteria

### Functional Requirements

- [ ] `Ctrl+B` toggles sidebar visibility
- [ ] Sidebar shows connections list in top section
- [ ] Active connection indicated with `●` marker and bold text
- [ ] `Enter` on connection switches to that database
- [ ] Schema tree shows: Schema → Tables/Views → Columns hierarchy
- [ ] Arrow keys expand/collapse tree nodes
- [ ] `Enter` on table/column inserts name at cursor in query editor
- [ ] `Tab` moves focus between sidebar and main content
- [ ] `↑/↓` at section boundaries moves between Connections and Schema
- [ ] `r` in schema section refreshes schema data
- [ ] Loading indicator while fetching schema
- [ ] Error message with retry option on schema fetch failure

### Non-Functional Requirements

- [ ] Sidebar width: 30 characters (fixed)
- [ ] Minimum terminal width: 90 columns when sidebar open
- [ ] Schema fetch completes in < 2 seconds for databases with < 500 objects
- [ ] Tree renders smoothly with 1000+ nodes (virtual scrolling)

### Configuration

- [ ] `sidebar.enabled` - Whether sidebar feature is available (default: true)
- [ ] `sidebar.width` - Sidebar width in characters (default: 30)
- [ ] `sidebar.default_visible` - Open on startup (default: false)
- [ ] Keybinding for toggle configurable via keymap system

## Test Plan

### Unit Tests

- [ ] Sidebar visibility toggle
- [ ] Layout calculation with/without sidebar
- [ ] Focus transitions between sections
- [ ] Tree state management (expand/collapse/select)
- [ ] Schema data structure building

### Integration Tests

- [ ] Schema fetch from real PostgreSQL database
- [ ] Connection switching via sidebar
- [ ] Text insertion into query editor

### Manual Testing

- [ ] Narrow terminal (< 90 columns) behavior
- [ ] Large schema (500+ tables) performance
- [ ] Keyboard-only navigation flow
- [ ] Modal interaction while sidebar open (should block sidebar toggle)

## Edge Cases

| Scenario | Behavior |
|----------|----------|
| No connections saved | Show "No connections. Press 'a' to add" |
| Not connected to database | Show "Connect to view schema" in tree area |
| Schema fetch fails | Show error with retry option |
| Terminal too narrow | Auto-hide sidebar or show warning |
| Modal open + Ctrl+B | Ignore toggle (modals block sidebar) |
| Very long object names | Truncate with ellipsis |

## Future Enhancements (Not in Scope)

- Resizable sidebar width (drag or keyboard)
- Search/filter within schema tree
- Column data types in tree
- Right-click context menus
- Multi-select for batch insert
- Stored procedures/functions in tree
- Index and constraint information

## References

### Internal
- Layout system: `src/app/app.rs:821-834`
- Modal pattern: `src/ui/help_popup.rs`
- Connection management: `src/ui/connection_manager.rs`
- Focus handling: `src/app/state.rs`
- Keymap system: `src/config/keymap.rs`

### External
- [tui-tree-widget](https://crates.io/crates/tui-tree-widget) - Tree widget for ratatui
- [ratatui Layout docs](https://ratatui.rs/concepts/layout/)
- [PostgreSQL information_schema](https://www.postgresql.org/docs/current/information-schema.html)
