//! Shared query execution identity and transaction-state tracking.
//!
//! Execution identity is intentionally independent from the active workspace.
//! This lets asynchronous database events prove which editor or notebook cell
//! they belong to before mutating application state.

/// Stable identity for a notebook cell.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CellId(pub(crate) u64);

/// Monotonic identity for one database execution.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ExecutionId(pub(crate) u64);

/// Workspace destination for execution output.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ExecutionTarget {
    Classic,
    Notebook(CellId),
}

/// Immutable identity captured when an asynchronous operation starts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionContext {
    pub id: ExecutionId,
    pub target: ExecutionTarget,
    pub source_revision: u64,
    pub connection_generation: u64,
}

/// How a user-visible query was started.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QueryExecutionKind {
    New,
    Refresh,
}

/// The one database execution allowed to run at a time in the MVP.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActiveExecution {
    pub context: ExecutionContext,
    pub(crate) kind: QueryExecutionKind,
    pub cancelling: bool,
}

/// Conservatively inferred PostgreSQL transaction state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TransactionState {
    /// The connection is known not to be inside a user transaction.
    Idle,
    /// A user transaction is active.
    Active,
    /// A statement failed while a user transaction was active.
    Failed,
    /// The submitted SQL prevents reliable inference.
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TransactionControl {
    Begin,
    Commit,
    CommitAndChain,
    Rollback,
    RollbackAndChain,
    Other,
    Ambiguous,
}

impl TransactionState {
    /// Applies the outcome of a user-submitted statement.
    pub(crate) fn after_execution(self, sql: &str, succeeded: bool) -> Self {
        let control = classify_transaction_control(sql);

        if !succeeded {
            return if self == Self::Active || self == Self::Failed {
                Self::Failed
            } else if control == TransactionControl::Ambiguous {
                Self::Unknown
            } else {
                self
            };
        }

        match control {
            TransactionControl::Begin => Self::Active,
            TransactionControl::Commit | TransactionControl::Rollback => Self::Idle,
            TransactionControl::CommitAndChain | TransactionControl::RollbackAndChain => {
                Self::Active
            }
            TransactionControl::Ambiguous => Self::Unknown,
            TransactionControl::Other => self,
        }
    }
}

pub(crate) fn classify_transaction_control(sql: &str) -> TransactionControl {
    let Ok(statement) = super::sql_lexer::single_statement(sql) else {
        return TransactionControl::Ambiguous;
    };
    let Ok(words) = super::sql_lexer::code_words(statement, 6) else {
        return TransactionControl::Ambiguous;
    };

    match words.as_slice() {
        [first, ..] if first == "BEGIN" => TransactionControl::Begin,
        [first, second, ..] if first == "START" && second == "TRANSACTION" => {
            TransactionControl::Begin
        }
        [first, rest @ ..] if first == "COMMIT" || first == "END" => {
            if contains_and_chain(rest) {
                TransactionControl::CommitAndChain
            } else {
                TransactionControl::Commit
            }
        }
        [first, rest @ ..] if first == "ROLLBACK" || first == "ABORT" => {
            if contains_and_chain(rest) {
                TransactionControl::RollbackAndChain
            } else if is_savepoint_rollback(rest) {
                TransactionControl::Other
            } else {
                TransactionControl::Rollback
            }
        }
        [first, ..] if first == "CALL" => TransactionControl::Ambiguous,
        [] => TransactionControl::Other,
        _ => TransactionControl::Other,
    }
}

fn is_savepoint_rollback(words: &[String]) -> bool {
    let rest = if words
        .first()
        .is_some_and(|word| word == "WORK" || word == "TRANSACTION")
    {
        &words[1..]
    } else {
        words
    };
    rest.first().is_some_and(|word| word == "TO")
}

fn contains_and_chain(words: &[String]) -> bool {
    words
        .windows(2)
        .any(|pair| pair[0] == "AND" && pair[1] == "CHAIN")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_transaction_control_with_comments_and_chain() {
        assert_eq!(
            classify_transaction_control("-- hello\n START TRANSACTION"),
            TransactionControl::Begin
        );
        assert_eq!(
            classify_transaction_control("COMMIT AND CHAIN;"),
            TransactionControl::CommitAndChain
        );
        assert_eq!(
            classify_transaction_control("rollback /* safe */ and chain"),
            TransactionControl::RollbackAndChain
        );
        assert_eq!(
            classify_transaction_control("ROLLBACK TO SAVEPOINT checkpoint"),
            TransactionControl::Other
        );
        assert_eq!(
            classify_transaction_control("ROLLBACK TRANSACTION TO SAVEPOINT checkpoint"),
            TransactionControl::Other
        );
        assert_eq!(
            classify_transaction_control("ABORT WORK TO checkpoint"),
            TransactionControl::Other
        );
    }

    #[test]
    fn classifies_ambiguous_statement_shapes_conservatively() {
        assert_eq!(
            classify_transaction_control("SELECT 1; SELECT 2"),
            TransactionControl::Ambiguous
        );
        assert_eq!(
            classify_transaction_control("CALL may_control_transaction()"),
            TransactionControl::Ambiguous
        );
        assert_eq!(
            classify_transaction_control("SELECT ';' /* ; */;"),
            TransactionControl::Other
        );
        assert_eq!(
            classify_transaction_control("SELECT $tag$hello;world$tag$ AS value;"),
            TransactionControl::Other
        );
        assert_eq!(
            classify_transaction_control("SELECT 1 /* outer ; /* nested ; */ still comment */;"),
            TransactionControl::Other
        );
        assert_eq!(
            classify_transaction_control("SELECT E'it\\'s; -- not a comment' AS value;"),
            TransactionControl::Other
        );
    }

    #[test]
    fn transaction_state_reducer_is_conservative() {
        let state = TransactionState::Idle.after_execution("BEGIN", true);
        assert_eq!(state, TransactionState::Active);
        let state = state.after_execution("SELECT broken", false);
        assert_eq!(state, TransactionState::Failed);
        let state = state.after_execution("ROLLBACK", true);
        assert_eq!(state, TransactionState::Idle);
        let state = state.after_execution("CALL unknown()", true);
        assert_eq!(state, TransactionState::Unknown);
        let state = TransactionState::Idle.after_execution("SELECT $$hello;world$$ AS value", true);
        assert_eq!(state, TransactionState::Idle);
    }
}
