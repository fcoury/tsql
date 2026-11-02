//! Integration tests for tsql.

mod common;

use std::time::{Duration, Instant};

use common::{require_test_database_url, TestDatabase};
use tsql::exec::{ExecError, ExecRequest, OutputFormat};

async fn exec(
    url: &str,
    sql: &str,
    read_only: bool,
    timeout: Option<Duration>,
    format: OutputFormat,
) -> Result<String, ExecError> {
    tsql::exec::run_query(ExecRequest {
        url: url.to_string(),
        sql: sql.to_string(),
        read_only,
        timeout,
        format,
    })
    .await
}

/// Test that we can connect to a PostgreSQL database successfully.
#[tokio::test]
async fn test_connection_success() {
    let admin_url = require_test_database_url();

    let test_db = TestDatabase::new(&admin_url).await.unwrap();
    let client = test_db.connect().await.unwrap();

    // Simple query to verify connection
    let rows = client.query("SELECT 1 as value", &[]).await.unwrap();
    assert_eq!(rows.len(), 1);

    let value: i32 = rows[0].get("value");
    assert_eq!(value, 1);
}

/// Test that connection with invalid URL fails with proper error.
#[tokio::test]
async fn test_connection_failure_invalid_host() {
    let result = tokio_postgres::connect(
        "postgres://user:pass@invalid-host-that-does-not-exist:5432/db",
        tokio_postgres::NoTls,
    )
    .await;

    assert!(result.is_err(), "Connection to invalid host should fail");
}

/// Test that query errors are properly reported (not just "db error").
#[tokio::test]
async fn test_query_error_message() {
    let admin_url = require_test_database_url();

    let test_db = TestDatabase::new(&admin_url).await.unwrap();
    let client = test_db.connect().await.unwrap();

    // Execute an invalid query
    let result = client
        .simple_query("SELECT * FROM nonexistent_table_xyz")
        .await;

    assert!(result.is_err(), "Query should fail");

    let err = result.unwrap_err();
    let error_message = tsql::util::format_pg_error(&err);

    // Verify the error message contains useful information, not just "db error"
    assert!(
        error_message.contains("nonexistent_table_xyz") || error_message.contains("does not exist"),
        "Error message should mention the table name or indicate it doesn't exist. Got: {}",
        error_message
    );
    assert!(
        error_message.contains("[42P01]"),
        "Error message should include the undefined-table SQLSTATE. Got: {error_message}"
    );
    assert!(
        error_message.contains("POSITION:"),
        "Error message should include the query position. Got: {error_message}"
    );

    let err = client
        .simple_query(
            "DO $$ BEGIN RAISE EXCEPTION 'notebook diagnostic' USING ERRCODE = '22023', DETAIL = 'extra context', HINT = 'check the input'; END $$",
        )
        .await
        .unwrap_err();
    let error_message = tsql::util::format_pg_error(&err);
    assert!(error_message.contains("[22023]"), "{error_message}");
    assert!(
        error_message.contains("DETAIL: extra context"),
        "{error_message}"
    );
    assert!(
        error_message.contains("HINT: check the input"),
        "{error_message}"
    );
}

/// Test that we can execute DDL statements.
#[tokio::test]
async fn test_ddl_create_table() {
    let admin_url = require_test_database_url();

    let test_db = TestDatabase::new(&admin_url).await.unwrap();
    let client = test_db.connect().await.unwrap();

    // Create a table
    client
        .execute(
            "CREATE TABLE test_table (id SERIAL PRIMARY KEY, name TEXT NOT NULL)",
            &[],
        )
        .await
        .unwrap();

    // Verify table exists
    let rows = client
        .query(
            "SELECT table_name FROM information_schema.tables WHERE table_name = 'test_table'",
            &[],
        )
        .await
        .unwrap();

    assert_eq!(rows.len(), 1);
}

/// Test insert and select operations.
#[tokio::test]
async fn test_insert_and_select() {
    let admin_url = require_test_database_url();

    let test_db = TestDatabase::new(&admin_url).await.unwrap();
    let client = test_db.connect().await.unwrap();

    // Create table and insert data
    client
        .execute("CREATE TABLE users (id SERIAL PRIMARY KEY, name TEXT)", &[])
        .await
        .unwrap();

    client
        .execute("INSERT INTO users (name) VALUES ('Alice'), ('Bob')", &[])
        .await
        .unwrap();

    // Query the data
    let rows = client
        .query("SELECT id, name FROM users ORDER BY id", &[])
        .await
        .unwrap();

    assert_eq!(rows.len(), 2);

    let name1: &str = rows[0].get("name");
    let name2: &str = rows[1].get("name");

    assert_eq!(name1, "Alice");
    assert_eq!(name2, "Bob");
}

/// Test that simple_query works and returns results.
#[tokio::test]
async fn test_simple_query() {
    let admin_url = require_test_database_url();

    let test_db = TestDatabase::new(&admin_url).await.unwrap();
    let client = test_db.connect().await.unwrap();

    // simple_query is what tsql uses for query execution
    let messages = client
        .simple_query("SELECT 1 as a, 2 as b; SELECT 3 as c;")
        .await
        .unwrap();

    // Count the rows returned
    let row_count = messages
        .iter()
        .filter(|m| matches!(m, tokio_postgres::SimpleQueryMessage::Row(_)))
        .count();

    assert_eq!(row_count, 2, "Should have 2 rows from 2 SELECT statements");
}

/// Test that multiple tests can run in parallel with isolated databases.
#[tokio::test]
async fn test_parallel_isolation_1() {
    let admin_url = require_test_database_url();

    let test_db = TestDatabase::new(&admin_url).await.unwrap();
    let client = test_db.connect().await.unwrap();

    // Create a table unique to this test
    client
        .execute("CREATE TABLE parallel_test_1 (id INT)", &[])
        .await
        .unwrap();

    // Small delay to simulate work
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Verify our table exists
    let rows = client
        .query(
            "SELECT 1 FROM information_schema.tables WHERE table_name = 'parallel_test_1'",
            &[],
        )
        .await
        .unwrap();

    assert_eq!(rows.len(), 1);
}

#[tokio::test]
async fn test_parallel_isolation_2() {
    let admin_url = require_test_database_url();

    let test_db = TestDatabase::new(&admin_url).await.unwrap();
    let client = test_db.connect().await.unwrap();

    // Create a different table
    client
        .execute("CREATE TABLE parallel_test_2 (id INT)", &[])
        .await
        .unwrap();

    // Small delay to simulate work
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Verify our table exists and the other test's table doesn't
    let rows = client
        .query(
            "SELECT 1 FROM information_schema.tables WHERE table_name = 'parallel_test_2'",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);

    let rows = client
        .query(
            "SELECT 1 FROM information_schema.tables WHERE table_name = 'parallel_test_1'",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(rows.len(), 0, "Should not see other test's table");
}

/// `tsql exec --read-only` returns JSON for a SELECT and refuses a DELETE
/// before the statement runs.
#[tokio::test]
async fn exec_read_only_returns_json_and_refuses_delete() {
    let admin_url = require_test_database_url();
    let test_db = TestDatabase::new(&admin_url).await.unwrap();
    let client = test_db.connect().await.unwrap();
    client
        .batch_execute(
            "CREATE TABLE agents (id int, name text);
             INSERT INTO agents VALUES (1, 'ada'), (2, NULL);",
        )
        .await
        .unwrap();

    let json = exec(
        &test_db.url,
        "SELECT id, name FROM agents ORDER BY id",
        true,
        Some(Duration::from_secs(30)),
        OutputFormat::Json,
    )
    .await
    .unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value[0]["id"], "1");
    assert_eq!(value[0]["name"], "ada");
    assert_eq!(value[1]["id"], "2");
    assert!(value[1]["name"].is_null(), "{json}");

    let table = exec(
        &test_db.url,
        "SELECT id, name FROM agents ORDER BY id",
        true,
        Some(Duration::from_secs(30)),
        OutputFormat::Table,
    )
    .await
    .unwrap();
    assert!(table.contains("ada"), "{table}");
    assert!(table.contains("(2 rows)"), "{table}");

    let refused = exec(
        &test_db.url,
        "DELETE FROM agents",
        true,
        Some(Duration::from_secs(30)),
        OutputFormat::Json,
    )
    .await
    .unwrap_err();
    assert_eq!(refused.exit_code(), 2);
    assert!(
        refused.to_string().contains("destructive"),
        "classifier should refuse before execution, got: {refused}"
    );

    let remaining: i64 = client
        .query_one("SELECT count(*) FROM agents", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(remaining, 2);

    let deleted = exec(
        &test_db.url,
        "DELETE FROM agents",
        false,
        Some(Duration::from_secs(30)),
        OutputFormat::Json,
    )
    .await
    .unwrap();
    let deleted: serde_json::Value = serde_json::from_str(&deleted).unwrap();
    assert_eq!(deleted["rows_affected"], 2);
    let remaining: i64 = client
        .query_one("SELECT count(*) FROM agents", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(remaining, 0);
}

/// A SELECT the classifier accepts still cannot write inside the read-only
/// transaction. The same statement writes when `--read-only` is off.
#[tokio::test]
async fn exec_read_only_transaction_blocks_writing_function() {
    let admin_url = require_test_database_url();
    let test_db = TestDatabase::new(&admin_url).await.unwrap();
    let client = test_db.connect().await.unwrap();
    client
        .batch_execute(
            "CREATE TABLE kept (id int);
             INSERT INTO kept VALUES (1);
             CREATE FUNCTION sneak_delete() RETURNS integer LANGUAGE sql AS $$
               DELETE FROM kept;
               SELECT 1;
             $$;",
        )
        .await
        .unwrap();

    let refused = exec(
        &test_db.url,
        "SELECT sneak_delete()",
        true,
        Some(Duration::from_secs(30)),
        OutputFormat::Json,
    )
    .await
    .unwrap_err();
    assert_eq!(refused.exit_code(), 2, "{refused}");
    assert!(
        refused.to_string().contains("read-only transaction"),
        "{refused}"
    );

    let remaining: i64 = client
        .query_one("SELECT count(*) FROM kept", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        remaining, 1,
        "the refused statement must not delete the row"
    );

    exec(
        &test_db.url,
        "SELECT sneak_delete()",
        false,
        Some(Duration::from_secs(30)),
        OutputFormat::Json,
    )
    .await
    .unwrap();
    let remaining: i64 = client
        .query_one("SELECT count(*) FROM kept", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(remaining, 0, "without --read-only the function writes");
}

/// `pg_sleep` is a read, so `--read-only` allows it, and `statement_timeout`
/// cancels it on the server.
#[tokio::test]
async fn exec_statement_timeout_cancels_pg_sleep() {
    let admin_url = require_test_database_url();
    let test_db = TestDatabase::new(&admin_url).await.unwrap();

    let started = Instant::now();
    let error = exec(
        &test_db.url,
        "SELECT pg_sleep(10)",
        true,
        Some(Duration::from_secs(1)),
        OutputFormat::Json,
    )
    .await
    .unwrap_err();
    let elapsed = started.elapsed();

    assert_eq!(error.exit_code(), 4, "{error}");
    let message = error.to_string();
    assert!(
        message.contains("statement timeout") || message.contains("57014"),
        "expected a server-side statement_timeout, got: {message} after {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(8),
        "timeout took too long: {elapsed:?} ({message})"
    );
}
