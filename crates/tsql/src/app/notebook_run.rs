//! Dependency-aware execution planning for notebook workflows.

use std::collections::{HashMap, HashSet, VecDeque};

use super::execution::CellId;
use super::refinement::{logical_result_references, LogicalResultReference};

/// Which portion of the notebook should be queued for execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NotebookRunScope {
    All,
    Above,
    Below,
    Dependents,
}

/// Produces a stable, dependency-first execution order for non-empty cells.
///
/// Numbered references create explicit graph edges. `@result` is ordered after
/// the nearest preceding source so a full replay preserves its publication
/// semantics. Dependencies outside a partial run are assumed to be retained.
#[cfg(test)]
pub(crate) fn notebook_run_plan(
    cells: &[(CellId, &str)],
    selected: CellId,
    scope: NotebookRunScope,
) -> Result<Vec<CellId>, String> {
    notebook_run_plan_inner(cells, selected, scope, true, &HashMap::new())
}

/// Produces a dependency-first plan with stable named result sources.
pub(crate) fn notebook_run_plan_with_names(
    cells: &[(CellId, &str)],
    selected: CellId,
    scope: NotebookRunScope,
    named_sources: &HashMap<String, CellId>,
) -> Result<Vec<CellId>, String> {
    notebook_run_plan_inner(cells, selected, scope, true, named_sources)
}

/// Produces a document-order plan for backends without logical-result references.
pub(crate) fn notebook_run_plan_without_references(
    cells: &[(CellId, &str)],
    selected: CellId,
    scope: NotebookRunScope,
) -> Result<Vec<CellId>, String> {
    notebook_run_plan_inner(cells, selected, scope, false, &HashMap::new())
}

fn notebook_run_plan_inner(
    cells: &[(CellId, &str)],
    selected: CellId,
    scope: NotebookRunScope,
    parse_references: bool,
    named_sources: &HashMap<String, CellId>,
) -> Result<Vec<CellId>, String> {
    let positions = cells
        .iter()
        .enumerate()
        .map(|(index, (id, _))| (*id, index))
        .collect::<HashMap<_, _>>();
    let selected_index = positions
        .get(&selected)
        .copied()
        .ok_or_else(|| format!("Cell {} is not present in the notebook", selected.0))?;
    let runnable = cells
        .iter()
        .filter_map(|(id, source)| (!source.trim().is_empty()).then_some(*id))
        .collect::<HashSet<_>>();

    let mut dependencies = HashMap::<CellId, Vec<CellId>>::new();
    for (index, (cell, source)) in cells.iter().enumerate() {
        if !runnable.contains(cell) {
            continue;
        }
        let mut sources = Vec::new();
        let references = if parse_references {
            logical_result_references(source)?
        } else {
            Vec::new()
        };
        for reference in references {
            let dependency = match reference {
                LogicalResultReference::Cell(source_cell) => {
                    if !positions.contains_key(&source_cell) {
                        return Err(format!(
                            "Cell {} references missing cell {}",
                            cell.0, source_cell.0
                        ));
                    }
                    Some(source_cell)
                }
                LogicalResultReference::Latest => cells[..index]
                    .iter()
                    .rev()
                    .find_map(|(id, _)| runnable.contains(id).then_some(*id)),
                LogicalResultReference::Named(name) => {
                    Some(*named_sources.get(&name).ok_or_else(|| {
                        format!("Cell {} references unknown result name '{}'", cell.0, name)
                    })?)
                }
            };
            if dependency == Some(*cell) {
                return Err(format!("Cell {} depends on itself", cell.0));
            }
            if let Some(dependency) = dependency.filter(|dependency| !sources.contains(dependency))
            {
                sources.push(dependency);
            }
        }
        dependencies.insert(*cell, sources);
    }

    let requested = match scope {
        NotebookRunScope::All => runnable.clone(),
        NotebookRunScope::Above => cells[..=selected_index]
            .iter()
            .filter_map(|(id, _)| runnable.contains(id).then_some(*id))
            .collect(),
        NotebookRunScope::Below => cells[selected_index..]
            .iter()
            .filter_map(|(id, _)| runnable.contains(id).then_some(*id))
            .collect(),
        NotebookRunScope::Dependents => {
            let mut reverse = HashMap::<CellId, Vec<CellId>>::new();
            for (cell, sources) in &dependencies {
                for source in sources {
                    reverse.entry(*source).or_default().push(*cell);
                }
            }
            let mut requested = HashSet::new();
            let mut pending = VecDeque::from([selected]);
            while let Some(cell) = pending.pop_front() {
                if !requested.insert(cell) {
                    continue;
                }
                if let Some(dependents) = reverse.get(&cell) {
                    pending.extend(dependents.iter().copied());
                }
            }
            requested.retain(|cell| runnable.contains(cell));
            requested
        }
    };

    let mut remaining = requested.clone();
    let mut ordered = Vec::with_capacity(requested.len());
    while !remaining.is_empty() {
        let next = cells.iter().map(|(id, _)| *id).find(|cell| {
            remaining.contains(cell)
                && dependencies
                    .get(cell)
                    .is_none_or(|sources| sources.iter().all(|source| !remaining.contains(source)))
        });
        let Some(next) = next else {
            return Err("Notebook result dependencies contain a cycle".to_string());
        };
        remaining.remove(&next);
        ordered.push(next);
    }

    Ok(ordered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_all_orders_dependencies_before_out_of_order_cells() {
        let cells = [
            (CellId(8), "SELECT * FROM @result_4"),
            (CellId(2), "SELECT 1"),
            (CellId(4), "SELECT * FROM @result_2"),
            (CellId(9), "  "),
        ];

        assert_eq!(
            notebook_run_plan(&cells, CellId(8), NotebookRunScope::All),
            Ok(vec![CellId(2), CellId(4), CellId(8)])
        );
    }

    #[test]
    fn run_dependents_only_queues_the_selected_tree() {
        let cells = [
            (CellId(1), "SELECT 1"),
            (CellId(2), "SELECT * FROM @result_1"),
            (CellId(3), "SELECT * FROM @result_2"),
            (CellId(4), "SELECT 4"),
        ];

        assert_eq!(
            notebook_run_plan(&cells, CellId(2), NotebookRunScope::Dependents),
            Ok(vec![CellId(2), CellId(3)])
        );
    }

    #[test]
    fn latest_reference_runs_after_the_nearest_preceding_source() {
        let cells = [
            (CellId(1), "SELECT 1"),
            (CellId(2), "SELECT 2"),
            (CellId(3), "SELECT * FROM @result"),
        ];

        assert_eq!(
            notebook_run_plan(&cells, CellId(2), NotebookRunScope::Below),
            Ok(vec![CellId(2), CellId(3)])
        );
    }

    #[test]
    fn multiple_numbered_sources_run_before_their_join() {
        let cells = [
            (
                CellId(9),
                "SELECT * FROM @result_4 a JOIN @result_2 b ON true",
            ),
            (CellId(2), "SELECT 2"),
            (CellId(4), "SELECT 4"),
        ];

        assert_eq!(
            notebook_run_plan(&cells, CellId(9), NotebookRunScope::All),
            Ok(vec![CellId(2), CellId(4), CellId(9)])
        );
    }

    #[test]
    fn detects_missing_sources_and_cycles() {
        let missing = [(CellId(1), "SELECT * FROM @result_9")];
        assert_eq!(
            notebook_run_plan(&missing, CellId(1), NotebookRunScope::All),
            Err("Cell 1 references missing cell 9".to_string())
        );

        let cycle = [
            (CellId(1), "SELECT * FROM @result_2"),
            (CellId(2), "SELECT * FROM @result_1"),
        ];
        assert_eq!(
            notebook_run_plan(&cycle, CellId(1), NotebookRunScope::All),
            Err("Notebook result dependencies contain a cycle".to_string())
        );
    }

    #[test]
    fn document_order_planner_does_not_parse_mongo_shell_as_sql() {
        let cells = [
            (CellId(4), "db.users.find({ note: `@result_foo` })"),
            (CellId(2), "// @result_1\ndb.users.find({ active: true })"),
            (CellId(8), "db.users.find({ text: '@result_99' })"),
        ];

        assert_eq!(
            notebook_run_plan_without_references(&cells, CellId(2), NotebookRunScope::All),
            Ok(vec![CellId(4), CellId(2), CellId(8)])
        );
        assert_eq!(
            notebook_run_plan_without_references(&cells, CellId(2), NotebookRunScope::Dependents),
            Ok(vec![CellId(2)])
        );
    }

    #[test]
    fn named_sources_participate_in_dependency_ordering() {
        let cells = [
            (CellId(8), "SELECT * FROM @result_recent_users"),
            (CellId(2), "SELECT 1"),
        ];
        let names = HashMap::from([("recent_users".to_string(), CellId(2))]);

        assert_eq!(
            notebook_run_plan_with_names(&cells, CellId(8), NotebookRunScope::All, &names),
            Ok(vec![CellId(2), CellId(8)])
        );
        assert_eq!(
            notebook_run_plan(&cells, CellId(8), NotebookRunScope::All),
            Err("Cell 8 references unknown result name 'recent_users'".to_string())
        );
    }
}
