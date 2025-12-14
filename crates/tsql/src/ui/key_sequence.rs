//! A reusable multi-key sequence handler with timeout-based hint display.
//!
//! This module provides a generic system for handling multi-key sequences like `gg`, `gc`, etc.
//! It tracks the pending key state and can trigger a hint popup after a configurable timeout.

use std::time::{Duration, Instant};

/// The type of pending key sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingKey {
    /// The `g` (goto) key prefix
    G,
    /// Schema table actions (started by Enter on a table in the schema panel)
    SchemaTable,
    // Future: Add more pending keys here (e.g., Z for fold commands)
}

impl PendingKey {
    /// Returns the display character for this pending key
    pub fn display_char(&self) -> char {
        match self {
            PendingKey::G => 'g',
            PendingKey::SchemaTable => '⏎',
        }
    }
}

/// Result of processing a key in a sequence
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySequenceResult<C = ()> {
    /// No sequence active, key was not consumed
    NotConsumed,
    /// Key started a new sequence (waiting for next key)
    Started(PendingKey),
    /// Sequence completed with an action
    Completed(KeySequenceCompletion<C>),
    /// Sequence was cancelled (invalid second key)
    Cancelled,
}

/// A completed key sequence, including optional caller-provided context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeySequenceCompletion<C = ()> {
    pub action: KeySequenceAction,
    pub context: Option<C>,
}

/// Actions that can result from completing a key sequence
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySequenceAction {
    /// Go to first row (grid) / document start (editor)
    GotoFirst,
    /// Go to the query editor
    GotoEditor,
    /// Go to the connections sidebar
    GotoConnections,
    /// Go to the tables/schema sidebar
    GotoTables,
    /// Go to the results grid
    GotoResults,

    // ─────────────────────────────────────────────────────────────────────
    // Schema panel table templates (Enter + key)
    // ─────────────────────────────────────────────────────────────────────
    SchemaTableSelect,
    SchemaTableInsert,
    SchemaTableUpdate,
    SchemaTableDelete,
    SchemaTableName,
}

/// Handles multi-key sequences with timeout-based hint display.
#[derive(Debug)]
pub struct KeySequenceHandlerWithContext<C> {
    /// Current pending key, if any
    pending: Option<PendingKey>,
    /// When the pending key was pressed
    pending_since: Option<Instant>,
    /// Timeout before showing the hint popup (in milliseconds)
    timeout_ms: u64,
    /// Whether the hint popup should be shown
    hint_shown: bool,
    /// Optional caller-provided context carried with the pending sequence.
    pending_context: Option<C>,
}

/// Backwards-compatible default handler with no context payload.
pub type KeySequenceHandler = KeySequenceHandlerWithContext<()>;

impl Default for KeySequenceHandlerWithContext<()> {
    fn default() -> Self {
        Self::new(500)
    }
}

impl<C> KeySequenceHandlerWithContext<C> {
    /// Creates a new handler with the specified timeout in milliseconds.
    pub fn new(timeout_ms: u64) -> Self {
        Self {
            pending: None,
            pending_since: None,
            timeout_ms,
            hint_shown: false,
            pending_context: None,
        }
    }

    /// Returns the current pending key, if any.
    pub fn pending(&self) -> Option<PendingKey> {
        self.pending
    }

    /// Returns true if a hint popup should be shown.
    ///
    /// This becomes true when:
    /// 1. There is a pending key
    /// 2. The timeout has elapsed since the key was pressed
    /// 3. The hint hasn't already been marked as shown (to avoid re-triggering)
    pub fn should_show_hint(&self) -> bool {
        // Once hint is shown, keep returning true until sequence completes/cancels
        if self.hint_shown {
            return self.pending.is_some();
        }
        if let (Some(_), Some(since)) = (self.pending, self.pending_since) {
            since.elapsed() >= Duration::from_millis(self.timeout_ms)
        } else {
            false
        }
    }

    /// Returns true if the hint is currently being shown.
    pub fn is_hint_shown(&self) -> bool {
        self.hint_shown
    }

    /// Marks the hint as shown. Call this after rendering the hint popup.
    pub fn mark_hint_shown(&mut self) {
        self.hint_shown = true;
    }

    /// Starts a new key sequence with caller-provided context.
    pub fn start_with_context(&mut self, key: PendingKey, context: C) {
        self.pending = Some(key);
        self.pending_since = Some(Instant::now());
        self.hint_shown = false;
        self.pending_context = Some(context);
    }

    /// Starts a new key sequence.
    pub fn start(&mut self, key: PendingKey) {
        self.pending = Some(key);
        self.pending_since = Some(Instant::now());
        self.hint_shown = false;
        self.pending_context = None;
    }

    /// Cancels the current sequence and clears any pending state.
    pub fn cancel(&mut self) {
        self.clear_state();
    }

    /// Completes the sequence and clears the pending state.
    fn complete(&mut self) {
        self.clear_state();
    }

    /// Clears all pending state. Used by both cancel() and complete().
    fn clear_state(&mut self) {
        self.pending = None;
        self.pending_since = None;
        self.hint_shown = false;
        self.pending_context = None;
    }

    /// Process the first key of a potential sequence.
    /// Returns `Started` if this key begins a sequence, `NotConsumed` otherwise.
    /// If a sequence is already pending, it is cancelled first (cancel+restart behavior).
    pub fn process_first_key(&mut self, c: char) -> KeySequenceResult<C> {
        // Cancel any existing pending sequence before starting a new one
        if self.pending.is_some() {
            self.cancel();
        }

        match c {
            'g' => {
                self.start(PendingKey::G);
                KeySequenceResult::Started(PendingKey::G)
            }
            _ => KeySequenceResult::NotConsumed,
        }
    }

    /// Process the second key of a sequence.
    /// Returns `Completed` with the action if valid, `Cancelled` otherwise.
    pub fn process_second_key(&mut self, c: char) -> KeySequenceResult<C> {
        let Some(pending) = self.pending else {
            return KeySequenceResult::NotConsumed;
        };

        let action = match pending {
            PendingKey::G => match c {
                'g' => Some(KeySequenceAction::GotoFirst),
                'e' => Some(KeySequenceAction::GotoEditor),
                'c' => Some(KeySequenceAction::GotoConnections),
                't' => Some(KeySequenceAction::GotoTables),
                'r' => Some(KeySequenceAction::GotoResults),
                _ => None,
            },
            PendingKey::SchemaTable => match c {
                's' => Some(KeySequenceAction::SchemaTableSelect),
                'i' => Some(KeySequenceAction::SchemaTableInsert),
                'u' => Some(KeySequenceAction::SchemaTableUpdate),
                'd' => Some(KeySequenceAction::SchemaTableDelete),
                'n' => Some(KeySequenceAction::SchemaTableName),
                _ => None,
            },
        };

        match action {
            Some(action) => {
                let context = self.pending_context.take();
                self.complete();
                KeySequenceResult::Completed(KeySequenceCompletion { action, context })
            }
            None => {
                self.cancel();
                KeySequenceResult::Cancelled
            }
        }
    }

    /// Returns true if there is a pending key sequence.
    pub fn is_waiting(&self) -> bool {
        self.pending.is_some()
    }

    /// Update the timeout value.
    pub fn set_timeout(&mut self, timeout_ms: u64) {
        self.timeout_ms = timeout_ms;
    }

    /// Get the current timeout value in milliseconds.
    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn test_g_sequence_gg() {
        let mut handler = KeySequenceHandler::new(500);

        // Press 'g' to start sequence
        let result = handler.process_first_key('g');
        assert_eq!(result, KeySequenceResult::Started(PendingKey::G));
        assert!(handler.is_waiting());

        // Press 'g' again to complete
        let result = handler.process_second_key('g');
        assert_eq!(
            result,
            KeySequenceResult::Completed(KeySequenceCompletion {
                action: KeySequenceAction::GotoFirst,
                context: None
            })
        );
        assert!(!handler.is_waiting());
    }

    #[test]
    fn test_g_sequence_ge() {
        let mut handler = KeySequenceHandler::new(500);

        handler.process_first_key('g');
        let result = handler.process_second_key('e');
        assert_eq!(
            result,
            KeySequenceResult::Completed(KeySequenceCompletion {
                action: KeySequenceAction::GotoEditor,
                context: None
            })
        );
    }

    #[test]
    fn test_g_sequence_gc() {
        let mut handler = KeySequenceHandler::new(500);

        handler.process_first_key('g');
        let result = handler.process_second_key('c');
        assert_eq!(
            result,
            KeySequenceResult::Completed(KeySequenceCompletion {
                action: KeySequenceAction::GotoConnections,
                context: None
            })
        );
    }

    #[test]
    fn test_g_sequence_gt() {
        let mut handler = KeySequenceHandler::new(500);

        handler.process_first_key('g');
        let result = handler.process_second_key('t');
        assert_eq!(
            result,
            KeySequenceResult::Completed(KeySequenceCompletion {
                action: KeySequenceAction::GotoTables,
                context: None
            })
        );
    }

    #[test]
    fn test_g_sequence_gr() {
        let mut handler = KeySequenceHandler::new(500);

        handler.process_first_key('g');
        let result = handler.process_second_key('r');
        assert_eq!(
            result,
            KeySequenceResult::Completed(KeySequenceCompletion {
                action: KeySequenceAction::GotoResults,
                context: None
            })
        );
    }

    #[test]
    fn test_cancelled_sequence() {
        let mut handler = KeySequenceHandler::new(500);

        handler.process_first_key('g');
        let result = handler.process_second_key('x'); // Invalid second key
        assert_eq!(result, KeySequenceResult::Cancelled);
        assert!(!handler.is_waiting());
    }

    #[test]
    fn test_non_sequence_key() {
        let mut handler = KeySequenceHandler::new(500);

        let result = handler.process_first_key('h');
        assert_eq!(result, KeySequenceResult::NotConsumed);
        assert!(!handler.is_waiting());
    }

    #[test]
    fn test_should_show_hint_before_timeout() {
        let handler = KeySequenceHandler::new(500);
        assert!(!handler.should_show_hint());
    }

    #[test]
    fn test_should_show_hint_after_timeout() {
        let mut handler = KeySequenceHandler::new(50); // 50ms timeout for test

        handler.process_first_key('g');
        assert!(!handler.should_show_hint());

        // Use larger margin (200ms sleep vs 50ms timeout) to reduce CI flakiness
        sleep(Duration::from_millis(200));
        assert!(handler.should_show_hint());
    }

    #[test]
    fn test_cancel_clears_state() {
        let mut handler = KeySequenceHandler::new(500);

        handler.process_first_key('g');
        assert!(handler.is_waiting());

        handler.cancel();
        assert!(!handler.is_waiting());
        assert!(!handler.should_show_hint());
    }
}
