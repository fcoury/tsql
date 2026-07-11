//! Notebook document state and revision-aware cell transitions.

use std::time::Duration;

use super::execution::{CellId, ExecutionContext};
use super::refinement::{
    normalize_result_name, RefinementAvailability, ResultVersion, RetainedResult,
};
use crate::session::{bounded_notebook_run_history, NotebookRunRecord};
use crate::ui::{GridModel, GridState, QueryEditor};

type RestoredNotebookCell = (
    Option<CellId>,
    Option<String>,
    String,
    bool,
    bool,
    Option<ResultVersion>,
    Vec<ResultVersion>,
    Vec<NotebookRunRecord>,
);

/// Which part of the selected notebook cell owns keyboard input.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum NotebookFocus {
    #[default]
    Cell,
    Editor,
    Result,
}

/// Current execution state for a cell.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CellExecutionState {
    #[default]
    NeverRun,
    Running(ExecutionContext),
    Succeeded,
    Failed,
    Cancelled,
}

/// Durable display output for a completed cell execution.
pub struct NotebookOutput {
    pub grid: GridModel,
    pub grid_state: GridState,
    pub command_tag: Option<String>,
    pub elapsed: Duration,
    pub error: Option<String>,
    pub retained: Option<RetainedResult>,
    pub refinement: RefinementAvailability,
    pub executed_revision: u64,
    pub truncated: bool,
}

/// One SQL/Mongo source cell and its latest durable output.
pub struct NotebookCell {
    pub id: CellId,
    pub result_name: Option<String>,
    pub revision: u64,
    pub editor: QueryEditor,
    pub editor_scroll: (u16, u16),
    pub execution: CellExecutionState,
    pub output: Option<NotebookOutput>,
    pub failure: Option<String>,
    pub dependency: Option<ResultVersion>,
    pub additional_dependencies: Vec<ResultVersion>,
    pub execution_history: Vec<NotebookRunRecord>,
    pub bound_connection_generation: Option<u64>,
    pub source_collapsed: bool,
    pub output_collapsed: bool,
}

impl NotebookCell {
    fn new(id: CellId) -> Self {
        Self {
            id,
            result_name: None,
            revision: 0,
            editor: QueryEditor::new(),
            editor_scroll: (0, 0),
            execution: CellExecutionState::NeverRun,
            output: None,
            failure: None,
            dependency: None,
            additional_dependencies: Vec::new(),
            execution_history: Vec::new(),
            bound_connection_generation: None,
            source_collapsed: false,
            output_collapsed: false,
        }
    }

    pub fn source(&self) -> String {
        self.editor.text()
    }

    pub fn is_empty(&self) -> bool {
        self.editor.text().trim().is_empty()
    }

    pub fn is_dirty(&self) -> bool {
        self.output
            .as_ref()
            .is_some_and(|output| output.executed_revision != self.revision)
    }

    pub fn output_is_stale(&self) -> bool {
        self.output.is_some()
            && (self.is_dirty()
                || matches!(
                    self.execution,
                    CellExecutionState::Running(_)
                        | CellExecutionState::Failed
                        | CellExecutionState::Cancelled
                ))
    }

    pub fn replace_source(&mut self, source: String) {
        if self.editor.text() != source {
            self.editor.set_text(source);
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub fn mark_edited(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// Adds a compact completed-run record, keeping only the newest bounded history.
    pub fn push_run(&mut self, record: NotebookRunRecord) {
        self.execution_history.push(record);
        self.execution_history =
            bounded_notebook_run_history(std::mem::take(&mut self.execution_history));
    }

    /// Replaces the source with one of this cell's previous execution sources.
    pub fn restore_run_source(&mut self, history_index: usize) -> bool {
        let Some(source) = self
            .execution_history
            .get(history_index)
            .map(|record| record.source.clone())
        else {
            return false;
        };
        self.replace_source(source);
        true
    }
}

/// Ordered notebook document with exactly one trailing draft cell.
pub struct NotebookState {
    pub cells: Vec<NotebookCell>,
    pub selected: CellId,
    pub focus: NotebookFocus,
    pub scroll_cell: usize,
    pub pending_delete: bool,
    next_cell_id: u64,
}

impl Default for NotebookState {
    fn default() -> Self {
        Self::new()
    }
}

impl NotebookState {
    pub fn new() -> Self {
        let first = CellId(1);
        Self {
            cells: vec![NotebookCell::new(first)],
            selected: first,
            focus: NotebookFocus::Cell,
            scroll_cell: 0,
            pending_delete: false,
            next_cell_id: 2,
        }
    }

    pub fn from_sources(
        cells: Vec<(String, bool, Option<ResultVersion>)>,
        selected_index: usize,
    ) -> Self {
        Self::from_sources_with_ids(
            cells
                .into_iter()
                .map(|(source, output_collapsed, dependency)| {
                    (None, source, output_collapsed, dependency)
                })
                .collect(),
            selected_index,
        )
    }

    /// Restores persisted cells without renumbering their structural identities.
    pub fn from_sources_with_ids(
        cells: Vec<(Option<CellId>, String, bool, Option<ResultVersion>)>,
        selected_index: usize,
    ) -> Self {
        Self::from_sources_with_dependencies(
            cells
                .into_iter()
                .map(|(id, source, collapsed, dependency)| {
                    (
                        id,
                        None,
                        source,
                        false,
                        collapsed,
                        dependency,
                        Vec::new(),
                        Vec::new(),
                    )
                })
                .collect(),
            selected_index,
        )
    }

    /// Restores all structural result bindings for cells that join multiple sources.
    pub fn from_sources_with_dependencies(
        cells: Vec<RestoredNotebookCell>,
        selected_index: usize,
    ) -> Self {
        let mut notebook = Self::new();
        notebook.cells.clear();
        notebook.next_cell_id = cells
            .iter()
            .filter_map(|(id, _, _, _, _, _, _, _)| id.map(|id| id.0))
            .max()
            .and_then(|id| id.checked_add(1))
            .unwrap_or(1);

        for (
            saved_id,
            result_name,
            source,
            source_collapsed,
            output_collapsed,
            dependency,
            additional_dependencies,
            execution_history,
        ) in cells
        {
            let id = saved_id
                .filter(|id| id.0 != 0 && !notebook.cells.iter().any(|cell| cell.id == *id))
                .unwrap_or_else(|| notebook.allocate_cell_id());
            let mut cell = NotebookCell::new(id);
            cell.replace_source(source);
            cell.editor.mark_saved();
            cell.result_name = result_name
                .and_then(|name| normalize_result_name(&name))
                .filter(|name| {
                    !notebook
                        .cells
                        .iter()
                        .any(|existing| existing.result_name.as_ref() == Some(name))
                });
            cell.source_collapsed = source_collapsed;
            cell.output_collapsed = output_collapsed;
            cell.dependency = dependency;
            cell.additional_dependencies = additional_dependencies;
            cell.execution_history = bounded_notebook_run_history(execution_history);
            notebook.cells.push(cell);
        }
        if notebook.cells.is_empty() {
            notebook.cells.push(NotebookCell::new(CellId(1)));
            notebook.next_cell_id = 2;
        }
        notebook.ensure_trailing_draft();
        let selected_index = selected_index.min(notebook.cells.len().saturating_sub(1));
        notebook.selected = notebook.cells[selected_index].id;
        notebook.pending_delete = false;
        notebook
    }

    pub fn selected_index(&self) -> usize {
        self.cells
            .iter()
            .position(|cell| cell.id == self.selected)
            .unwrap_or(0)
    }

    pub fn selected_cell(&self) -> &NotebookCell {
        &self.cells[self.selected_index()]
    }

    pub fn selected_cell_mut(&mut self) -> &mut NotebookCell {
        let index = self.selected_index();
        &mut self.cells[index]
    }

    pub fn cell_mut(&mut self, id: CellId) -> Option<&mut NotebookCell> {
        self.cells.iter_mut().find(|cell| cell.id == id)
    }

    pub fn select_previous(&mut self) {
        let index = self.selected_index().saturating_sub(1);
        self.selected = self.cells[index].id;
    }

    pub fn select_next(&mut self) {
        let index = (self.selected_index() + 1).min(self.cells.len().saturating_sub(1));
        self.selected = self.cells[index].id;
    }

    pub fn select_first(&mut self) {
        if let Some(cell) = self.cells.first() {
            self.selected = cell.id;
        }
    }

    pub fn select_last(&mut self) {
        if let Some(cell) = self.cells.last() {
            self.selected = cell.id;
        }
    }

    /// Selects the empty tail, preserving a non-empty tail and appending a new draft.
    pub fn select_or_create_draft(&mut self) {
        let needs_draft = self.cells.last().is_none_or(|cell| !cell.is_empty());
        if needs_draft {
            let id = self.allocate_cell_id();
            self.cells.push(NotebookCell::new(id));
        }
        self.select_last();
        self.focus = NotebookFocus::Editor;
    }

    /// Preserves an executed/non-empty cell and ensures an empty trailing draft exists.
    pub fn ensure_trailing_draft(&mut self) {
        if self.cells.last().is_none_or(|cell| !cell.is_empty()) {
            let id = self.allocate_cell_id();
            self.cells.push(NotebookCell::new(id));
        }
    }

    pub fn insert_refinement(&mut self, version: ResultVersion) -> CellId {
        let source_index = self.selected_index();
        let id = self.allocate_cell_id();
        let mut cell = NotebookCell::new(id);
        cell.replace_source(format!("SELECT *\nFROM @result_{}", version.source_cell.0));
        cell.dependency = Some(version);
        self.cells.insert(source_index + 1, cell);
        self.selected = id;
        self.focus = NotebookFocus::Editor;
        self.ensure_trailing_draft();
        id
    }

    /// Inserts a fresh source cell next to the selection and focuses its editor.
    pub fn insert_cell(&mut self, below: bool) -> CellId {
        let index = self.selected_index().saturating_add(usize::from(below));
        let id = self.allocate_cell_id();
        self.cells
            .insert(index.min(self.cells.len()), NotebookCell::new(id));
        self.selected = id;
        self.focus = NotebookFocus::Editor;
        self.ensure_trailing_draft();
        id
    }

    /// Duplicates source and lineage without copying stale runtime output.
    pub fn duplicate_selected(&mut self) -> CellId {
        let index = self.selected_index();
        let source = self.cells[index].source();
        let source_collapsed = self.cells[index].source_collapsed;
        let dependency = self.cells[index].dependency;
        let additional_dependencies = self.cells[index].additional_dependencies.clone();
        let id = self.allocate_cell_id();
        let mut cell = NotebookCell::new(id);
        cell.replace_source(source);
        cell.source_collapsed = source_collapsed;
        cell.dependency = dependency;
        cell.additional_dependencies = additional_dependencies;
        self.cells.insert(index + 1, cell);
        self.selected = id;
        self.focus = NotebookFocus::Cell;
        self.ensure_trailing_draft();
        id
    }

    /// Moves the selected cell one position while keeping its stable identity.
    pub fn move_selected(&mut self, down: bool) -> bool {
        let index = self.selected_index();
        let last_movable = if self.cells.last().is_some_and(NotebookCell::is_empty) {
            self.cells.len().saturating_sub(2)
        } else {
            self.cells.len().saturating_sub(1)
        };
        if index > last_movable {
            return false;
        }
        let target = if down {
            index.saturating_add(1)
        } else {
            index.saturating_sub(1)
        };
        if target == index || target > last_movable {
            return false;
        }
        self.cells.swap(index, target);
        true
    }

    fn allocate_cell_id(&mut self) -> CellId {
        let mut candidate = self.next_cell_id.max(1);
        while self.cells.iter().any(|cell| cell.id.0 == candidate) {
            candidate = candidate.checked_add(1).unwrap_or(1);
        }
        self.next_cell_id = candidate.checked_add(1).unwrap_or(1);
        CellId(candidate)
    }

    pub fn remove_cell(&mut self, id: CellId) -> Option<NotebookCell> {
        let index = self.cells.iter().position(|cell| cell.id == id)?;
        if self.cells.len() == 1 {
            self.cells[0].replace_source(String::new());
            self.cells[0].result_name = None;
            self.cells[0].output = None;
            self.cells[0].output_collapsed = false;
            self.cells[0].source_collapsed = false;
            self.cells[0].failure = None;
            self.cells[0].dependency = None;
            self.cells[0].additional_dependencies.clear();
            self.cells[0].execution_history.clear();
            self.cells[0].execution = CellExecutionState::NeverRun;
            return None;
        }
        let removed = self.cells.remove(index);
        let selected_index = index.min(self.cells.len().saturating_sub(1));
        self.selected = self.cells[selected_index].id;
        self.ensure_trailing_draft();
        Some(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_nonempty_tail_and_appends_one_draft() {
        let mut notebook = NotebookState::new();
        notebook
            .selected_cell_mut()
            .replace_source("select 1".to_string());

        notebook.select_or_create_draft();
        notebook.select_or_create_draft();

        assert_eq!(notebook.cells.len(), 2);
        assert_eq!(notebook.cells[0].source(), "select 1");
        assert!(notebook.cells[1].is_empty());
    }

    #[test]
    fn removing_the_only_cell_resets_all_persisted_cell_state() {
        let mut notebook = NotebookState::new();
        let cell_id = notebook.selected;
        let cell = notebook.selected_cell_mut();
        cell.replace_source("select 1".to_string());
        cell.result_name = Some("recent_rows".to_string());
        cell.source_collapsed = true;
        cell.output_collapsed = true;

        assert!(notebook.remove_cell(cell_id).is_none());
        let cell = notebook.selected_cell();
        assert!(cell.is_empty());
        assert_eq!(cell.result_name, None);
        assert!(!cell.source_collapsed);
        assert!(!cell.output_collapsed);
        assert_eq!(cell.execution, CellExecutionState::NeverRun);
    }

    #[test]
    fn source_revision_makes_previous_output_dirty() {
        let mut cell = NotebookCell::new(CellId(1));
        cell.replace_source("select 1".to_string());
        cell.output = Some(NotebookOutput {
            grid: GridModel::empty(),
            grid_state: GridState::default(),
            command_tag: None,
            elapsed: Duration::ZERO,
            error: None,
            retained: None,
            refinement: RefinementAvailability::Unavailable(
                crate::app::RefinementUnavailableReason::SnapshotDisabled,
            ),
            executed_revision: cell.revision,
            truncated: false,
        });

        cell.replace_source("select 2".to_string());

        assert!(cell.is_dirty());
    }

    #[test]
    fn restores_stable_cell_ids_and_keeps_refinement_lineage() {
        let version = ResultVersion {
            source_cell: CellId(4),
            source_execution: super::super::execution::ExecutionId(7),
            source_revision: 2,
        };

        let mut notebook = NotebookState::from_sources_with_ids(
            vec![
                (Some(CellId(4)), "select 1".to_string(), false, None),
                (
                    Some(CellId(9)),
                    "select * from @result_4".to_string(),
                    false,
                    Some(version),
                ),
            ],
            1,
        );

        assert_eq!(
            notebook
                .cells
                .iter()
                .map(|cell| cell.id)
                .collect::<Vec<_>>(),
            [CellId(4), CellId(9), CellId(10)]
        );
        assert_eq!(notebook.selected, CellId(9));
        assert_eq!(notebook.selected_cell().dependency, Some(version));

        notebook.select_first();
        let refinement = notebook.insert_refinement(version);

        assert_eq!(refinement, CellId(11));
        assert_eq!(notebook.selected_cell().dependency, Some(version));
        assert_eq!(
            notebook.selected_cell().source(),
            "SELECT *\nFROM @result_4"
        );
    }

    #[test]
    fn restores_legacy_and_duplicate_ids_without_collisions() {
        let notebook = NotebookState::from_sources_with_ids(
            vec![
                (Some(CellId(2)), "select 1".to_string(), false, None),
                (None, "select 2".to_string(), false, None),
                (Some(CellId(2)), "select 3".to_string(), false, None),
                (Some(CellId(0)), "select 4".to_string(), false, None),
            ],
            0,
        );

        assert_eq!(
            notebook
                .cells
                .iter()
                .map(|cell| cell.id)
                .collect::<Vec<_>>(),
            [CellId(2), CellId(3), CellId(4), CellId(5), CellId(6)]
        );
    }

    #[test]
    fn restores_v2_cells_without_ids_using_legacy_ordinals() {
        let version = ResultVersion {
            source_cell: CellId(1),
            source_execution: super::super::execution::ExecutionId(4),
            source_revision: 2,
        };

        let notebook = NotebookState::from_sources(
            vec![
                ("select 1".to_string(), false, None),
                ("select * from @result_1".to_string(), false, Some(version)),
            ],
            1,
        );

        assert_eq!(
            notebook
                .cells
                .iter()
                .map(|cell| cell.id)
                .collect::<Vec<_>>(),
            [CellId(1), CellId(2), CellId(3)]
        );
        assert_eq!(notebook.selected_cell().dependency, Some(version));
    }

    #[test]
    fn restored_sources_start_unmodified() {
        let notebook = NotebookState::from_sources_with_ids(
            vec![(Some(CellId(7)), "select 1".to_string(), false, None)],
            0,
        );

        assert!(!notebook.cells[0].editor.is_modified());
        assert!(!notebook.cells[0].is_dirty());
    }

    #[test]
    fn restores_unique_normalized_result_names_and_discards_duplicates() {
        let notebook = NotebookState::from_sources_with_dependencies(
            vec![
                (
                    Some(CellId(4)),
                    Some("Recent_Users".to_string()),
                    "SELECT 1".to_string(),
                    false,
                    false,
                    None,
                    Vec::new(),
                    Vec::new(),
                ),
                (
                    Some(CellId(5)),
                    Some("recent_users".to_string()),
                    "SELECT 2".to_string(),
                    false,
                    false,
                    None,
                    Vec::new(),
                    Vec::new(),
                ),
                (
                    Some(CellId(6)),
                    Some("not valid".to_string()),
                    "SELECT 3".to_string(),
                    false,
                    false,
                    None,
                    Vec::new(),
                    Vec::new(),
                ),
            ],
            0,
        );

        assert_eq!(
            notebook.cells[0].result_name.as_deref(),
            Some("recent_users")
        );
        assert!(notebook.cells[1].result_name.is_none());
        assert!(notebook.cells[2].result_name.is_none());
    }

    #[test]
    fn cell_operations_preserve_stable_ids_sources_and_lineage() {
        let version = ResultVersion {
            source_cell: CellId(1),
            source_execution: super::super::execution::ExecutionId(3),
            source_revision: 2,
        };
        let mut notebook = NotebookState::new();
        notebook.cells[0].replace_source("SELECT * FROM @result_1".to_string());
        notebook.cells[0].dependency = Some(version);
        notebook.ensure_trailing_draft();

        let duplicate = notebook.duplicate_selected();
        assert_ne!(duplicate, CellId(1));
        assert_eq!(notebook.selected_cell().source(), "SELECT * FROM @result_1");
        assert_eq!(notebook.selected_cell().dependency, Some(version));
        assert!(notebook.selected_cell().output.is_none());
        assert!(notebook.move_selected(false));
        assert_eq!(notebook.cells[0].id, duplicate);

        let inserted = notebook.insert_cell(true);
        assert_eq!(notebook.selected, inserted);
        assert!(notebook.selected_cell().is_empty());
        assert_eq!(notebook.focus, NotebookFocus::Editor);
    }

    #[test]
    fn moving_cells_does_not_move_or_replace_the_trailing_draft() {
        let mut notebook = NotebookState::new();
        notebook.cells[0].replace_source("SELECT 1".to_string());
        notebook.ensure_trailing_draft();
        notebook.selected = CellId(1);

        assert!(!notebook.move_selected(true));
        assert_eq!(notebook.cells.len(), 2);
        assert_eq!(notebook.cells[0].id, CellId(1));
        assert!(notebook.cells[1].is_empty());

        notebook.select_last();
        assert!(!notebook.move_selected(false));
        assert_eq!(notebook.cells.len(), 2);
        assert!(notebook.cells[1].is_empty());
    }

    #[test]
    fn restores_and_bounds_run_history_and_can_restore_previous_source() {
        use crate::session::{NotebookRunStatus, MAX_NOTEBOOK_RUN_HISTORY};

        let history = (0..MAX_NOTEBOOK_RUN_HISTORY + 3)
            .map(|index| NotebookRunRecord {
                execution_id: index as u64,
                source_revision: index as u64,
                source: format!("SELECT {index}"),
                status: NotebookRunStatus::Succeeded,
                finished_at: "2026-07-11T12:00:00Z".parse().unwrap(),
                elapsed_ms: 5,
                row_count: 1,
                headers: vec!["value".to_string()],
                rows: vec![vec![index.to_string()]],
                error: None,
                truncated: false,
            })
            .collect();
        let mut notebook = NotebookState::from_sources_with_dependencies(
            vec![(
                Some(CellId(4)),
                None,
                "SELECT current".to_string(),
                false,
                false,
                None,
                Vec::new(),
                history,
            )],
            0,
        );

        let cell = notebook.selected_cell_mut();
        assert_eq!(cell.execution_history.len(), MAX_NOTEBOOK_RUN_HISTORY);
        assert_eq!(cell.execution_history[0].execution_id, 3);
        assert!(cell.restore_run_source(0));
        assert_eq!(cell.source(), "SELECT 3");
        assert!(!cell.restore_run_source(MAX_NOTEBOOK_RUN_HISTORY));
    }

    #[test]
    fn pushing_completed_runs_keeps_only_newest_records() {
        use crate::session::{NotebookRunStatus, MAX_NOTEBOOK_RUN_HISTORY};

        let mut cell = NotebookCell::new(CellId(1));
        for index in 0..MAX_NOTEBOOK_RUN_HISTORY + 1 {
            cell.push_run(NotebookRunRecord {
                execution_id: index as u64,
                source_revision: index as u64,
                source: format!("SELECT {index}"),
                status: NotebookRunStatus::Succeeded,
                finished_at: "2026-07-11T12:00:00Z".parse().unwrap(),
                elapsed_ms: 2,
                row_count: 1,
                headers: vec!["value".to_string()],
                rows: vec![vec![index.to_string()]],
                error: None,
                truncated: false,
            });
        }

        assert_eq!(cell.execution_history.len(), MAX_NOTEBOOK_RUN_HISTORY);
        assert_eq!(cell.execution_history.first().unwrap().execution_id, 1);
        assert_eq!(
            cell.execution_history.last().unwrap().execution_id,
            MAX_NOTEBOOK_RUN_HISTORY as u64
        );
    }
}
