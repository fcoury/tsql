#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Query,
    Grid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
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
