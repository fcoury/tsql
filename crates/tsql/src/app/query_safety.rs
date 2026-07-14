//! Conservative query classification used by write confirmations and saved
//! read-only connections.

use super::sql_lexer::{code_words_with_depth, single_statement};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QuerySafety {
    ReadOnly,
    Write,
    Destructive,
    Unknown,
}

impl QuerySafety {
    pub(crate) fn requires_write_access(self) -> bool {
        self != Self::ReadOnly
    }
}

pub(crate) fn classify_postgres(source: &str) -> QuerySafety {
    let single = single_statement(source);
    let words = match code_words_with_depth(source) {
        Ok(words) => words,
        Err(_) => return QuerySafety::Unknown,
    };
    let Some(first) = words.first().map(|(word, _)| word.as_str()) else {
        return QuerySafety::ReadOnly;
    };

    if words
        .iter()
        .any(|(word, _)| word == "DROP" || word == "TRUNCATE")
    {
        return QuerySafety::Destructive;
    }

    for (index, (word, write_depth)) in words.iter().enumerate() {
        if matches!(word.as_str(), "UPDATE" | "DELETE")
            && !words[index + 1..]
                .iter()
                .any(|(word, depth)| word == "WHERE" && depth == write_depth)
        {
            return QuerySafety::Destructive;
        }
    }

    let has_write = words.iter().any(|(word, _)| {
        matches!(
            word.as_str(),
            "INSERT"
                | "UPDATE"
                | "DELETE"
                | "MERGE"
                | "COPY"
                | "CREATE"
                | "ALTER"
                | "DROP"
                | "TRUNCATE"
                | "GRANT"
                | "REVOKE"
                | "COMMENT"
                | "VACUUM"
                | "ANALYZE"
                | "REINDEX"
                | "CLUSTER"
                | "REFRESH"
        )
    });
    if has_write {
        return QuerySafety::Write;
    }

    if single.is_err() {
        return QuerySafety::Unknown;
    }

    match first {
        "SELECT" | "TABLE" | "VALUES" | "SHOW" => QuerySafety::ReadOnly,
        "WITH" if !has_write => QuerySafety::ReadOnly,
        "EXPLAIN" if !words.iter().any(|(word, _)| word == "ANALYZE") => QuerySafety::ReadOnly,
        _ => QuerySafety::Unknown,
    }
}

pub(crate) fn bounded_dml_returning(source: &str, max_rows: usize) -> Option<String> {
    let statement = single_statement(source).ok()?;
    let words = code_words_with_depth(statement).ok()?;
    let (first, write_depth) = words.first()?;
    if !matches!(first.as_str(), "INSERT" | "UPDATE" | "DELETE" | "MERGE")
        || !words
            .iter()
            .any(|(word, depth)| word == "RETURNING" && depth == write_depth)
    {
        return None;
    }

    Some(format!(
        "WITH __tsql_affected_rows AS ({statement}) \
         SELECT * FROM __tsql_affected_rows LIMIT {}",
        max_rows.saturating_add(1)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_reads_writes_and_destructive_statements() {
        assert_eq!(classify_postgres("SELECT 1"), QuerySafety::ReadOnly);
        assert_eq!(
            classify_postgres("WITH value AS (SELECT 1) SELECT * FROM value"),
            QuerySafety::ReadOnly
        );
        assert_eq!(
            classify_postgres("UPDATE users SET active = true WHERE id = 1"),
            QuerySafety::Write
        );
        assert_eq!(
            classify_postgres("DELETE FROM users"),
            QuerySafety::Destructive
        );
        assert_eq!(
            classify_postgres(
                "UPDATE users SET active = EXISTS (SELECT 1 FROM audit WHERE audit.id = users.id)"
            ),
            QuerySafety::Destructive
        );
        assert_eq!(
            classify_postgres(
                "UPDATE users SET active = EXISTS (SELECT 1 FROM audit WHERE audit.id = users.id) WHERE users.id = 1"
            ),
            QuerySafety::Write
        );
        assert_eq!(
            classify_postgres("DROP TABLE users"),
            QuerySafety::Destructive
        );
        assert_eq!(
            classify_postgres("SELECT 'DROP TABLE users'"),
            QuerySafety::ReadOnly
        );
    }

    #[test]
    fn fails_closed_for_ambiguous_or_procedural_sql() {
        for query in [
            "CALL mutate_users()",
            "DO $$ BEGIN DELETE FROM users; END $$",
            "SET default_transaction_read_only = off",
            "SELECT 1; SELECT 2",
            "SELECT 'unterminated",
        ] {
            assert_eq!(classify_postgres(query), QuerySafety::Unknown, "{query}");
        }
    }

    #[test]
    fn explain_analyze_of_a_write_requires_write_access() {
        assert_eq!(
            classify_postgres("EXPLAIN ANALYZE DELETE FROM users WHERE id = 1"),
            QuerySafety::Write
        );
        assert_eq!(
            classify_postgres("EXPLAIN DELETE FROM users WHERE id = 1"),
            QuerySafety::Write
        );
    }

    #[test]
    fn bounds_direct_dml_returning_without_rewriting_other_statements() {
        assert_eq!(
            bounded_dml_returning(
                "UPDATE users SET active = false WHERE id = 1 RETURNING id, active;",
                2_000
            )
            .as_deref(),
            Some(
                "WITH __tsql_affected_rows AS (UPDATE users SET active = false WHERE id = 1 RETURNING id, active) SELECT * FROM __tsql_affected_rows LIMIT 2001"
            )
        );
        assert!(bounded_dml_returning("UPDATE users SET active = false", 2_000).is_none());
        assert!(bounded_dml_returning(
            "WITH changed AS (UPDATE users SET active = false RETURNING *) SELECT * FROM changed",
            2_000
        )
        .is_none());
    }
}
