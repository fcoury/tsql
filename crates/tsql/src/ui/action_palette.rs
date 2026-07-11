//! Context-aware actions for the command palette.

/// An action selected from the contextual palette.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaletteAction {
    Run,
    History,
    Snippets,
    SaveSnippet,
    ExportCsv,
    ExportJson,
    ExportTsv,
    ExportSql,
    GenerateInsert,
    GenerateUpdate,
    GenerateDelete,
    SortAscending,
    SortDescending,
    AddSortAscending,
    AddSortDescending,
    FilterCurrentValue,
    ExcludeCurrentValue,
    FilterNull,
    FilterNotNull,
    FilterContains,
    FilterNotContains,
    CustomFilter,
    ChooseColumns,
    GroupCountCurrentColumn,
    ClearFilters,
    ClearSorting,
    ResetResult,
    CopyTransformedSql,
    OpenTransformedSql,
    NewCell,
    RefineCell,
    RunAll,
    RunAbove,
    RunBelow,
    RunDependents,
    ExplainCell,
    ToggleOutput,
    ToggleSource,
    ClearCell,
    CellHistory,
    InspectError,
    JumpToError,
    JumpToActivity,
    SwitchNotebook,
    SwitchClassic,
}

/// State used to decide which actions are relevant to the current workspace.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ActionContext {
    pub notebook: bool,
    pub has_query: bool,
    pub has_result: bool,
    pub has_refinable_result: bool,
    pub has_selection: bool,
    pub has_column: bool,
    pub has_cell: bool,
    pub has_transform: bool,
    pub has_filters: bool,
    pub has_sort: bool,
    pub has_error: bool,
    pub has_cell_history: bool,
    pub has_activity: bool,
}

/// A searchable, human-readable palette item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionEntry {
    pub action: PaletteAction,
    pub label: &'static str,
    pub detail: &'static str,
}

impl ActionEntry {
    fn new(action: PaletteAction, label: &'static str, detail: &'static str) -> Self {
        Self {
            action,
            label,
            detail,
        }
    }

    /// Text used by the fuzzy picker for both display and matching.
    pub fn display(&self) -> String {
        format!("{}  {}", self.label, self.detail)
    }
}

/// Builds the palette in priority order for the current workspace and focus.
pub fn action_entries(context: ActionContext) -> Vec<ActionEntry> {
    let mut entries = Vec::new();
    if context.has_query {
        entries.push(ActionEntry::new(
            PaletteAction::Run,
            if context.notebook {
                "Run cell"
            } else {
                "Run query"
            },
            "Ctrl+E",
        ));
    }
    entries.push(ActionEntry::new(
        PaletteAction::History,
        "Query history",
        ":history",
    ));
    entries.push(ActionEntry::new(
        PaletteAction::Snippets,
        "Saved snippets",
        ":snippets",
    ));
    if context.has_query {
        entries.push(ActionEntry::new(
            PaletteAction::SaveSnippet,
            "Save current query as snippet",
            ":snippet-save <name>",
        ));
    }

    if context.has_result {
        let detail = if context.has_selection {
            "selected rows"
        } else {
            "all result rows"
        };
        entries.extend([
            ActionEntry::new(PaletteAction::ExportCsv, "Export CSV", detail),
            ActionEntry::new(PaletteAction::ExportJson, "Export JSON", detail),
            ActionEntry::new(PaletteAction::ExportTsv, "Export TSV", detail),
            ActionEntry::new(PaletteAction::ExportSql, "Export SQL inserts", detail),
            ActionEntry::new(
                PaletteAction::GenerateInsert,
                "Generate INSERT template",
                ":gen insert",
            ),
            ActionEntry::new(
                PaletteAction::GenerateUpdate,
                "Generate UPDATE template",
                ":gen update",
            ),
            ActionEntry::new(
                PaletteAction::GenerateDelete,
                "Generate DELETE template",
                ":gen delete",
            ),
        ]);
    }

    if !context.notebook && context.has_result && context.has_column {
        entries.extend([
            ActionEntry::new(
                PaletteAction::SortAscending,
                "Sort current column ascending",
                "replace sorting",
            ),
            ActionEntry::new(
                PaletteAction::SortDescending,
                "Sort current column descending",
                "replace sorting",
            ),
            ActionEntry::new(
                PaletteAction::AddSortAscending,
                "Add current column as secondary sort ascending",
                "keep existing sorting",
            ),
            ActionEntry::new(
                PaletteAction::AddSortDescending,
                "Add current column as secondary sort descending",
                "keep existing sorting",
            ),
        ]);

        if context.has_cell {
            entries.extend([
                ActionEntry::new(
                    PaletteAction::FilterCurrentValue,
                    "Filter to current value",
                    "current cell",
                ),
                ActionEntry::new(
                    PaletteAction::ExcludeCurrentValue,
                    "Exclude current value",
                    "current cell",
                ),
                ActionEntry::new(
                    PaletteAction::FilterContains,
                    "Filter current column containing value",
                    "current cell",
                ),
                ActionEntry::new(
                    PaletteAction::FilterNotContains,
                    "Filter current column not containing value",
                    "current cell",
                ),
            ]);
        }

        entries.extend([
            ActionEntry::new(
                PaletteAction::FilterNull,
                "Filter current column to NULL",
                "IS NULL",
            ),
            ActionEntry::new(
                PaletteAction::FilterNotNull,
                "Filter current column to non-NULL",
                "IS NOT NULL",
            ),
            ActionEntry::new(
                PaletteAction::CustomFilter,
                "Add custom filter",
                "choose operator and value",
            ),
            ActionEntry::new(
                PaletteAction::ChooseColumns,
                "Choose result columns",
                "show, hide, or reorder",
            ),
            ActionEntry::new(
                PaletteAction::GroupCountCurrentColumn,
                "Group and count by current column",
                "GROUP BY + count(*)",
            ),
        ]);
    }

    if !context.notebook && context.has_result {
        if context.has_filters {
            entries.push(ActionEntry::new(
                PaletteAction::ClearFilters,
                "Clear result filters",
                "keep sorting and columns",
            ));
        }
        if context.has_sort {
            entries.push(ActionEntry::new(
                PaletteAction::ClearSorting,
                "Clear result sorting",
                "keep filters and columns",
            ));
        }
        if context.has_transform {
            entries.extend([
                ActionEntry::new(
                    PaletteAction::ResetResult,
                    "Reset result transformations",
                    "restore original query result",
                ),
                ActionEntry::new(
                    PaletteAction::CopyTransformedSql,
                    "Copy transformed SQL",
                    "clipboard",
                ),
                ActionEntry::new(
                    PaletteAction::OpenTransformedSql,
                    "Open transformed SQL in editor",
                    "inspect or edit",
                ),
            ]);
        }
    }

    if context.notebook {
        entries.push(ActionEntry::new(
            PaletteAction::NewCell,
            "Insert cell below",
            "n",
        ));
        if context.has_refinable_result {
            entries.push(ActionEntry::new(
                PaletteAction::RefineCell,
                "Refine result in new cell",
                "r",
            ));
        }
        if context.has_result {
            entries.push(ActionEntry::new(
                PaletteAction::ToggleOutput,
                "Expand or collapse result",
                "h / l",
            ));
            entries.push(ActionEntry::new(
                PaletteAction::ExplainCell,
                "Explain cell query",
                ":explain-cell",
            ));
        }
        entries.extend([
            ActionEntry::new(PaletteAction::RunAll, "Run all cells", ":run-all"),
            ActionEntry::new(PaletteAction::RunAbove, "Run cells above", ":run-above"),
            ActionEntry::new(PaletteAction::RunBelow, "Run cells below", ":run-below"),
            ActionEntry::new(
                PaletteAction::RunDependents,
                "Run dependent cells",
                ":run-dependents",
            ),
            ActionEntry::new(
                PaletteAction::ToggleSource,
                "Expand or collapse cell source",
                ":toggle-source",
            ),
            ActionEntry::new(
                PaletteAction::ClearCell,
                "Clear cell execution and dependents",
                "x",
            ),
        ]);
        if context.has_cell_history {
            entries.push(ActionEntry::new(
                PaletteAction::CellHistory,
                "Cell execution history",
                ":cell-history",
            ));
        }
        if context.has_error {
            entries.push(ActionEntry::new(
                PaletteAction::JumpToError,
                "Go to error location",
                ":error-jump",
            ));
            entries.push(ActionEntry::new(
                PaletteAction::InspectError,
                "Inspect cell error",
                ":error",
            ));
        }
        if context.has_activity {
            entries.push(ActionEntry::new(
                PaletteAction::JumpToActivity,
                "Go to latest cell activity",
                ":activity",
            ));
        }
        entries.push(ActionEntry::new(
            PaletteAction::SwitchClassic,
            "Switch to Classic view",
            ":mode classic",
        ));
    } else {
        entries.push(ActionEntry::new(
            PaletteAction::SwitchNotebook,
            "Switch to Notebook view",
            ":notebook",
        ));
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::{action_entries, ActionContext, PaletteAction};

    #[test]
    fn classic_palette_only_offers_relevant_actions() {
        let actions = action_entries(ActionContext {
            has_query: true,
            ..ActionContext::default()
        })
        .into_iter()
        .map(|entry| entry.action)
        .collect::<Vec<_>>();

        assert!(actions.contains(&PaletteAction::Run));
        assert!(actions.contains(&PaletteAction::SaveSnippet));
        assert!(actions.contains(&PaletteAction::SwitchNotebook));
        assert!(!actions.contains(&PaletteAction::ExportCsv));
        assert!(!actions.contains(&PaletteAction::RunAll));
    }

    #[test]
    fn notebook_result_palette_includes_execution_and_selection_actions() {
        let entries = action_entries(ActionContext {
            notebook: true,
            has_query: true,
            has_result: true,
            has_refinable_result: true,
            has_selection: true,
            has_error: true,
            has_cell_history: true,
            has_activity: true,
            ..ActionContext::default()
        });
        let actions = entries.iter().map(|entry| entry.action).collect::<Vec<_>>();

        assert!(actions.contains(&PaletteAction::ExportSql));
        assert!(actions.contains(&PaletteAction::RefineCell));
        assert!(actions.contains(&PaletteAction::RunDependents));
        assert!(actions.contains(&PaletteAction::CellHistory));
        assert!(actions.contains(&PaletteAction::JumpToError));
        assert!(actions.contains(&PaletteAction::JumpToActivity));
        assert!(actions.contains(&PaletteAction::SwitchClassic));
        assert!(entries
            .iter()
            .find(|entry| entry.action == PaletteAction::ExportCsv)
            .is_some_and(|entry| entry.detail == "selected rows"));
    }

    #[test]
    fn notebook_without_output_hides_result_actions() {
        let actions = action_entries(ActionContext {
            notebook: true,
            ..ActionContext::default()
        })
        .into_iter()
        .map(|entry| entry.action)
        .collect::<Vec<_>>();

        assert!(actions.contains(&PaletteAction::NewCell));
        assert!(actions.contains(&PaletteAction::RunAll));
        assert!(!actions.contains(&PaletteAction::Run));
        assert!(!actions.contains(&PaletteAction::RefineCell));
        assert!(!actions.contains(&PaletteAction::ExportSql));
        assert!(!actions.contains(&PaletteAction::JumpToError));
    }

    #[test]
    fn notebook_preview_result_does_not_offer_unavailable_refinement() {
        let actions = action_entries(ActionContext {
            notebook: true,
            has_query: true,
            has_result: true,
            ..ActionContext::default()
        })
        .into_iter()
        .map(|entry| entry.action)
        .collect::<Vec<_>>();

        assert!(actions.contains(&PaletteAction::ExportCsv));
        assert!(actions.contains(&PaletteAction::ExplainCell));
        assert!(!actions.contains(&PaletteAction::RefineCell));
    }

    #[test]
    fn classic_result_palette_offers_contextual_transform_actions() {
        let actions = action_entries(ActionContext {
            has_query: true,
            has_result: true,
            has_column: true,
            has_cell: true,
            has_transform: true,
            has_filters: true,
            has_sort: true,
            ..ActionContext::default()
        })
        .into_iter()
        .map(|entry| entry.action)
        .collect::<Vec<_>>();

        for action in [
            PaletteAction::SortAscending,
            PaletteAction::SortDescending,
            PaletteAction::AddSortAscending,
            PaletteAction::AddSortDescending,
            PaletteAction::FilterCurrentValue,
            PaletteAction::ExcludeCurrentValue,
            PaletteAction::FilterNull,
            PaletteAction::FilterNotNull,
            PaletteAction::FilterContains,
            PaletteAction::FilterNotContains,
            PaletteAction::CustomFilter,
            PaletteAction::ChooseColumns,
            PaletteAction::GroupCountCurrentColumn,
            PaletteAction::ClearFilters,
            PaletteAction::ClearSorting,
            PaletteAction::ResetResult,
            PaletteAction::CopyTransformedSql,
            PaletteAction::OpenTransformedSql,
        ] {
            assert!(actions.contains(&action), "missing action: {action:?}");
        }
    }

    #[test]
    fn classic_empty_result_hides_cell_value_actions_and_inactive_resets() {
        let actions = action_entries(ActionContext {
            has_query: true,
            has_result: true,
            has_column: true,
            ..ActionContext::default()
        })
        .into_iter()
        .map(|entry| entry.action)
        .collect::<Vec<_>>();

        assert!(actions.contains(&PaletteAction::SortAscending));
        assert!(actions.contains(&PaletteAction::FilterNull));
        assert!(actions.contains(&PaletteAction::CustomFilter));
        assert!(actions.contains(&PaletteAction::ChooseColumns));
        assert!(actions.contains(&PaletteAction::GroupCountCurrentColumn));
        assert!(!actions.contains(&PaletteAction::FilterCurrentValue));
        assert!(!actions.contains(&PaletteAction::ExcludeCurrentValue));
        assert!(!actions.contains(&PaletteAction::FilterContains));
        assert!(!actions.contains(&PaletteAction::FilterNotContains));
        assert!(!actions.contains(&PaletteAction::ClearFilters));
        assert!(!actions.contains(&PaletteAction::ClearSorting));
        assert!(!actions.contains(&PaletteAction::ResetResult));
        assert!(!actions.contains(&PaletteAction::CopyTransformedSql));
        assert!(!actions.contains(&PaletteAction::OpenTransformedSql));
    }

    #[test]
    fn notebook_result_hides_classic_transform_actions() {
        let actions = action_entries(ActionContext {
            notebook: true,
            has_query: true,
            has_result: true,
            has_column: true,
            has_cell: true,
            has_transform: true,
            has_filters: true,
            has_sort: true,
            ..ActionContext::default()
        })
        .into_iter()
        .map(|entry| entry.action)
        .collect::<Vec<_>>();

        assert!(!actions.contains(&PaletteAction::SortAscending));
        assert!(!actions.contains(&PaletteAction::FilterCurrentValue));
        assert!(!actions.contains(&PaletteAction::ChooseColumns));
        assert!(!actions.contains(&PaletteAction::ClearFilters));
        assert!(!actions.contains(&PaletteAction::ResetResult));
    }
}
