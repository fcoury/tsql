//! PostgreSQL session for one headless statement.
//!
//! Read-only mode enables `default_transaction_read_only` and then runs the
//! statement inside `BEGIN READ ONLY`. That is the database-level backstop for
//! statements the shared classifier accepts (a `SELECT` that calls a writing
//! function, for example). Timeouts are `statement_timeout` on the server.

use std::sync::Arc;
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use tokio_postgres::{Client, NoTls};
use tokio_postgres_rustls_improved::MakeRustlsConnect;
use webpki_roots::TLS_SERVER_ROOTS;

use crate::config::SslMode;
use crate::util::format_pg_error;

use super::output::{messages_to_sets, ResultSet};
use super::{format_timeout, ExecError};

const CLIENT_TIMEOUT_GRACE: Duration = Duration::from_secs(2);

#[derive(Clone, Copy)]
enum TlsKind {
    Plain,
    Insecure,
    Verify,
}

struct Session {
    client: Client,
    tls: TlsKind,
}

struct RanQuery {
    result: Result<Vec<tokio_postgres::SimpleQueryMessage>, ExecError>,
    /// False when the client-side backstop dropped the in-flight query. The
    /// connection may still be busy, so it must not be reused.
    reuse_client: bool,
}

pub(crate) async fn execute(
    url: &str,
    sql: &str,
    read_only: bool,
    timeout: Option<Duration>,
) -> Result<Vec<ResultSet>, ExecError> {
    let session = connect(url).await?;
    if read_only {
        run_read_only(&session, sql, timeout).await
    } else {
        run_writable(&session, sql, timeout).await
    }
}

async fn run_read_only(
    session: &Session,
    sql: &str,
    timeout: Option<Duration>,
) -> Result<Vec<ResultSet>, ExecError> {
    batch(&session.client, "SET default_transaction_read_only = on").await?;
    batch(&session.client, "BEGIN READ ONLY").await?;
    if let Some(limit) = timeout {
        if let Err(error) = set_statement_timeout(&session.client, limit, true).await {
            let _ = session.client.batch_execute("ROLLBACK").await;
            return Err(error);
        }
    }

    let ran = simple_query_bounded(session, sql, timeout).await;
    if ran.reuse_client {
        if ran.result.is_ok() {
            if session.client.batch_execute("COMMIT").await.is_err() {
                let _ = session.client.batch_execute("ROLLBACK").await;
            }
        } else {
            let _ = session.client.batch_execute("ROLLBACK").await;
        }
    }
    Ok(messages_to_sets(ran.result?))
}

async fn run_writable(
    session: &Session,
    sql: &str,
    timeout: Option<Duration>,
) -> Result<Vec<ResultSet>, ExecError> {
    if let Some(limit) = timeout {
        set_statement_timeout(&session.client, limit, false).await?;
    }
    let ran = simple_query_bounded(session, sql, timeout).await;
    Ok(messages_to_sets(ran.result?))
}

async fn batch(client: &Client, sql: &str) -> Result<(), ExecError> {
    client
        .batch_execute(sql)
        .await
        .map_err(|error| map_statement_error(error, None))
}

async fn set_statement_timeout(
    client: &Client,
    timeout: Duration,
    local: bool,
) -> Result<(), ExecError> {
    let millis = statement_timeout_ms(timeout)?;
    let scope = if local { "LOCAL " } else { "" };
    client
        .batch_execute(&format!("SET {scope}statement_timeout = {millis}"))
        .await
        .map_err(|error| map_statement_error(error, None))
}

fn statement_timeout_ms(timeout: Duration) -> Result<i32, ExecError> {
    let millis = timeout.as_millis();
    if millis == 0 {
        return Err(ExecError::Usage(
            "timeout must be greater than zero".to_string(),
        ));
    }
    i32::try_from(millis).map_err(|_| {
        ExecError::Usage("timeout is too large for PostgreSQL statement_timeout".to_string())
    })
}

async fn simple_query_bounded(session: &Session, sql: &str, timeout: Option<Duration>) -> RanQuery {
    let pending = session.client.simple_query(sql);
    let Some(limit) = timeout else {
        return RanQuery {
            result: pending
                .await
                .map_err(|error| map_statement_error(error, None)),
            reuse_client: true,
        };
    };

    let grace = limit.saturating_add(CLIENT_TIMEOUT_GRACE);
    let token = session.client.cancel_token();
    match tokio::time::timeout(grace, pending).await {
        Ok(Ok(messages)) => RanQuery {
            result: Ok(messages),
            reuse_client: true,
        },
        Ok(Err(error)) => RanQuery {
            result: Err(map_statement_error(error, Some(limit))),
            reuse_client: true,
        },
        Err(_) => {
            session.tls.cancel(token).await;
            RanQuery {
                result: Err(ExecError::Timeout(format!(
                    "timed out after {}",
                    format_timeout(limit)
                ))),
                reuse_client: false,
            }
        }
    }
}

fn map_statement_error(error: tokio_postgres::Error, timeout: Option<Duration>) -> ExecError {
    if let Some(limit) = timeout {
        if is_statement_timeout(&error) {
            return ExecError::Timeout(format!(
                "timed out after {}: {}",
                format_timeout(limit),
                format_pg_error(&error)
            ));
        }
    }
    if is_read_only_violation(&error) {
        return ExecError::Refused(format!(
            "read-only transaction refused the statement: {}",
            format_pg_error(&error)
        ));
    }
    ExecError::Query(format_pg_error(&error))
}

fn is_statement_timeout(error: &tokio_postgres::Error) -> bool {
    match error.as_db_error() {
        Some(db) => db.code().code() == "57014" || db.message().contains("statement timeout"),
        None => false,
    }
}

fn is_read_only_violation(error: &tokio_postgres::Error) -> bool {
    match error.as_db_error() {
        Some(db) => db.code().code() == "25006" || db.message().contains("read-only transaction"),
        None => false,
    }
}

async fn connect(conn: &str) -> Result<Session, ExecError> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    match ssl_mode_of(conn)? {
        SslMode::Disable => connect_with(conn, TlsKind::Plain).await,
        SslMode::Require => connect_with(conn, TlsKind::Insecure).await,
        SslMode::Prefer => match connect_with(conn, TlsKind::Insecure).await {
            Ok(session) => Ok(session),
            Err(_) => connect_with(conn, TlsKind::Plain).await,
        },
        SslMode::VerifyCa | SslMode::VerifyFull => connect_with(conn, TlsKind::Verify).await,
    }
}

async fn connect_with(conn: &str, tls: TlsKind) -> Result<Session, ExecError> {
    // Each TLS connector produces a different connection stream type, so the
    // background task has to be spawned inside the matching branch.
    let client = match tls {
        TlsKind::Plain => {
            let (client, connection) = tokio_postgres::connect(conn, NoTls)
                .await
                .map_err(connect_error)?;
            tokio::spawn(async move {
                let _ = connection.await;
            });
            client
        }
        TlsKind::Insecure => {
            let (client, connection) = tokio_postgres::connect(conn, insecure_tls())
                .await
                .map_err(connect_error)?;
            tokio::spawn(async move {
                let _ = connection.await;
            });
            client
        }
        TlsKind::Verify => {
            let (client, connection) = tokio_postgres::connect(conn, verified_tls())
                .await
                .map_err(connect_error)?;
            tokio::spawn(async move {
                let _ = connection.await;
            });
            client
        }
    };
    Ok(Session { client, tls })
}

fn connect_error(error: tokio_postgres::Error) -> ExecError {
    ExecError::Connect(format!("could not connect: {}", format_pg_error(&error)))
}

pub(crate) fn ssl_mode_of(conn: &str) -> Result<SslMode, ExecError> {
    if conn.contains("://") {
        return sslmode_from_url(conn);
    }
    sslmode_from_keywords(conn)
}

fn sslmode_from_url(conn: &str) -> Result<SslMode, ExecError> {
    let url = url::Url::parse(conn)
        .map_err(|error| ExecError::Usage(format!("invalid connection URL: {error}")))?;
    for (key, value) in url.query_pairs() {
        if key.eq_ignore_ascii_case("sslmode") {
            return parse_sslmode(&value);
        }
    }
    Ok(SslMode::Disable)
}

fn sslmode_from_keywords(conn: &str) -> Result<SslMode, ExecError> {
    let parts: Vec<&str> = conn.split_whitespace().collect();
    let mut index = 0;
    while index < parts.len() {
        let part = parts[index];
        if let Some((key, value)) = part.split_once('=') {
            if key.eq_ignore_ascii_case("sslmode") {
                let actual = if value.is_empty() && index + 1 < parts.len() {
                    index += 1;
                    parts[index]
                } else {
                    value
                };
                return parse_sslmode(actual);
            }
        } else if index + 2 < parts.len()
            && parts[index + 1] == "="
            && part.eq_ignore_ascii_case("sslmode")
        {
            return parse_sslmode(parts[index + 2]);
        }
        index += 1;
    }
    Ok(SslMode::Disable)
}

fn parse_sslmode(value: &str) -> Result<SslMode, ExecError> {
    SslMode::parse(value).ok_or_else(|| {
        ExecError::Usage(format!(
            "unsupported sslmode '{value}' (expected disable, prefer, require, verify-ca, or verify-full)"
        ))
    })
}

impl TlsKind {
    async fn cancel(self, token: tokio_postgres::CancelToken) {
        let result = match self {
            Self::Plain => token.cancel_query(NoTls).await,
            Self::Insecure => token.cancel_query(insecure_tls()).await,
            Self::Verify => token.cancel_query(verified_tls()).await,
        };
        let _ = result;
    }
}

#[derive(Debug)]
struct SkipServerVerification(Arc<CryptoProvider>);

impl SkipServerVerification {
    fn new() -> Arc<Self> {
        Arc::new(Self(
            Arc::new(rustls::crypto::aws_lc_rs::default_provider()),
        ))
    }
}

impl ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn insecure_tls() -> MakeRustlsConnect {
    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(SkipServerVerification::new())
        .with_no_client_auth();
    MakeRustlsConnect::new(config)
}

fn verified_tls() -> MakeRustlsConnect {
    let mut root_store = RootCertStore::empty();
    root_store.extend(TLS_SERVER_ROOTS.iter().cloned());
    let config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    MakeRustlsConnect::new(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sslmode_comes_from_the_url_or_keywords_and_defaults_to_disable() {
        assert_eq!(
            ssl_mode_of("postgres://localhost/app").unwrap(),
            SslMode::Disable
        );
        assert_eq!(
            ssl_mode_of("postgres://localhost/app?sslmode=require").unwrap(),
            SslMode::Require
        );
        assert_eq!(
            ssl_mode_of("host=localhost sslmode=verify-full dbname=app").unwrap(),
            SslMode::VerifyFull
        );
        assert!(ssl_mode_of("postgres://localhost/app?sslmode=nope").is_err());
    }

    #[test]
    fn statement_timeout_is_milliseconds_within_postgres_range() {
        assert_eq!(
            statement_timeout_ms(Duration::from_secs(30)).unwrap(),
            30_000
        );
        assert_eq!(statement_timeout_ms(Duration::from_millis(5)).unwrap(), 5);
        assert!(statement_timeout_ms(Duration::from_millis(u64::MAX)).is_err());
    }
}
