//! Notebook document state and revision-aware cell transitions.

use std::time::Duration;

use super::execution::{CellId, ExecutionContext};
use super::refinement::{RefinementAvailability, ResultVersion, RetainedResult};
use crate::ui::{GridModel, GridState, QueryEditor};

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
    pub revision: u64,
    pub editor: QueryEditor,
    pub editor_scroll: (u16, u16),
    pub execution: CellExecutionState,
    pub output: Option<NotebookOutput>,
    pub failure: Option<String>,
    pub dependency: Option<ResultVersion>,
    pub bound_connection_generation: Option<u64>,
    pub output_collapsed: bool,
}

impl NotebookCell {
    fn new(id: CellId) -> Self {
        Self {
            id,
            revision: 0,
            editor: QueryEditor::new(),
            editor_scroll: (0, 0),
            execution: CellExecutionState::NeverRun,
            output: None,
            failure: None,
            dependency: None,
            bound_connection_generation: None,
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
        let mut notebook = Self::new();
        notebook.cells.clear();
        notebook.next_cell_id = cells
            .iter()
            .filter_map(|(id, _, _, _)| id.map(|id| id.0))
            .max()
            .and_then(|id| id.checked_add(1))
            .unwrap_or(1);

        for (saved_id, source, output_collapsed, dependency) in cells {
            let id = saved_id
                .filter(|id| id.0 != 0 && !notebook.cells.iter().any(|cell| cell.id == *id))
                .unwrap_or_else(|| notebook.allocate_cell_id());
            let mut cell = NotebookCell::new(id);
            cell.replace_source(source);
            cell.output_collapsed = output_collapsed;
            cell.dependency = dependency;
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
            self.cells[0].output = None;
            self.cells[0].failure = None;
            self.cells[0].dependency = None;
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
}
