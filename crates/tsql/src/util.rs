use std::error::Error as StdError;

use serde_json::Value as JsonValue;

/// Format a postgres error with its full chain of causes
pub fn format_pg_error(e: &tokio_postgres::Error) -> String {
    let mut msg = e.to_string();

    // Try to get the database error details
    if let Some(db_err) = e.as_db_error() {
        msg = db_err.to_string();
    } else if let Some(source) = e.source() {
        // Fall back to source error
        msg = format!("{}: {}", msg, source);
    }

    msg
}

/// Check if a string looks like JSON (starts/ends with {} or [])
pub fn looks_like_json(value: &str) -> bool {
    let trimmed = value.trim();
    (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'))
}

/// Try to parse string as JSON and return pretty-printed version.
/// Returns None if not valid JSON.
pub fn try_format_json(value: &str) -> Option<String> {
    serde_json::from_str::<JsonValue>(value)
        .ok()
        .map(|v| serde_json::to_string_pretty(&v).unwrap_or_else(|_| value.to_string()))
}

/// Check if a value is valid JSON.
pub fn is_valid_json(value: &str) -> bool {
    serde_json::from_str::<JsonValue>(value).is_ok()
}

/// Determine if value should open in multiline editor.
/// Returns true if:
/// - Value contains newlines, OR
/// - Value looks like JSON (always use multiline for JSON to benefit from syntax highlighting)
pub fn should_use_multiline_editor(value: &str) -> bool {
    value.contains('\n') || looks_like_json(value)
}

/// Check if a column type is a JSON type (json or jsonb).
pub fn is_json_column_type(col_type: &str) -> bool {
    let lower = col_type.to_lowercase();
    lower == "json" || lower == "jsonb"
}

/// Content type for syntax highlighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    /// Plain text (no highlighting)
    Plain,
    /// JSON content
    Json,
    /// HTML content
    Html,
    /// SQL content
    Sql,
}

impl ContentType {
    /// Returns the language name for syntax highlighting.
    pub fn language_name(&self) -> Option<&'static str> {
        match self {
            ContentType::Plain => None,
            ContentType::Json => Some("json"),
            ContentType::Html => Some("html"),
            ContentType::Sql => Some("sql"),
        }
    }
}

/// Check if content looks like HTML.
/// Uses conservative detection - requires known HTML tags or DOCTYPE.
pub fn looks_like_html(value: &str) -> bool {
    let trimmed = value.trim();

    // Check for DOCTYPE
    if trimmed.starts_with("<!DOCTYPE") || trimmed.starts_with("<!doctype") {
        return true;
    }

    // Check for common HTML opening tags (case-insensitive)
    let lower = trimmed.to_lowercase();

    // Must start with an HTML tag
    if !lower.starts_with('<') {
        return false;
    }

    // Check for known HTML tags at the start
    let html_tags = [
        "<html", "<head", "<body", "<div", "<span", "<p>", "<p ", "<a ", "<a>", "<ul", "<ol",
        "<li", "<table", "<tr", "<td", "<th", "<form", "<input", "<button", "<img", "<script",
        "<style", "<link", "<meta", "<title", "<header", "<footer", "<nav", "<main", "<section",
        "<article", "<aside", "<h1", "<h2", "<h3", "<h4", "<h5", "<h6", "<br", "<hr",
    ];

    for tag in &html_tags {
        if lower.starts_with(tag) {
            return true;
        }
    }

    false
}

/// Check if content looks like SQL.
pub fn looks_like_sql(value: &str) -> bool {
    let trimmed = value.trim().to_uppercase();

    // Check for common SQL keywords at the start
    let sql_keywords = [
        "SELECT", "INSERT", "UPDATE", "DELETE", "CREATE", "DROP", "ALTER", "TRUNCATE", "WITH",
        "EXPLAIN", "BEGIN", "COMMIT", "ROLLBACK", "GRANT", "REVOKE", "SET", "SHOW",
    ];

    for keyword in &sql_keywords {
        if let Some(rest) = trimmed.strip_prefix(keyword) {
            // Make sure it's followed by whitespace (not just end of string)
            // A keyword alone is not a valid SQL statement
            if rest.starts_with(char::is_whitespace) {
                return true;
            }
        }
    }

    false
}

/// Check if a string looks like a UUID.
/// UUIDs have the format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
/// (8-4-4-4-12 hex digits separated by hyphens)
pub fn is_uuid(value: &str) -> bool {
    let trimmed = value.trim();

    // UUID is exactly 36 characters (32 hex + 4 hyphens)
    if trimmed.len() != 36 {
        return false;
    }

    // Check format: 8-4-4-4-12
    let parts: Vec<&str> = trimmed.split('-').collect();
    if parts.len() != 5 {
        return false;
    }

    // Check each part has correct length and is hex
    let expected_lengths = [8, 4, 4, 4, 12];
    for (i, part) in parts.iter().enumerate() {
        if part.len() != expected_lengths[i] {
            return false;
        }
        if !part.chars().all(|c| c.is_ascii_hexdigit()) {
            return false;
        }
    }

    true
}

/// Truncate a UUID to first 8 characters + ellipsis.
/// Returns the original value if not a UUID.
pub fn truncate_uuid(value: &str, max_len: usize) -> String {
    if is_uuid(value) && value.len() > max_len {
        // Get first 8 chars (the first segment before the hyphen)
        let first_segment = &value[..8.min(max_len.saturating_sub(1))];
        format!("{}...", first_segment)
    } else {
        value.to_string()
    }
}

/// Detect the content type for syntax highlighting.
///
/// Priority: JSON (object/array) > HTML > SQL > Plain
/// This is because JSON objects/arrays and HTML are more specific formats,
/// while SQL detection is broader.
///
/// Note: Simple JSON values like numbers, strings, booleans are treated as plain text
/// since they don't benefit from JSON syntax highlighting.
pub fn detect_content_type(value: &str) -> ContentType {
    // Check for JSON object or array first (most specific)
    // We only consider it JSON if it's an object {} or array []
    // Simple values like numbers, strings, booleans are treated as plain text
    if looks_like_json(value) && is_valid_json(value) {
        return ContentType::Json;
    }

    // Check for HTML
    if looks_like_html(value) {
        return ContentType::Html;
    }

    // Check for SQL
    if looks_like_sql(value) {
        return ContentType::Sql;
    }

    ContentType::Plain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_looks_like_json_object() {
        assert!(looks_like_json(r#"{"key": "value"}"#));
        assert!(looks_like_json(r#"  {"key": "value"}  "#));
        assert!(looks_like_json("{}"));
    }

    #[test]
    fn test_looks_like_json_array() {
        assert!(looks_like_json(r#"[1, 2, 3]"#));
        assert!(looks_like_json(r#"  [1, 2, 3]  "#));
        assert!(looks_like_json("[]"));
    }

    #[test]
    fn test_looks_like_json_negative() {
        assert!(!looks_like_json("hello"));
        assert!(!looks_like_json("{incomplete"));
        assert!(!looks_like_json("[incomplete"));
        assert!(!looks_like_json("123"));
        assert!(!looks_like_json(""));
    }

    #[test]
    fn test_looks_like_json_rejects_html() {
        // HTML should never be detected as JSON
        assert!(!looks_like_json("<html></html>"));
        assert!(!looks_like_json(
            "<!DOCTYPE html><html><body></body></html>"
        ));
        assert!(!looks_like_json("<div>content</div>"));
        assert!(!looks_like_json("  <html>  "));

        // Large HTML content should not be detected as JSON
        let large_html = format!(
            "<html><head><title>Test</title></head><body>{}</body></html>",
            "<p>paragraph</p>".repeat(1000)
        );
        assert!(!looks_like_json(&large_html));
    }

    #[test]
    fn test_try_format_json_valid() {
        let formatted = try_format_json(r#"{"a":1,"b":2}"#).unwrap();
        assert!(formatted.contains('\n'));
        assert!(formatted.contains("\"a\": 1"));
    }

    #[test]
    fn test_try_format_json_invalid() {
        assert!(try_format_json("not json").is_none());
        assert!(try_format_json("{incomplete").is_none());
    }

    #[test]
    fn test_is_valid_json() {
        assert!(is_valid_json(r#"{"key": "value"}"#));
        assert!(is_valid_json("[1, 2, 3]"));
        assert!(is_valid_json("null"));
        assert!(is_valid_json("123"));
        assert!(is_valid_json("\"string\""));
        assert!(!is_valid_json("{incomplete"));
        assert!(!is_valid_json("not json"));
    }

    #[test]
    fn test_should_use_multiline_editor() {
        // JSON should always use multiline
        assert!(should_use_multiline_editor(r#"{"key": "value"}"#));
        assert!(should_use_multiline_editor("[1, 2, 3]"));

        // Newlines should use multiline
        assert!(should_use_multiline_editor("line1\nline2"));

        // Simple values should not
        assert!(!should_use_multiline_editor("hello"));
        assert!(!should_use_multiline_editor("123"));
    }

    #[test]
    fn test_is_json_column_type() {
        assert!(is_json_column_type("json"));
        assert!(is_json_column_type("jsonb"));
        assert!(is_json_column_type("JSON"));
        assert!(is_json_column_type("JSONB"));
        assert!(!is_json_column_type("text"));
        assert!(!is_json_column_type("varchar"));
    }

    #[test]
    fn test_looks_like_html() {
        // Should detect HTML
        assert!(looks_like_html("<html></html>"));
        assert!(looks_like_html("<!DOCTYPE html><html></html>"));
        assert!(looks_like_html("<div>content</div>"));
        assert!(looks_like_html("<p>paragraph</p>"));
        assert!(looks_like_html("  <html>  "));
        assert!(looks_like_html("<TABLE><TR><TD>cell</TD></TR></TABLE>"));

        // Should not detect as HTML
        assert!(!looks_like_html("plain text"));
        assert!(!looks_like_html(r#"{"key": "value"}"#));
        assert!(!looks_like_html("[1, 2, 3]"));
        assert!(!looks_like_html("< not a tag"));
        assert!(!looks_like_html("<custom-element>"));
    }

    #[test]
    fn test_looks_like_sql() {
        // Should detect SQL
        assert!(looks_like_sql("SELECT * FROM users"));
        assert!(looks_like_sql("INSERT INTO table VALUES (1)"));
        assert!(looks_like_sql("UPDATE users SET name = 'test'"));
        assert!(looks_like_sql("DELETE FROM users"));
        assert!(looks_like_sql("CREATE TABLE test (id INT)"));
        assert!(looks_like_sql("  select * from users  "));

        // Should not detect as SQL
        assert!(!looks_like_sql("plain text"));
        assert!(!looks_like_sql("SELECT")); // Keyword alone without space
        assert!(!looks_like_sql(r#"{"select": "value"}"#));
    }

    #[test]
    fn test_detect_content_type() {
        // JSON detection
        assert_eq!(
            detect_content_type(r#"{"key": "value"}"#),
            ContentType::Json
        );
        assert_eq!(detect_content_type("[1, 2, 3]"), ContentType::Json);

        // HTML detection
        assert_eq!(
            detect_content_type("<html><body>test</body></html>"),
            ContentType::Html
        );
        assert_eq!(
            detect_content_type("<!DOCTYPE html><html></html>"),
            ContentType::Html
        );

        // SQL detection
        assert_eq!(
            detect_content_type("SELECT * FROM users"),
            ContentType::Sql
        );

        // Plain text
        assert_eq!(detect_content_type("hello world"), ContentType::Plain);
        assert_eq!(detect_content_type("123"), ContentType::Plain);
    }

    #[test]
    fn test_content_type_language_name() {
        assert_eq!(ContentType::Json.language_name(), Some("json"));
        assert_eq!(ContentType::Html.language_name(), Some("html"));
        assert_eq!(ContentType::Sql.language_name(), Some("sql"));
        assert_eq!(ContentType::Plain.language_name(), None);
    }

    #[test]
    fn test_is_uuid() {
        // Valid UUIDs
        assert!(is_uuid("550e8400-e29b-41d4-a716-446655440000"));
        assert!(is_uuid("AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE"));
        assert!(is_uuid("  550e8400-e29b-41d4-a716-446655440000  ")); // with whitespace

        // Invalid UUIDs
        assert!(!is_uuid("550e8400-e29b-41d4-a716-44665544000")); // too short
        assert!(!is_uuid("550e8400-e29b-41d4-a716-4466554400000")); // too long
        assert!(!is_uuid("550e8400e29b41d4a716446655440000")); // no hyphens
        assert!(!is_uuid("550e8400-e29b-41d4-a716")); // incomplete
        assert!(!is_uuid("GGGGGGGG-GGGG-GGGG-GGGG-GGGGGGGGGGGG")); // invalid hex
        assert!(!is_uuid("hello-world")); // not a UUID
        assert!(!is_uuid("")); // empty
    }

    #[test]
    fn test_truncate_uuid() {
        // UUID should be truncated
        assert_eq!(
            truncate_uuid("550e8400-e29b-41d4-a716-446655440000", 12),
            "550e8400..."
        );

        // With different max length
        assert_eq!(
            truncate_uuid("550e8400-e29b-41d4-a716-446655440000", 8),
            "550e840..."
        );

        // Non-UUID should not be truncated
        assert_eq!(truncate_uuid("hello world", 12), "hello world");

        // Short values should not be truncated
        assert_eq!(truncate_uuid("abc", 12), "abc");
    }
}
