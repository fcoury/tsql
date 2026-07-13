# Notebook Mode Follow-up Plan

Status: implemented (P0-P2, F1-F2, core F3 tooling, and Actions/workflow sweep)
Date: 2026-07-11
Original baseline: `feat/notebook-mode` at `918e16b` / PR #44
Integrated baseline: `origin/master` through `802f092` (merge `ceba832`)
Primary reference: `docs/NOTEBOOK_MODE_PLAN.md`

This plan records the post-MVP notebook sweep: correctness and lifecycle bugs,
user-facing gaps, performance risks, and the most useful next features. It is
intended to make the next implementation pass incremental and reviewable, not
to broaden the MVP in one large change.

## Implementation outcome (2026-07-11)

The follow-up implementation is complete for the prioritized correctness,
resource-bound, UX, and workflow work. The findings and reproduction notes
below are retained as historical context; they describe the original baseline,
not the behavior of the integrated branch.

Shipped in this pass:

- **Classic reconciliation and execution safety.** Integrated the current
  schema-select and `Alt+M` workspace changes, scoped asynchronous Classic and
  Notebook events by execution/connection identity, fixed savepoint and
  ambiguous transaction handling, and made cancellation, stale completion,
  deletion, reconnect, and snapshot cleanup ownership-safe.
- **SQL and snapshot correctness.** Added a shared UTF-8-safe PostgreSQL lexer
  for escape strings, quotes, dollar bodies, and nested comments; structural
  logical references; semantic eligibility checks; unique physical TEMP names;
  identity-checked reads/drops; immutable pinned versions; true snapshot LRU;
  independent row/byte limits; zero-row metadata; and safe preview fallback.
- **Bounded, navigable results.** Non-snapshot PostgreSQL and Mongo previews
  now stream within row/byte/timeout budgets. Retained tables page on demand,
  `G` reaches the last row, paging/cancellation is scoped, and older previews
  compact under the aggregate display budget. Search explicitly reports
  incomplete data; full retained results stream to disk and selected loaded
  rows can be exported immediately.
- **Notebook workflows.** Added latest, numbered, named, and multiple result
  references; serial dependency-aware run-all/above/below/dependents; portable
  versioned notebook open/save; insert, duplicate, move, source collapse,
  outline/jump, dependency-aware clear/delete, dirty prompts, and per-editor
  query history. `:explain-cell` creates a safe non-materialized PostgreSQL
  EXPLAIN cell with inherited dependencies.
- **User-facing polish and performance.** Restored editor/keymap parity,
  mouse/table focus, Visual selection, SQL and Mongo-shell highlighting,
  clipping and source-availability cues, theme fallbacks, accurate row-count
  status, wrapped SQLSTATE/detail/hint/position diagnostics with
  `:error`/`:copy-error`, ASCII disclosure/rail fallback (`TSQL_ASCII=1`), and
  bounded visible-cell rendering for 100 long-source cells. The narrow-terminal
  smoke also caught and fixed hidden-editor horizontal-scroll corruption in Cell
  focus.
- **Actions and workflow sweep (items 1-7).** Added a contextual, keyboard/mouse
  Actions palette (`Ctrl+Shift+P`, `Cmd+K`, or `:actions`); live PostgreSQL
  `@result`/numbered/named completion with source metadata; persistent named
  snippets; bounded session-local per-cell run history with source restoration;
  full retained-result streaming export (CSV/JSON/TSV/SQL INSERT) and selected
  row export; Unicode-aware source-mapped PostgreSQL diagnostics with
  `:error-jump`; and bounded, jumpable off-screen cell activity notifications.
  Portable notebook documents remain source-and-lineage-only and never embed
  historical result previews.

Validation completed against local PostgreSQL 18.4:

```sh
cargo fmt --all -- --check
git diff --check
cargo clippy --locked --offline --all --all-targets -- -D warnings
TEST_DATABASE_URL=postgresql://127.0.0.1/sqatch cargo test --locked --offline --quiet --lib --bins
TEST_DATABASE_URL=postgresql://127.0.0.1/sqatch cargo test --locked --offline --quiet -p tsql --test integration_tests
TEST_DATABASE_URL=postgresql://127.0.0.1/sqatch cargo test --locked --offline --quiet -p tsql --test startup_nonblocking
cargo build --locked --offline --quiet -p tsql
```

Results: **826 library tests passed, 3 pre-existing keychain tests ignored;
22 binary tests, 14 syntax-highlighter tests, 8 PostgreSQL integration tests,
and 2 startup tests passed; clippy, formatting, and whitespace checks clean.**
The 100-cell/4 KiB-source focused-result render regression completes in about
70 ms in the debug test binary and renders only the seven-cell visible window.

The tmux smoke covered snapshot creation and paging, trailing comments and
escape strings, joins across numbered/named sources, restore and Run All,
clear plus Run Dependents, collapse/expand, EXPLAIN, rich mapped errors and
error-jump, named-reference completion, bracketed paste, snippets, restored
cell-run history/output previews, off-screen activity, contextual Actions in
Classic and Notebook, selected CSV plus full retained CSV/JSON/TSV/SQL export,
a 250,000-row snapshot-off preview, Classic schema-select/`Alt+M`, and a 60x20
ASCII terminal.

Remaining optional follow-ons, deliberately outside these passes:
- Add a typed, bounded Mongo refinement provider and Mongo-specific result
  references once a Mongo-backed integration environment is available.
- Add a dedicated release-profile benchmark harness and phase-level timings;
  the debug render regression guards the visible-window invariant but is not a
  substitute for repeatable latency/peak-memory benchmarks.

## Executive summary

The core notebook flow works, but several boundaries need hardening before it
can safely become a dependable daily workspace:

1. PR #44 is currently unmergeable with `master`. The two newer Classic-mode
   changes (`c02e022` and `802f092`) conflict primarily in `app.rs` and must be
   reconciled before further notebook work.
2. Execution/transaction ownership is not yet fully target-aware. Stale events,
   an incorrect savepoint classification, and edits during an in-flight
   execution can make the app believe a transaction is idle and allow a
   snapshot commit to commit user work.
3. Snapshot eligibility, logical-reference compilation, identity validation,
   and cleanup have several correctness holes. Valid SQL can fail, an evicted
   immutable dependency can silently rebind, and cleanup can drop an unrelated
   replacement TEMP relation.
4. Result and memory limits are not independent or consistently enforced.
   Complete snapshots can expose only their initial display rows, while
   non-snapshot execution buffers the entire server response before truncating.
5. The notebook renderer/input model needs a focused UX and performance pass:
   invisible Visual selections, false snapshot badges, hidden focus, clipped
   SQL/errors, expensive hidden Classic rendering, and repeated whole-document
   scans are all observable.

The recommended order is: mergeability, execution safety, snapshot/reference
correctness, bounded results, UX, performance, then new features.

## Sweep evidence

The sweep inspected the full `918e16b` diff and related callers, then exercised
the debug binary against PostgreSQL 18.4 in tmux with both normal and deliberately
small limits. The following failures were reproduced live:

- A complete 20-row retained result with `connection.max_rows = 5` displays and
  navigates only five rows, reports `at least 6 total`, and cannot reach row 6.
- With `snapshot_max_rows = 1` and display max 5, a five-row query displays only
  two rows because staging uses `snapshot_max_rows + 1`.
- Replacing a dependent cell's SQL with `SELECT 42 AS standalone_value` fails
  with `dependent cell is missing its logical result reference`; clearing the
  execution does not make the cell reusable.
- After a source is rerun and deleted, a dependent pinned to its older result
  still reruns successfully. Inspecting the active TEMP schema shows the old
  `__tsql_nb_*` relation remains.
- Editing a slow query while it runs finishes with a `SUCCEEDED` header but no
  output and leaves another unreachable TEMP relation.
- `SELECT E'it\'s @result_1' AS note` succeeds directly in PostgreSQL but
  fails in Notebook with `unterminated SQL literal`.
- Data-changing CTEs, `SELECT ... INTO`, `SELECT ROW(1,2)`, a trailing `--`
  comment, an alias following `@result_N`, and generated-name collisions all
  produce avoidable notebook failures.
- Hiding a focused schema sidebar leaves the status at `NOTEBOOK · RESULTS`
  with keyboard focus on an invisible Classic grid.
- Disconnecting after a successful snapshot continues to show `SESSION
  SNAPSHOT · COMPLETE`; restoring a saved, unchanged notebook immediately
  produces an unsaved-changes prompt.
- A configured editor binding such as `ctrl+s = execute_query` executes into
  the hidden Classic grid instead of the notebook cell.

Static and PostgreSQL probes additionally confirmed transaction, cancellation,
eligibility, and resource-limit problems described below. Existing automated
tests predominantly exercise the three-row snapshot happy path and do not
cover these boundaries.

## Prioritized findings

### P0 - unblock delivery and protect user data

#### P0.1 Reconcile the branch with current `master`

PR #44 reports `mergeStateStatus: DIRTY`. `master` has two commits that are not
in the notebook branch:

- `c02e022 fix(schema): execute generated select query`
- `802f092 feat(ui): maximize results with alt-m`

`git merge-tree` reports overlapping changes in `README.md`,
`crates/tsql/src/app/app.rs`, `crates/tsql/src/config/keymap.rs`,
`crates/tsql/src/ui/confirm_prompt.rs`, and
`crates/tsql/src/ui/help_popup.rs`, with actual conflict markers in `app.rs`.

Implementation:

- Rebase or merge current `master` before changing notebook behavior.
- Preserve the new Classic results-maximization state and schema-table execution
  behavior while retaining the notebook workspace/focus and target-aware
  execution paths.
- Add a small regression matrix for Classic `Alt+M`, schema-table selection,
  notebook entry/exit, and switching modes during execution.

Exit: the branch is mergeable and both newer Classic features work unchanged.

#### P0.2 Make the execution coordinator and transaction state authoritative

Relevant paths: `crates/tsql/src/app/execution.rs`,
`crates/tsql/src/app/app.rs` (`DbEvent`, `execute_*`, `cancel_query`, and event
handling).

Problems:

- `ROLLBACK TRANSACTION TO SAVEPOINT ...` is classified as a full rollback
  because the classifier checks only whether the first remaining word is `TO`.
  PostgreSQL keeps the transaction active. A later snapshot starts and commits
  its transaction, which can commit the user's outer transaction. This was
  verified against PostgreSQL 18.
- On an in-flight source edit or deletion, success/error handling can return
  before transaction reduction. A non-snapshot multi-statement cell can leave a
  transaction open while the app continues to report `Idle`.
- Classic completion/error/paging/metadata and cancellation events remain
  unscoped. A delayed event from an old connection/execution can clear
  `db.running`, overwrite current state, or permit a second execution.
- A connection-global cancel can hit schema loading/cleanup while a notebook
  query waits on the shared-client lock; the queued query can then execute and
  publish successfully despite cancellation.

Implementation:

- Give every database event an execution ID, target, and connection generation;
  reject stale events uniformly. Make one coordinator the source of truth
  instead of independently mutating `db.running`, `active_query_kind`, and
  `active_execution`.
- Capture submitted SQL/transaction intent in the immutable active execution.
  Always reduce transaction state from that captured SQL, even when the editor
  revision or destination cell changes.
- Correctly recognize optional `WORK`/`TRANSACTION` before `TO SAVEPOINT`,
  conservatively mark ambiguous transaction-control-containing multi-statements
  as unsafe, and refuse snapshot materialization whenever transaction safety is
  uncertain.
- Scope cancellation to the execution and check its cancellation flag after
  acquiring the client lock but before dispatching SQL.

Required tests:

- savepoint rollback followed by snapshot and final rollback leaves no committed
  user rows;
- successful/failed in-flight edits and deletion still update transaction state;
- stale Classic completion/error/cancel/paging events cannot affect a newer
  notebook execution or connection;
- cancel while schema load/cleanup owns the client lock prevents the queued
  cell from running.

#### P0.3 Never read or drop an untracked/replaced TEMP relation

Relevant paths: snapshot execution/cleanup in `app.rs` and
`crates/tsql/src/app/pg_snapshot.rs`.

Problems:

- Delete, clear, invalidation, and eviction issue `DROP TABLE IF EXISTS
  pg_temp."name"` without verifying the recorded backend PID and relation OID.
  If the original snapshot was dropped/recreated, or a pooler changes backend,
  cleanup can delete an unrelated user TEMP table.
- Dependent execution through `:run-without-snapshot`, an active transaction,
  or another simple-query fallback compiles the physical TEMP name but skips
  PID/OID validation. It can silently read a replacement relation.
- Snapshot physical names contain only execution/revision information. A
  leaked relation on a reused backend can collide with a fresh process.

Implementation:

- Centralize a conditional, identity-checked drop: under the same client lock,
  compare `pg_backend_pid()` and `to_regclass(... )::oid`, drop only on exact
  match, and report cleanup failures without destroying unknown objects.
- Validate source identity immediately before every dependent execution,
  including simple/no-snapshot paths.
- Add a session nonce or collision-resistant component to generated physical
  names while respecting the 63-byte identifier limit.

Required tests: replace a backing relation under the same name, change backend
identity, and simulate cleanup during an aborted transaction; no unrelated
relation may be read or dropped.

### P1 - snapshot and logical-reference correctness

#### P1.1 Replace the three handwritten SQL scanners with one dialect-aware lexer

Relevant paths: `execution.rs::single_statement`,
`pg_snapshot.rs::snapshot_source`, and
`refinement.rs::visit_logical_result_references`.

The scanners duplicate quote/comment/semicolon logic and do not understand
PostgreSQL escape strings. A valid `E'...'` containing a backslash-escaped
quote, semicolon, comment marker, or apparent `@result_N` can be mis-tokenized
and rejected before it reaches PostgreSQL. The new production scanners also
contain unchecked `unwrap()` character advances.

Implementation:

- Introduce one small, tested PostgreSQL-aware lexical iterator that yields
  structural tokens/ranges while skipping single/double quotes, escape strings,
  dollar quotes, line comments, and nested block comments. Keep byte offsets
  UTF-8-safe and use checked character advancement.
- Reuse it for statement counting, transaction classification, logical-reference
  detection/rewrite, and source-position mapping.
- Do not run the SQL lexer over Mongo shell/JavaScript input. Until Mongo has a
  provider, execute Mongo cells directly and report refinement unavailable.

Required tests: `E'it\'s @result_1'`, escaped semicolons/comment markers,
Unicode, dollar quotes, nested comments, multiple statements, Mongo template
literals/`//` comments, and malformed input.

#### P1.2 Make snapshot eligibility semantic and explicitly fall back before execution

Relevant paths: `pg_snapshot.rs::snapshot_source`,
`app.rs::execute_notebook_cell_with_snapshot`.

Current eligibility checks tree validity and the first keyword. It admits
`SELECT ... INTO` and data-changing CTEs, which prepare successfully but fail
when nested in CTAS. It also routes pseudo-type/unbound-parameter queries into
snapshot errors instead of executing them normally once. Conversely, the
bundled grammar rejects standalone `VALUES` and `TABLE`, so two documented
forms can never produce snapshots. Roles without TEMP privilege and hot
standbys similarly fail valid SELECTs.

Implementation:

- Add a typed preflight result such as `Eligible`, `Ineligible(reason)`, and
  `Invalid(error)`. Walk the semantic tree and reject data-changing CTEs,
  `SELECT INTO`, locking/transaction/write statements, and unknown shapes.
- Explicitly support or conservatively normalize `VALUES` and `TABLE` if they
  remain part of the public contract.
- Before materialization, check TEMP privilege/recovery state, output columns,
  unbound parameters, and all PostgreSQL pseudo-types (including arrays such as
  `record[]` and types such as `cstring`).
- Route known-ineligible-but-valid queries to normal execution exactly once and
  publish a precise unavailable reason. Never retry after materialization may
  have executed user SQL.
- Place a newline around interpolated source SQL in CTAS so a valid trailing
  `-- comment` cannot comment out the wrapper suffix.

Required tests: ordinary SELECT/read-only CTE/VALUES/TABLE; data-changing CTE,
`SELECT INTO`, locking clauses, `record`, `record[]`, `cstring`, `$1`, TEMP
denied, recovery/read-only, and trailing line comment.

#### P1.3 Fix logical-result aliases, pinned identity, and dependency editing

Relevant paths: `refinement.rs::compile_logical_reference`,
`app.rs::resolve_notebook_result_dependency`, dependency rendering/actions.

Problems:

- Compilation always appends a generated alias. Natural SQL such as
  `FROM @result_1 AS r` becomes two aliases and fails at `AS`.
- When a live numbered dependency is evicted, resolution falls back to the
  latest source result and silently rebases it. This violates immutable
  lineage. Restored/unbound references and runtime-pinned-but-evicted
  references need distinct states.
- Removing the logical reference from a dependent cell leaves its dependency
  pinned. The cell can never run independent SQL; clear/rebind/rebase do not
  detach it.

Implementation:

- Preserve caller aliases or compile logical relations into collision-safe
  generated CTEs; support explicit/shorthand aliases and repeated self-use.
- Track whether lineage is restored/unbound versus live-pinned. Only the former
  may automatically bind a source's latest result. An evicted live binding must
  report an actionable `:rebase`/rerun error.
- Clear lineage when an edit removes its last logical reference, or add an
  explicit `:detach` action with clear UI feedback.

Required tests: aliased/shorthand/self-joined references, pinned v1 followed by
source v2 and v1 eviction, restored rebind, reference removal/replacement, and cyclic or
malformed dependencies.

#### P1.4 Make generated identifiers collision-safe and byte-correct

Relevant path: `pg_snapshot.rs::normalize_column_names` and CTAS construction.

Output normalization truncates by characters, not PostgreSQL's 63-byte limit,
then appends `_2`, `_3`, and so on. Three duplicate 62-byte UTF-8 aliases were
reproduced failing with `column ... specified more than once`. A user alias
matching `__tsql_row_ordinal_<execution>` also collides with the internal
ordinal column.

Implementation:

- Reserve the internal ordinal name and all suffix bytes before truncating.
  Truncate at a valid UTF-8 boundary, then deduplicate the final emitted
  identifier, not the pre-truncation candidate.
- Apply the same byte budget to all generated relation/alias names.

Required tests: empty names, three or more duplicates, 63-byte ASCII and
multibyte aliases, quoted names, large suffixes, and ordinal-name collisions.

### P1 - lifecycle, availability, and bounded results

#### P1.5 Centralize snapshot publication, invalidation, deletion, and eviction

Relevant paths: the three snapshot registries in `app.rs`,
`NotebookQueryFinished`, delete/clear/reconnect, and result summaries.

Problems:

- Deleting a rerun source drops only its current output handle. Older immutable
  versions remain registered/server-resident, contradict the delete prompt, and
  keep pinned dependents runnable.
- Completion registers a new snapshot before checking destination existence or
  revision. An edited/deleted in-flight cell can insert an unreachable snapshot,
  evict a useful one, and render `SUCCEEDED` with no output.
- Eviction/invalidation changes `refinement` but leaves `output.retained`; the
  UI continues to say `SESSION SNAPSHOT · COMPLETE`, `:rebase` can claim
  success with a dead handle, and a newly published snapshot can be evicted
  before its output is marked available.
- The advertised LRU is FIFO: access/refinement never refreshes recency.
  Publication order for `@result` and access order for eviction are distinct.

Implementation:

- Move registry/lifetime operations into a small snapshot manager indexed by
  handle, result version, and source cell. Return whether publication survived
  budget enforcement and expose one authoritative availability state.
- Validate destination/revision before publication; conditionally drop any
  discarded result. On source deletion, remove every version for that source;
  keep rendered downstream output but mark reruns unavailable.
- Remove duplicated `retained`/availability and unused error state from
  `NotebookOutput`, or enforce a single invariant. Make badges, Refine, and
  `:rebase` consult live registry state.
- Implement a true access-order LRU and maintain separate publication order for
  `@result`. Honor explicit zero limits predictably.

Required tests: source reruns/deletion, clear tree, edited/deleted in-flight at
capacity, reconnect/eviction/new-snapshot-immediate-eviction, real LRU access,
zero limits, and no orphan relations.

#### P1.6 Separate retention, display, and memory budgets; bound every execution path

Relevant paths: `pg_snapshot.rs::execute`, simple/Mongo execution and result
handling in `app.rs`, notebook result/grid state.

Problems:

- CTAS stages only `snapshot_max_rows + 1`. If the retention cap is lower than
  the display cap, users lose legitimate display rows.
- If a snapshot is complete but larger than the initial display cap, its
  retained rows are not pageable: Result focus, search, copy, and export see
  only the in-memory prefix. Metadata also reports `at least N+1` instead of
  using the exact retained row count.
- Non-snapshot Notebook queries use `simple_query`, which buffers the complete
  server result before applying `max_rows`; Notebook bypasses Classic paging.
  Large results can exhaust memory, and the configured timeout is not applied
  consistently outside snapshot execution.
- Oversized snapshots are read into the UI before being rejected; accumulated
  cell grids have no aggregate display-memory budget.
- Complete zero-row snapshots lose their column headers because headers are
  inferred only from the first returned row.

Implementation:

- Stage up to the independently required display/retention row boundary while
  keeping explicit byte safeguards. Decide retention before reading an
  oversized result back into the UI and fetch only a byte-bounded preview.
- Reuse cursor/portal paging or stream with a hard row/byte cutoff for ordinary
  notebook queries; enforce timeout/cancellation consistently.
- Add lazy paging against a retained snapshot so focused grids/search/export can
  intentionally access all retained rows without loading them all at once.
- Track fetched, exact total, and visible counts separately; use retained row
  count when known and seed headers from prepared output metadata for zero-row
  results.
- Add an aggregate display-memory policy (evict/spill/collapse old previews)
  separate from backend snapshot retention.

Required tests: unequal display/retention caps, `N-1/N/N+1`, byte-cap boundary,
zero rows with columns, snapshots off/ineligible/active transaction with large
results, timeout/cancel, retained paging/search/export, and multiple large
cells.

### P2 - user-facing correctness, accessibility, and discoverability

#### P2.1 Fix focus, editor parity, and prompts

Relevant paths: focus/sidebar/editor dispatch and rendering in `app.rs`,
`notebook.rs`, `ui/help_popup.rs`, and `ui/theme.rs`.

Implementation:

- When a focused sidebar is hidden in Notebook mode, return focus to the
  notebook (preserving Cell/Result intent), never to an invisible Classic
  Query/Grid. Make mode commands repair inconsistent focus if encountered.
- Replace the temporary `self.editor` swap with explicit editor-target
  dispatch. Configured notebook-editor actions such as `execute_query`,
  `focus_grid`, and `goto_results` must target the notebook cell/result, not
  hidden Classic state.
- Populate notebook editor/shared history so the advertised `Ctrl-p/n` works.
- Restore persisted cell editors as saved and track source dirtiness separately
  from execution state. A restored `NEEDS RUN` cell is not automatically an
  unsaved edit and must not trigger a false quit/connection prompt.
- Use stable `cell.id` in deletion confirmation and count both bound and
  textual/unbound numbered dependents. Keep cancellation semantics explicit:
  `Ctrl+C` cancels a notebook run; `Esc` changes focus and must not unexpectedly
  cancel from Cell focus.
- Add `:notebook`, `:mode`, `:rebase`, `:rebind`, `:detach`, and
  `:run-without-snapshot` to in-app Commands help. Clearly identify a running
  notebook cell when viewing Classic mode.

Required tests: hide sidebar from each section/subfocus, configured editor
actions, editor history, restored unchanged notebook, stable/out-of-order cell
IDs and unbound dependents, mode switch while running, and keyboard-only flows.

#### P2.2 Restore editor visuals, useful errors, and theme/terminal fallbacks

Notebook composers currently render plain text rather than the shared
highlighted editor. SQL/Mongo syntax, search matches, logical-reference tokens,
and Vim Visual selections are invisible; Visual operations can therefore be
destructive without any selection cue. Long sources and errors are silently
clipped, and partial light/custom themes fall back to hard-coded dark notebook
styles.

Implementation:

- Reuse `HighlightedTextArea`/the shared highlighter for the visible composer
  window, including cursor, Visual selection, search state, syntax scopes, and
  a distinct logical-reference token. Avoid highlighting off-screen source.
- Show explicit lines-above/lines-below or `+N lines` source indicators and a
  clear horizontal-clipping cue. Keep state/source completeness visible at
  60x20.
- Replace flattened failure strings with a structured, inspectable/copyable
  error view containing message, SQLSTATE, position, detail, and hint; wrap
  inline errors and offer a detail action for long diagnostics.
- Derive missing notebook theme scopes from the already-resolved background,
  elevated background, text, muted, accent, warning, and grid styles. Add
  light/partial-custom/monochrome and ASCII-safe disclosure/rail fallbacks.

Required tests: Visual selection/search visibility, logical token styling,
multiline/horizontal clipping, long errors at 60 columns, built-in dark/light,
legacy partial custom theme, and no-Unicode/monochrome terminal behavior.

### P2 - rendering and execution performance

#### P2.3 Stop rendering hidden work and remove whole-document hot-path scans

Relevant paths: the main render loop, notebook renderer, editor-key dispatch,
snapshot classifier, and `tui-syntax` highlighter.

Problems:

- Every Notebook frame highlights/renders the hidden Classic editor/grid and
  then clears it before rendering Notebook. Long Classic SQL remains expensive
  at idle and running frame rates.
- Each notebook-editor key clones and compares the complete SQL twice, even for
  cursor motion.
- Renderer availability construction repeatedly scans snapshot order and all
  cells for every cell, producing approximately `O(retained * cells^2)` work per
  frame. Unsaved-state checks and reference parsing repeatedly allocate/scan
  full sources.
- Composer rendering reserves capacity using the full line count despite
  displaying only a small window, and compact summaries join the entire source.
- Snapshot eligibility reparses SQL several times and comment masking uses
  repeated `replace_range`, which is quadratic for many comments.

Implementation:

- Branch on workspace before Classic highlight/layout/render. Render only the
  active workspace and only visible notebook cells/source windows.
- Have editor actions report whether they mutated text; update revision/dirty
  state without serializing the full buffer on navigation keys.
- Maintain `latest_by_cell`, availability, dirty counts, and parsed logical
  references by revision. Build per-frame lookup state once in `O(retained +
  cells)` and bound summaries by visible width.
- Classify once per execution, pass the validated source/preflight result
  through completion, and build a comment-masked parser buffer in one pass.
- Maintain aggregate retained-byte totals and batch validated cleanup where
  possible. Add debug-only phase timings for classify, lock wait, materialize,
  inspect, display, and cleanup.

Required benchmarks: 100 clean cells, 100 long-source cells, a very long
multiline composer, repeated cursor movement, focused-result navigation, and
snapshot-off large-result execution. Record frame/input latency and peak memory.

## Recommended next features

These should follow the correctness/bounds work; each can ship independently.

- **F1 - page retained results.** Make a complete snapshot genuinely
  inspectable beyond the initial prefix without loading it all into memory.
  Fetch a focused grid page from the retained TEMP relation, show exact
  total/range, and preserve cursor/search/export semantics.
- **F1 - Run All / run dependents.** Turn a notebook into a repeatable workflow
  and remove manual rerun ordering. Build a dependency graph, run topologically
  with one active execution, show progress/cancel, stop on error, and clear
  stale downstream results.
- **F2 - named and multiple result sources.** Make joins/comparisons readable
  and remove the current one-source limitation. Add stable names and a parsed
  set of references, validate each source/version, and support joins and
  explicit aliases.
- **F2 - notebook files: save/load/export/share.** Allow multiple reusable
  investigations instead of one global session snapshot. Define a versioned
  source/lineage-only file format, open/save/save-as, dirty indicator, and safe
  migration.
- **F2 - cell operations/outline.** Improve larger-document navigation and
  composition with duplicate, move/reorder, insert above/below, collapse
  source, outline/jump, and dependency-aware delete confirmation.
- **F3 - rich error/result tools.** Improve diagnosis and sharing with error
  detail/copy, explain action, export selected/full retained rows, and per-cell
  execution history.
- **F3 - Mongo refinement provider.** Extend the model without pretending SQL
  semantics apply. Add typed bounded BSON snapshots, capability/version gating,
  and explicit Mongo-specific reference compilation.

`@result` should continue to mean latest *published* result. Accessing an older
snapshot for LRU purposes must not change that meaning. Multiple-source and
Run All work should use explicit result versions so a notebook remains
reproducible.

## Suggested implementation slices

Keep the follow-up reviewable by landing these in order:

1. **Merge current master and protect Classic regressions.** Resolve conflicts,
   restore CI, and establish the combined baseline.
2. **Execution coordinator/transaction safety.** Scope all events/cancellation,
   fix transaction inference, and add concurrency/reconnect/savepoint tests.
3. **Snapshot identity and lifecycle manager.** Conditional drops, simple-path
   validation, publication/invalidation invariants, all-version deletion, and
   real LRU.
4. **Shared SQL lexer and semantic eligibility.** Escape strings/comments,
   supported/rejected shapes, pseudo-types/parameters, privileges/recovery, and
   safe pre-execution fallback.
5. **Reference ergonomics and generated names.** Aliases, detach/edit behavior,
   immutable eviction semantics, and UTF-8/ordinal collision safety.
6. **Bounded results and retained paging.** Independent limits, streaming/simple
   fallback, timeout, zero-row metadata, exact counts, and F1 paging.
7. **Notebook UX/accessibility.** Focus/editor parity, prompts/history/help,
   highlighting/selection, source/error cues, and theme/terminal fallbacks.
8. **Performance pass.** Remove hidden rendering/whole-document work, cache
   derived state, add benchmarks/phase timings, and set latency/memory budgets.
9. **Workflow features.** Run All/dependents, named/multiple sources, notebook
   files, and larger-document operations.

## Validation matrix for the follow-up

Use the existing targeted notebook/unit suite plus PostgreSQL integration and
live tmux checks. At minimum, the completed follow-up should demonstrate:

- Classic paging, query execution, schema actions, `Alt+M`, completion, history,
  grid editing, reconnect, and cancellation still work.
- Notebook source/refinement/latest-reference flows work with normal, aliased,
  restored, evicted, edited, cleared, deleted, and reordered cells.
- PostgreSQL tests cover read-only candidates, valid non-materializable SQL,
  write-producing CTE/INTO exclusion, pseudo-types/parameters, trailing
  comments/escape strings, zero rows, duplicate UTF-8 names, privilege/recovery,
  exact row/byte boundaries, immutable versions, replaced relations, and
  transaction/cancellation races.
- Large results remain bounded with snapshots both on and off; focused retained
  results can reach all retained rows without loading them all at once.
- Keyboard-only, mouse, tmux, narrow 60x20, dark/light/partial custom theme,
  and monochrome/ASCII terminal flows remain legible and usable.
- A 100-cell notebook and long SQL composer meet explicit frame/input-latency
  and memory targets without rendering hidden Classic content.

No snapshot path may execute a user statement twice, silently redirect an
immutable dependency, commit an unknown user transaction, read/drop an
untracked relation, or buffer an unbounded server result in the TUI process.
