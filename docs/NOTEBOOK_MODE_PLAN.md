# Notebook Mode Implementation Plan

Status: proposed  
Worktree: `/Users/fcoury/code/tsql.feat-notebook-mode`  
Branch: `feat/notebook-mode`  
Base: `ui-improvements` at `d54379c`  
Date: 2026-07-10

## 1. Recommendation

Add an opt-in `WorkspaceMode::Notebook` that replaces only the main
query/results split. The existing connection/schema sidebar, overlays, and
full-width status strip remain available.

The notebook should combine two interaction models:

- OpenCode's visual rhythm: elevated composer cards with a colored left rail,
  sparse metadata, and responses rendered directly on a dark canvas.
- Jupyter's document model: ordered cells, a selected-cell command state,
  explicit editor and result focus, durable inline outputs, and top-to-bottom
  navigation.

For PostgreSQL refinement, the MVP should create a bounded, session-scoped
temporary snapshot while the source cell is originally executed. A downstream
cell then queries that snapshot with normal PostgreSQL SQL. This preserves
PostgreSQL types, NULLs, custom types, and execute-once semantics.

The notebook domain itself must not depend on PostgreSQL. It asks the active
database adapter for one of four capabilities: native snapshot, bounded client
snapshot, explicit live lineage, or unavailable. PostgreSQL is the first
provider; SQLite can use connection-local TEMP tables when a SQLite driver is
added; MongoDB can later use bounded retained BSON with `$documents`.

Do **not** describe that implementation as an in-memory table. PostgreSQL temp
tables are session-local but can use server storage. The UI should call it a
`session snapshot` and show whether it is complete. Literal client-memory SQL
would require a typed result layer plus a separate engine such as DataFusion or
DuckDB; that is a later, explicitly labelled backend with different SQL
semantics.

## 2. Locked product decisions for the first release

1. Classic mode remains the default and must remain behaviorally unchanged.
2. `WorkspaceMode` is separate from the existing Vim `Mode` enum.
3. The notebook owns one vertically scrolling document and at most one empty
   trailing draft composer. Refine may insert a non-empty `NEEDS RUN` cell.
4. Inputs look like OpenCode composers; outputs are unboxed on the canvas.
5. Only one database execution runs at a time, but users may navigate and edit
   other cells while it runs.
6. Query errors are durable inline cell outputs, not global modal errors.
7. Existing grid search/copy/export/detail behavior is reused for focused
   notebook results. Result-row editing is deferred.
8. Refinement is enabled only for a complete, retained result snapshot.
9. A truncated/preview result is never silently exposed as a complete table.
10. The MVP core ships a database-neutral refinement contract with PostgreSQL
    as its first provider. Mongo cells may execute in notebook mode, but Refine
    is initially disabled. SQLite refinement ships with SQLite driver support,
    not as part of the PostgreSQL notebook milestone.
11. Results and server snapshot handles are not persisted across app restarts.
    Cell source, order, lineage, selection, and collapse state are persisted.
12. Completing a query never steals focus or scroll position if the user has
    navigated away.

## 3. Goals and non-goals

### Goals

- Let users build an exploratory SQL narrative without replacing the current
  database/session model.
- Make `new query`, `edit and rerun`, and `refine this result` distinct actions.
- Keep source, output, timing, completeness, and lineage visible together.
- Preserve existing Vim editing, completion, history, AI assistance, schema
  insertion, grid navigation, cancellation, and theming where applicable.
- Remain usable at 60x20, responsive on wide terminals, keyboard-complete, and
  understandable without color.
- Provide exact, explicitly labelled snapshot semantics for every backend that
  advertises a supported refinement provider.

### Non-goals for the MVP

- `.ipynb` compatibility or a general notebook file format.
- Concurrent/background cell execution.
- Automatic dependency-graph execution or reactive reruns.
- Cell reordering, branching visualization, charts, or Markdown cells.
- Editing database rows through a materialized notebook result.
- Refining only the visible/truncated rows.
- Silently rerunning an upstream query as a CTE.
- Embedding DuckDB/DataFusion or emulating SQL filters in application code.
- Adding SQLite or another database driver as part of the PostgreSQL notebook
  milestone.
- Supporting PostgreSQL transaction/statement poolers for snapshot refinement.

## 4. UX specification

### 4.1 Layout

The main workspace becomes a borderless vertical document:

```text
 CONNECTIONS/SCHEMA     NOTEBOOK CANVAS
 ──────────────────     ┃ CELL 1 · SQL · main/app · SUCCEEDED
                        ┃ select customer_id, sum(total)
                        ┃ from orders group by customer_id;
                        ┃ Ctrl+E run · r refine · n new

                          ✓ SELECT · 842 rows · 318ms
                          SESSION SNAPSHOT · result 1.2 · COMPLETE
                          customer_id │ sum
                          ────────────┼────────
                          1042        │ 918.20
                          … 834 more · o inspect result

                        ┃ CELL 2 · SQL · from cell 1/run 2 · NEEDS RUN
                        ┃ select * from @result_1
                        ┃ where sum > 500;
                        ┃ Ctrl+E run · Esc cell mode

                        ┃ DRAFT · SQL · main/app
                        ┃
                        ┃ Ctrl+E run · n new
 ─────────────────────────────────────────────────────────────
 NOTEBOOK · CELL · 2/3 · main/app · Ready
```

Visual rules:

- The canvas uses `ui.notebook.canvas`, falling back to `ui.background`.
- A composer uses an elevated surface, two columns of horizontal padding, one
  top row of padding, and a left accent rail. Avoid a full box border.
- The active cell has both an accent rail and a textual focus/state label.
- Cell metadata is compact and secondary metadata disappears first at narrow
  widths.
- Composer language is explicit: `SQL` for PostgreSQL and `MONGOSH` for Mongo.
- Logical `@result_N` references render as a distinct lineage token even if the
  underlying SQL grammar marks `@` as nonstandard; completion can offer only
  result versions retained by the current notebook.
- Output begins under the composer with the same indentation as OpenCode
  responses. It has no surrounding card or zone title.
- Result tables retain header/separator styling but not the classic RESULTS
  pane chrome.
- A short document places the trailing draft near the bottom, like OpenCode's
  composer. Once content exceeds the viewport, the draft is an ordinary final
  document cell rather than a permanently fixed overlay.
- Historical cells do not show a second blinking cursor while another cell is
  being edited.

### 4.2 One selected cell, three notebook focus states

Notebook focus is an outer state; Vim mode remains an inner editor state:

```rust
enum NotebookFocus {
    Cell,
    Editor,
    Result,
}
```

The status strip must expose both layers:

```text
NOTEBOOK · CELL · 3/8
NOTEBOOK · EDITOR · NORMAL · 3/8
NOTEBOOK · EDITOR · INSERT · 3/8
NOTEBOOK · RESULT · cell 3 · row 12/842 · col 4/19
```

Behavior:

- `Cell`: keys operate on whole cells and move through the document.
- `Editor`: the selected cell's `QueryEditor` receives existing Vim/editor
  behavior.
- `Result`: the selected cell's `GridState` receives existing grid behavior.
- `Esc` from Result returns to Cell.
- With Vim enabled, the first `Esc` in Insert returns to editor Normal; a
  second `Esc` returns to Cell.
- Async completion updates the target cell but does not change selection,
  focus, or document scroll.
- If another cell is already executing, Execute leaves the requested cell
  unchanged and reports `cell N is running`; the MVP has no queue.

### 4.3 Default notebook bindings

All actions must be represented in `Action`, configurable through the keymap,
and discoverable through help and `:` commands. Modified Enter may be an alias,
never the only binding, because legacy terminals often cannot distinguish it.

| Context | Action | Default |
|---|---|---|
| Cell | previous/next cell | `k` / `j`, Up / Down |
| Cell | first/last cell | `gg` / `G`, Home / End |
| Cell | enter editor | `Enter` or `e` |
| Cell | inspect result | `o` |
| Cell | execute in place | `Ctrl+E` |
| Cell | execute and advance | `E`; modified Enter may be an alias |
| Cell | select/create unrelated trailing draft | `n` |
| Cell | refine selected result | `r` |
| Cell | collapse/expand output | `z` |
| Cell | delete cell | `dd`, confirm non-empty/executed cells |
| Cell | page document | PageUp / PageDown, `Ctrl+U` / `Ctrl+D` |
| Cell | return to trailing draft | `G` when draft is last |
| Editor | execute in place | existing `Ctrl+E` |
| Editor Normal | execute in place | existing `Enter` behavior |
| Editor | leave editor | second `Esc` from editor Normal |
| Result | leave result | `Esc` |
| Any | cancel running query | existing `Ctrl+C`/cancel action |

Footer hints show at most the two or three most relevant actions for the
current focus. Never overload `Esc` as both leave-focus and cancel-query.

### 4.4 New, rerun, and refine are separate

**New query**

- Selects the existing empty tail composer. If the tail contains source text,
  preserve it as `NEEDS RUN` and append exactly one new empty tail composer.
- Inherits live connection/database/transaction context only.
- Has no dependency on an earlier result.
- Shows `source: database`.

**Edit and rerun**

- Edits a historical cell in place.
- The old output remains visible but is immediately labelled `STALE OUTPUT`.
- Successful rerun replaces that cell's output and snapshot.
- Failed/cancelled rerun does not present the old output as current.

**Refine result**

- Creates a cell immediately below the selected source cell.
- Records a structural dependency on an immutable executed result version.
- Prefills an immediately runnable PostgreSQL query:

  ```sql
  SELECT *
  FROM @result_3
  ```

- Shows `from cell 3 · run 2 · PostgreSQL · session snapshot`.
- If unavailable, `r` remains discoverable but reports a precise reason.
- A source rerun never silently rebinds this dependency. Offer `rebase to cell
  3 run 3` after a new source snapshot commits.

### 4.5 Cell lifecycle and output contract

Every rendered state/freshness badge uses text/symbols in addition to color:

- `DRAFT`
- `NEEDS RUN`
- `DIRTY`
- `RUNNING`
- `SUCCEEDED`
- `FAILED`
- `CANCELLED`
- `STALE OUTPUT`
- `STALE DEPENDENCY`
- `SOURCE UNAVAILABLE`
- `SNAPSHOT EVICTED`

Output completeness is independent of run status:

```rust
struct ResultExtent {
    fetched_rows: usize,
    total_rows: TotalRows,
    retained_rows: Option<usize>,
}

enum TotalRows { Exact(usize), AtLeast(usize), Unknown }
```

The UI must distinguish:

- `SESSION SNAPSHOT · COMPLETE · 842 rows` — refinement enabled.
- `12 visible · 2,000 fetched · at least 2,001 total · no snapshot` —
  refinement disabled. Visible rows, fetched rows, total extent, and retained
  snapshot size are never conflated.
- `LIVE QUERY` — a future explicit fallback that reruns upstream SQL.
- `LOCAL MEMORY · DataFusion/DuckDB` — a possible future backend whose engine
  and dialect must be named.

### 4.6 Scrolling and result inspection

- In Cell focus the document owns vertical wheel/page scrolling. In Editor
  focus a composer capped by `composer_max_rows` and available viewport space
  owns cursor-driven internal editor scrolling; page commands may still move
  the document when explicitly invoked.
- Selection is stable by `CellId`, never by screen row.
- Use cell-aligned outer scrolling for the MVP. Composer/output caps guarantee
  the selected cell fits; render only complete cells that fit rather than
  restarting `HighlightedTextArea` or `DataGrid` inside a clipped rectangle.
- Treat the short-document spacer that bottom-aligns the tail composer as
  visual layout only, never as logical scroll height.
- Cache measured heights to avoid jumps as off-screen cells update.
- Auto-scroll only enough to reveal the selected composer or focused result.
- A result preview is bounded to roughly 40% of the viewport, with a frozen
  header and a visible range/footer.
- Result focus activates its internal vertical/horizontal grid scrolling.
- `Esc` returns scroll ownership to the document.
- If a running off-screen cell updates, show a `↓ cell N updated` affordance;
  do not jump to it.
- Support independent collapse of source and output in the domain model, even
  if source collapse ships after output collapse.

### 4.7 Switching between Classic and Notebook

- Keep independent Classic and Notebook workspace state; switching modes never
  destroys the other workspace.
- On the first switch to Notebook, copy a non-empty Classic editor buffer into
  the initial notebook draft. Do not import the Classic grid as a refinable
  result because no snapshot relation exists; show `run to create snapshot`.
- Later switches restore the last selected cell, document scroll, and focus.
- Switching while a cell runs is allowed after target-aware events land. The
  status strip shows `notebook cell N running`; Classic execution remains
  disabled until the single active execution finishes or is cancelled.
- Leaving Notebook does not drop its retained snapshots. Disconnect/reconnect,
  eviction, deletion, or backend-session close on app exit owns that lifecycle.
- If Notebook is entered while Classic has an Active/Failed transaction,
  ordinary cells may execute against that current transaction, but composers
  show `TXN`, snapshot refinement remains disabled, and the status tells the
  user to switch to Classic for COMMIT/ROLLBACK in the MVP.
- On restart, restored cells contain source and lineage only. They render
  `NEEDS RUN`; dependencies render `SOURCE UNAVAILABLE · run/rebase cell N`
  because no output or PostgreSQL backend snapshot is persisted.

Deletion policy is locked: `dd` opens `ConfirmContext::DeleteNotebookCell` for
any non-empty, executed, or dependency-bearing cell. The empty tail composer is
protected; deleting it clears its contents and leaves one empty tail. Deleting
a source reports the dependent count, drops its tracked snapshots, preserves
already-rendered downstream output, and marks dependent reruns
`SOURCE UNAVAILABLE`.

## 5. Why the current grid cannot become a safe relation

The current execution path is globally single-target:

```text
QueryEditor
  -> App::execute_query_text
  -> async PostgreSQL/Mongo task
  -> unaddressed DbEvent
  -> App::apply_db_event
  -> one global GridModel/GridState
```

The displayed result is insufficient for exact reconstruction:

- `QueryResult` and `GridModel` store `Vec<Vec<String>>`.
- `simple_query` returns text values.
- SQL NULL is converted to the string `"NULL"`, making it indistinguishable
  from an actual text value containing those four characters.
- Arbitrary expression/CTE types are generally blank; metadata lookup only
  works for simple extracted source tables.
- Results beyond `effective_max_rows` are not held by the app.
- BSON is also flattened into display strings.

Rebuilding a relation with `VALUES`, JSON, or application filters would lose
types, precision, arrays, custom/domain/composite values, binary data, NULL
identity, and unfetched rows. It is therefore explicitly rejected for the MVP.

## 6. Refinement backend architecture

### 6.1 Database-neutral contract

Notebook cells and logical `@result_N` references are database-neutral. The
active adapter supplies the retention and compilation behavior:

```rust
enum RefinementCapability {
    NativeSnapshot(RefinementProviderId),
    ClientSnapshot(RefinementProviderId),
    LiveLineage(RefinementProviderId),
    Unavailable(RefinementUnavailableReason),
}

enum RefinementProviderId {
    PostgresTemp,
    SqliteTemp,
    MongoDocuments,
    LocalArrow,
}

struct RetainedResult {
    provider: RefinementProviderId,
    handle: RetainedResultHandle,
    version: ResultVersion,
    rows: usize,
    retained_bytes: usize,
    connection_generation: u64,
}

struct RetainedResultHandle(u64);

struct CellOutput {
    display: GridModel,
    retained: Option<RetainedResult>,
    // extent, execution metadata, provenance, and grid state omitted here
}
```

`RetainedResultHandle` is opaque outside the provider registry and is never
persisted. Provider managers map it to `PgTempSnapshot`,
`SqliteTempSnapshot`, retained Mongo BSON, or a future Arrow object. Adding an
adapter therefore extends provider dispatch without putting backend resource
types in `NotebookCell`.

The provider contract owns:

- capability/preflight and a typed unavailable reason;
- executing a source exactly once while retaining a complete result when
  supported;
- compiling a structural ResultVersion into an engine-specific source;
- validating lifetime/connection provenance before use;
- cancellation and cleanup;
- row/byte retention accounting;
- the user-visible semantic label and dialect.

Provider selection follows this order:

1. native connection/session snapshot in the source database dialect;
2. bounded client snapshot re-submitted through a native typed mechanism;
3. explicit live lineage that reruns upstream SQL/pipeline;
4. unavailable with a precise reason.

Never silently move a cell between these semantics. The composer/output badge
must identify `SESSION SNAPSHOT`, `CONNECTION SNAPSHOT`, `CLIENT BSON SNAPSHOT`,
`LIVE QUERY`, or `LOCAL MEMORY`, plus the active dialect.

| Backend | First safe refinement provider | Initial status |
|---|---|---|
| PostgreSQL | session-local TEMP CTAS | Notebook MVP |
| SQLite | connection-local TEMP CTAS | ships with future SQLite driver |
| MongoDB 6.0+ | bounded retained BSON via `$documents` | follow-up; disabled in MVP |
| Other SQL engines | adapter-proven native temp relation | per-driver |
| Stateless/unsupported engines | explicit live lineage or none | disabled by default |

#### SQLite provider (with SQLite driver support)

SQLite should implement `NativeSnapshot`; it does not need PostgreSQL,
DataFusion, or DuckDB for refinement. Keep one dedicated SQLite connection
alive and execute database work on a dedicated blocking worker. Opening a new
connection per cell would lose the `temp` schema and would create an unrelated
database for `:memory:` connections.

For an eligible query:

```sql
CREATE TEMP TABLE "__tsql_nb_<nonce>_3_2" AS
SELECT *
FROM (<validated SQLite SELECT>)
LIMIT <snapshot_max_rows + 1>;
```

- retain native SQLite values (`NULL`, integer, real, text, blob), never
  reconstruct from display strings;
- bind `@result_3` to an immutable revision-specific table in `temp`;
- use the same N+1 completeness rule and row/total retention budgets;
- use CTAS-assigned ascending `rowid` for source delivery order in display,
  while still requiring `ORDER BY` for relational guarantees;
- snapshot only from proven autocommit state in the first version;
- cancel with the SQLite interrupt handle/progress mechanism;
- invalidate all handles when the dedicated connection closes or is replaced.

Call this a `CONNECTION SNAPSHOT` by default. SQLite temporary tables may be
memory-backed or file-backed according to its build and `PRAGMA temp_store`.
Only label `LOCAL MEMORY` when the adapter can prove the effective
`temp_store=MEMORY`; a `:memory:` main database alone does not prove where the
separate temp database is stored. Set any desired temp-store policy at
connection initialization; changing it after snapshots exist deletes temp
objects.

SQLite is not currently a TSQL backend. Its delivery therefore also requires a
new DbKind/connection adapter, schema introspection, typed row decoding,
dedicated-worker execution, and interrupt-driven cancellation. The notebook
refinement contract should already accommodate it, but those driver concerns
remain a separate feature track.

#### MongoDB provider follow-up

Do not use `$out` or `$merge` automatically: they write ordinary visible
collections, require write permissions, and are not session-local snapshots.

For MongoDB 6.0+, retain the original `Vec<Document>` before flattening it for
GridModel. If the result is complete and below a conservative BSON byte cap, a
refinement can compile to a database-level aggregation whose first stage is:

```javascript
{ $documents: <retained BSON documents> }
```

Subsequent stages use native MongoDB aggregation semantics. Label this
`CLIENT BSON SNAPSHOT · MONGODB`; it is transferred back to the server for each
refinement. MongoDB versions without `$documents`, oversized/incomplete
results, and non-document outputs keep Refine disabled. A future live-pipeline
fallback may rerun the upstream operation only through an explicit `LIVE`
action.

#### Future adapters and local engines

An SQL adapter may use a native snapshot only if it proves relation lifetime,
connection affinity, type fidelity, bounded retention, cancellation, and
cleanup. A common local DataFusion/DuckDB fallback comes only after a typed
CellValue/Arrow result layer exists. It must display the local engine/dialect
and cannot silently reinterpret source-database SQL.

### 6.2 PostgreSQL semantics

An eligible cell is materialized **during its original execution**, not when a
later Refine action is pressed. This guarantees that refinement observes the
same snapshot shown to the user and does not rerun volatile SQL.

PostgreSQL `CREATE TEMP TABLE AS` evaluates the query once and retains native
types. Temporary tables survive commits with `ON COMMIT PRESERVE ROWS` and are
dropped when the database session ends.

### 6.3 PostgreSQL eligibility/preflight

Add `SnapshotEligibility`; do not reuse the current first-token
`is_row_returning_query` helper.

Before executing a snapshot candidate:

1. Require PostgreSQL, a connected direct/session-affine client, safe
   transaction state, and snapshot feature enabled.
2. Resolve structured `@result_<cell>` tokens outside literals/comments to
   their immutable, safely quoted physical snapshot identifiers. A reference
   without matching dependency metadata is an error, not free-form text
   substitution.
3. Use the existing `tree-sitter-sequel` grammar on the compiled SQL as a
   conservative semantic
   classifier for statement shape, transaction control, data-changing CTEs,
   `SELECT ... INTO`, and SQL-aware source/reference ranges. Unknown or
   error-containing trees are non-materializable. `Client::prepare` and CTAS
   remain the PostgreSQL authorities.
4. Call `Client::prepare` on the compiled SQL. Extended protocol rejects
   multiple statements and exposes output columns and PostgreSQL `Type`
   metadata without executing the query.
5. Require no unbound parameters and at least one output column.
6. Initially accept one `SELECT`, read-only `WITH ... SELECT`, `VALUES`, or
   `TABLE` statement.
7. Reject `SELECT ... INTO`, locking clauses (`FOR UPDATE/SHARE`), `SHOW`,
   `EXPLAIN`, `COPY`, transaction control, DML/DDL, data-changing CTEs, and
   `INSERT/UPDATE/DELETE/MERGE ... RETURNING` in the first release.
8. Reject pseudo output types such as anonymous `record` or `void` from
   snapshotting. Because prepare discovers these before execution, run the
   source normally once and mark it non-refinable.
9. Normalize empty/duplicate output names through an explicit CTAS column list
   (`id`, `id_2`, `column_3`) and show those names in the output.
10. Enforce PostgreSQL's 63-byte identifier limit on generated table/column
    names. Reserve suffix space before UTF-8-safe truncation so normalized
    duplicates remain unique.
11. Use a SQL-aware trailing-semicolon scanner. Do not use
   `trim_end_matches(';')` or reject semicolons inside strings/comments.
12. Check known capabilities with
   `has_database_privilege(current_database(), 'TEMP')` and
   `pg_is_in_recovery()`. The actual CTAS remains authoritative.

If eligibility is known to be unavailable before execution, run normally and
return a non-refinable result. If an attempted snapshot fails unexpectedly, do
not automatically rerun the source normally: volatile/nontransactional effects
such as sequence advancement might happen twice. Offer an explicit `Run without
snapshot` action.

### 6.4 PostgreSQL bounded transactional flow

Configuration defaults:

```toml
[notebook]
snapshot_mode = "auto" # auto | off
snapshot_max_rows = 2000
snapshot_max_bytes = 67108864
snapshot_total_bytes = 134217728
max_retained_snapshots = 8
composer_max_rows = 10
output_preview_rows = 12
live_refinement_fallback = false
```

For cell 3, run 2:

```sql
-- transaction started by tokio_postgres::Client::transaction()
SET LOCAL statement_timeout = <configured milliseconds>;

CREATE TEMP TABLE "__tsql_nb_<nonce>_3_2" (
    "__tsql_row_ordinal_<nonce>", "id", "name", "total"
) ON COMMIT PRESERVE ROWS AS
SELECT row_number() OVER (), "__tsql_source".*
FROM (
    <validated source SQL>
) AS "__tsql_source"
LIMIT <snapshot_max_rows + 1>;

-- Inspect row count and pg_total_relation_size(...).
-- Fetch rows for display in stable ordinal order:
SELECT "id", "name", "total"
FROM pg_temp."__tsql_nb_<nonce>_3_2"
ORDER BY "__tsql_row_ordinal_<nonce>";

-- transaction committed through the Rust transaction handle
```

Use exactly one transaction mechanism: acquire the mutable SharedClient guard,
create `tokio_postgres::Transaction`, issue `SET LOCAL statement_timeout` from
`connection.query_timeout_secs` when nonzero (skip the statement when it is
zero), and call transaction commit or rollback. The SQL above illustrates the
server sequence.

Every successful backing table is immutable and revision-specific. The source
cell exposes only a logical `@result_3` reference; dependency metadata binds it
to cell 3/run 2 and compilation substitutes the collision-safe physical table.
No stable per-cell temp view is replaced, so a failed rerun cannot silently
redirect an older dependency. Physical names stay out of normal UI, source
history, and exported SQL unless a diagnostic view is requested.

The worker holds the existing SharedClient mutex, publishes only after the
transaction has finished, and explicitly rolls back every materialization
error/cancellation path before releasing the guard. Generated wrapper errors
retain an internal/source classification, and PostgreSQL source positions are
remapped through a source map covering both wrapper offsets and expanded
`@result_N` tokens back into cell coordinates.

Limit behavior:

- `rows <= snapshot_max_rows` and bytes within budget: complete snapshot;
  commit relation and enable Refine.
- `rows == snapshot_max_rows + 1`: fetch up to the GridModel/connection row
  limit, drop staging inside the transaction, commit, label Preview, and
  disable Refine.
- byte limit exceeded: fetch the bounded display result, drop staging, commit,
  and disable Refine.
- later, an explicit `Refine first N preview rows` may be added, but it must not
  be called a complete snapshot.

Do not roll back an otherwise successful source solely because it exceeded a
retention limit: SELECT can call state-changing functions, so commit versus
rollback must not depend on result size. Rollback is reserved for execution,
materialization, cancellation, or cleanup errors.

`output_preview_rows` is only the on-screen result viewport height.
`connection.max_rows` bounds rows fetched into GridModel/search/export/detail.
`snapshot_max_rows + 1` decides whether a complete snapshot may be retained.
They are independent and all four quantities (visible, fetched, total,
retained) are shown accurately.

The hidden ordinal preserves source delivery order for display and is omitted
from the public view. Tables remain logically unordered: a refinement whose
result order matters must still include its own `ORDER BY`.

### 6.5 PostgreSQL transaction safety prerequisite

`DbSession::in_transaction` is currently unreliable: `CommandComplete` is
converted to `"N rows"`, then later code looks for tags beginning with `BEGIN`,
`COMMIT`, or `ROLLBACK`.

Before snapshot execution:

- introduce `TransactionState::{Idle, Active, Failed, Unknown}`;
- initialize Idle only on a fresh connection;
- use the conservative SQL classifier on every submitted statement, including
  `COMMIT/ROLLBACK ... AND CHAIN`;
- move unclassified multi-statements, `CALL`, and ambiguous control flow to
  Unknown;
- mark errors inside an active transaction as Failed;
- reset on connection generation changes;
- prohibit snapshot execution unless state is Idle;
- prohibit manual transaction-control cells in Notebook MVP, while Classic
  retains existing commands;
- never let an internal snapshot `COMMIT` commit a user transaction.

Recover Failed/Unknown only through a successfully classified ROLLBACK or a new
connection. `tokio-postgres` does not expose the wire protocol's ReadyForQuery
transaction byte through this path, so optimistic inference is not acceptable.
The conservative reducer is: BEGIN/START -> Active; successful plain
COMMIT/ROLLBACK -> Idle; `AND CHAIN` -> Active; any database error while Active
-> Failed; ambiguous/multi-statement/CALL -> Unknown.

The app-owned snapshot transaction is safe only after this prerequisite. A
later design may support snapshots created inside user transactions with
explicit rollback invalidation.

### 6.6 PostgreSQL connection affinity and validation

Each retained snapshot records:

```rust
struct PgTempSnapshot {
    result_version: ResultVersion,
    logical_reference: String,
    physical_name: String,
    relation_oid: u32,
    connection_generation: u64,
    backend_pid: i32,
    row_count: usize,
    byte_size: u64,
}
```

Before running a refinement, validate the same backend and relation:

```sql
SELECT pg_backend_pid(),
       to_regclass('pg_temp.__tsql_nb_<nonce>_3_2');
```

Perform validation and the downstream materialization inside the same database
transaction so no validation/use race exists. Compare the resolved relation
OID as well as backend PID/generation; never drop an untracked object and never
use `CASCADE` during cleanup.

Invalidate refinement handles on disconnect, reconnect, connection loss,
backend PID change, notebook clear, cell deletion, eviction, or missing
relation. Keep the old display snapshot visible with `SNAPSHOT EVICTED`.

PgBouncer transaction and statement pooling are incompatible with cross-cell
temp state. Exact refinement requires a direct PostgreSQL connection or
PgBouncer session pooling. `SHOW pool_mode` is generally limited to PgBouncer's
admin console, so detection is best-effort and cannot prove future affinity.
Add a saved-connection capability `temp_relations = auto | session | disabled`:
`session` is an explicit user assertion, `disabled` covers transaction/statement
poolers, and `auto` performs probes plus use-time validation while labelling the
capability best-effort. Surface a precise reason when affinity is lost.

### 6.7 PostgreSQL cancellation and cleanup

- Give every run an `ExecutionId`; cancellation and completion are matched by
  execution, cell, revision, and connection generation.
- A cancel request is best-effort. Do not emit final `Cancelled` immediately;
  wait for the query future/server response.
- Roll back the staging transaction on cancel/error.
- Keep prior immutable result versions only while retention permits; a failed
  rerun never redirects dependents away from their recorded version.
- On cell deletion/eviction, best-effort drop tracked backing tables.
- PostgreSQL backend-session teardown is the final cleanup guarantee. A
  frontend disconnect through a proxy does not necessarily terminate/reset the
  backend, which is another reason pooled temp relations require explicit care.
- Keep total retained snapshots within a configurable LRU budget. Never evict a
  currently running source; mark dependents stale when a required source is
  evicted.

PostgreSQL's `temp_file_limit` does not cover explicit temp tables. The row cap,
post-creation `pg_total_relation_size` check, LRU budget, statement timeout, and
server-side permissions are the practical safeguards. The byte check limits
retained size, not peak staging allocation.

### 6.8 PostgreSQL refinement fallback matrix

| Situation | Execute cell | Refine |
|---|---:|---:|
| Complete eligible PostgreSQL result | yes | session snapshot |
| Result exceeds row/byte cap | preview | disabled with limit reason |
| PostgreSQL lacks TEMP privilege | normal execution | disabled |
| Hot standby/read-only server rejects temp DDL | normal execution | disabled |
| Active/failed/unknown user transaction | normal execution | disabled |
| Multi-statement or non-materializable output | normal execution | disabled |
| Pseudo output type known from prepare | one normal execution | disabled |
| Unexpected CTAS failure after attempt | no automatic rerun | explicit retry without snapshot |
| Connection changed/backend missing | keep display | stale/disabled |
| PgBouncer transaction/statement pooling | normal execution | disabled |
| MongoDB | normal notebook execution | disabled |

Disabled reasons are typed domain values, not ad hoc status strings.

## 7. Application architecture

### 7.1 Domain model

```rust
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct CellId(u64);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ExecutionId(u64);

enum WorkspaceMode {
    Classic,
    Notebook,
}

struct NotebookState {
    cells: Vec<NotebookCell>,
    selected: CellId,
    focus: NotebookFocus,
    scroll: NotebookScroll,
    next_cell_id: u64,
    tail_follow: bool,
}

struct NotebookCell {
    id: CellId,
    revision: u64,
    language: CellLanguage,
    editor: QueryEditor,
    editor_scroll: (u16, u16),
    execution: CellExecutionState,
    freshness: SourceFreshness,
    output: Option<CellOutput>,
    dependency: DependencyState,
    collapse: CellCollapse,
}

struct ResultVersion {
    source_cell: CellId,
    source_execution: ExecutionId,
    source_revision: u64,
}

enum CellLanguage { PostgresSql, SqliteSql, MongoShell }
enum CellExecutionState {
    NeverRun,
    Running(ActiveCellRun),
    Succeeded(CompletedCellRun),
    Failed(CompletedCellRun, QueryFailure),
    Cancelled(CompletedCellRun),
}
enum SourceFreshness { Clean, Dirty }
enum DependencyState {
    None,
    Available(ResultVersion),
    SourceDirtyButVersionRetained(ResultVersion),
    Unavailable(ResultVersion, RefinementUnavailableReason),
}

struct CellOutput {
    grid: GridModel,
    grid_state: GridState,
    extent: ResultExtent,
    retained: Option<RetainedResult>,
    refinement: RefinementAvailability,
    command_tag: Option<String>,
    elapsed: Duration,
    executed_sql: String,
    executed_revision: u64,
    provenance: ExecutionProvenance,
}

struct ActiveCellRun {
    context: ExecutionContext,
    execution_count: u64,
    started_at: Instant,
    provenance: ExecutionProvenance,
}

struct CompletedCellRun {
    context: ExecutionContext,
    execution_count: u64,
    started_at: SystemTime,
    elapsed: Duration,
    provenance: ExecutionProvenance,
}

struct ExecutionProvenance {
    backend: DbKind,
    saved_connection: Option<String>,
    connection_generation: u64,
    backend_pid: Option<i32>,
    database: String,
    schema: Option<String>,
    transaction_state: TransactionState,
}
```

`QueryEditor` currently owns its own history cursor. Extract reusable editor
buffer state from history navigation so every cell does not duplicate history.
Persistent `History` remains app-level and history selection targets the active
cell.

These dimensions are intentionally orthogonal: a cell can be Running while an
older output is stale, or have a valid already-produced output while its source
snapshot has been evicted. Render label priority is `RUNNING`, `FAILED`,
`CANCELLED`, `DIRTY`, `STALE DEPENDENCY`, `NEEDS RUN`, then `SUCCEEDED`; output
and snapshot badges remain visible as secondary state.

Every editor mutation must go through a revision-aware cell API. Direct
textarea input, completion, AI acceptance, history loading, schema insertion,
external-editor return, and query replacement all increment revision and
recompute output/dependency freshness consistently.

A historical cell is bound to its recorded connection provenance. If the live
connection/database differs, Execute prompts for an explicit rebind (or asks
the user to reconnect); it never silently runs the cell against a different
database. A dependent may continue using its immutable prior ResultVersion
while that snapshot is retained, even if the source cell is now dirty or a
rerun failed. A newly committed source result is adopted only through explicit
rebase. Eviction disables future dependent reruns but does not invalidate an
already-produced downstream output.

### 7.2 Target every asynchronous operation

The current `DbEvent` variants have no request/cell identity. Introduce:

```rust
enum ExecutionTarget {
    Classic,
    Notebook(CellId),
}

struct ExecutionContext {
    id: ExecutionId,
    target: ExecutionTarget,
    source_revision: u64,
    connection_generation: u64,
}
```

Carry `ExecutionContext` on QueryFinished, QueryError, QueryCancelled,
RowsAppended, and MetadataLoaded. Ignore any event whose execution, revision,
cell, or connection generation is no longer current.

Replace `DbSession::running: bool` and `active_query_kind` with one
`Option<ActiveExecution>`. Keep one active query globally for the MVP. Allocate
ExecutionId and display execution counts in the app/execution coordinator, not
inside NotebookState, so Classic and Notebook share one identity namespace.

Notebook MVP does not retain a server-side pager per historical cell. It
fetches at most the configured GridModel row limit, closes/drains any cursor,
and publishes a bounded durable result. Classic keeps its existing pager.
Classic `RowsAppended` events validate against the execution ID stored with the
target pager even after foreground ActiveExecution has cleared.

Move command tag, elapsed time, executed SQL, provenance, and completeness into
the target CellOutput/CompletedCellRun. During migration, global
`DbSession.last_command_tag`, `last_elapsed`, `last_executed_query`, and
`paged_query` remain Classic-only presentation state.

Completion, history, AI acceptance, schema template insertion, external editor,
search, confirm prompts, JSON/row detail, and future cell edits also need an
explicit target captured when the action opens:

```rust
enum EditorTarget { Classic, Notebook(CellId) }
enum ResultTarget { Classic, Notebook(CellId) }
```

This prevents a modal or async response from mutating whichever cell happens to
be selected later.

Integrate global focus as `Focus::Notebook` plus `NotebookState.focus`; update
`focus_navigation_action`, `set_focus`, pane cycling, sidebar directional
navigation, Focus::label, and terminal cursor-style selection. In Notebook,
route Esc through NotebookFocus before the current global Escape handler:
Insert -> Normal -> Cell and Result -> Cell never cancel a query. Ctrl+C remains
the explicit notebook cancellation path.

Add central `workspace_has_unsaved_changes()` and target-aware
`query_editor_has_content()` helpers before replacing direct Classic checks in
quit and connection-switch confirmation paths.

Command semantics in Notebook:

- `:refresh` reruns the selected executed cell in place;
- global UI/connection commands remain global and do not create output cells;
- schema templates, history selection, and AI proposals populate the selected
  editor target without executing;
- Mongo helper/query commands execute into the selected MongoShell cell;
- export/generate/detail commands read the selected ResultTarget;
- only user-authored source enters History, never generated CTAS, validation,
  snapshot cleanup, or display-select SQL.

On explicit disconnect, advance/invalidate connection generation, close pager
channels, resolve the active execution safely, and invalidate all snapshot
handles so late events cannot apply.

### 7.3 Rendering decomposition

`app.rs` is already the main integration risk. Extract before adding notebook
branches:

```text
App::run
  render_app
    render_sidebar
    render_classic_workspace
    render_notebook_workspace
    render_status
    render_overlays
```

Add `ui/notebook.rs` with:

- pure cell measurement;
- logical-to-screen viewport translation;
- composer rendering;
- inline output/error/status rendering;
- cell-aligned visible-range rendering (move to slice-aware/offscreen-buffer
  virtualization only if smooth row scrolling is added later);
- focus/cursor placement;
- mouse hit testing through `NotebookLayoutSnapshot`.

Refactor `DataGrid` into reusable chrome and body layers:

- `DataGrid` keeps the classic zone wrapper;
- `GridViewport` renders header/body/scrollbar without a zone;
- both consume the same `GridModel`, `GridState`, and `UiTheme`.

Replace global `render_query_area`/`render_grid_area` assumptions in notebook
mode with per-cell rectangles keyed by CellId and region.

### 7.4 Theme additions

Add component-merged scopes with fallbacks to the existing tonal theme:

- `ui.notebook.canvas`
- `ui.notebook.composer`
- `ui.notebook.composer.focused`
- `ui.notebook.rail`
- `ui.notebook.output`
- `ui.notebook.output.header`
- `ui.notebook.meta`
- `ui.notebook.stale`

Every built-in theme gets coherent values. When a custom theme omits these
scopes, UiTheme resolution explicitly uses the corresponding existing
`ui.background`, `ui.background.elevated`, `ui.text`, `ui.text.muted`,
`ui.accent`, `ui.warning`, and `ui.grid.header` values; unrelated theme scopes
do not inherit automatically.

### 7.5 Structured errors

The current pipeline flattens PostgreSQL errors into a string. Add a structured
`QueryFailure` carrying severity, SQLSTATE, message, position, detail, and hint
where available. Classic mode may continue showing its modal; Notebook renders
the same diagnostic inline and durably.

## 8. File-by-file implementation map

Current load-bearing hotspots (line numbers are from `d54379c`):

- `app.rs:728-790`: `QueryResult` and `PagedQueryState`;
- `app.rs:2110`: targetless `DbEvent`;
- `app.rs:2205`: `DbSession`;
- `app.rs:2431`: global `App` editor/grid state;
- `app.rs:2970`: main render/event loop;
- `app.rs:3751`: key routing and modal precedence;
- `app.rs:4555-4840`: global query/grid mouse rectangles;
- `app.rs:9449-10330`: PostgreSQL/Mongo execution;
- `app.rs:10389`: global event application;
- `app.rs:10774`: status-line composition;
- `ui/editor.rs:83`: `QueryEditor`;
- `ui/grid.rs:193`: `GridState`, with `GridModel`/`DataGrid` later in the file;
- `app/state.rs:1`: focus and mode definitions;
- `config/keymap.rs:11`: action/keymap definitions;
- `session.rs:18`: versioned session persistence.

### Existing files

- `crates/tsql/Cargo.toml`
  - add direct `tree-sitter`/`tree-sitter-sequel` dependencies for conservative
    statement classification and logical-reference rewriting.
- `crates/tsql/src/app/state.rs`
  - add `WorkspaceMode`, notebook focus integration, transaction state, and
    target identifiers/re-exports.
- `crates/tsql/src/app/app.rs`
  - shrink rendering/execution blocks; route workspace, targets, commands,
    events, connection invalidation, and status.
- `crates/tsql/src/ui/editor.rs`
  - separate per-buffer editing from global history navigation.
- `crates/tsql/src/ui/grid.rs`
  - derive/move reusable result model state and extract borderless grid body.
- `crates/tsql/src/ui/theme.rs`
  - resolve notebook styles and helpers with explicit fallbacks from existing
    UiTheme fields; unrelated theme scopes do not inherit automatically.
- `crates/tsql/src/ui/mod.rs`
  - register notebook widgets and result body.
- `crates/tsql/src/config/keymap.rs`
  - add notebook cell actions, `KeymapConfig.notebook`, a Notebook Cell-focus
    keymap, and `App::build_notebook_keymap`; Editor/Result reuse existing maps.
- `crates/tsql/src/config/schema.rs`
  - add startup workspace and `[notebook]` retention/render settings.
- `crates/tsql/src/session.rs`
  - bump schema version; persist notebook source/order/lineage/collapse only.
- `crates/tsql/src/main.rs`
  - restore workspace and optionally accept `--notebook` after the mode is
    stable.
- `crates/tsql/src/ui/help_popup.rs`
  - context-sensitive Cell/Editor/Result help.
- `crates/tsql/src/ui/confirm_prompt.rs`
  - add target-stable notebook deletion/rebind confirmations.
- `crates/tsql/src/config/connections.rs`
  - store optional temp-relation/session-affinity capability for saved
    connections.
- `README.md` and `config.example.toml`
  - document mode, semantics, limits, pool compatibility, and bindings.

### New files/modules

- `crates/tsql/src/app/notebook.rs`
  - notebook reducer/state transitions, dirty/stale propagation, cell actions,
    persistence conversion.
- `crates/tsql/src/app/refinement.rs`
  - database-neutral capability, opaque `RetainedResultHandle` registry,
    structural logical references, provider dispatch, validation/cleanup
    contract, and shared unavailable reasons.
- `crates/tsql/src/ui/notebook.rs`
  - measurement, viewport, composer/canvas/output rendering, hit map.
- `crates/tsql/src/app/execution.rs`
  - target-aware query request/event orchestration shared by Classic/Notebook.
- `crates/tsql/src/app/pg_snapshot.rs`
  - PostgreSQL implementation of the refinement contract: capability probe,
    prepare/eligibility, logical-result compilation, CTAS transaction,
    validation, cleanup, and typed unavailable reasons.

Future driver tracks add `sqlite_snapshot.rs` and `mongo_snapshot.rs` only when
their database adapters land. Provider-specific snapshot resource types remain
inside their managers and never enter `NotebookCell`.

Register both through `app/mod.rs`; do not introduce an otherwise-empty `db`
module in this feature. Put PostgreSQL DbError extraction/position remapping
with the execution layer and reuse it from the existing `format_pg_error` path.

Do not put the entire notebook renderer or snapshot manager back into
`app.rs`.

## 9. Phased delivery plan

Each phase should be a reviewable conventional commit, preserve Classic mode,
run formatting/clippy/unit tests, and avoid committing `.aidocs`.

### Phase 0 — Spikes and characterization

- Add characterization tests around classic execution, paging, cancellation,
  transaction commands, focus, and session restore.
- Prove `Client::prepare` metadata for SELECT/WITH/VALUES/TABLE and rejection of
  multiple statements/pseudo types.
- Prove bounded CTAS, duplicate-column normalization, immutable result-version
  binding, cancellation rollback, volatile execute-once behavior, and type
  fidelity on supported PostgreSQL versions.
- Confirm PgBouncer behavior or add a reproducible compatibility fixture.
- Lock `tree-sitter-sequel` classification, `@result_N` compilation, physical
  identifier length/escaping, and error-position remapping rules.

Exit: no user-visible changes; hard constraints are captured in tests.

### Phase 1 — Target-aware execution seam

- Add `ExecutionId`, `ExecutionTarget`, `ExecutionContext`, and
  `ActiveExecution`.
- Tag every query/paging/metadata/cancel event.
- Ignore stale run/connection events.
- Fix cancellation finalization and transaction-state tracking.
- Extract execution orchestration from `app.rs` while keeping Classic behavior.
- Make paged cursor names execution-specific for Classic, or explicitly retain
  one pager with assertions.

Exit: full existing test suite passes; Classic UI is unchanged.

### Phase 2 — Notebook domain and persistence

- Add `WorkspaceMode`, `NotebookState`, `NotebookCell`, lifecycle reducer, and
  at-most-one empty trailing draft invariant.
- Add the database-neutral `RefinementCapability`, `RetainedResult`, logical
  `ResultVersion`, and typed unavailable-reason contract. No provider is
  enabled in this phase.
- Add `EditorTarget`/`ResultTarget` and active-target accessors.
- Add notebook actions/keymap internally, but do not expose a startup/switching
  path until a minimally usable renderer exists.
- Bump session schema and migrate v1 safely.
- Persist sources/order/selection/lineage/collapse, not outputs.

Exit: domain transitions and persistence round-trip in tests; Classic remains
the only publicly selectable workspace.

### Phase 3 — Composer canvas and navigation

- Extract `render_classic_workspace` and overlays.
- Implement notebook measurement/virtualization and OpenCode-style composers.
- Implement the empty state, bottom-aligned visual spacer, and trailing
  composer as part of the base canvas rather than later polish.
- Add Cell/Editor/Result focus and status-line labels.
- Extract borderless `GridViewport` and show bounded output previews.
- Add inline running, command, failure, cancellation, dirty, and stale states.
- Route keyboard, cursor, completion, search, history, external editor, AI, and
  schema insertion to the selected cell.
- Add layout hit maps and mouse behavior after keyboard behavior is stable.
- Expose `:mode notebook`, `:mode classic`, `:notebook`, startup config, and
  tested Classic/Notebook transition semantics.

Exit: ordinary Postgres/Mongo queries run in ordered cells, with no refinement.

### Phase 4 — PostgreSQL session snapshots

- Implement the first refinement provider: capability/preflight and
  `PgSnapshotManager` behind the neutral contract.
- Gate on reliable idle transaction state and session affinity.
- Materialize eligible source cells with row+1 and byte budgets.
- Publish immutable result versions only for complete results.
- Add Refine action, structural `@result_N` compilation, explicit dependency
  rebase, validation, cleanup, and LRU eviction.
- Surface typed unavailable reasons and explicit run-without-snapshot retry.

Exit: a supported complete result can be refined without rerunning upstream;
unsupported cases are explicit.

### Phase 5 — Parity, polish, and rollout

- Re-enable safe result search/copy/export/detail in notebook output focus.
- Add contextual help, README/config docs, narrow layout, light theme, no-color
  cues, and ASCII-safe fallbacks.
- Add `--notebook` only after config/session switching is stable.
- Profile 100 cells and large previews; remove avoidable full-document work.
- Add screenshots/GIFs and a manual terminal compatibility matrix.
- Keep feature opt-in for at least one release before considering Notebook as a
  default.

Exit: acceptance criteria below pass and Classic regressions remain green.

### Later phases

- Named/renamable logical results and richer dependency references.
- Explicit live PostgreSQL CTE fallback, labelled as rerunning upstream.
- SQLite adapter milestone: add `DbKind::Sqlite`, connection/schema support,
  typed row decoding, a pinned blocking worker, interrupt cancellation, and
  `SqliteSnapshotManager` using connection-local TEMP CTAS.
- Mongo refinement milestone: retain bounded typed BSON and compile dependent
  aggregations through MongoDB 6.0+ `$documents`; keep older versions,
  incomplete results, and oversized payloads disabled.
- Per-driver native providers for other databases only after their relation
  lifetime, type fidelity, connection affinity, limits, cancellation, and
  cleanup are characterized.
- Run above/below/all, duplicate/reorder, outline, export, and notebook files.
- Typed `CellValue`/Arrow ingestion followed by an optional DataFusion or
  DuckDB local-memory engine with its dialect always visible.

## 10. Test strategy

### Unit/domain tests

- at most one empty trailing draft plus non-empty `NEEDS RUN` cells;
- insertion/deletion and selected-cell invariants;
- execute during another active run leaves the requested cell unchanged;
- source edits increment revision and mark output/dependents stale;
- dependencies remain bound to immutable ResultVersion until explicit rebase;
- connection-provenance mismatch requires explicit cell rebind;
- connection generation invalidates snapshots without deleting display output;
- snapshot eviction and disabled-reason transitions;
- v1-to-v2 session migration and corrupt/future schema behavior;
- restored source cells become `NEEDS RUN`; restored dependents become
  `SOURCE UNAVAILABLE` until their exact source result is rerun/rebased;
- capability dispatch selects only the active backend's provider and preserves
  its semantic label/dialect;
- logical result references reject backend, connection-generation, and result-
  version mismatches before execution;
- native/client snapshot handle invalidation keeps display output while making
  dependent execution unavailable with a typed reason;
- SQL-aware statement/semicolon classification;
- `@result_N` rewriting ignores quoted identifiers, single/dollar-quoted
  strings, and line/block comments;
- identifier quoting and duplicate/empty output-name normalization;
- row/byte boundary decisions at `N-1`, `N`, and `N+1`.

### Renderer/buffer tests

- One Dark and GitHub Light cells have explicit legible foreground/background;
- Cell/Editor/Result focus is visible by text/symbol as well as color;
- 60x20 clips secondary metadata but retains state and source completeness;
- composer cursor/selection and syntax highlighting use the active theme;
- output is unboxed and aligned with the response gutter;
- selected-cell auto-scroll reveals without recentering;
- off-screen completion does not move viewport/focus;
- cached cell heights remain stable across off-screen updates;
- result focus owns nested scrolling and Esc returns document ownership;
- mouse hit tests map to the correct CellId and region.

### PostgreSQL integration tests

- ordinary SELECT, CTE, VALUES, TABLE, zero-row result;
- NULL versus text `"NULL"`;
- numeric precision, timestamptz, UUID, JSONB, bytea, arrays;
- enum, domain, composite/custom types where CTAS supports them;
- pseudo-type snapshot rejection with one normal execution;
- duplicate/empty column names are normalized predictably;
- volatile source executes once;
- base-table changes after execution do not alter the retained snapshot;
- chained refine preserves PostgreSQL types;
- source over row/byte cap becomes Preview and cannot refine;
- snapshot `SET LOCAL statement_timeout` honors `query_timeout_secs`;
- failed/cancelled rerun leaves prior committed snapshot intact but stale;
- logical references bind to the exact immutable result version;
- wrapped-query database error positions map back to cell source positions;
- deletion/eviction cleanup;
- reconnect/backend PID change invalidates relation;
- revoked TEMP privilege and hot-standby behavior;
- active/failed transaction gating;
- PgBouncer transaction mode is rejected or fails with the documented reason.

### Future SQLite provider integration tests

These gate the separate SQLite-driver milestone, not the first Notebook
release:

- file-backed and `:memory:` databases retain TEMP snapshots only while the
  pinned connection remains alive;
- TEMP CTAS captures zero rows and the `N-1`, `N`, and `N+1` boundaries;
- NULL, integer, real, text, and blob values retain native SQLite identity;
- immutable logical versions resolve to distinct temp tables and cleanup does
  not redirect dependents;
- CTAS `rowid` preserves source delivery order for display, while refinement
  makes no ordering promise without `ORDER BY`;
- connection replacement and `PRAGMA temp_store` changes invalidate handles;
- the badge says `CONNECTION SNAPSHOT`, and says `LOCAL MEMORY` only when the
  adapter has proved that storage mode;
- interrupt cancellation leaves no published partial snapshot.

### Mongo tests

- cells execute and render normally;
- in the Notebook MVP, Refine is visible but disabled with a Mongo-specific
  provider/version reason;
- switching between Mongo/Postgres invalidates old snapshot capabilities.

The later Mongo provider additionally tests typed BSON retention,
`$documents` pipeline compilation, immutable versions, row/byte/BSON limits,
MongoDB version gating, and cancellation. It must never issue `$out` or
`$merge` implicitly.

### Regression/performance/manual tests

- all existing Classic tests remain unchanged and pass;
- classic paging, refresh, grid edit, AI, history, completion, cancellation, and
  session restore remain functional;
- 100-cell notebook navigation does not flicker or rerender all cell content;
- large results remain bounded in UI and snapshot retention;
- keyboard-only operation with mouse capture disabled;
- legacy terminal, kitty keyboard protocol terminal, tmux, and SSH;
- dark/light/custom theme and monochrome/readability audit.

## 11. Acceptance criteria

The first Notebook release enables PostgreSQL refinement only. SQLite and
MongoDB provider criteria gate their separate adapter milestones. The first
release is complete when:

1. `:mode notebook` replaces query/results with one scrollable top-to-bottom
   canvas while sidebar/status remain functional.
2. Users can create, edit, execute, rerun, delete, navigate, collapse, cancel,
   and inspect cells without a mouse.
3. Executing two cells in sequence produces stable ordered output and async
   completion never steals focus.
4. Editing executed SQL immediately labels its old output stale.
5. Complete PostgreSQL results expose a stable session snapshot and Refine runs
   against it without rerunning the upstream query.
6. Preview/truncated results cannot be mistaken for complete snapshots and do
   not enable Refine.
7. The notebook domain and logical-reference model do not assume PostgreSQL;
   an unsupported provider remains safely unavailable without UI special
   cases.
8. Every unavailable refinement shows an actionable, backend-specific reason.
9. A dependent remains bound to an explicit source result version until the
   user rebases it; source reruns never redirect it silently.
10. Rerunning a historical cell on a different connection/database requires an
   explicit rebind.
11. PostgreSQL versus future/local dialect and snapshot versus live semantics
   are always visible.
12. Connection changes and snapshot eviction retain display output but disable
   invalid refinement safely.
13. A 60x20 terminal and a 100-cell document remain usable.
14. Both built-in themes and partial custom themes remain legible.
15. Classic mode's existing behavior and test suite remain green.

## 12. Risks and mitigations

| Risk | Mitigation |
|---|---|
| `app.rs` has hundreds of direct global editor/grid references | Land target accessors and rendering/execution extraction before notebook behavior |
| Async result lands in wrong cell | Execution/cell/revision/connection IDs on every event |
| Old output appears current after edit | Separate source revision from executed revision and render `STALE OUTPUT` |
| Truncated rows are refined silently | `ResultExtent` separates total/fetched/retained counts; Refine requires a published retained handle |
| CTAS failure causes a double execution | Never auto-fallback after an attempted materialization |
| User transaction is committed accidentally | Reliable transaction state; snapshot only from Idle; notebook transaction control initially prohibited |
| Pooler loses temp relation | Declared connection capability plus best-effort backend/OID validation |
| Temp tables consume server disk | row+1 cap, byte check, LRU, timeout, cleanup, opt-in feature |
| Backend adapters imply different snapshot semantics | One capability contract plus mandatory provider, storage, dialect, and live/snapshot labels |
| SQLite connection is reopened and loses TEMP snapshots | Pin one worker-owned connection and invalidate every handle on replacement or close |
| SQLite TEMP storage is assumed to be RAM | Default to `CONNECTION SNAPSHOT`; claim `LOCAL MEMORY` only after checking effective `temp_store` |
| Mongo refinement exceeds BSON limits or creates server objects | Conservative retained-byte/document caps, `$documents` version gating, and no implicit `$out`/`$merge` |
| Modified Enter is unavailable | ordinary configurable keys are primary; modified Enter aliases only |
| Nested scrolling is confusing | focus text identifies document, editor, or result as the active scroll owner |
| Embedded engine creates dialect surprise | defer and label engine/dialect if later introduced |

## 13. Research basis

### UX and notebook model

- [JupyterLab notebooks: command and edit mode](https://jupyterlab.readthedocs.io/en/stable/user/notebook.html)
- [Jupyter nbformat cell/output model](https://nbformat.readthedocs.io/en/latest/format_description.html)
- [JupyterLab commands](https://jupyterlab.readthedocs.io/en/stable/user/commands.html)
- [VS Code Jupyter notebook interaction](https://code.visualstudio.com/docs/datascience/jupyter-notebooks)
- [OpenCode TUI](https://dev.opencode.ai/docs/tui/)
- [OpenCode keybindings](https://dev.opencode.ai/docs/keybinds)
- [OpenCode prompt source](https://github.com/anomalyco/opencode/blob/dev/packages/tui/src/component/prompt/index.tsx)
- [OpenCode session canvas source](https://github.com/anomalyco/opencode/blob/dev/packages/tui/src/routes/session/index.tsx)
- [Hex SQL cell references](https://learn.hex.tech/docs/explore-data/cells/sql-cells/sql-cells-introduction)
- [Databricks notebook outputs](https://docs.databricks.com/aws/notebooks/notebook-outputs)
- [Kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/)
- [Jupyter accessibility](https://jupyterlab.readthedocs.io/en/stable/getting_started/accessibility.html)

### Database and engine behavior

- [PostgreSQL `CREATE TABLE AS`](https://www.postgresql.org/docs/current/sql-createtableas.html)
- [PostgreSQL temporary tables](https://www.postgresql.org/docs/current/sql-createtable.html)
- [PostgreSQL privileges](https://www.postgresql.org/docs/current/ddl-priv.html)
- [PostgreSQL protocol flow](https://www.postgresql.org/docs/current/protocol-flow.html)
- [PostgreSQL resource limits](https://www.postgresql.org/docs/current/runtime-config-resource.html)
- [PostgreSQL CTE behavior](https://www.postgresql.org/docs/current/queries-with.html)
- [PgBouncer feature compatibility](https://www.pgbouncer.org/features.html)
- [`tokio-postgres::Client`](https://docs.rs/tokio-postgres/latest/tokio_postgres/struct.Client.html)
- [`postgres_types::Kind`](https://docs.rs/postgres-types/latest/postgres_types/enum.Kind.html)
- [SQLite `CREATE TABLE AS` and TEMP tables](https://www.sqlite.org/lang_createtable.html)
- [SQLite in-memory databases and connection lifetime](https://www.sqlite.org/inmemorydb.html)
- [SQLite `PRAGMA temp_store`](https://www.sqlite.org/pragma.html#pragma_temp_store)
- [SQLite operation interruption](https://www.sqlite.org/c3ref/interrupt.html)
- [MongoDB `$documents` aggregation stage](https://www.mongodb.com/docs/manual/reference/operator/aggregation/documents/)
- [MongoDB BSON/document limits](https://www.mongodb.com/docs/manual/reference/limits/)
- [MongoDB `$out`](https://www.mongodb.com/docs/manual/reference/operator/aggregation/out/)
- [MongoDB `$merge`](https://www.mongodb.com/docs/v8.2/reference/operator/aggregation/merge/)
- [DataFusion in-memory engine](https://datafusion.apache.org/user-guide/introduction.html)
- [DuckDB Rust client](https://duckdb.org/docs/stable/clients/rust.html)

## 14. First implementation step

Start with Phase 0 and Phase 1 only. Do not begin the composer renderer until
target-aware execution, cancellation finalization, and transaction-state tests
are in place. Those seams determine whether notebook output and snapshot
lineage can be correct; the visual layer can then be built without embedding a
second execution architecture in `app.rs`.
