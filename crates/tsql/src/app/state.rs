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
