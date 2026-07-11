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
    let Some(statement) = single_statement(sql) else {
        return TransactionControl::Ambiguous;
    };
    let words: Vec<String> = statement
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .filter(|word| !word.is_empty())
        .take(5)
        .map(str::to_ascii_uppercase)
        .collect();

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
            } else if rest.first().is_some_and(|word| word == "TO") {
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

fn contains_and_chain(words: &[String]) -> bool {
    words
        .windows(2)
        .any(|pair| pair[0] == "AND" && pair[1] == "CHAIN")
}

/// Returns one comment-aware statement, or `None` for multiple/unterminated SQL.
fn single_statement(sql: &str) -> Option<String> {
    let mut result = String::with_capacity(sql.len());
    let bytes = sql.as_bytes();
    let mut index = 0;
    let mut statement_ended = false;

    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' => {
                if statement_ended {
                    return None;
                }
                let start = index;
                let delimiter = bytes[index];
                index += 1;
                loop {
                    let offset = sql[index..].find(delimiter as char)?;
                    index += offset + 1;
                    if bytes.get(index) == Some(&delimiter) {
                        index += 1;
                    } else {
                        break;
                    }
                }
                result.push_str(&sql[start..index]);
            }
            b'$' => {
                let Some(tag_end_offset) = sql[index + 1..].find('$') else {
                    if statement_ended {
                        return None;
                    }
                    result.push('$');
                    index += 1;
                    continue;
                };
                let tag_end = index + 1 + tag_end_offset;
                let tag = &sql[index + 1..tag_end];
                let mut characters = tag.chars();
                let valid_tag = characters.next().is_none_or(|character| {
                    (character.is_ascii_alphabetic() || character == '_')
                        && characters
                            .all(|character| character.is_ascii_alphanumeric() || character == '_')
                });
                if !valid_tag {
                    if statement_ended {
                        return None;
                    }
                    result.push('$');
                    index += 1;
                    continue;
                }
                if statement_ended {
                    return None;
                }
                let delimiter = &sql[index..=tag_end];
                let content_start = tag_end + 1;
                let close_offset = sql[content_start..].find(delimiter)?;
                let end = content_start + close_offset + delimiter.len();
                result.push_str(&sql[index..end]);
                index = end;
            }
            b'-' if bytes.get(index + 1) == Some(&b'-') => {
                index = sql[index..]
                    .find('\n')
                    .map_or(bytes.len(), |offset| index + offset + 1);
                result.push(' ');
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index += 2;
                let mut depth = 1;
                while index < bytes.len() && depth > 0 {
                    if bytes.get(index..index + 2) == Some(b"/*") {
                        depth += 1;
                        index += 2;
                    } else if bytes.get(index..index + 2) == Some(b"*/") {
                        depth -= 1;
                        index += 2;
                    } else {
                        index += sql[index..].chars().next()?.len_utf8();
                    }
                }
                if depth > 0 {
                    return None;
                }
                result.push(' ');
            }
            b';' => {
                statement_ended = true;
                index += 1;
            }
            _ => {
                let character = sql[index..].chars().next()?;
                if statement_ended && !character.is_whitespace() {
                    return None;
                }
                result.push(character);
                index += character.len_utf8();
            }
        }
    }
    Some(result)
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
