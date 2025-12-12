use std::error::Error as StdError;

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
