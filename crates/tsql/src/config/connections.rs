//! Connection management for saved database connections.
//!
//! This module handles:
//! - Loading and saving connections to ~/.tsql/connections.toml
//! - Secure password storage via OS keychain (keyring crate)
//! - URL parsing and construction
//! - Connection entry validation

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use url::Url;

/// Service name for keyring storage
const KEYRING_SERVICE: &str = "tsql";

/// Database engine kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DbKind {
    #[default]
    Postgres,
    Mongo,
}

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
    /// All SSL mode variants in order.
    const fn all() -> [Self; 5] {
        [
            SslMode::Disable,
            SslMode::Prefer,
            SslMode::Require,
            SslMode::VerifyCa,
            SslMode::VerifyFull,
        ]
    }

    /// Number of SSL mode variants (for UI cycling).
    /// Derived from all() to avoid manual maintenance.
    pub const COUNT: usize = Self::all().len();

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
    /// Database engine kind (defaults to Postgres for backward compatibility).
    #[serde(default)]
    pub kind: DbKind,
    /// Unique name for this connection
    pub name: String,
    /// Full connection URI (used primarily for MongoDB).
    #[serde(default)]
    pub uri: Option<String>,
    /// Database host
    pub host: String,
    /// Database port.
    ///
    /// Defaults via `default_port` (5432), which is Postgres-centric.
    /// For Mongo connections, the port in `uri` is authoritative and this
    /// field is mostly legacy/backward-compat metadata.
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
    /// 1Password secret reference (e.g. "op://vault/item/field")
    #[serde(default)]
    pub password_onepassword: Option<String>,
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

    // --- v2 metadata fields (all serde(default) for backward compatibility) ---
    /// Free-form description shown in the manager detail pane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// User-provided tags used for filtering / grouping.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,

    /// Optional folder / group label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,

    /// Postgres application_name connection parameter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application_name: Option<String>,

    /// Per-connection override for `config.connection.connect_timeout_secs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_timeout_secs: Option<u64>,

    /// PG sslrootcert path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssl_root_cert: Option<PathBuf>,

    /// PG sslcert path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssl_client_cert: Option<PathBuf>,

    /// PG sslkey path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssl_client_key: Option<PathBuf>,

    /// Timestamp of the last successful connect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<DateTime<Utc>>,

    /// Number of successful connects using this entry.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub use_count: u64,

    /// Manual ordering offset for non-favorite entries (smaller = higher).
    #[serde(default, skip_serializing_if = "is_zero_i32")]
    pub order: i32,
}

/// Best-effort sanitiser for Mongo URIs that didn't round-trip through
/// `Url::parse`. Strips any `user:password@` userinfo by replacing it
/// with a bare `@`, so a malformed URI can't end up on screen with
/// credentials attached. Kept separate from `sanitize_url` (which
/// relies on the url crate) for exactly this fallback case.
fn sanitize_mongo_uri_fallback(uri: &str) -> String {
    // Find the first `://` so we don't touch anything before the
    // scheme, then look for `user[:password]@` up to the first `/` or
    // `?` after that.
    let Some(scheme_end) = uri.find("://") else {
        return uri.to_string();
    };
    let after = scheme_end + 3;
    let tail = &uri[after..];
    let stop = tail.find(['/', '?']).unwrap_or(tail.len());
    let authority = &tail[..stop];
    match authority.rfind('@') {
        Some(at) => {
            let mut out = String::with_capacity(uri.len());
            out.push_str(&uri[..after]);
            out.push_str(&authority[at..]); // includes the `@`
            out.push_str(&tail[stop..]);
            out
        }
        None => uri.to_string(),
    }
}

fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

fn is_zero_i32(v: &i32) -> bool {
    *v == 0
}

fn default_port() -> u16 {
    // Postgres default. Mongo defaults come from the parsed URI.
    5432
}

impl Default for ConnectionEntry {
    fn default() -> Self {
        Self {
            kind: DbKind::Postgres,
            name: String::new(),
            uri: None,
            host: "localhost".to_string(),
            port: 5432,
            database: String::new(),
            user: String::new(),
            password_in_keychain: false,
            password_env: None,
            password_onepassword: None,
            no_password_required: false,
            color: ConnectionColor::None,
            favorite: None,
            ssl_mode: None,
            description: None,
            tags: Vec::new(),
            folder: None,
            application_name: None,
            connect_timeout_secs: None,
            ssl_root_cert: None,
            ssl_client_cert: None,
            ssl_client_key: None,
            last_used_at: None,
            use_count: 0,
            order: 0,
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

    /// Build a connection URL from the entry fields.
    ///
    /// If `password` is provided, it will be included in the URL.
    /// Otherwise, the URL will not contain authentication.
    pub fn to_url(&self, password: Option<&str>) -> String {
        if self.kind == DbKind::Mongo {
            if let Some(uri) = self.uri.as_ref() {
                if let Some(pwd) = password {
                    if let Ok(mut parsed) = Url::parse(uri) {
                        // Best effort: set password only when username exists.
                        if !parsed.username().is_empty() {
                            let _ = parsed.set_password(Some(pwd));
                            return parsed.to_string();
                        }
                    }
                }
                // No password supplied — the stored URI may still carry
                // one (imported files or manual edits can embed
                // `mongodb://user:password@host/…`). Strip it so yank /
                // copy-as-CLI never leaks credentials to the clipboard,
                // even when the URI doesn't round-trip through the url
                // crate (the fallback handles that).
                if let Ok(mut parsed) = Url::parse(uri) {
                    if parsed.password().is_some() {
                        let _ = parsed.set_password(None);
                        return parsed.to_string();
                    }
                    return uri.clone();
                }
                return sanitize_mongo_uri_fallback(uri);
            }
            return "mongodb://localhost".to_string();
        }

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

        // Collect query parameters the user configured at entry level.
        // Without threading these through, the values the UI captures
        // would be inert and silently ignored at connect time.
        let mut query: Vec<(&str, String)> = Vec::new();
        if let Some(mode) = self.ssl_mode {
            query.push(("sslmode", mode.as_str().to_string()));
        }
        if let Some(secs) = self.connect_timeout_secs {
            // libpq / tokio-postgres accept `connect_timeout` in seconds.
            query.push(("connect_timeout", secs.to_string()));
        }
        if let Some(app) = self.application_name.as_deref() {
            let trimmed = app.trim();
            if !trimmed.is_empty() {
                query.push(("application_name", trimmed.to_string()));
            }
        }
        // SSL cert paths: libpq-compatible `sslrootcert` / `sslcert` /
        // `sslkey`. Without these, saved TLS-configured entries would
        // connect anyway but without the user's CA / client cert,
        // silently failing handshakes or skipping verification.
        if let Some(p) = self.ssl_root_cert.as_ref() {
            query.push(("sslrootcert", p.to_string_lossy().into_owned()));
        }
        if let Some(p) = self.ssl_client_cert.as_ref() {
            query.push(("sslcert", p.to_string_lossy().into_owned()));
        }
        if let Some(p) = self.ssl_client_key.as_ref() {
            query.push(("sslkey", p.to_string_lossy().into_owned()));
        }
        if !query.is_empty() {
            url.push('?');
            for (i, (k, v)) in query.iter().enumerate() {
                if i > 0 {
                    url.push('&');
                }
                url.push_str(k);
                url.push('=');
                url.push_str(&urlencoding::encode(v));
            }
        }

        url
    }

    /// Parse a URL and create a ConnectionEntry.
    ///
    /// Returns the entry and the password if one was found in the URL.
    pub fn from_url(name: &str, url_str: &str) -> Result<(Self, Option<String>)> {
        let url = Url::parse(url_str).context("Invalid URL format")?;
        let kind = match url.scheme() {
            "postgres" | "postgresql" => DbKind::Postgres,
            "mongodb" | "mongodb+srv" => DbKind::Mongo,
            _ => {
                return Err(anyhow!(
                    "URL must use postgres://, postgresql://, mongodb://, or mongodb+srv:// scheme"
                ))
            }
        };

        let host = url
            .host_str()
            .ok_or_else(|| anyhow!("URL must contain a host"))?
            .to_string();

        let port = match kind {
            DbKind::Postgres => url.port().unwrap_or(5432),
            DbKind::Mongo => url.port().unwrap_or(27017),
        };

        let database = url.path().trim_start_matches('/').to_string();
        if kind == DbKind::Postgres && database.is_empty() {
            return Err(anyhow!("URL must contain a database name"));
        }

        let user = if url.username().is_empty() {
            match kind {
                DbKind::Postgres => std::env::var("USER")
                    .or_else(|_| std::env::var("USERNAME"))
                    .unwrap_or_else(|_| "postgres".to_string()),
                DbKind::Mongo => String::new(),
            }
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

        let ssl_mode = if kind == DbKind::Postgres {
            url.query_pairs().find_map(|(k, v)| {
                if k.eq_ignore_ascii_case("sslmode") {
                    SslMode::parse(&v)
                } else {
                    None
                }
            })
        } else {
            None
        };

        let sanitized_uri = if kind == DbKind::Mongo {
            let mut clean = url.clone();
            let _ = clean.set_password(None);
            Some(clean.to_string())
        } else {
            None
        };

        let entry = ConnectionEntry {
            kind,
            name: name.to_string(),
            uri: sanitized_uri,
            host,
            port,
            database,
            user,
            password_in_keychain: false,
            password_env: None,
            password_onepassword: None,
            no_password_required,
            color: ConnectionColor::None,
            favorite: None,
            ssl_mode,
            ..Default::default()
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

    /// Invoke the 1Password CLI to read a secret reference.
    ///
    /// Uses `/bin/sh -c` to inherit the user's `PATH` and active `op` session token.
    /// Returns `Ok(Some(password))` on success.
    /// Returns `Ok(None)` when the reference is empty.
    /// Returns `Err(...)` when the CLI cannot be executed or returns a failure status.
    ///
    /// Note: this method is available on Unix-like systems (Linux/macOS).
    #[cfg(unix)]
    fn read_from_onepassword(op_ref: &str) -> Result<Option<String>> {
        let op_ref = op_ref.trim();
        if op_ref.is_empty() {
            return Ok(None);
        }

        let cmd = format!("op read '{}'", op_ref.replace('\'', "'\\''"));
        let out = std::process::Command::new("/bin/sh")
            .args(["-c", &cmd])
            .stdin(std::process::Stdio::null())
            .output()
            .context("Failed to execute 1Password CLI")?;
        if out.status.success() {
            Ok(Some(
                String::from_utf8_lossy(&out.stdout).trim().to_string(),
            ))
        } else {
            let code = out
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "terminated by signal".to_string());
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            Err(anyhow!(
                "1Password CLI failed (status {}): stderr='{}', stdout='{}'",
                code,
                stderr,
                stdout
            ))
        }
    }

    #[cfg(not(unix))]
    fn read_from_onepassword(_op_ref: &str) -> Result<Option<String>> {
        Err(anyhow!(
            "1Password integration is only supported on Unix-like systems (Linux/macOS)"
        ))
    }

    /// Get the password using the configured method.
    ///
    /// Precedence: environment variable → OS keychain.
    ///
    /// This method keeps 1Password disabled by default so callers that want
    /// 1Password lookup must opt in via [`Self::get_password_with_options`].
    pub fn get_password(&self) -> Result<Option<String>> {
        self.get_password_with_options(false)
    }

    /// Get the password using the configured method, optionally disabling 1Password.
    ///
    /// Precedence: environment variable → 1Password CLI (if enabled) → OS keychain.
    pub fn get_password_with_options(&self, onepassword_enabled: bool) -> Result<Option<String>> {
        // Try environment variable first (non-blocking)
        if let Some(ref env_var) = self.password_env {
            // Handle both "$VAR" and "VAR" formats
            let var_name = env_var.strip_prefix('$').unwrap_or(env_var);
            if let Ok(pwd) = std::env::var(var_name) {
                return Ok(Some(pwd));
            }
        }

        // Try 1Password CLI if configured and enabled
        if onepassword_enabled {
            if let Some(ref op_ref) = self.password_onepassword {
                if let Some(pwd) = Self::read_from_onepassword(op_ref)? {
                    return Ok(Some(pwd));
                }
            }
        }

        // Try OS keychain if configured
        if self.password_in_keychain {
            if let Ok(Some(pwd)) = self.get_password_from_keychain() {
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
        self.get_password_with_timeout_and_options(timeout_ms, true)
    }

    /// Get the password with timeout, optionally disabling 1Password.
    pub fn get_password_with_timeout_and_options(
        &self,
        timeout_ms: u64,
        onepassword_enabled: bool,
    ) -> Result<Option<String>> {
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

        let use_keychain = self.password_in_keychain;
        let op_ref = if onepassword_enabled {
            self.password_onepassword.clone()
        } else {
            None
        };

        // If neither keychain nor enabled 1Password is configured, return None
        if !use_keychain && op_ref.is_none() {
            return Ok(None);
        }

        // Spawn blocking retrieval in a separate thread with timeout
        let name = self.name.clone();
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let result = (|| -> Result<Option<String>> {
                // Try 1Password CLI first if configured and enabled
                if let Some(ref op_ref) = op_ref {
                    if let Some(pwd) = Self::read_from_onepassword(op_ref)? {
                        return Ok(Some(pwd));
                    }
                }

                if !use_keychain {
                    return Ok(None);
                }

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
                // Password retrieval is blocked (1Password CLI or keychain may be prompting).
                // Return None to trigger password prompt.
                Ok(None)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // Thread panicked or was killed
                Ok(None)
            }
        }
    }

    /// Human-friendly label for the password source.
    pub fn password_source_label(&self) -> &'static str {
        if self.no_password_required {
            "none required"
        } else if self.password_in_keychain {
            "keychain"
        } else if self.password_env.is_some() {
            "env var"
        } else if self.password_onepassword.is_some() {
            "1Password"
        } else {
            "prompt"
        }
    }

    /// Short relative-time description for `last_used_at` (e.g. "2m ago").
    pub fn last_used_label(&self) -> String {
        match self.last_used_at {
            None => "never".to_string(),
            Some(ts) => {
                let now = Utc::now();
                let delta = now.signed_duration_since(ts);
                let secs = delta.num_seconds();
                if secs < 0 {
                    return "just now".to_string();
                }
                if secs < 60 {
                    return format!("{}s ago", secs);
                }
                let mins = secs / 60;
                if mins < 60 {
                    return format!("{}m ago", mins);
                }
                let hours = mins / 60;
                if hours < 24 {
                    return format!("{}h ago", hours);
                }
                let days = hours / 24;
                if days < 30 {
                    return format!("{}d ago", days);
                }
                ts.format("%Y-%m-%d").to_string()
            }
        }
    }

    /// Sanitised `tsql <url>` CLI form for yanking to clipboard.
    pub fn to_cli_command(&self) -> String {
        // Shell-quote the URL so characters like `&` (common in Mongo
        // query strings, e.g. `?replicaSet=rs0&ssl=true`) don't get
        // interpreted by the user's shell when they paste the command.
        let url = self.to_url(None);
        let quoted = shlex::try_quote(&url)
            .map(|cow| cow.into_owned())
            .unwrap_or_else(|_| format!("'{}'", url.replace('\'', "'\\''")));
        format!("tsql {}", quoted)
    }

    /// Parse and normalise a comma-separated tag string. Whitespace trimmed;
    /// empty tags dropped; duplicates collapsed preserving order.
    pub fn parse_tags(input: &str) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        input
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .filter_map(|s| {
                let t = s.to_string();
                if seen.insert(t.clone()) {
                    Some(t)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Format connection for display (without password)
    pub fn display_string(&self) -> String {
        if self.kind == DbKind::Mongo {
            // Imported / hand-edited entries can embed credentials in
            // `uri`. The detail pane and status line call into this
            // directly, so strip the password before handing the string
            // back — otherwise opening the connection manager on a
            // wide terminal would render plaintext credentials. Cover
            // the Url::parse success branch AND the unparseable
            // fallback: a slightly-off URI (e.g. literal space in the
            // host) was still going through verbatim and leaking.
            let uri = self.uri.as_deref().unwrap_or("mongodb://localhost");
            if let Ok(mut parsed) = Url::parse(uri) {
                if parsed.password().is_some() {
                    let _ = parsed.set_password(None);
                    return parsed.to_string();
                }
                return uri.to_string();
            }
            return sanitize_mongo_uri_fallback(uri);
        }
        format!(
            "{}@{}:{}/{}",
            self.user, self.host, self.port, self.database
        )
    }

    /// Short display format for status line
    pub fn short_display(&self) -> String {
        if self.kind == DbKind::Mongo {
            let uri = self
                .uri
                .as_deref()
                .unwrap_or("mongodb://localhost")
                .to_string();
            if let Ok(parsed) = Url::parse(&uri) {
                let host = parsed.host_str().unwrap_or("localhost");
                let db = parsed.path().trim_start_matches('/');
                if db.is_empty() {
                    return format!("mongodb://{}", host);
                }
                return format!("mongodb://{}/{}", host, db);
            }
            // Unparseable fallback: strip anything that looks like
            // `user:password@` so a malformed-but-credential-bearing
            // URI doesn't end up on screen verbatim.
            return sanitize_mongo_uri_fallback(&uri);
        }
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
        match self.kind {
            DbKind::Postgres => {
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
            }
            DbKind::Mongo => {
                let Some(uri) = self.uri.as_deref() else {
                    return Err(anyhow!("Mongo connections require a URI"));
                };
                let parsed = Url::parse(uri).context("Invalid Mongo URI format")?;
                if parsed.scheme() != "mongodb" && parsed.scheme() != "mongodb+srv" {
                    return Err(anyhow!(
                        "Mongo URI must use mongodb:// or mongodb+srv:// scheme"
                    ));
                }
            }
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
    /// Sort mode last chosen in the connection manager. Persisted so the
    /// user's preference survives restarts.
    #[serde(default, skip_serializing_if = "is_default_sort")]
    pub last_sort_mode: SortMode,
}

fn is_default_sort(mode: &SortMode) -> bool {
    *mode == SortMode::default()
}

impl ConnectionsFile {
    /// Create a new empty connections file
    pub fn new() -> Self {
        Self {
            connections: Vec::new(),
            last_sort_mode: SortMode::default(),
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

        let mut entry = entry;
        // Once the user has manually reordered any non-favorite entry,
        // some `order` values become > 0. A freshly added / imported
        // entry defaults to `order = 0` and would otherwise jump to
        // the top of the manual list — surprising behaviour after
        // every `:import-connections` or "add" click. Append to the
        // bottom instead when anyone else has been reordered.
        if entry.favorite.is_none() && entry.order == 0 {
            let max_order = self
                .connections
                .iter()
                .filter(|c| c.favorite.is_none())
                .map(|c| c.order)
                .max()
                .unwrap_or(0);
            if max_order > 0 {
                entry.order = max_order.saturating_add(1);
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
        self.sorted_by(SortMode::FavoritesAlpha)
    }

    /// Get connections sorted by the given mode.
    pub fn sorted_by(&self, mode: SortMode) -> Vec<&ConnectionEntry> {
        let mut sorted: Vec<_> = self.connections.iter().collect();
        match mode {
            SortMode::FavoritesAlpha => {
                sorted.sort_by(|a, b| match (a.favorite, b.favorite) {
                    (Some(fa), Some(fb)) => fa.cmp(&fb),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => a
                        .order
                        .cmp(&b.order)
                        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
                });
            }
            SortMode::Recent => {
                sorted.sort_by(|a, b| {
                    b.last_used_at
                        .cmp(&a.last_used_at)
                        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
            }
            SortMode::MostUsed => {
                sorted.sort_by(|a, b| {
                    b.use_count
                        .cmp(&a.use_count)
                        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
            }
            SortMode::Alpha => {
                sorted.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            }
            SortMode::Folder => {
                sorted.sort_by(|a, b| {
                    let fa = a.folder.as_deref().unwrap_or("~");
                    let fb = b.folder.as_deref().unwrap_or("~");
                    fa.to_lowercase()
                        .cmp(&fb.to_lowercase())
                        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
            }
        }
        sorted
    }

    /// Touch an entry to record a successful use. Returns whether the entry
    /// was found. Caller should persist via `save_connections_debounced`.
    pub fn touch_use(&mut self, name: &str) -> bool {
        if let Some(entry) = self.find_by_name_mut(name) {
            entry.last_used_at = Some(Utc::now());
            entry.use_count = entry.use_count.saturating_add(1);
            true
        } else {
            false
        }
    }

    /// Return all distinct folder labels in insertion order.
    pub fn folders(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for c in &self.connections {
            if let Some(f) = c.folder.as_deref() {
                let f = f.to_string();
                if seen.insert(f.clone()) {
                    out.push(f);
                }
            }
        }
        out
    }

    /// Return all distinct tag labels across entries.
    pub fn all_tags(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for c in &self.connections {
            for t in &c.tags {
                if seen.insert(t.clone()) {
                    out.push(t.clone());
                }
            }
        }
        out.sort();
        out
    }

    /// Filter entries by a lowercase needle across name / host / database /
    /// tags / description / folder. Empty needle returns all.
    pub fn filtered(&self, needle: &str) -> Vec<&ConnectionEntry> {
        let needle = needle.trim().to_lowercase();
        if needle.is_empty() {
            return self.connections.iter().collect();
        }
        self.connections
            .iter()
            .filter(|c| entry_matches(c, &needle))
            .collect()
    }
}

fn entry_matches(c: &ConnectionEntry, needle_lc: &str) -> bool {
    if c.name.to_lowercase().contains(needle_lc) {
        return true;
    }
    if c.host.to_lowercase().contains(needle_lc) {
        return true;
    }
    if c.database.to_lowercase().contains(needle_lc) {
        return true;
    }
    if c.user.to_lowercase().contains(needle_lc) {
        return true;
    }
    if let Some(f) = &c.folder {
        if f.to_lowercase().contains(needle_lc) {
            return true;
        }
    }
    if let Some(d) = &c.description {
        if d.to_lowercase().contains(needle_lc) {
            return true;
        }
    }
    for t in &c.tags {
        if t.to_lowercase().contains(needle_lc) {
            return true;
        }
    }
    false
}

/// Sort modes available in the connection manager. Persisted across
/// restarts via `ConnectionsFile.last_sort_mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SortMode {
    /// Favorites first (by slot), then manual order, then alphabetical.
    #[default]
    FavoritesAlpha,
    /// Most recently used first.
    Recent,
    /// Highest use_count first.
    MostUsed,
    /// Pure alphabetical order by name.
    Alpha,
    /// Grouped by folder, then alphabetical.
    Folder,
}

impl SortMode {
    /// Short label shown in UI.
    pub fn label(self) -> &'static str {
        match self {
            SortMode::FavoritesAlpha => "favorites",
            SortMode::Recent => "recent",
            SortMode::MostUsed => "most used",
            SortMode::Alpha => "alpha",
            SortMode::Folder => "folder",
        }
    }

    /// Cycle to the next sort mode.
    pub fn next(self) -> Self {
        match self {
            SortMode::FavoritesAlpha => SortMode::Recent,
            SortMode::Recent => SortMode::MostUsed,
            SortMode::MostUsed => SortMode::Alpha,
            SortMode::Alpha => SortMode::Folder,
            SortMode::Folder => SortMode::FavoritesAlpha,
        }
    }
}

/// Conflict-resolution strategy when importing an entry whose name already
/// exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportConflict {
    /// Skip the incoming entry.
    Skip,
    /// Overwrite the existing entry in place.
    Overwrite,
    /// Rename the incoming entry by appending " (imported)".
    Rename,
}

/// Returns the connections file path (inside `config_dir()`).
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

/// Save connections to the default path (atomic: writes to a tmp file and
/// renames into place so a crash mid-write cannot corrupt the store).
pub fn save_connections(file: &ConnectionsFile) -> Result<()> {
    let path = connections_path().ok_or_else(|| anyhow!("Could not determine config directory"))?;
    write_connections_atomic(&path, file)
}

/// Write a `ConnectionsFile` to a specific path atomically. Public so tests
/// and import/export can reuse the write path.
pub fn write_connections_atomic(path: &Path, file: &ConnectionsFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory: {}", parent.display()))?;
    }

    let content = toml::to_string_pretty(file).context("Failed to serialize connections")?;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("Failed to create tempfile in {}", parent.display()))?;
    tmp.write_all(content.as_bytes())
        .context("Failed to write connections temp file")?;
    tmp.as_file()
        .sync_all()
        .context("Failed to fsync connections temp file")?;

    // `NamedTempFile::persist` does a rename, but tempfile's own docs
    // note that on Windows persist-over-an-existing-file is not
    // guaranteed. Convert to a `TempPath` (giving up the destructor)
    // and use `std::fs::rename`, which is documented to replace on
    // every supported platform: Unix atomic rename, Windows
    // `MoveFileEx` with `MOVEFILE_REPLACE_EXISTING`.
    let tmp_path = tmp.into_temp_path();
    std::fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "Failed to rename temp connections file into place at {}",
            path.display()
        )
    })?;
    // rename consumed the inode but the TempPath still points at the
    // old location — detach it so its Drop doesn't try to unlink a
    // path that no longer exists.
    let _ = tmp_path.keep();

    Ok(())
}

/// Export a subset of entries to a TOML file at the given path (atomic).
pub fn export_to_path(path: &Path, entries: Vec<ConnectionEntry>) -> Result<()> {
    let file = ConnectionsFile {
        connections: entries,
        last_sort_mode: SortMode::default(),
    };
    write_connections_atomic(path, &file)
}

/// Summary returned from an import operation.
#[derive(Debug, Default, Clone)]
pub struct ImportSummary {
    pub imported: usize,
    pub renamed: usize,
    pub overwritten: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

/// Import connections from a TOML file at `path`, merging into `target`
/// using the given conflict strategy. Does NOT persist — caller must call
/// `save_connections` afterwards.
pub fn import_from_path(
    target: &mut ConnectionsFile,
    path: &Path,
    strategy: ImportConflict,
) -> Result<ImportSummary> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read import file: {}", path.display()))?;
    let incoming: ConnectionsFile = toml::from_str(&content)
        .with_context(|| format!("Failed to parse import file: {}", path.display()))?;

    let mut summary = ImportSummary::default();

    for mut entry in incoming.connections.into_iter() {
        // Drop favorite slot to avoid collisions on import; the user can
        // reassign after review.
        entry.favorite = None;

        if entry.validate().is_err() {
            summary
                .errors
                .push(format!("Skipped invalid entry '{}'", entry.name));
            summary.skipped += 1;
            continue;
        }

        if target.find_by_name(&entry.name).is_some() {
            match strategy {
                ImportConflict::Skip => {
                    summary.skipped += 1;
                    continue;
                }
                ImportConflict::Overwrite => {
                    let name = entry.name.clone();
                    match target.update(&name, entry) {
                        Ok(()) => summary.overwritten += 1,
                        Err(e) => {
                            summary.errors.push(format!("{}: {}", name, e));
                            summary.skipped += 1;
                        }
                    }
                }
                ImportConflict::Rename => {
                    // Must not contain whitespace (connection names are
                    // validated against that), so we use '-' separators.
                    let mut candidate = format!("{}-imported", entry.name);
                    let mut n = 2;
                    while target.find_by_name(&candidate).is_some() {
                        candidate = format!("{}-imported-{}", entry.name, n);
                        n += 1;
                    }
                    entry.name = candidate;
                    match target.add(entry) {
                        Ok(()) => summary.renamed += 1,
                        Err(e) => {
                            summary.errors.push(e.to_string());
                            summary.skipped += 1;
                        }
                    }
                }
            }
        } else {
            match target.add(entry) {
                Ok(()) => summary.imported += 1,
                Err(e) => {
                    summary.errors.push(e.to_string());
                    summary.skipped += 1;
                }
            }
        }
    }

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_entry_default() {
        let entry = ConnectionEntry::default();
        assert_eq!(entry.kind, DbKind::Postgres);
        assert_eq!(entry.host, "localhost");
        assert_eq!(entry.port, 5432);
        assert_eq!(entry.color, ConnectionColor::None);
        assert!(entry.favorite.is_none());
        assert!(entry.ssl_mode.is_none());
        assert!(entry.description.is_none());
        assert!(entry.tags.is_empty());
        assert!(entry.folder.is_none());
        assert_eq!(entry.use_count, 0);
        assert!(entry.last_used_at.is_none());
        assert_eq!(entry.order, 0);
    }

    #[test]
    fn test_metadata_fields_round_trip_via_toml() {
        let mut entry = ConnectionEntry::new("prod-db");
        entry.host = "db.internal".to_string();
        entry.database = "main".to_string();
        entry.user = "app".to_string();
        entry.description = Some("Production — be careful".to_string());
        entry.tags = vec!["prod".to_string(), "critical".to_string()];
        entry.folder = Some("Production".to_string());
        entry.application_name = Some("tsql-cli".to_string());
        entry.connect_timeout_secs = Some(15);
        entry.use_count = 42;
        let mut file = ConnectionsFile::new();
        file.add(entry.clone()).unwrap();
        let toml_str = toml::to_string_pretty(&file).unwrap();
        let reparsed: ConnectionsFile = toml::from_str(&toml_str).unwrap();
        let got = reparsed.find_by_name("prod-db").unwrap();
        assert_eq!(got.description.as_deref(), Some("Production — be careful"));
        assert_eq!(got.tags, vec!["prod", "critical"]);
        assert_eq!(got.folder.as_deref(), Some("Production"));
        assert_eq!(got.application_name.as_deref(), Some("tsql-cli"));
        assert_eq!(got.connect_timeout_secs, Some(15));
        assert_eq!(got.use_count, 42);
    }

    #[test]
    fn test_legacy_toml_loads_with_defaults_for_new_fields() {
        // Simulates an existing connections.toml written before v2.
        let toml_str = r#"
[[connection]]
kind = "postgres"
name = "old"
host = "localhost"
port = 5432
database = "db"
user = "me"
"#;
        let parsed: ConnectionsFile = toml::from_str(toml_str).unwrap();
        let entry = parsed.find_by_name("old").unwrap();
        assert!(entry.description.is_none());
        assert!(entry.tags.is_empty());
        assert_eq!(entry.use_count, 0);
    }

    #[test]
    fn test_mongo_display_string_strips_password() {
        // Regression: the detail pane renders `display_string()` for
        // the "Target" row. If the stored URI embedded credentials
        // (imported / hand-edited), the password used to appear on
        // screen when the user opened the manager.
        let entry = ConnectionEntry {
            kind: DbKind::Mongo,
            name: "m".to_string(),
            uri: Some("mongodb://user:topsecret@host:27017/db".to_string()),
            host: "host".to_string(),
            port: 27017,
            database: "db".to_string(),
            user: "user".to_string(),
            ..Default::default()
        };
        let display = entry.display_string();
        assert!(!display.contains("topsecret"), "leaked: {display}");
        assert!(display.contains("user"));
        assert!(display.contains("host"));
    }

    #[test]
    fn test_sanitize_mongo_uri_fallback_strips_userinfo() {
        assert_eq!(
            sanitize_mongo_uri_fallback("mongodb://user:pw@host/db"),
            "mongodb://@host/db"
        );
        assert_eq!(
            sanitize_mongo_uri_fallback("mongodb://host/db"),
            "mongodb://host/db"
        );
        assert_eq!(
            sanitize_mongo_uri_fallback("mongodb://host/db?ssl=true"),
            "mongodb://host/db?ssl=true"
        );
        assert_eq!(sanitize_mongo_uri_fallback("not a url"), "not a url");
    }

    #[test]
    fn test_add_appends_new_entry_below_manually_ordered_list() {
        // Regression: after the user reorders manually (bumping some
        // `order` values > 0), adding a new entry with default
        // `order = 0` would put it at the top of the visible list.
        // `add()` must now append to the bottom in that case.
        let mut file = ConnectionsFile::new();
        for name in ["a", "b", "c"] {
            file.add(ConnectionEntry {
                name: name.to_string(),
                host: "h".to_string(),
                database: "d".to_string(),
                user: "u".to_string(),
                ..Default::default()
            })
            .unwrap();
        }
        // Simulate user reordering: b bumped to 1, c bumped to 2.
        // Everyone was at 0, so now the list has mixed orders.
        file.find_by_name_mut("b").unwrap().order = 1;
        file.find_by_name_mut("c").unwrap().order = 2;

        // Add a new entry. Default order = 0 would have sorted first
        // (`a:0, d:0, b:1, c:2` — actually alphabetically among zeros,
        // so `a, d, b, c`). We want append-to-bottom semantics.
        file.add(ConnectionEntry {
            name: "d".to_string(),
            host: "h".to_string(),
            database: "x".to_string(),
            user: "u".to_string(),
            ..Default::default()
        })
        .unwrap();

        let sorted: Vec<&str> = file
            .sorted_by(SortMode::FavoritesAlpha)
            .into_iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(
            sorted.last().copied(),
            Some("d"),
            "new entry must land at the bottom after the user has manually reordered; got {sorted:?}"
        );
    }

    #[test]
    fn test_add_leaves_order_zero_when_nobody_has_reordered() {
        // Fresh file with no manual ordering — new entries stay at
        // the default 0 so the alphabetical tiebreak keeps working.
        let mut file = ConnectionsFile::new();
        file.add(ConnectionEntry {
            name: "alpha".to_string(),
            host: "h".to_string(),
            database: "d".to_string(),
            user: "u".to_string(),
            ..Default::default()
        })
        .unwrap();
        file.add(ConnectionEntry {
            name: "bravo".to_string(),
            host: "h".to_string(),
            database: "d".to_string(),
            user: "u".to_string(),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(file.find_by_name("alpha").unwrap().order, 0);
        assert_eq!(file.find_by_name("bravo").unwrap().order, 0);
    }

    #[test]
    fn test_mongo_display_string_strips_password_when_uri_not_parseable() {
        // Regression: `display_string` used to `return uri.to_string()`
        // unchanged if Url::parse failed, leaking embedded credentials
        // in the connection-manager detail pane.
        let entry = ConnectionEntry {
            kind: DbKind::Mongo,
            name: "m".to_string(),
            uri: Some("mongodb://user:topsecret@bad host/db".to_string()),
            host: "bad host".to_string(),
            port: 27017,
            database: "db".to_string(),
            user: "user".to_string(),
            ..Default::default()
        };
        let display = entry.display_string();
        assert!(!display.contains("topsecret"), "leaked: {display}");
    }

    #[test]
    fn test_mongo_to_url_none_strips_password_even_when_uri_not_parseable() {
        // Regression: `Url::parse` could reject a hand-edited URI
        // while still carrying credentials; the previous fallback
        // returned it verbatim, so yank/copy-as-CLI leaked the
        // password for those inputs.
        let entry = ConnectionEntry {
            kind: DbKind::Mongo,
            name: "m".to_string(),
            // Intentionally crafted to look malformed to url crate
            // (the space in the host breaks parsing) yet still embed
            // credentials in the `userinfo@` prefix.
            uri: Some("mongodb://user:topsecret@bad host/db".to_string()),
            host: "bad host".to_string(),
            port: 27017,
            database: "db".to_string(),
            user: "user".to_string(),
            ..Default::default()
        };
        let url = entry.to_url(None);
        assert!(!url.contains("topsecret"), "leaked: {url}");
    }

    #[test]
    fn test_mongo_to_url_none_strips_password_from_stored_uri() {
        // Regression: imported/manually-edited connections.toml entries
        // can carry passwords embedded in the `uri`. `to_url(None)` is
        // used by yank and copy-as-CLI actions, which advertise "no
        // password". Ensure the password is actually stripped.
        let entry = ConnectionEntry {
            kind: DbKind::Mongo,
            name: "m".to_string(),
            uri: Some("mongodb://user:secret@host:27017/db".to_string()),
            host: "host".to_string(),
            port: 27017,
            database: "db".to_string(),
            user: "user".to_string(),
            ..Default::default()
        };
        let url = entry.to_url(None);
        assert!(!url.contains("secret"), "leaked password: {url}");
        assert!(url.contains("user"));
        assert!(url.contains("host"));
    }

    #[test]
    fn test_to_cli_command_quotes_url_with_shell_meta_chars() {
        // Regression: URLs with `&`, `?`, `*`, etc. (common in Mongo
        // query strings like `?replicaSet=rs0&ssl=true`) must be
        // shell-escaped or the pasted command breaks / runs unintended
        // things.
        let entry = ConnectionEntry {
            kind: DbKind::Mongo,
            name: "m".to_string(),
            uri: Some("mongodb://host/db?replicaSet=rs0&ssl=true".to_string()),
            host: "host".to_string(),
            port: 27017,
            database: "db".to_string(),
            user: String::new(),
            ..Default::default()
        };
        let cmd = entry.to_cli_command();
        // The URL must be inside quotes (single or double), i.e. the
        // command should NOT be just `tsql mongodb://…&…`.
        assert!(
            cmd.starts_with("tsql '") || cmd.starts_with("tsql \""),
            "cli command not quoted: {cmd}",
        );
        assert!(cmd.contains("mongodb://host/db?replicaSet=rs0&ssl=true"));
    }

    #[test]
    fn test_parse_tags_trims_dedupes_and_drops_empty() {
        let got = ConnectionEntry::parse_tags(" prod ,  , staging, prod, ");
        assert_eq!(got, vec!["prod".to_string(), "staging".to_string()]);
    }

    #[test]
    fn test_touch_use_bumps_counter_and_timestamp() {
        let mut file = ConnectionsFile::new();
        let entry = ConnectionEntry {
            name: "x".to_string(),
            host: "h".to_string(),
            database: "d".to_string(),
            user: "u".to_string(),
            ..Default::default()
        };
        file.add(entry).unwrap();
        assert!(file.touch_use("x"));
        assert_eq!(file.find_by_name("x").unwrap().use_count, 1);
        assert!(file.find_by_name("x").unwrap().last_used_at.is_some());
        assert!(file.touch_use("x"));
        assert_eq!(file.find_by_name("x").unwrap().use_count, 2);
        assert!(!file.touch_use("missing"));
    }

    #[test]
    fn test_sort_modes_differ() {
        let mut file = ConnectionsFile::new();
        let mut a = ConnectionEntry::new("alpha");
        a.host = "h".into();
        a.database = "d".into();
        a.user = "u".into();
        a.use_count = 1;
        let mut b = ConnectionEntry::new("bravo");
        b.host = "h".into();
        b.database = "d".into();
        b.user = "u".into();
        b.use_count = 10;
        b.last_used_at = Some(Utc::now());
        file.add(a).unwrap();
        file.add(b).unwrap();

        let by_alpha: Vec<&str> = file
            .sorted_by(SortMode::Alpha)
            .into_iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(by_alpha, vec!["alpha", "bravo"]);

        let by_most_used: Vec<&str> = file
            .sorted_by(SortMode::MostUsed)
            .into_iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(by_most_used, vec!["bravo", "alpha"]);

        let by_recent: Vec<&str> = file
            .sorted_by(SortMode::Recent)
            .into_iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(by_recent, vec!["bravo", "alpha"]);

        assert_eq!(SortMode::FavoritesAlpha.next(), SortMode::Recent);
    }

    #[test]
    fn test_filtered_matches_across_fields() {
        let mut file = ConnectionsFile::new();
        let mut a = ConnectionEntry::new("alpha");
        a.host = "prod-host".into();
        a.database = "sales".into();
        a.user = "u".into();
        a.tags = vec!["prod".into()];
        a.description = Some("Main read replica".into());
        let mut b = ConnectionEntry::new("bravo");
        b.host = "staging".into();
        b.database = "sandbox".into();
        b.user = "u".into();
        b.folder = Some("Staging".into());
        file.add(a).unwrap();
        file.add(b).unwrap();

        let hits: Vec<&str> = file
            .filtered("prod")
            .into_iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(hits, vec!["alpha"]);

        let hits: Vec<&str> = file
            .filtered("Staging")
            .into_iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(hits, vec!["bravo"]);

        let hits: Vec<&str> = file
            .filtered("")
            .into_iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(hits, vec!["alpha", "bravo"]);
    }

    #[test]
    fn test_atomic_write_and_import_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("connections.toml");

        let mut file = ConnectionsFile::new();
        let entry = ConnectionEntry {
            name: "alpha".to_string(),
            host: "h".to_string(),
            database: "d".to_string(),
            user: "u".to_string(),
            tags: vec!["prod".to_string()],
            ..Default::default()
        };
        file.add(entry).unwrap();
        write_connections_atomic(&path, &file).unwrap();
        assert!(path.exists());

        // Import into a fresh file: should come in clean.
        let mut target = ConnectionsFile::new();
        let summary = import_from_path(&mut target, &path, ImportConflict::Rename).unwrap();
        assert_eq!(summary.imported, 1);
        assert_eq!(summary.renamed, 0);
        assert_eq!(target.find_by_name("alpha").unwrap().tags, vec!["prod"]);

        // Re-import into same target with Rename strategy: should
        // produce a renamed copy.
        let summary2 = import_from_path(&mut target, &path, ImportConflict::Rename).unwrap();
        assert_eq!(summary2.renamed, 1);
        assert!(target.find_by_name("alpha-imported").is_some());

        // Skip strategy: no-op on collision.
        let summary3 = import_from_path(&mut target, &path, ImportConflict::Skip).unwrap();
        assert_eq!(summary3.skipped, 1);
        assert_eq!(summary3.imported, 0);
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
    fn test_connection_to_url_threads_ssl_cert_paths() {
        // Regression: saved per-entry SSL cert paths were inert at
        // connect / test time because `to_url` never emitted them
        // as `sslrootcert` / `sslcert` / `sslkey` query params.
        use std::path::PathBuf;
        let entry = ConnectionEntry {
            name: "test".to_string(),
            host: "localhost".to_string(),
            port: 5432,
            database: "mydb".to_string(),
            user: "postgres".to_string(),
            ssl_mode: Some(SslMode::VerifyFull),
            ssl_root_cert: Some(PathBuf::from("/etc/ssl/ca.pem")),
            ssl_client_cert: Some(PathBuf::from("/etc/ssl/client.pem")),
            ssl_client_key: Some(PathBuf::from("/etc/ssl/client.key")),
            ..Default::default()
        };
        let url = entry.to_url(None);
        assert!(url.contains("sslmode=verify-full"), "url: {url}");
        assert!(
            url.contains("sslrootcert=") && (url.contains("ca.pem") || url.contains("ca%2Epem")),
            "url: {url}"
        );
        assert!(url.contains("sslcert="), "url: {url}");
        assert!(url.contains("sslkey="), "url: {url}");
    }

    #[test]
    fn test_connection_to_url_threads_timeout_and_app_name() {
        // Regression: per-entry `connect_timeout_secs` and
        // `application_name` must end up as query params, otherwise
        // setting them in the UI is silently inert.
        let entry = ConnectionEntry {
            name: "test".to_string(),
            host: "localhost".to_string(),
            port: 5432,
            database: "mydb".to_string(),
            user: "postgres".to_string(),
            connect_timeout_secs: Some(5),
            application_name: Some("tsql-prod".to_string()),
            ssl_mode: Some(SslMode::Require),
            ..Default::default()
        };
        let url = entry.to_url(None);
        assert!(url.contains("sslmode=require"), "url: {url}");
        assert!(url.contains("connect_timeout=5"), "url: {url}");
        assert!(url.contains("application_name=tsql-prod"), "url: {url}");
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
    fn test_connection_from_url_preserves_sslmode_disable() {
        // Verify that explicit sslmode=disable is preserved (not converted to None)
        let (entry, _) =
            ConnectionEntry::from_url("test", "postgres://user@localhost/mydb?sslmode=disable")
                .unwrap();

        assert_eq!(
            entry.ssl_mode,
            Some(SslMode::Disable),
            "Explicit sslmode=disable should be preserved as Some(Disable), not None"
        );
    }

    #[test]
    fn test_connection_url_round_trip_all_ssl_modes() {
        // Verify from_url/to_url round-trip consistency for all SSL modes
        let test_cases = [
            (
                "postgres://user@localhost/mydb?sslmode=disable",
                Some(SslMode::Disable),
            ),
            (
                "postgres://user@localhost/mydb?sslmode=prefer",
                Some(SslMode::Prefer),
            ),
            (
                "postgres://user@localhost/mydb?sslmode=require",
                Some(SslMode::Require),
            ),
            (
                "postgres://user@localhost/mydb?sslmode=verify-ca",
                Some(SslMode::VerifyCa),
            ),
            (
                "postgres://user@localhost/mydb?sslmode=verify-full",
                Some(SslMode::VerifyFull),
            ),
            ("postgres://user@localhost/mydb", None), // No sslmode
        ];

        for (url, expected_mode) in test_cases {
            let (entry, _) = ConnectionEntry::from_url("test", url).unwrap();
            assert_eq!(entry.ssl_mode, expected_mode, "Failed for URL: {}", url);

            // Verify round-trip: to_url should produce URL with same sslmode
            let regenerated_url = entry.to_url(None);
            let (re_entry, _) = ConnectionEntry::from_url("test", &regenerated_url).unwrap();
            assert_eq!(
                re_entry.ssl_mode, expected_mode,
                "Round-trip failed for URL: {} -> {}",
                url, regenerated_url
            );
        }
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
    fn test_connection_from_url_mongodb_parses_kind_and_sanitizes_uri() {
        let (entry, password) = ConnectionEntry::from_url(
            "mongo",
            "mongodb://admin:secret@mongo.example.com:27018/sample?authSource=admin",
        )
        .unwrap();
        assert_eq!(entry.kind, DbKind::Mongo);
        assert_eq!(entry.host, "mongo.example.com");
        assert_eq!(entry.port, 27018);
        assert_eq!(entry.database, "sample");
        assert_eq!(entry.user, "admin");
        assert_eq!(password.as_deref(), Some("secret"));
        assert!(entry.uri.is_some());
        let uri = entry.uri.as_deref().unwrap_or("");
        assert!(!uri.contains("secret"));
        assert!(uri.contains("authSource=admin"));
    }

    #[test]
    fn test_connection_from_url_mongodb_without_username_keeps_empty_user() {
        let (entry, password) =
            ConnectionEntry::from_url("mongo", "mongodb://mongo.example.com:27017/sample").unwrap();
        assert_eq!(entry.kind, DbKind::Mongo);
        assert_eq!(entry.user, "");
        assert!(password.is_none());
    }

    #[test]
    fn test_connection_from_url_mongodb_without_database_is_allowed() {
        let (entry, password) = ConnectionEntry::from_url("mongo", "mongodb://localhost").unwrap();
        assert_eq!(entry.kind, DbKind::Mongo);
        assert_eq!(entry.database, "");
        assert_eq!(entry.user, "");
        assert!(password.is_none());
        assert!(entry.uri.is_some());
    }

    #[test]
    fn test_connection_to_url_mongodb_includes_password_for_runtime_connection() {
        let entry = ConnectionEntry {
            kind: DbKind::Mongo,
            name: "mongo".to_string(),
            uri: Some("mongodb://admin@mongo.example.com:27018/sample".to_string()),
            host: "mongo.example.com".to_string(),
            port: 27018,
            database: "sample".to_string(),
            user: "admin".to_string(),
            ..Default::default()
        };
        let url = entry.to_url(Some("secret"));
        assert_eq!(url, "mongodb://admin:secret@mongo.example.com:27018/sample");
    }

    #[test]
    fn test_connection_validate_mongodb_requires_uri() {
        let entry = ConnectionEntry {
            kind: DbKind::Mongo,
            name: "mongo".to_string(),
            uri: None,
            host: "mongo.example.com".to_string(),
            port: 27017,
            database: "sample".to_string(),
            user: "admin".to_string(),
            ..Default::default()
        };
        assert!(entry.validate().is_err());
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
    // https://github.com/rekurt/tsql/issues/16
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
