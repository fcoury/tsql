/// Which section of the sidebar is focused
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarSection {
    #[default]
    Connections,
    Schema,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Query,
    Grid,
    Sidebar(SidebarSection),
}

impl Focus {
    pub fn label(self) -> &'static str {
        match self {
            Focus::Query => "QUERY",
            Focus::Grid => "RESULTS",
            Focus::Sidebar(SidebarSection::Connections) => "CONNECTIONS",
            Focus::Sidebar(SidebarSection::Schema) => "SCHEMA",
        }
    }
}

/// Direction for spatial pane navigation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelDirection {
    Left,
    Down,
    Up,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual => "VISUAL",
        }
    }
}

/// Target for the search prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchTarget {
    /// Search in the query editor.
    Editor,
    /// Search in the results grid.
    Grid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DbStatus {
    Disconnected,
    Connecting,
    Connected,
    Error,
}

impl DbStatus {
    pub fn label(&self) -> &'static str {
        match self {
            DbStatus::Disconnected => "DISCONNECTED",
            DbStatus::Connecting => "CONNECTING",
            DbStatus::Connected => "CONNECTED",
            DbStatus::Error => "ERROR",
        }
    }
}
