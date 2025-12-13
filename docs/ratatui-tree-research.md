# Ratatui Tree View Research

Research findings on implementing tree views with expand/collapse functionality in ratatui (version 0.29).

## Executive Summary

- **Built-in Widgets**: Ratatui has `List` widget but no native tree widget
- **Tree Widget Library**: `tui-tree-widget` v0.23.1 is the standard solution
- **Layout System**: `Layout` with `Constraint` for split panels
- **State Management**: `TreeState` tracks selection and expansion
- **Database TUI Examples**: rainfrog (PostgreSQL TUI) uses ratatui but not tree widgets

---

## 1. Built-in Widgets for Lists and Hierarchical Data

### List Widget

Ratatui's `List` widget is designed for flat lists, not hierarchical data. However, it provides excellent state management patterns that can be adapted.

```rust
use ratatui::{
    layout::Rect,
    style::{Style, Stylize},
    widgets::{Block, List, ListItem, ListState},
    Frame,
};

// Create a stateful list
let mut state = ListState::default();
let items = ["Item 1", "Item 2", "Item 3"];
let list = List::new(items)
    .block(Block::bordered().title("List"))
    .highlight_style(Style::new().reversed())
    .highlight_symbol(">> ")
    .repeat_highlight_symbol(true);

frame.render_stateful_widget(list, area, &mut state);
```

**Key Features**:
- `ListState` for selection tracking
- `highlight_style()` for visual feedback
- `highlight_symbol()` for current selection marker
- `direction()` for top-to-bottom or bottom-to-top scrolling
- `scroll_padding()` to keep items visible around selection

**Limitations**:
- No built-in expand/collapse
- No hierarchical structure
- Cannot represent parent-child relationships

---

## 2. tui-tree-widget Crate

The standard solution for tree views in ratatui.

### Installation

```toml
[dependencies]
ratatui = "0.29"
tui-tree-widget = "0.23.1"
```

### Core Components

#### TreeItem

Individual nodes that can be nested to form hierarchical structures:

```rust
use tui_tree_widget::TreeItem;

// Leaf nodes
let leaf = TreeItem::new_leaf("id1", "Alfa");

// Parent nodes with children
let parent = TreeItem::new(
    "id2",
    "Bravo",
    vec![
        TreeItem::new_leaf("id3", "Charlie"),
        TreeItem::new_leaf("id4", "Delta"),
    ]
).expect("all item identifiers are unique");
```

**Important**: Each TreeItem must have a unique identifier.

#### TreeState

Manages selection and expansion state:

```rust
use tui_tree_widget::TreeState;

struct App {
    state: TreeState<&'static str>,  // Generic over identifier type
    items: Vec<TreeItem<'static, &'static str>>,
}
```

**Key Methods**:
- `toggle_selected()` - Expand/collapse the selected node
- `key_left()` - Collapse current node
- `key_right()` - Expand current node
- `select_first()` - Select first item
- `select_last()` - Select last item
- `select_previous()` / `select_next()` - Navigate up/down

#### Tree Widget

The renderable widget:

```rust
use tui_tree_widget::Tree;
use ratatui::{
    widgets::{Block, Widget},
    style::{Style, Color},
};

let widget = Tree::new(&app.items)
    .block(Block::bordered().title("Tree Widget"))
    .highlight_style(Style::new().fg(Color::Black).bg(Color::LightGreen))
    .highlight_symbol(">> ");

frame.render_stateful_widget(widget, area, &mut app.state);
```

### Complete Example

```rust
use ratatui::{
    crossterm::event::{self, Event, KeyCode},
    layout::Rect,
    style::{Color, Style},
    widgets::Block,
    Frame,
};
use tui_tree_widget::{Tree, TreeItem, TreeState};

struct App {
    state: TreeState<&'static str>,
    items: Vec<TreeItem<'static, &'static str>>,
}

impl App {
    fn new() -> Self {
        let items = vec![
            TreeItem::new_leaf("a", "Alfa"),
            TreeItem::new(
                "b",
                "Bravo",
                vec![
                    TreeItem::new_leaf("b1", "Bravo-1"),
                    TreeItem::new_leaf("b2", "Bravo-2"),
                ],
            )
            .expect("unique identifiers"),
            TreeItem::new_leaf("c", "Charlie"),
        ];

        Self {
            state: TreeState::default(),
            items,
        }
    }

    fn handle_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.state.toggle_selected();
            }
            KeyCode::Left => {
                self.state.key_left();
            }
            KeyCode::Right => {
                self.state.key_right();
            }
            KeyCode::Down => {
                self.state.key_down();
            }
            KeyCode::Up => {
                self.state.key_up();
            }
            KeyCode::Home => {
                self.state.select_first();
            }
            KeyCode::End => {
                self.state.select_last();
            }
            KeyCode::PageDown => {
                self.state.scroll_down(10);
            }
            KeyCode::PageUp => {
                self.state.scroll_up(10);
            }
            _ => {}
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect) {
        let widget = Tree::new(&self.items)
            .block(Block::bordered().title("Tree Widget"))
            .highlight_style(Style::new().fg(Color::Black).bg(Color::LightGreen))
            .highlight_symbol(">> ");

        frame.render_stateful_widget(widget, area, &mut self.state);
    }
}
```

### Expand/Collapse Functionality

The tree widget provides multiple ways to expand/collapse:

1. **Toggle**: `state.toggle_selected()` - Toggles expansion of selected node
2. **Explicit Control**:
   - `state.key_left()` - Always collapses
   - `state.key_right()` - Always expands
3. **Navigation**: Selection automatically expands parents as needed

---

## 3. Layout System for Split Panels

Ratatui's `Layout` system divides screen space using constraints.

### Basic Layout

```rust
use ratatui::layout::{Layout, Constraint, Direction};

// Vertical split (top/bottom)
let vertical = Layout::vertical([
    Constraint::Length(5),    // Fixed height: 5 lines
    Constraint::Min(0),       // Remaining space
]);
let [top, bottom] = vertical.areas(area);

// Horizontal split (left/right)
let horizontal = Layout::horizontal([
    Constraint::Percentage(30),  // 30% of width
    Constraint::Percentage(70),  // 70% of width
]);
let [left, right] = horizontal.areas(area);
```

### Database Schema Browser Layout Example

```rust
use ratatui::layout::{Constraint, Layout};

fn render_schema_browser(frame: &mut Frame, area: Rect) {
    // Three-panel layout: header, content (split), footer
    let main_layout = Layout::vertical([
        Constraint::Length(1),      // Header
        Constraint::Min(0),         // Content area
        Constraint::Length(3),      // Footer/help
    ]);
    let [header, content, footer] = main_layout.areas(area);

    // Split content into tree (left) and details (right)
    let content_layout = Layout::horizontal([
        Constraint::Percentage(30),  // Tree view
        Constraint::Percentage(70),  // Table details/preview
    ]);
    let [tree_area, details_area] = content_layout.areas(content);

    // Render widgets in each area
    render_header(frame, header);
    render_tree(frame, tree_area);
    render_details(frame, details_area);
    render_footer(frame, footer);
}
```

### Constraint Types

```rust
use ratatui::layout::Constraint;

// Fixed size
Constraint::Length(10)      // Exactly 10 rows/columns

// Flexible size
Constraint::Min(5)          // At least 5, can grow
Constraint::Max(20)         // At most 20, can shrink
Constraint::Percentage(50)  // 50% of available space
Constraint::Ratio(1, 3)     // 1/3 of available space
Constraint::Fill(1)         // Fill remaining space (weighted)
```

### Advanced Layout with Spacing

```rust
let layout = Layout::vertical([
    Constraint::Length(5),
    Constraint::Min(0),
])
.spacing(1)  // Add 1 line spacing between sections
.margin(2);  // Add 2-line margin around entire layout

let areas = layout.split(area);
```

---

## 4. Stateful Widget Patterns

Ratatui uses the `StatefulWidget` trait for widgets that maintain state between renders.

### Pattern Overview

```rust
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    widgets::{StatefulWidget, StatefulWidgetRef},
};

// State is separate from the widget
struct MyState {
    selected: usize,
    offset: usize,
}

impl MyState {
    fn select_next(&mut self, total_items: usize) {
        self.selected = (self.selected + 1) % total_items;
    }
}

// Widget is ephemeral (created each frame)
struct MyWidget<'a> {
    items: &'a [String],
}

impl StatefulWidget for MyWidget<'_> {
    type State = MyState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        // Rendering logic using state
    }
}
```

### Complete Stateful Widget Example

```rust
use ratatui::{
    backend::TestBackend,
    widgets::{List, ListItem, ListState, StatefulWidget},
    Terminal,
};

struct Events {
    items: Vec<String>,      // Application data
    state: ListState,         // UI state
}

impl Events {
    fn new(items: Vec<String>) -> Self {
        Self {
            items,
            state: ListState::default(),
        }
    }

    // State modification methods
    fn next(&mut self) {
        let i = match self.state.selected() {
            Some(i) => {
                if i >= self.items.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.state.select(Some(i));
    }

    fn previous(&mut self) {
        let i = match self.state.selected() {
            Some(i) => {
                if i == 0 {
                    self.items.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.state.select(Some(i));
    }

    fn unselect(&mut self) {
        self.state.select(None);
    }
}

// Usage in event loop
let mut events = Events::new(vec!["Item 1".into(), "Item 2".into()]);

loop {
    terminal.draw(|f| {
        // Convert application data to UI items
        let items: Vec<ListItem> = events
            .items
            .iter()
            .map(|i| ListItem::new(i.as_str()))
            .collect();

        let list = List::new(items);

        // Render with state
        f.render_stateful_widget(list, f.size(), &mut events.state);
    })?;

    // Handle events
    if let Event::Key(key) = event::read()? {
        match key.code {
            KeyCode::Down => events.next(),
            KeyCode::Up => events.previous(),
            _ => {}
        }
    }
}
```

### Tree State Management Pattern

For tree widgets, the state tracks:
- **Current selection** (which node)
- **Opened nodes** (which nodes are expanded)
- **Scroll offset** (for large trees)

```rust
use tui_tree_widget::TreeState;

struct SchemaTree {
    state: TreeState<String>,  // Using String as identifier
    items: Vec<TreeItem<'static, String>>,
}

impl SchemaTree {
    fn new() -> Self {
        Self {
            state: TreeState::default(),
            items: vec![],
        }
    }

    fn expand_all(&mut self) {
        // Expand all nodes
        for item in &self.items {
            self.expand_recursive(item);
        }
    }

    fn expand_recursive(&mut self, item: &TreeItem<String>) {
        self.state.open(vec![item.identifier().clone()]);
        for child in item.children() {
            self.expand_recursive(child);
        }
    }

    fn collapse_all(&mut self) {
        self.state.close_all();
    }
}
```

---

## 5. Database Schema Browser Implementations

### rainfrog Analysis

[rainfrog](https://github.com/achristmascarl/rainfrog) is a PostgreSQL TUI built with ratatui 0.29.

**Architecture**:
- **No tree widget**: Uses menu-based navigation instead
- **Dependencies**: ratatui, tui-textarea, sqlx
- **Navigation**: Vim-like keybindings
  - `h`/`←` - Focus schemas
  - `l`/`→` - Focus tables
  - `/` - Filter tables
  - `Enter` - Navigate into schema

**Key Insights**:
- Uses ratatui's component template
- Implements collapsible menu without tree widget
- Relies on `List` widget with filtering
- State managed through custom components

### Alternative Approach: Custom Tree Implementation

You could build a custom tree using `List` widget:

```rust
// Flatten tree structure with indentation
fn flatten_tree(nodes: &[SchemaNode], indent: usize) -> Vec<String> {
    let mut result = vec![];
    for node in nodes {
        let prefix = if node.is_expanded { "▼ " } else { "▶ " };
        let indent_str = "  ".repeat(indent);
        result.push(format!("{}{}{}", indent_str, prefix, node.name));

        if node.is_expanded {
            result.extend(flatten_tree(&node.children, indent + 1));
        }
    }
    result
}

// Use with List widget
let items = flatten_tree(&schema_tree, 0);
let list = List::new(items)
    .highlight_symbol(">> ");
```

However, **tui-tree-widget is recommended** as it handles:
- Unique identifiers
- Efficient state management
- Proper rendering
- Standard keybindings

---

## 6. Recommended Implementation Pattern

For a database schema browser with expand/collapse:

### Project Structure

```toml
[dependencies]
ratatui = "0.29"
tui-tree-widget = "0.23.1"
crossterm = "0.28"
```

### Data Model

```rust
use tui_tree_widget::TreeItem;

#[derive(Clone)]
struct SchemaObject {
    name: String,
    object_type: ObjectType,
}

enum ObjectType {
    Database,
    Schema,
    Table,
    Column,
}

fn build_tree_items(schemas: Vec<Schema>) -> Vec<TreeItem<'static, String>> {
    schemas
        .into_iter()
        .map(|schema| {
            let tables = schema.tables
                .into_iter()
                .map(|table| {
                    let columns = table.columns
                        .into_iter()
                        .map(|col| TreeItem::new_leaf(
                            col.name.clone(),
                            col.name,
                        ))
                        .collect();

                    TreeItem::new(
                        table.name.clone(),
                        table.name,
                        columns,
                    ).unwrap()
                })
                .collect();

            TreeItem::new(
                schema.name.clone(),
                schema.name,
                tables,
            ).unwrap()
        })
        .collect()
}
```

### Application State

```rust
use tui_tree_widget::TreeState;

struct App {
    tree_state: TreeState<String>,
    tree_items: Vec<TreeItem<'static, String>>,
    selected_object: Option<SchemaObject>,
}

impl App {
    fn new(schemas: Vec<Schema>) -> Self {
        let tree_items = build_tree_items(schemas);
        let mut tree_state = TreeState::default();
        tree_state.select_first();

        Self {
            tree_state,
            tree_items,
            selected_object: None,
        }
    }

    fn get_selected_object(&self) -> Option<SchemaObject> {
        // Lookup object based on tree_state.selected()
        // Return the corresponding SchemaObject
        None  // Implement based on your data structure
    }
}
```

### Event Handling

```rust
use crossterm::event::{Event, KeyCode};

fn handle_events(app: &mut App) -> Result<()> {
    if let Event::Key(key) = event::read()? {
        match key.code {
            // Tree navigation
            KeyCode::Up => app.tree_state.key_up(),
            KeyCode::Down => app.tree_state.key_down(),
            KeyCode::Left => app.tree_state.key_left(),
            KeyCode::Right => app.tree_state.key_right(),
            KeyCode::Enter => app.tree_state.toggle_selected(),

            // Utility
            KeyCode::Home => app.tree_state.select_first(),
            KeyCode::End => app.tree_state.select_last(),
            KeyCode::PageDown => app.tree_state.scroll_down(10),
            KeyCode::PageUp => app.tree_state.scroll_up(10),

            KeyCode::Char('q') => return Ok(()),
            _ => {}
        }

        // Update selected object
        app.selected_object = app.get_selected_object();
    }
    Ok(())
}
```

### Rendering

```rust
fn render_ui(frame: &mut Frame, app: &mut App) {
    let layout = Layout::horizontal([
        Constraint::Percentage(30),  // Tree
        Constraint::Percentage(70),  // Details
    ]);
    let [tree_area, details_area] = layout.areas(frame.area());

    // Render tree
    let tree = Tree::new(&app.tree_items)
        .block(Block::bordered().title("Schema"))
        .highlight_style(Style::default().bg(Color::Blue).fg(Color::White))
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(tree, tree_area, &mut app.tree_state);

    // Render details
    if let Some(obj) = &app.selected_object {
        let details = Paragraph::new(format!("Selected: {}", obj.name))
            .block(Block::bordered().title("Details"));
        frame.render_widget(details, details_area);
    }
}
```

---

## 7. Performance Considerations

### Large Trees

For databases with many objects:

```rust
// Use lazy loading
impl App {
    fn expand_node(&mut self, node_id: &str) {
        // Load children only when expanded
        if !self.loaded_nodes.contains(node_id) {
            let children = self.load_children(node_id);
            self.add_children(node_id, children);
            self.loaded_nodes.insert(node_id.to_string());
        }
    }
}
```

### Filtering

Implement filtering for large schemas:

```rust
impl App {
    fn filter_tree(&mut self, query: &str) {
        self.tree_items = self.all_items
            .iter()
            .filter(|item| item.text().contains(query))
            .cloned()
            .collect();
    }
}
```

---

## 8. Key Takeaways

1. **Use tui-tree-widget** for tree views - it's the standard, well-maintained solution
2. **State management** is separate from widgets (StatefulWidget pattern)
3. **Layout system** is flexible with constraints for responsive design
4. **TreeState** handles all expand/collapse logic automatically
5. **Unique identifiers** are required for each TreeItem
6. **Two-panel layout** (tree + details) is common for schema browsers
7. **rainfrog** shows an alternative list-based approach, but tree widget is more intuitive

---

## References

- [ratatui Documentation](https://docs.rs/ratatui/0.29)
- [tui-tree-widget GitHub](https://github.com/EdJoPaTo/tui-rs-tree-widget)
- [tui-tree-widget crates.io](https://crates.io/crates/tui-tree-widget)
- [rainfrog - PostgreSQL TUI](https://github.com/achristmascarl/rainfrog)
- [ratatui Layout Examples](https://github.com/ratatui/ratatui/tree/main/examples)
- [awesome-ratatui](https://github.com/ratatui/awesome-ratatui)
- [Building A Database Management TUI With Ratatui And Rust](https://undercodetesting.com/building-a-database-management-tui-with-ratatui-and-rust/)

---

## Example Projects to Study

1. **mqttui** - Original use case for tui-tree-widget: [GitHub](https://github.com/EdJoPaTo/mqttui)
2. **rainfrog** - PostgreSQL TUI with menu navigation: [GitHub](https://github.com/achristmascarl/rainfrog)
3. **gobang** - Cross-platform database TUI: [GitHub](https://github.com/TaKO8Ki/gobang)
4. **ratatui examples** - Official examples directory: [GitHub](https://github.com/ratatui/ratatui/tree/main/examples)
