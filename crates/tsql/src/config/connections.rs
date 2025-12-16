//! Connection management for saved database connections.
//!
//! This module handles:
//! - Loading and saving connections to ~/.config/tsql/connections.toml
//! - Secure password storage via OS keychain (keyring crate)
//! - URL parsing and construction
//! - Connection entry validation

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use url::Url;

/// Service name for keyring storage
const KEYRING_SERVICE: &str = "tsql";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SslMode {
    Disable,
    Prefer,
    Require,
    VerifyCa,
    VerifyFull,
}

impl SslMode {
    /// Number of SSL mode variants (for UI cycling).
    pub const COUNT: usize = 5;

    pub fn as_str(self) -> &'static str {
        match self {
            SslMode::Disable => "disable",
            SslMode::Prefer => "prefer",
            SslMode::Require => "require",
            SslMode::VerifyCa => "verify-ca",
            SslMode::VerifyFull => "verify-full",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "disable" => Some(SslMode::Disable),
            "prefer" => Some(SslMode::Prefer),
            "require" => Some(SslMode::Require),
            "verify-ca" => Some(SslMode::VerifyCa),
            "verify-full" => Some(SslMode::VerifyFull),
            _ => None,
        }
    }

    /// Convert to a numeric index for UI cycling.
    pub fn to_index(self) -> usize {
        match self {
            SslMode::Disable => 0,
            SslMode::Prefer => 1,
            SslMode::Require => 2,
            SslMode::VerifyCa => 3,
            SslMode::VerifyFull => 4,
        }
    }

    /// Convert from a numeric index, defaulting to Disable for invalid values.
    pub fn from_index(index: usize) -> Self {
        match index {
            1 => SslMode::Prefer,
            2 => SslMode::Require,
            3 => SslMode::VerifyCa,
            4 => SslMode::VerifyFull,
            _ => SslMode::Disable,
        }
    }
}

/// Named colors for connection visual identification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionColor {
    #[default]
    None,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    Gray,
}

impl ConnectionColor {
    /// Convert to ratatui Color
    pub fn to_ratatui_color(&self) -> Option<ratatui::style::Color> {
        use ratatui::style::Color;
        match self {
            ConnectionColor::None => None,
            ConnectionColor::Red => Some(Color::Red),
            ConnectionColor::Green => Some(Color::Green),
            ConnectionColor::Yellow => Some(Color::Yellow),
            ConnectionColor::Blue => Some(Color::Blue),
            ConnectionColor::Magenta => Some(Color::Magenta),
            ConnectionColor::Cyan => Some(Color::Cyan),
            ConnectionColor::White => Some(Color::White),
            ConnectionColor::Gray => Some(Color::Gray),
        }
    }

    /// Get all available color names for UI display
    pub fn all_names() -> &'static [&'static str] {
        &[
            "none", "red", "green", "yellow", "blue", "magenta", "cyan", "white", "gray",
        ]
    }
}

impl std::str::FromStr for ConnectionColor {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "none" | "" => Ok(ConnectionColor::None),
            "red" => Ok(ConnectionColor::Red),
            "green" => Ok(ConnectionColor::Green),
            "yellow" => Ok(ConnectionColor::Yellow),
            "blue" => Ok(ConnectionColor::Blue),
            "magenta" => Ok(ConnectionColor::Magenta),
            "cyan" => Ok(ConnectionColor::Cyan),
            "white" => Ok(ConnectionColor::White),
            "gray" | "grey" => Ok(ConnectionColor::Gray),
            _ => Err(anyhow!("Unknown color: {}", s)),
        }
    }
}

impl std::fmt::Display for ConnectionColor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ConnectionColor::None => "none",
            ConnectionColor::Red => "red",
            ConnectionColor::Green => "green",
            ConnectionColor::Yellow => "yellow",
            ConnectionColor::Blue => "blue",
            ConnectionColor::Magenta => "magenta",
            ConnectionColor::Cyan => "cyan",
            ConnectionColor::White => "white",
            ConnectionColor::Gray => "gray",
        };
        write!(f, "{}", s)
    }
}

/// A saved database connection entry
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionEntry {
    /// Unique name for this connection
    pub name: String,
    /// Database host
    pub host: String,
    /// Database port (default: 5432)
    #[serde(default = "default_port")]
    pub port: u16,
    /// Database name
    pub database: String,
    /// Username
    pub user: String,
    /// Whether password is stored in OS keychain
    #[serde(default)]
    pub password_in_keychain: bool,
    /// Environment variable name containing password (fallback)
    #[serde(default)]
    pub password_env: Option<String>,
    /// Whether this connection requires no password (auto-detected when saved with empty password)
    #[serde(default)]
    pub no_password_required: bool,
    /// Visual color indicator
    #[serde(default)]
    pub color: ConnectionColor,
    /// Favorite position (1-9) for quick switch, None if not a favorite
    #[serde(default)]
    pub favorite: Option<u8>,
    /// SSL mode (for Phase 7)
    #[serde(default)]
    pub ssl_mode: Option<SslMode>,
}

fn default_port() -> u16 {
    5432
}

impl Default for ConnectionEntry {
    fn default() -> Self {
        Self {
            name: String::new(),
            host: "localhost".to_string(),
            port: 5432,
            database: String::new(),
            user: String::new(),
            password_in_keychain: false,
            password_env: None,
            no_password_required: false,
            color: ConnectionColor::None,
            favorite: None,
            ssl_mode: None,
        }
    }
}

impl ConnectionEntry {
    /// Create a new connection entry with the given name
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    /// Build a PostgreSQL connection URL from the entry fields
    ///
    /// If `password` is provided, it will be included in the URL.
    /// Otherwise, the URL will not contain authentication.
    pub fn to_url(&self, password: Option<&str>) -> String {
        let mut url = "postgres://".to_string();

        // Add user
        url.push_str(&self.user);

        // Add password if provided
        if let Some(pwd) = password {
            url.push(':');
            url.push_str(&urlencoding::encode(pwd));
        }

        // Add host and port
        url.push('@');
        url.push_str(&self.host);
        if self.port != 5432 {
            url.push(':');
            url.push_str(&self.port.to_string());
        }

        // Add database
        url.push('/');
        url.push_str(&self.database);

        if let Some(mode) = self.ssl_mode {
            url.push_str("?sslmode=");
            url.push_str(mode.as_str());
        }

        url
    }

    /// Parse a PostgreSQL URL and create a ConnectionEntry
    ///
    /// Returns the entry and the password if one was found in the URL.
    pub fn from_url(name: &str, url_str: &str) -> Result<(Self, Option<String>)> {
        let url = Url::parse(url_str).context("Invalid URL format")?;

        if url.scheme() != "postgres" && url.scheme() != "postgresql" {
            return Err(anyhow!("URL must use postgres:// or postgresql:// scheme"));
        }

        let host = url
            .host_str()
            .ok_or_else(|| anyhow!("URL must contain a host"))?
            .to_string();

        let port = url.port().unwrap_or(5432);

        let database = url.path().trim_start_matches('/').to_string();
        if database.is_empty() {
            return Err(anyhow!("URL must contain a database name"));
        }

        let user = if url.username().is_empty() {
            // Try to get from environment or use a default
            std::env::var("USER")
                .or_else(|_| std::env::var("USERNAME"))
                .unwrap_or_else(|_| "postgres".to_string())
        } else {
            url.username().to_string()
        };

        let password = url.password().map(|p| {
            urlencoding::decode(p)
                .map(|s| s.into_owned())
                .unwrap_or_else(|_| p.to_string())
        });

        // If password is in URL, we have a password; if not, assume no password required
        let no_password_required = password.is_none();

        let ssl_mode = url
            .query_pairs()
            .find_map(|(k, v)| {
                if k.eq_ignore_ascii_case("sslmode") {
                    SslMode::parse(&v)
                } else {
                    None
                }
            })
            .filter(|m| *m != SslMode::Disable);

        let entry = ConnectionEntry {
            name: name.to_string(),
            host,
            port,
            database,
            user,
            password_in_keychain: false,
            password_env: None,
            no_password_required,
            color: ConnectionColor::None,
            favorite: None,
            ssl_mode,
        };

        Ok((entry, password))
    }

    /// Get the password from the OS keychain
    pub fn get_password_from_keychain(&self) -> Result<Option<String>> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, &self.name)
            .context("Failed to create keyring entry")?;

        match entry.get_password() {
            Ok(password) => Ok(Some(password)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(anyhow!("Failed to get password from keychain: {}", e)),
        }
    }

    /// Store the password in the OS keychain
    pub fn set_password_in_keychain(&self, password: &str) -> Result<()> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, &self.name)
            .context("Failed to create keyring entry")?;

        entry
            .set_password(password)
            .context("Failed to store password in keychain")?;

        Ok(())
    }

    /// Delete the password from the OS keychain
    pub fn delete_password_from_keychain(&self) -> Result<()> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, &self.name)
            .context("Failed to create keyring entry")?;

        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()), // Already deleted
            Err(e) => Err(anyhow!("Failed to delete password from keychain: {}", e)),
        }
    }

    /// Get the password using the configured method (keychain, env var, or None)
    pub fn get_password(&self) -> Result<Option<String>> {
        // Try keychain first if configured
        if self.password_in_keychain {
            if let Ok(Some(pwd)) = self.get_password_from_keychain() {
                return Ok(Some(pwd));
            }
        }

        // Try environment variable
        if let Some(ref env_var) = self.password_env {
            // Handle both "$VAR" and "VAR" formats
            let var_name = env_var.strip_prefix('$').unwrap_or(env_var);
            if let Ok(pwd) = std::env::var(var_name) {
                return Ok(Some(pwd));
            }
        }

        Ok(None)
    }

    /// Get the password with a timeout to avoid blocking the UI.
    ///
    /// On macOS, keychain access can block indefinitely if the system
    /// shows a permission dialog. This method spawns the keychain access
    /// in a separate thread with a short timeout to prevent UI freezes.
    ///
    /// Returns:
    /// - Ok(Some(password)) if password was retrieved
    /// - Ok(None) if no password available or timeout occurred
    /// - Err if there was an error (other than timeout/no entry)
    pub fn get_password_with_timeout(&self, timeout_ms: u64) -> Result<Option<String>> {
        use std::sync::mpsc;
        use std::thread;
        use std::time::Duration;

        // Try environment variable first (no blocking risk)
        if let Some(ref env_var) = self.password_env {
            let var_name = env_var.strip_prefix('$').unwrap_or(env_var);
            if let Ok(pwd) = std::env::var(var_name) {
                return Ok(Some(pwd));
            }
        }

        // If not configured for keychain, return None
        if !self.password_in_keychain {
            return Ok(None);
        }

        // Spawn keychain access in a separate thread with timeout
        let name = self.name.clone();
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let result = (|| -> Result<Option<String>> {
                let entry = keyring::Entry::new(KEYRING_SERVICE, &name)
                    .context("Failed to create keyring entry")?;

                match entry.get_password() {
                    Ok(password) => Ok(Some(password)),
                    Err(keyring::Error::NoEntry) => Ok(None),
                    Err(e) => Err(anyhow!("Failed to get password from keychain: {}", e)),
                }
            })();
            let _ = tx.send(result);
        });

        // Wait for result with timeout
        match rx.recv_timeout(Duration::from_millis(timeout_ms)) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Keychain access is blocked (probably showing system dialog)
                // Return None to trigger password prompt
                Ok(None)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // Thread panicked or was killed
                Ok(None)
            }
        }
    }

    /// Format connection for display (without password)
    pub fn display_string(&self) -> String {
        format!(
            "{}@{}:{}/{}",
            self.user, self.host, self.port, self.database
        )
    }

    /// Short display format for status line
    pub fn short_display(&self) -> String {
        if self.port == 5432 {
            format!("{}@{}/{}", self.user, self.host, self.database)
        } else {
            format!(
                "{}@{}:{}/{}",
                self.user, self.host, self.port, self.database
            )
        }
    }

    /// Validate the connection entry
    pub fn validate(&self) -> Result<()> {
        if self.name.is_empty() {
            return Err(anyhow!("Connection name cannot be empty"));
        }
        if self.name.contains(char::is_whitespace) {
            return Err(anyhow!("Connection name cannot contain whitespace"));
        }
        if self.host.is_empty() {
            return Err(anyhow!("Host cannot be empty"));
        }
        if self.database.is_empty() {
            return Err(anyhow!("Database name cannot be empty"));
        }
        if self.user.is_empty() {
            return Err(anyhow!("Username cannot be empty"));
        }
        if self.port == 0 {
            return Err(anyhow!("Port cannot be 0"));
        }
        if let Some(fav) = self.favorite {
            if fav == 0 || fav > 9 {
                return Err(anyhow!("Favorite must be between 1 and 9"));
            }
        }
        Ok(())
    }
}

/// URL encoding helper (simple implementation for passwords)
mod urlencoding {
    pub fn encode(s: &str) -> String {
        let mut result = String::with_capacity(s.len());
        for c in s.chars() {
            match c {
                'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => {
                    result.push(c);
                }
                _ => {
                    for byte in c.to_string().as_bytes() {
                        result.push_str(&format!("%{:02X}", byte));
                    }
                }
            }
        }
        result
    }

    pub fn decode(s: &str) -> Result<std::borrow::Cow<'_, str>, std::string::FromUtf8Error> {
        let mut result = Vec::with_capacity(s.len());
        let mut chars = s.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '%' {
                let hex: String = chars.by_ref().take(2).collect();
                if hex.len() == 2 {
                    if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                        result.push(byte);
                        continue;
                    }
                }
                // Invalid escape, just push the %
                result.push(b'%');
                for c in hex.chars() {
                    for b in c.to_string().as_bytes() {
                        result.push(*b);
                    }
                }
            } else {
                for b in c.to_string().as_bytes() {
                    result.push(*b);
                }
            }
        }

        String::from_utf8(result).map(std::borrow::Cow::Owned)
    }
}

/// Container for the connections file
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConnectionsFile {
    /// List of saved connections
    #[serde(default, rename = "connection")]
    pub connections: Vec<ConnectionEntry>,
}

impl ConnectionsFile {
    /// Create a new empty connections file
    pub fn new() -> Self {
        Self {
            connections: Vec::new(),
        }
    }

    /// Find a connection by name
    pub fn find_by_name(&self, name: &str) -> Option<&ConnectionEntry> {
        self.connections.iter().find(|c| c.name == name)
    }

    /// Find a connection by name (mutable)
    pub fn find_by_name_mut(&mut self, name: &str) -> Option<&mut ConnectionEntry> {
        self.connections.iter_mut().find(|c| c.name == name)
    }

    /// Find a connection by favorite number
    pub fn find_by_favorite(&self, favorite: u8) -> Option<&ConnectionEntry> {
        self.connections
            .iter()
            .find(|c| c.favorite == Some(favorite))
    }

    /// Add a new connection (validates and checks for duplicates)
    pub fn add(&mut self, entry: ConnectionEntry) -> Result<()> {
        entry.validate()?;

        if self.find_by_name(&entry.name).is_some() {
            return Err(anyhow!(
                "A connection named '{}' already exists",
                entry.name
            ));
        }

        // Check for favorite conflicts
        if let Some(fav) = entry.favorite {
            if let Some(existing) = self.find_by_favorite(fav) {
                return Err(anyhow!(
                    "Favorite {} is already assigned to '{}'",
                    fav,
                    existing.name
                ));
            }
        }

        self.connections.push(entry);
        Ok(())
    }

    /// Update an existing connection
    pub fn update(&mut self, name: &str, entry: ConnectionEntry) -> Result<()> {
        entry.validate()?;

        // If name changed, check for conflicts
        if name != entry.name && self.find_by_name(&entry.name).is_some() {
            return Err(anyhow!(
                "A connection named '{}' already exists",
                entry.name
            ));
        }

        // Check for favorite conflicts (excluding current entry)
        if let Some(fav) = entry.favorite {
            if let Some(existing) = self.find_by_favorite(fav) {
                if existing.name != name {
                    return Err(anyhow!(
                        "Favorite {} is already assigned to '{}'",
                        fav,
                        existing.name
                    ));
                }
            }
        }

        if let Some(existing) = self.connections.iter_mut().find(|c| c.name == name) {
            *existing = entry;
            Ok(())
        } else {
            Err(anyhow!("Connection '{}' not found", name))
        }
    }

    /// Remove a connection by name
    pub fn remove(&mut self, name: &str) -> Result<ConnectionEntry> {
        let idx = self
            .connections
            .iter()
            .position(|c| c.name == name)
            .ok_or_else(|| anyhow!("Connection '{}' not found", name))?;

        Ok(self.connections.remove(idx))
    }

    /// Set a favorite position for a connection, handling conflicts by swapping
    pub fn set_favorite(&mut self, name: &str, favorite: Option<u8>) -> Result<()> {
        if let Some(fav) = favorite {
            if fav == 0 || fav > 9 {
                return Err(anyhow!("Favorite must be between 1 and 9"));
            }
        }

        // First, find the current favorite of the target connection
        let target_current_fav = self.find_by_name(name).and_then(|c| c.favorite);

        // Check if target exists
        if self.find_by_name(name).is_none() {
            return Err(anyhow!("Connection '{}' not found", name));
        }

        // If setting a new favorite, handle swap with any existing holder
        if let Some(fav) = favorite {
            // Find and update any connection that currently has this favorite
            for conn in &mut self.connections {
                if conn.favorite == Some(fav) && conn.name != name {
                    conn.favorite = target_current_fav;
                    break;
                }
            }
        }

        // Set the new favorite on the target
        if let Some(entry) = self.find_by_name_mut(name) {
            entry.favorite = favorite;
        }

        Ok(())
    }

    /// Get connections sorted by favorite first, then alphabetically
    pub fn sorted(&self) -> Vec<&ConnectionEntry> {
        let mut sorted: Vec<_> = self.connections.iter().collect();
        sorted.sort_by(|a, b| match (a.favorite, b.favorite) {
            (Some(fa), Some(fb)) => fa.cmp(&fb),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.name.cmp(&b.name),
        });
        sorted
    }
}

/// Returns the connections file path (~/.config/tsql/connections.toml)
pub fn connections_path() -> Option<PathBuf> {
    super::config_dir().map(|p| p.join("connections.toml"))
}

/// Load connections from the default path
pub fn load_connections() -> Result<ConnectionsFile> {
    if let Some(path) = connections_path() {
        if path.exists() {
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read connections file: {}", path.display()))?;
            let file: ConnectionsFile = toml::from_str(&content)
                .with_context(|| format!("Failed to parse connections file: {}", path.display()))?;
            return Ok(file);
        }
    }
    Ok(ConnectionsFile::new())
}

/// Save connections to the default path
pub fn save_connections(file: &ConnectionsFile) -> Result<()> {
    let path = connections_path().ok_or_else(|| anyhow!("Could not determine config directory"))?;

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory: {}", parent.display()))?;
    }

    let content = toml::to_string_pretty(file).context("Failed to serialize connections")?;

    std::fs::write(&path, content)
        .with_context(|| format!("Failed to write connections file: {}", path.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_entry_default() {
        let entry = ConnectionEntry::default();
        assert_eq!(entry.host, "localhost");
        assert_eq!(entry.port, 5432);
        assert_eq!(entry.color, ConnectionColor::None);
        assert!(entry.favorite.is_none());
        assert!(entry.ssl_mode.is_none());
    }

    #[test]
    fn test_connection_to_url_without_password() {
        let entry = ConnectionEntry {
            name: "test".to_string(),
            host: "localhost".to_string(),
            port: 5432,
            database: "mydb".to_string(),
            user: "postgres".to_string(),
            ..Default::default()
        };

        let url = entry.to_url(None);
        assert_eq!(url, "postgres://postgres@localhost/mydb");
    }

    #[test]
    fn test_connection_to_url_with_password() {
        let entry = ConnectionEntry {
            name: "test".to_string(),
            host: "localhost".to_string(),
            port: 5432,
            database: "mydb".to_string(),
            user: "postgres".to_string(),
            ..Default::default()
        };

        let url = entry.to_url(Some("secret"));
        assert_eq!(url, "postgres://postgres:secret@localhost/mydb");
    }

    #[test]
    fn test_connection_to_url_includes_sslmode() {
        let entry = ConnectionEntry {
            name: "test".to_string(),
            host: "localhost".to_string(),
            port: 5432,
            database: "mydb".to_string(),
            user: "postgres".to_string(),
            ssl_mode: Some(SslMode::Require),
            ..Default::default()
        };

        let url = entry.to_url(None);
        assert_eq!(url, "postgres://postgres@localhost/mydb?sslmode=require");
    }

    #[test]
    fn test_connection_to_url_with_special_password() {
        let entry = ConnectionEntry {
            name: "test".to_string(),
            host: "localhost".to_string(),
            port: 5432,
            database: "mydb".to_string(),
            user: "postgres".to_string(),
            ..Default::default()
        };

        let url = entry.to_url(Some("p@ss:word/123"));
        assert!(url.contains("p%40ss%3Aword%2F123"));
    }

    #[test]
    fn test_connection_to_url_non_default_port() {
        let entry = ConnectionEntry {
            name: "test".to_string(),
            host: "localhost".to_string(),
            port: 5433,
            database: "mydb".to_string(),
            user: "postgres".to_string(),
            ..Default::default()
        };

        let url = entry.to_url(None);
        assert_eq!(url, "postgres://postgres@localhost:5433/mydb");
    }

    #[test]
    fn test_connection_from_url_basic() {
        let (entry, password) =
            ConnectionEntry::from_url("test", "postgres://user@localhost/mydb").unwrap();

        assert_eq!(entry.name, "test");
        assert_eq!(entry.host, "localhost");
        assert_eq!(entry.port, 5432);
        assert_eq!(entry.database, "mydb");
        assert_eq!(entry.user, "user");
        assert!(password.is_none());
        assert!(entry.ssl_mode.is_none());
    }

    #[test]
    fn test_connection_from_url_with_password() {
        let (entry, password) =
            ConnectionEntry::from_url("test", "postgres://user:secret@localhost/mydb").unwrap();

        assert_eq!(entry.user, "user");
        assert_eq!(password, Some("secret".to_string()));
    }

    #[test]
    fn test_connection_from_url_with_port() {
        let (entry, _) =
            ConnectionEntry::from_url("test", "postgres://user@localhost:5433/mydb").unwrap();

        assert_eq!(entry.port, 5433);
    }

    #[test]
    fn test_connection_from_url_postgresql_scheme() {
        let (entry, _) =
            ConnectionEntry::from_url("test", "postgresql://user@localhost/mydb").unwrap();

        assert_eq!(entry.host, "localhost");
    }

    #[test]
    fn test_connection_from_url_parses_sslmode() {
        let (entry, _) =
            ConnectionEntry::from_url("test", "postgres://user@localhost/mydb?sslmode=require")
                .unwrap();

        assert_eq!(entry.ssl_mode, Some(SslMode::Require));
    }

    #[test]
    fn test_connection_from_url_invalid_scheme() {
        let result = ConnectionEntry::from_url("test", "mysql://user@localhost/mydb");
        assert!(result.is_err());
    }

    #[test]
    fn test_connection_from_url_no_database() {
        let result = ConnectionEntry::from_url("test", "postgres://user@localhost/");
        assert!(result.is_err());
    }

    #[test]
    fn test_connection_validate_valid() {
        let entry = ConnectionEntry {
            name: "test".to_string(),
            host: "localhost".to_string(),
            port: 5432,
            database: "mydb".to_string(),
            user: "postgres".to_string(),
            ..Default::default()
        };

        assert!(entry.validate().is_ok());
    }

    #[test]
    fn test_connection_validate_empty_name() {
        let entry = ConnectionEntry {
            name: "".to_string(),
            host: "localhost".to_string(),
            port: 5432,
            database: "mydb".to_string(),
            user: "postgres".to_string(),
            ..Default::default()
        };

        assert!(entry.validate().is_err());
    }

    #[test]
    fn test_connection_validate_name_with_whitespace() {
        let entry = ConnectionEntry {
            name: "my connection".to_string(),
            host: "localhost".to_string(),
            port: 5432,
            database: "mydb".to_string(),
            user: "postgres".to_string(),
            ..Default::default()
        };

        assert!(entry.validate().is_err());
    }

    #[test]
    fn test_connection_validate_invalid_favorite() {
        let entry = ConnectionEntry {
            name: "test".to_string(),
            host: "localhost".to_string(),
            port: 5432,
            database: "mydb".to_string(),
            user: "postgres".to_string(),
            favorite: Some(10),
            ..Default::default()
        };

        assert!(entry.validate().is_err());
    }

    #[test]
    fn test_connection_color_from_str() {
        assert_eq!(
            "red".parse::<ConnectionColor>().unwrap(),
            ConnectionColor::Red
        );
        assert_eq!(
            "GREEN".parse::<ConnectionColor>().unwrap(),
            ConnectionColor::Green
        );
        assert_eq!(
            "grey".parse::<ConnectionColor>().unwrap(),
            ConnectionColor::Gray
        );
        assert!("invalid".parse::<ConnectionColor>().is_err());
    }

    #[test]
    fn test_connections_file_add() {
        let mut file = ConnectionsFile::new();
        let entry = ConnectionEntry {
            name: "test".to_string(),
            host: "localhost".to_string(),
            port: 5432,
            database: "mydb".to_string(),
            user: "postgres".to_string(),
            ..Default::default()
        };

        assert!(file.add(entry.clone()).is_ok());
        assert!(file.find_by_name("test").is_some());

        // Duplicate should fail
        assert!(file.add(entry).is_err());
    }

    #[test]
    fn test_connections_file_remove() {
        let mut file = ConnectionsFile::new();
        let entry = ConnectionEntry {
            name: "test".to_string(),
            host: "localhost".to_string(),
            port: 5432,
            database: "mydb".to_string(),
            user: "postgres".to_string(),
            ..Default::default()
        };

        file.add(entry).unwrap();
        let removed = file.remove("test").unwrap();
        assert_eq!(removed.name, "test");
        assert!(file.find_by_name("test").is_none());
    }

    #[test]
    fn test_connections_file_favorites() {
        let mut file = ConnectionsFile::new();

        let entry1 = ConnectionEntry {
            name: "conn1".to_string(),
            host: "localhost".to_string(),
            database: "db1".to_string(),
            user: "user".to_string(),
            favorite: Some(1),
            ..Default::default()
        };

        let entry2 = ConnectionEntry {
            name: "conn2".to_string(),
            host: "localhost".to_string(),
            database: "db2".to_string(),
            user: "user".to_string(),
            ..Default::default()
        };

        file.add(entry1).unwrap();
        file.add(entry2).unwrap();

        assert!(file.find_by_favorite(1).is_some());
        assert_eq!(file.find_by_favorite(1).unwrap().name, "conn1");
    }

    #[test]
    fn test_connections_file_set_favorite_swap() {
        let mut file = ConnectionsFile::new();

        let entry1 = ConnectionEntry {
            name: "conn1".to_string(),
            host: "localhost".to_string(),
            database: "db1".to_string(),
            user: "user".to_string(),
            favorite: Some(1),
            ..Default::default()
        };

        let entry2 = ConnectionEntry {
            name: "conn2".to_string(),
            host: "localhost".to_string(),
            database: "db2".to_string(),
            user: "user".to_string(),
            favorite: Some(2),
            ..Default::default()
        };

        file.add(entry1).unwrap();
        file.add(entry2).unwrap();

        // Set conn2 to favorite 1, should swap with conn1
        file.set_favorite("conn2", Some(1)).unwrap();

        assert_eq!(file.find_by_name("conn2").unwrap().favorite, Some(1));
        assert_eq!(file.find_by_name("conn1").unwrap().favorite, Some(2));
    }

    #[test]
    fn test_connections_file_sorted() {
        let mut file = ConnectionsFile::new();

        // Add in non-sorted order
        let entries = vec![
            ConnectionEntry {
                name: "zebra".to_string(),
                host: "localhost".to_string(),
                database: "db".to_string(),
                user: "user".to_string(),
                ..Default::default()
            },
            ConnectionEntry {
                name: "apple".to_string(),
                host: "localhost".to_string(),
                database: "db".to_string(),
                user: "user".to_string(),
                favorite: Some(2),
                ..Default::default()
            },
            ConnectionEntry {
                name: "banana".to_string(),
                host: "localhost".to_string(),
                database: "db".to_string(),
                user: "user".to_string(),
                favorite: Some(1),
                ..Default::default()
            },
        ];

        for e in entries {
            file.add(e).unwrap();
        }

        let sorted = file.sorted();
        assert_eq!(sorted[0].name, "banana"); // favorite 1
        assert_eq!(sorted[1].name, "apple"); // favorite 2
        assert_eq!(sorted[2].name, "zebra"); // no favorite, alphabetical
    }

    #[test]
    fn test_connections_file_serialize() {
        let mut file = ConnectionsFile::new();
        let entry = ConnectionEntry {
            name: "test".to_string(),
            host: "localhost".to_string(),
            port: 5432,
            database: "mydb".to_string(),
            user: "postgres".to_string(),
            color: ConnectionColor::Green,
            favorite: Some(1),
            ..Default::default()
        };

        file.add(entry).unwrap();

        let toml_str = toml::to_string_pretty(&file).unwrap();
        assert!(toml_str.contains("[[connection]]"));
        assert!(toml_str.contains("name = \"test\""));
        assert!(toml_str.contains("color = \"green\""));
        assert!(toml_str.contains("favorite = 1"));
    }

    #[test]
    fn test_connections_file_deserialize() {
        let toml_str = r#"
[[connection]]
name = "local"
host = "localhost"
port = 5432
database = "mydb"
user = "postgres"
color = "blue"
favorite = 1

[[connection]]
name = "remote"
host = "db.example.com"
port = 5433
database = "prod"
user = "admin"
password_in_keychain = true
"#;

        let file: ConnectionsFile = toml::from_str(toml_str).unwrap();
        assert_eq!(file.connections.len(), 2);

        let local = file.find_by_name("local").unwrap();
        assert_eq!(local.color, ConnectionColor::Blue);
        assert_eq!(local.favorite, Some(1));

        let remote = file.find_by_name("remote").unwrap();
        assert!(remote.password_in_keychain);
        assert_eq!(remote.port, 5433);
    }

    #[test]
    fn test_url_encoding_special_chars() {
        let encoded = urlencoding::encode("p@ss:word/123");
        assert_eq!(encoded, "p%40ss%3Aword%2F123");

        let decoded = urlencoding::decode(&encoded).unwrap();
        assert_eq!(decoded, "p@ss:word/123");
    }

    #[test]
    fn test_display_string() {
        let entry = ConnectionEntry {
            name: "test".to_string(),
            host: "db.example.com".to_string(),
            port: 5433,
            database: "mydb".to_string(),
            user: "admin".to_string(),
            ..Default::default()
        };

        assert_eq!(entry.display_string(), "admin@db.example.com:5433/mydb");

        let entry_default_port = ConnectionEntry {
            port: 5432,
            ..entry
        };
        assert_eq!(
            entry_default_port.short_display(),
            "admin@db.example.com/mydb"
        );
    }

    // ========== Issue #16 Reproduction Tests ==========
    // https://github.com/fcoury/tsql/issues/16
    // "Password missing even though the test connection worked"

    /// This test reproduces issue #16: when a user creates a new connection,
    /// enters a password, tests it successfully, saves WITHOUT checking
    /// "Save to keychain", and then tries to connect - the password is missing.
    ///
    /// The bug: password is only saved to keychain if save_password=true.
    /// If user doesn't check the checkbox, password is lost after save.
    #[test]
    fn test_issue_16_password_missing_after_save_without_keychain() {
        use tempfile::TempDir;

        // Create a temp directory for the test
        let temp_dir = TempDir::new().unwrap();
        let connections_file = temp_dir.path().join("connections.toml");

        // Step 1: User creates a new connection with password
        let entry = ConnectionEntry {
            name: "test-pg".to_string(),
            host: "localhost".to_string(),
            port: 5433,
            database: "testdb".to_string(),
            user: "testuser".to_string(),
            // User did NOT check "Save to keychain"
            password_in_keychain: false,
            ..Default::default()
        };

        // The password that user entered (this works during test connection)
        let password = "testpass123";

        // Step 2: User tests the connection - this works because password is in memory
        // (Simulated - in real app, entry.to_url(Some(password)) is used)
        let test_url = entry.to_url(Some(password));
        assert!(
            test_url.contains("testpass123"),
            "Test URL should contain password"
        );

        // Step 3: User saves the connection
        let mut connections = ConnectionsFile::new();
        connections.add(entry.clone()).unwrap();

        // Save to file
        let content = toml::to_string_pretty(&connections).unwrap();
        std::fs::write(&connections_file, &content).unwrap();

        // Note: In the real app, if save_password=false, the password is NOT
        // saved to keychain here. The password is simply discarded.

        // Step 4: User closes form, connection is saved
        // Step 5: User tries to connect to the saved connection

        // Load connections from file (simulating app restart or re-reading)
        let loaded_content = std::fs::read_to_string(&connections_file).unwrap();
        let loaded_connections: ConnectionsFile = toml::from_str(&loaded_content).unwrap();

        let loaded_entry = loaded_connections.find_by_name("test-pg").unwrap();

        // Step 6: Try to get password for connection
        // BUG: This returns None because password was never saved!
        let retrieved_password = loaded_entry.get_password().unwrap();

        // This assertion demonstrates the bug - password is None
        // When this test fails, it means the bug is fixed
        assert!(
            retrieved_password.is_none(),
            "BUG REPRODUCED: Password is missing after save without keychain. \
             User entered password '{}', tested successfully, saved connection, \
             but password is now None. This is issue #16.",
            password
        );

        // What SHOULD happen (after fix):
        // The password should be retrievable somehow, either:
        // 1. Force save to keychain (change default behavior), or
        // 2. Prompt user that password won't be saved, or
        // 3. Store password in config file (less secure), or
        // 4. Always prompt for password on connect if not in keychain

        // For now, this test documents the bug by asserting the current
        // (broken) behavior. When we fix it, we'll update this test.
    }

    /// Test that demonstrates the expected workflow when password IS saved to keychain
    #[test]
    fn test_password_saved_to_keychain_workflow() {
        // This test shows the WORKING case - when user checks "Save to keychain"

        let entry = ConnectionEntry {
            name: "keychain-test".to_string(),
            host: "localhost".to_string(),
            port: 5432,
            database: "testdb".to_string(),
            user: "testuser".to_string(),
            // User DID check "Save to keychain"
            password_in_keychain: true,
            ..Default::default()
        };

        // In the real app, when save_password=true:
        // entry.set_password_in_keychain("password") is called
        // Then entry.get_password() would return Some("password")

        // We can't actually test keychain in unit tests without mocking,
        // but this documents the expected behavior.
        assert!(
            entry.password_in_keychain,
            "Entry should have password_in_keychain=true"
        );
    }

    /// Test to verify that ConnectionEntry without keychain has no way to retrieve password
    #[test]
    fn test_connection_entry_get_password_returns_none_without_keychain() {
        let entry = ConnectionEntry {
            name: "no-password".to_string(),
            host: "localhost".to_string(),
            port: 5432,
            database: "testdb".to_string(),
            user: "testuser".to_string(),
            password_in_keychain: false,
            ..Default::default()
        };

        // This entry has no way to retrieve password
        let password = entry.get_password().unwrap();
        assert!(
            password.is_none(),
            "Entry without keychain or env var should return None for password"
        );
    }

    /// Test keychain save and retrieve - run with --ignored flag
    /// This test actually writes to the system keychain
    #[test]
    #[ignore] // Ignored by default because it modifies system keychain
    fn test_keychain_save_and_retrieve() {
        let entry = ConnectionEntry {
            name: "tsql-keychain-test".to_string(),
            host: "localhost".to_string(),
            port: 5432,
            database: "testdb".to_string(),
            user: "testuser".to_string(),
            password_in_keychain: true,
            ..Default::default()
        };

        let test_password = "test-secret-password-123";

        // Save password to keychain
        let save_result = entry.set_password_in_keychain(test_password);
        assert!(
            save_result.is_ok(),
            "Failed to save password to keychain: {:?}",
            save_result.err()
        );

        // Retrieve password from keychain
        let retrieved = entry.get_password_from_keychain();
        assert!(
            retrieved.is_ok(),
            "Failed to retrieve password from keychain: {:?}",
            retrieved.err()
        );

        let password = retrieved.unwrap();
        assert_eq!(
            password,
            Some(test_password.to_string()),
            "Retrieved password doesn't match saved password"
        );

        // Clean up - delete the test password
        let _ = entry.delete_password_from_keychain();
    }

    /// Test keyring crate directly to verify it's working
    #[test]
    #[ignore]
    fn test_keyring_direct() {
        let service = "tsql-test-direct";
        let user = "test-user";
        let password = "test-password-123";

        println!(
            "Creating keyring entry with service='{}' user='{}'",
            service, user
        );

        let entry = keyring::Entry::new(service, user);
        println!("Entry creation result: {:?}", entry);

        match entry {
            Ok(e) => {
                println!("Setting password...");
                let set_result = e.set_password(password);
                println!("Set password result: {:?}", set_result);

                println!("Getting password...");
                let get_result = e.get_password();
                println!("Get password result: {:?}", get_result);

                // Cleanup
                let _ = e.delete_credential();
            }
            Err(e) => {
                println!("Failed to create entry: {:?}", e);
            }
        }
    }

    /// Test the full issue #16 flow WITH keychain enabled
    /// This reproduces the bug where even with "Save to keychain" checked,
    /// the password is still missing when connecting
    #[test]
    #[ignore] // Ignored by default because it modifies system keychain
    fn test_issue_16_with_keychain_enabled() {
        let entry = ConnectionEntry {
            name: "tsql-issue16-keychain-test".to_string(),
            host: "localhost".to_string(),
            port: 5433,
            database: "testdb".to_string(),
            user: "testuser".to_string(),
            password_in_keychain: true,
            ..Default::default()
        };

        let test_password = "testpass123";

        // Step 1: Save password to keychain (simulating what app.rs should do)
        println!("Saving password to keychain...");
        let save_result = entry.set_password_in_keychain(test_password);
        println!("Save result: {:?}", save_result);

        if let Err(ref e) = save_result {
            println!("ERROR: Failed to save to keychain: {}", e);
        }

        // Step 2: Retrieve password (simulating connect flow)
        println!("Retrieving password from entry.get_password()...");
        let password = entry.get_password();
        println!("Get password result: {:?}", password);

        match password {
            Ok(Some(pwd)) => {
                println!("SUCCESS: Password retrieved: {}", pwd);
                assert_eq!(pwd, test_password);
            }
            Ok(None) => {
                println!("BUG: Password is None even though password_in_keychain=true");
                println!("This is issue #16!");
            }
            Err(e) => {
                println!("ERROR: Failed to get password: {}", e);
            }
        }

        // Clean up
        let _ = entry.delete_password_from_keychain();
    }

    // ========== SSL Mode Parsing Edge-Case Tests ==========

    #[test]
    fn test_ssl_mode_parse_case_insensitive() {
        // Standard lowercase
        assert_eq!(SslMode::parse("require"), Some(SslMode::Require));
        assert_eq!(SslMode::parse("disable"), Some(SslMode::Disable));
        assert_eq!(SslMode::parse("verify-ca"), Some(SslMode::VerifyCa));
        assert_eq!(SslMode::parse("verify-full"), Some(SslMode::VerifyFull));

        // Uppercase
        assert_eq!(SslMode::parse("REQUIRE"), Some(SslMode::Require));
        assert_eq!(SslMode::parse("DISABLE"), Some(SslMode::Disable));
        assert_eq!(SslMode::parse("VERIFY-CA"), Some(SslMode::VerifyCa));
        assert_eq!(SslMode::parse("VERIFY-FULL"), Some(SslMode::VerifyFull));

        // Mixed case
        assert_eq!(SslMode::parse("Require"), Some(SslMode::Require));
        assert_eq!(SslMode::parse("Verify-Ca"), Some(SslMode::VerifyCa));
        assert_eq!(SslMode::parse("vErIfY-fUlL"), Some(SslMode::VerifyFull));
    }

    #[test]
    fn test_ssl_mode_parse_whitespace_trimming() {
        assert_eq!(SslMode::parse("  require  "), Some(SslMode::Require));
        assert_eq!(SslMode::parse("\trequire\n"), Some(SslMode::Require));
        assert_eq!(SslMode::parse(" verify-ca "), Some(SslMode::VerifyCa));
    }

    #[test]
    fn test_ssl_mode_parse_invalid_values() {
        assert_eq!(SslMode::parse(""), None);
        assert_eq!(SslMode::parse("   "), None);
        assert_eq!(SslMode::parse("invalid"), None);
        assert_eq!(SslMode::parse("ssl"), None);
        assert_eq!(SslMode::parse("true"), None);
        assert_eq!(SslMode::parse("false"), None);
        assert_eq!(SslMode::parse("yes"), None);
        assert_eq!(SslMode::parse("no"), None);
        // Almost correct but not quite
        assert_eq!(SslMode::parse("requires"), None);
        assert_eq!(SslMode::parse("verifyca"), None);
        assert_eq!(SslMode::parse("verify_ca"), None);
        assert_eq!(SslMode::parse("verify ca"), None);
    }

    #[test]
    fn test_ssl_mode_index_round_trip() {
        // All modes should round-trip through index conversion
        for mode in [
            SslMode::Disable,
            SslMode::Prefer,
            SslMode::Require,
            SslMode::VerifyCa,
            SslMode::VerifyFull,
        ] {
            let index = mode.to_index();
            let recovered = SslMode::from_index(index);
            assert_eq!(recovered, mode, "Round-trip failed for {:?}", mode);
        }
    }

    #[test]
    fn test_ssl_mode_from_index_out_of_bounds() {
        // Out-of-bounds indexes should default to Disable
        assert_eq!(SslMode::from_index(5), SslMode::Disable);
        assert_eq!(SslMode::from_index(100), SslMode::Disable);
        assert_eq!(SslMode::from_index(usize::MAX), SslMode::Disable);
    }

    #[test]
    fn test_ssl_mode_count_matches_variants() {
        // Verify COUNT matches the actual number of variants
        assert_eq!(SslMode::COUNT, 5);

        // All indexes from 0 to COUNT-1 should produce distinct modes
        let modes: Vec<SslMode> = (0..SslMode::COUNT).map(SslMode::from_index).collect();
        assert_eq!(modes.len(), SslMode::COUNT);

        // Check they're all distinct
        let mut seen = std::collections::HashSet::new();
        for mode in &modes {
            assert!(seen.insert(mode), "Duplicate mode found: {:?}", mode);
        }
    }

    #[test]
    fn test_ssl_mode_as_str_round_trip() {
        // All modes should round-trip through string representation
        for mode in [
            SslMode::Disable,
            SslMode::Prefer,
            SslMode::Require,
            SslMode::VerifyCa,
            SslMode::VerifyFull,
        ] {
            let s = mode.as_str();
            let recovered = SslMode::parse(s);
            assert_eq!(
                recovered,
                Some(mode),
                "String round-trip failed for {:?}",
                mode
            );
        }
    }

    #[test]
    fn test_ssl_mode_all_indexes_unique() {
        // Ensure each mode has a unique index
        let indexes = [
            SslMode::Disable.to_index(),
            SslMode::Prefer.to_index(),
            SslMode::Require.to_index(),
            SslMode::VerifyCa.to_index(),
            SslMode::VerifyFull.to_index(),
        ];

        let mut seen = std::collections::HashSet::new();
        for (i, &idx) in indexes.iter().enumerate() {
            assert!(
                seen.insert(idx),
                "Duplicate index {} found at position {}",
                idx,
                i
            );
        }
    }

    #[test]
    fn test_ssl_mode_indexes_are_contiguous() {
        // Indexes should be 0, 1, 2, 3, 4 (contiguous from 0)
        assert_eq!(SslMode::Disable.to_index(), 0);
        assert_eq!(SslMode::Prefer.to_index(), 1);
        assert_eq!(SslMode::Require.to_index(), 2);
        assert_eq!(SslMode::VerifyCa.to_index(), 3);
        assert_eq!(SslMode::VerifyFull.to_index(), 4);
    }
}
