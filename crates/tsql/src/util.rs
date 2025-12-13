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
}
