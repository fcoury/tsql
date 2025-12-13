//! Integration tests for tsql.

mod common;

use common::{TestDatabase, get_test_database_url};

/// Test that we can connect to a PostgreSQL database successfully.
#[tokio::test]
async fn test_connection_success() {
    let Some(admin_url) = get_test_database_url() else {
        eprintln!("Skipping: TEST_DATABASE_URL not set");
        return;
    };

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
    let Some(admin_url) = get_test_database_url() else {
        eprintln!("Skipping: TEST_DATABASE_URL not set");
        return;
    };

    let test_db = TestDatabase::new(&admin_url).await.unwrap();
    let client = test_db.connect().await.unwrap();

    // Execute an invalid query
    let result = client.simple_query("SELECT * FROM nonexistent_table_xyz").await;

    assert!(result.is_err(), "Query should fail");

    let err = result.unwrap_err();
    let error_message = tsql::util::format_pg_error(&err);

    // Verify the error message contains useful information, not just "db error"
    assert!(
        error_message.contains("nonexistent_table_xyz") || error_message.contains("does not exist"),
        "Error message should mention the table name or indicate it doesn't exist. Got: {}",
        error_message
    );
}

/// Test that we can execute DDL statements.
#[tokio::test]
async fn test_ddl_create_table() {
    let Some(admin_url) = get_test_database_url() else {
        eprintln!("Skipping: TEST_DATABASE_URL not set");
        return;
    };

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
    let Some(admin_url) = get_test_database_url() else {
        eprintln!("Skipping: TEST_DATABASE_URL not set");
        return;
    };

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
    let Some(admin_url) = get_test_database_url() else {
        eprintln!("Skipping: TEST_DATABASE_URL not set");
        return;
    };

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
    let Some(admin_url) = get_test_database_url() else {
        eprintln!("Skipping: TEST_DATABASE_URL not set");
        return;
    };

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
    let Some(admin_url) = get_test_database_url() else {
        eprintln!("Skipping: TEST_DATABASE_URL not set");
        return;
    };

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
