//! Status line component with responsive layout.

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// Priority levels for status segments (lower = higher priority, shown first)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Critical = 0,
    High = 1,
    Medium = 2,
    Low = 3,
}

/// A segment of the status line with priority and styling
#[derive(Debug, Clone)]
pub struct StatusSegment {
    /// The content to display
    pub content: String,
    /// Priority for display (lower = more important)
    pub priority: Priority,
    /// Style for this segment
    pub style: Style,
    /// Minimum width needed to show this segment
    pub min_width: u16,
    /// Whether this segment should be right-aligned
    pub right_align: bool,
}

impl StatusSegment {
    pub fn new(content: impl Into<String>, priority: Priority) -> Self {
        Self {
            content: content.into(),
            priority,
            style: Style::default(),
            min_width: 0,
            right_align: false,
        }
    }

    pub fn style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    pub fn min_width(mut self, width: u16) -> Self {
        self.min_width = width;
        self
    }

    pub fn right_align(mut self) -> Self {
        self.right_align = true;
        self
    }

    /// Get the display width of this segment
    pub fn width(&self) -> u16 {
        self.content.chars().count() as u16
    }
}

/// Connection info extracted from a connection string
#[derive(Debug, Clone, Default)]
pub struct ConnectionInfo {
    pub user: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub database: Option<String>,
}

impl ConnectionInfo {
    /// Parse a PostgreSQL connection string
    /// Supports formats:
    /// - postgres://user:pass@host:port/database?params
    /// - postgresql://user:pass@host:port/database
    /// - host=localhost user=postgres dbname=mydb port=5432
    pub fn parse(conn_str: &str) -> Self {
        let mut info = ConnectionInfo::default();

        if conn_str.starts_with("postgres://") || conn_str.starts_with("postgresql://") {
            // URL format
            info.parse_url(conn_str);
        } else {
            // Key=value format
            info.parse_key_value(conn_str);
        }

        info
    }

    fn parse_url(&mut self, conn_str: &str) {
        // Remove the scheme
        let without_scheme = conn_str
            .strip_prefix("postgres://")
            .or_else(|| conn_str.strip_prefix("postgresql://"))
            .unwrap_or(conn_str);

        // Split off query params
        let (main_part, _params) = without_scheme
            .split_once('?')
            .unwrap_or((without_scheme, ""));

        // Split user:pass@host:port/database
        let (auth_host, database) = main_part.rsplit_once('/').unwrap_or((main_part, ""));

        if !database.is_empty() {
            self.database = Some(database.to_string());
        }

        let (auth, host_port) = if auth_host.contains('@') {
            let (a, h) = auth_host.rsplit_once('@').unwrap();
            (Some(a), h)
        } else {
            (None, auth_host)
        };

        // Parse user from auth (user:pass)
        if let Some(auth) = auth {
            let user = auth.split(':').next().unwrap_or(auth);
            if !user.is_empty() {
                self.user = Some(user.to_string());
            }
        }

        // Parse host:port
        if let Some((host, port_str)) = host_port.rsplit_once(':') {
            if !host.is_empty() {
                self.host = Some(host.to_string());
            }
            if let Ok(port) = port_str.parse() {
                self.port = Some(port);
            }
        } else if !host_port.is_empty() {
            self.host = Some(host_port.to_string());
        }
    }

    fn parse_key_value(&mut self, conn_str: &str) {
        for part in conn_str.split_whitespace() {
            if let Some((key, value)) = part.split_once('=') {
                match key.to_lowercase().as_str() {
                    "host" => self.host = Some(value.to_string()),
                    "port" => self.port = value.parse().ok(),
                    "user" => self.user = Some(value.to_string()),
                    "dbname" | "database" => self.database = Some(value.to_string()),
                    _ => {}
                }
            }
        }
    }

    /// Format connection info at various detail levels
    pub fn format(&self, max_width: u16) -> String {
        let full = self.format_full();
        if full.len() <= max_width as usize {
            return full;
        }

        let medium = self.format_medium();
        if medium.len() <= max_width as usize {
            return medium;
        }

        let short = self.format_short();
        if short.len() <= max_width as usize {
            return short;
        }

        // Truncate if still too long
        if max_width >= 3 {
            let truncated: String = short.chars().take((max_width - 2) as usize).collect();
            format!("{}…", truncated)
        } else {
            short.chars().take(max_width as usize).collect()
        }
    }

    /// Full format: user@host:port/database
    fn format_full(&self) -> String {
        let mut parts = Vec::new();

        if let Some(ref user) = self.user {
            parts.push(user.clone());
            parts.push("@".to_string());
        }

        if let Some(ref host) = self.host {
            parts.push(host.clone());
        }

        if let Some(port) = self.port {
            if port != 5432 {
                // Only show non-default port
                parts.push(format!(":{}", port));
            }
        }

        if let Some(ref db) = self.database {
            parts.push("/".to_string());
            parts.push(db.clone());
        }

        if parts.is_empty() {
            "disconnected".to_string()
        } else {
            parts.join("")
        }
    }

    /// Medium format: user@host/database (no port)
    fn format_medium(&self) -> String {
        let mut parts = Vec::new();

        if let Some(ref user) = self.user {
            parts.push(user.clone());
            parts.push("@".to_string());
        }

        if let Some(ref host) = self.host {
            parts.push(host.clone());
        }

        if let Some(ref db) = self.database {
            parts.push("/".to_string());
            parts.push(db.clone());
        }

        if parts.is_empty() {
            "disconnected".to_string()
        } else {
            parts.join("")
        }
    }

    /// Short format: just database name
    fn format_short(&self) -> String {
        self.database
            .clone()
            .or_else(|| self.host.clone())
            .unwrap_or_else(|| "disconnected".to_string())
    }
}

/// Builder for creating a responsive status line
pub struct StatusLineBuilder {
    segments: Vec<StatusSegment>,
    separator: String,
    separator_style: Style,
}

impl Default for StatusLineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl StatusLineBuilder {
    pub fn new() -> Self {
        Self {
            segments: Vec::new(),
            separator: " │ ".to_string(),
            separator_style: Style::default().fg(Color::DarkGray),
        }
    }

    pub fn separator(mut self, sep: impl Into<String>) -> Self {
        self.separator = sep.into();
        self
    }

    pub fn segment(mut self, segment: StatusSegment) -> Self {
        self.segments.push(segment);
        self
    }

    /// Add a segment only if the condition is true
    pub fn segment_if(self, condition: bool, segment: StatusSegment) -> Self {
        if condition {
            self.segment(segment)
        } else {
            self
        }
    }

    /// Add a segment only if the option has a value
    pub fn add_some<F>(self, option: Option<impl AsRef<str>>, f: F) -> Self
    where
        F: FnOnce(&str) -> StatusSegment,
    {
        if let Some(value) = option {
            self.segment(f(value.as_ref()))
        } else {
            self
        }
    }

    /// Build the status line for a given width
    pub fn build(self, available_width: u16) -> Line<'static> {
        if available_width == 0 {
            return Line::from("");
        }

        let separator_width = self.separator.chars().count() as u16;

        // Separate left-aligned and right-aligned segments
        let (right_segments, left_segments): (Vec<_>, Vec<_>) =
            self.segments.into_iter().partition(|s| s.right_align);

        // Sort by priority (lower = higher priority)
        let mut left_segments = left_segments;
        left_segments.sort_by_key(|s| s.priority);

        let mut right_segments = right_segments;
        right_segments.sort_by_key(|s| s.priority);

        // Calculate widths
        let right_width: u16 = right_segments.iter().map(|s| s.width()).sum();
        let right_sep_width = if !right_segments.is_empty() && !left_segments.is_empty() {
            separator_width
        } else {
            0
        };

        // Available width for left segments
        let left_available = available_width
            .saturating_sub(right_width)
            .saturating_sub(right_sep_width);

        // Select left segments that fit
        let mut left_used: u16 = 0;
        let mut selected_left: Vec<&StatusSegment> = Vec::new();

        for segment in &left_segments {
            let needed = segment.width()
                + if selected_left.is_empty() {
                    0
                } else {
                    separator_width
                };
            if segment.min_width <= available_width && left_used + needed <= left_available {
                left_used += needed;
                selected_left.push(segment);
            }
        }

        // Build the spans
        let mut spans: Vec<Span<'static>> = Vec::new();

        // Add left segments
        for (i, segment) in selected_left.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(self.separator.clone(), self.separator_style));
            }
            spans.push(Span::styled(segment.content.clone(), segment.style));
        }

        // Add padding to push right segments to the right
        let current_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        let padding_needed = (available_width as usize)
            .saturating_sub(current_width)
            .saturating_sub(right_width as usize)
            .saturating_sub(if !right_segments.is_empty() && !selected_left.is_empty() {
                separator_width as usize
            } else {
                0
            });

        if padding_needed > 0 {
            spans.push(Span::raw(" ".repeat(padding_needed)));
        }

        // Add right segments
        for (i, segment) in right_segments.iter().enumerate() {
            if i > 0 || !selected_left.is_empty() {
                spans.push(Span::styled(self.separator.clone(), self.separator_style));
            }
            spans.push(Span::styled(segment.content.clone(), segment.style));
        }

        Line::from(spans)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_postgres_url() {
        let info = ConnectionInfo::parse("postgres://user:pass@localhost:5432/mydb");
        assert_eq!(info.user, Some("user".to_string()));
        assert_eq!(info.host, Some("localhost".to_string()));
        assert_eq!(info.port, Some(5432));
        assert_eq!(info.database, Some("mydb".to_string()));
    }

    #[test]
    fn test_parse_postgres_url_no_port() {
        let info = ConnectionInfo::parse("postgres://user@localhost/mydb");
        assert_eq!(info.user, Some("user".to_string()));
        assert_eq!(info.host, Some("localhost".to_string()));
        assert_eq!(info.port, None);
        assert_eq!(info.database, Some("mydb".to_string()));
    }

    #[test]
    fn test_parse_postgres_url_with_params() {
        let info =
            ConnectionInfo::parse("postgres://user:pass@localhost:5432/mydb?sslmode=require");
        assert_eq!(info.user, Some("user".to_string()));
        assert_eq!(info.host, Some("localhost".to_string()));
        assert_eq!(info.port, Some(5432));
        assert_eq!(info.database, Some("mydb".to_string()));
    }

    #[test]
    fn test_parse_key_value_format() {
        let info = ConnectionInfo::parse("host=localhost port=5432 user=postgres dbname=mydb");
        assert_eq!(info.user, Some("postgres".to_string()));
        assert_eq!(info.host, Some("localhost".to_string()));
        assert_eq!(info.port, Some(5432));
        assert_eq!(info.database, Some("mydb".to_string()));
    }

    #[test]
    fn test_format_full() {
        let info = ConnectionInfo {
            user: Some("user".to_string()),
            host: Some("localhost".to_string()),
            port: Some(5433), // non-default port
            database: Some("mydb".to_string()),
        };
        assert_eq!(info.format_full(), "user@localhost:5433/mydb");
    }

    #[test]
    fn test_format_full_default_port() {
        let info = ConnectionInfo {
            user: Some("user".to_string()),
            host: Some("localhost".to_string()),
            port: Some(5432), // default port - should be hidden
            database: Some("mydb".to_string()),
        };
        assert_eq!(info.format_full(), "user@localhost/mydb");
    }

    #[test]
    fn test_format_medium() {
        let info = ConnectionInfo {
            user: Some("user".to_string()),
            host: Some("localhost".to_string()),
            port: Some(5433),
            database: Some("mydb".to_string()),
        };
        assert_eq!(info.format_medium(), "user@localhost/mydb");
    }

    #[test]
    fn test_format_short() {
        let info = ConnectionInfo {
            user: Some("user".to_string()),
            host: Some("localhost".to_string()),
            port: Some(5432),
            database: Some("mydb".to_string()),
        };
        assert_eq!(info.format_short(), "mydb");
    }

    #[test]
    fn test_format_truncation() {
        let info = ConnectionInfo {
            user: Some("user".to_string()),
            host: Some("localhost".to_string()),
            port: Some(5432),
            database: Some("very_long_database_name".to_string()),
        };
        let formatted = info.format(10);
        // Use char count, not byte count (since we have unicode ellipsis)
        assert!(
            formatted.chars().count() <= 10,
            "formatted='{}' len={}",
            formatted,
            formatted.chars().count()
        );
        assert!(formatted.ends_with('…'));
    }

    #[test]
    fn test_status_line_builder_basic() {
        let line = StatusLineBuilder::new()
            .segment(StatusSegment::new("NORMAL", Priority::Critical))
            .segment(StatusSegment::new("localhost/db", Priority::High))
            .build(50);

        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("NORMAL"));
        assert!(text.contains("localhost/db"));
    }

    #[test]
    fn test_status_line_builder_priority_filtering() {
        let line = StatusLineBuilder::new()
            .segment(StatusSegment::new("CRITICAL", Priority::Critical))
            .segment(StatusSegment::new("LOW_PRIORITY_LONG_TEXT", Priority::Low))
            .build(20); // Not enough space for both

        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("CRITICAL"));
        // Low priority might be excluded if it doesn't fit
    }

    #[test]
    fn test_status_line_builder_right_align() {
        let line = StatusLineBuilder::new()
            .segment(StatusSegment::new("LEFT", Priority::Critical))
            .segment(StatusSegment::new("RIGHT", Priority::Critical).right_align())
            .build(30);

        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("LEFT"));
        assert!(text.contains("RIGHT"));
        // RIGHT should be at the end (after padding)
        assert!(text.trim_end().ends_with("RIGHT"));
    }

    #[test]
    fn test_status_line_builder_min_width() {
        let line = StatusLineBuilder::new()
            .segment(StatusSegment::new("ALWAYS", Priority::Critical))
            .segment(StatusSegment::new("WIDE_ONLY", Priority::Critical).min_width(100))
            .build(50);

        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("ALWAYS"));
        assert!(!text.contains("WIDE_ONLY")); // min_width not met
    }
}
