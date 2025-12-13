//! Test utilities for tsql integration tests.
//!
//! Provides a `TestDatabase` struct that creates a unique PostgreSQL database
//! for each test and automatically drops it when the test completes.

use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_postgres::{Client, NoTls};
use uuid::Uuid;

/// A test database that is automatically cleaned up when dropped.
///
/// Each test gets its own unique database to enable parallel test execution.
pub struct TestDatabase {
    /// The connection URL for the test database
    pub url: String,
    /// The name of the test database
    pub db_name: String,
    /// Connection to the admin database (used for cleanup)
    admin_client: Arc<Mutex<Client>>,
    /// Tokio runtime handle for cleanup
    rt: tokio::runtime::Handle,
}

impl TestDatabase {
    /// Creates a new test database with a unique name.
    ///
    /// # Arguments
    /// * `admin_url` - Connection URL to an admin database with CREATEDB privileges
    ///
    /// # Returns
    /// A `TestDatabase` instance with a newly created database
    pub async fn new(admin_url: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // Connect to the admin database
        let (admin_client, connection) = tokio_postgres::connect(admin_url, NoTls).await?;

        // Spawn the connection handler
        let rt = tokio::runtime::Handle::current();
        rt.spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("Admin connection error: {}", e);
            }
        });

        // Generate a unique database name
        let db_name = format!("tsql_test_{}", Uuid::new_v4().to_string().replace('-', "_"));

        // Create the test database
        admin_client
            .execute(&format!("CREATE DATABASE {}", db_name), &[])
            .await?;

        // Build the connection URL for the test database
        let url = build_test_url(admin_url, &db_name)?;

        Ok(Self {
            url,
            db_name,
            admin_client: Arc::new(Mutex::new(admin_client)),
            rt,
        })
    }

    /// Creates a new client connection to the test database.
    pub async fn connect(&self) -> Result<Client, tokio_postgres::Error> {
        let (client, connection) = tokio_postgres::connect(&self.url, NoTls).await?;

        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("Test database connection error: {}", e);
            }
        });

        Ok(client)
    }

}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        let admin_client = self.admin_client.clone();
        let db_name = self.db_name.clone();

        // Use block_on to run the cleanup synchronously in Drop
        // This ensures the database is dropped even if the test panics
        let _ = self.rt.block_on(async move {
            let client = admin_client.lock().await;

            // Terminate connections
            let _ = client
                .execute(
                    &format!(
                        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{}'",
                        db_name
                    ),
                    &[],
                )
                .await;

            // Drop database
            let _ = client
                .execute(&format!("DROP DATABASE IF EXISTS {}", db_name), &[])
                .await;
        });
    }
}

/// Builds a connection URL for a specific database from an admin URL.
fn build_test_url(
    admin_url: &str,
    db_name: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    // Parse the admin URL and replace the database name
    // Format: postgres://user:pass@host:port/database
    if let Some(last_slash) = admin_url.rfind('/') {
        let base = &admin_url[..last_slash + 1];
        Ok(format!("{}{}", base, db_name))
    } else {
        Err("Invalid database URL format".into())
    }
}

/// Gets the test database URL from environment variables.
///
/// Tries `TEST_DATABASE_URL` first, then falls back to `DATABASE_URL`.
/// Loads from `.env` file if available.
pub fn get_test_database_url() -> Option<String> {
    // Try to load .env file (ignore errors if it doesn't exist)
    let _ = dotenvy::dotenv();

    std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .ok()
}

/// Skips a test if no database URL is configured.
///
/// Returns `Some(url)` if a database is available, `None` otherwise.
/// Prints a message when skipping.
#[allow(dead_code)]
pub fn require_database() -> Option<String> {
    match get_test_database_url() {
        Some(url) => Some(url),
        None => {
            eprintln!("Skipping test: TEST_DATABASE_URL not set");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_database_creation_and_cleanup() {
        let Some(admin_url) = get_test_database_url() else {
            eprintln!("Skipping: TEST_DATABASE_URL not set");
            return;
        };

        // Create a test database
        let test_db = TestDatabase::new(&admin_url).await.unwrap();
        let db_name = test_db.db_name.clone();

        // Verify we can connect to it
        let client = test_db.connect().await.unwrap();
        let rows = client.query("SELECT 1 as value", &[]).await.unwrap();
        assert_eq!(rows.len(), 1);

        // Drop the test database
        drop(test_db);

        // Verify the database was dropped
        let (admin_client, connection) = tokio_postgres::connect(&admin_url, NoTls).await.unwrap();
        tokio::spawn(async move {
            let _ = connection.await;
        });

        let result = admin_client
            .query(
                "SELECT 1 FROM pg_database WHERE datname = $1",
                &[&db_name],
            )
            .await
            .unwrap();

        assert!(result.is_empty(), "Database should have been dropped");
    }
}
