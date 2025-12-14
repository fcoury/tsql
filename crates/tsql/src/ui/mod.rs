mod completion;
mod confirm_prompt;
mod connection_form;
mod connection_manager;
mod editor;
pub mod fuzzy_picker;
mod grid;
mod help_popup;
mod highlighted_editor;
mod json_editor;
mod key_hint_popup;
mod key_sequence;
mod mouse_util;
pub use mouse_util::{is_inside, MOUSE_SCROLL_LINES};
mod row_detail;
pub mod sidebar;
mod status_line;

pub use completion::{
    determine_context, get_word_before_cursor, ColumnInfo, CompletionContext, CompletionItem,
    CompletionKind, CompletionPopup, SchemaCache, TableInfo,
};
pub use confirm_prompt::{ConfirmContext, ConfirmPrompt, ConfirmResult};
pub use connection_form::{ConnectionFormAction, ConnectionFormModal, FormField};
pub use connection_manager::{ConnectionManagerAction, ConnectionManagerModal};
pub use editor::{CommandPrompt, QueryEditor, SearchPrompt};
pub use fuzzy_picker::{FilteredItem, FuzzyPicker, PickerAction};
pub use grid::{
    escape_sql_value, quote_identifier, DataGrid, GridKeyResult, GridModel, GridSearch, GridState,
    ResizeAction,
};
pub use help_popup::{HelpAction, HelpPopup};
pub use highlighted_editor::{create_sql_highlighter, HighlightedTextArea};
pub use json_editor::{JsonEditorAction, JsonEditorModal};
pub use key_hint_popup::KeyHintPopup;
pub use key_sequence::{KeySequenceAction, KeySequenceHandler, KeySequenceResult, PendingKey};
pub use row_detail::{RowDetailAction, RowDetailModal};
pub use sidebar::{Sidebar, SidebarAction};
pub use status_line::{ConnectionInfo, Priority, StatusLineBuilder, StatusSegment};
