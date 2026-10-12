# Parity with MongoDB Compass

Where Mongo Raabit stands against MongoDB Compass (feature list from Compass's public
documentation) and, where useful, Studio 3T (public documentation and observable behaviour).
Competitors are named only in this table. Clean-room: no competitor code, icons or artwork.

The query sidebar interaction gaps and acceptance criteria are tracked in
[the Studio 3T query-builder reference](studio3t-query-builder.md). Passing the current
sidebar tests does not establish interaction parity.

## How to read this

**Status**

- **done**: implemented and backed by evidence in the evidence column.
- **partial**: some of the feature exists; the gap is stated.
- **missing**: nothing in the code.
- **implemented, unverified**: present in code, but no test or screenshot proves it today. This is
  never counted as done.

**Evidence** is a test path (`file::test_name`) or a screenshot path.

- **Core** = `crates/mongo-raabit-core`. Live tests (`tests/live_*.rs`) need a server at
  `MONGO_RAABIT_TEST_URI` (default `mongodb://127.0.0.1:27017`) and skip with a message when none is
  reachable. An earlier run used MongoDB Community 8.0.32.
- **UI** = `crates/mongo-raabit` (views on archipel-sdk and archipel-kit), hosted by the standalone
  app `crates/raabit` and by the IDE plugin. Its tests run in `cargo test --workspace`; the flows
  that talk to a server (`tests/app_flow.rs`, `stage_forms.rs`) skip without one, like the core
  live tests. Some UI rows below still say "implemented, unverified" until each gets its
  conformance or audit test and a screenshot.
- **Screenshots** are in `docs/screenshots/`.
- Per CLAUDE.md, every interactive element needs a `kit::testing::conformance` test and every screen a
  `testing::audit` test.

Abbreviations: C = Compass, S3T = Studio 3T. Y = has it, N = does not, P = partly.

## Write safety

Rules and limits: [safety.md](safety.md).

| Feature | Compass | S3T | Mongo Raabit | Evidence |
|---|---|---|---|---|
| Read-only connections | Y | Y | done | `crates/mongo-raabit-core/tests/live_write_paths.rs::a_read_only_connection_refuses_every_write`, `crates/mongo-raabit/tests/confirm.rs::a_read_only_connection_disables_write_actions` |
| Confirm every write with its statement and affected count (on by default) | P | P | done | `confirm.rs::an_inline_edit_asks_first_with_its_set_query`, `::a_delete_is_confirmed_as_destructive`, `::a_shell_script_that_writes_is_confirmed`, `::what_is_confirmed_is_what_runs` |
| Edits never overwrite data changed since it was loaded | P | N | done | `live_write_paths.rs::an_edit_does_not_overwrite_a_value_changed_since_it_was_loaded`, `::a_save_does_not_overwrite_a_document_changed_since_it_was_loaded`, `confirm.rs::a_conflicting_edit_says_so_and_writes_nothing` |
| Outcome checked against the preview | N | N | done | `live_write_paths.rs::outcomes_that_differ_from_the_preview_are_reported`, `confirm.rs::an_outcome_that_differs_from_the_preview_is_reported` |
| Every write leaves exactly the state it shows | n/a | n/a | done | `live_write_paths.rs::every_write_leaves_exactly_the_state_it_shows`, `crates/mongo-raabit-core/tests/write_contract.rs` (property tests) |
| Reads bounded by the query timeout | Y | Y | done | `live_write_paths.rs::reads_stop_at_the_query_timeout` |
| Saved connections survive app updates (versioned, atomic, backed up) | Y | Y | done | `crates/mongo-raabit-core/tests/persisted_data.rs`, `crates/mongo-raabit/tests/persisted.rs` |

## Connections

| Feature | Compass | S3T | Mongo Raabit | Evidence |
|---|---|---|---|---|
| Connection string / URI import and export | Y | Y | done (engine); UI implemented, unverified | `crates/mongo-raabit-core/src/profile.rs::imports_replica_set_uri_with_options`, `::srv_defaults_to_tls_and_masks_password` |
| Saved connections, folders (favorites) | Y | Y | partial: engine done, UI unverified | `profile.rs::store_round_trip_and_folders`; recent-connections list missing |
| Standalone, replica set, sharded, SRV targets | Y | Y | partial | `profile.rs::validation_catches_mistakes`; only standalone verified against a live server |
| SCRAM-SHA-1/256 | Y | Y | partial | `profile.rs::validation_catches_mistakes`; URI generation only. The live tests do not authenticate unless the test server enforces auth, so no authenticated-login evidence |
| X.509, LDAP (PLAIN), AWS IAM | Y | Y | partial: URI generation only | `profile.rs` mechanism enum; no live test |
| Kerberos (GSSAPI) | Y | Y | missing | needs the driver `gssapi-auth` feature, not enabled |
| OIDC / workforce identity | Y | N | missing | none |
| TLS (CA file, client cert, allow invalid) | Y | Y | partial: URI options only | `profile.rs` TLS settings; no live TLS test |
| SSH tunnel (password, key, agent) | Y | Y | partial | `crates/mongo-raabit-core/src/tunnel.rs::builds_forward_arguments`, `::password_auth_requires_secret`; never run against a real sshd |
| Proxy (SOCKS5) | Y | Y | missing | none |
| Secret storage (OS keychain) | Y | Y | partial | `secrets.rs::memory_store_round_trip`; memory store only, OS keychain pending |
| Test connection, server version and topology | Y | Y | done | `live_server.rs::metadata_and_server_info` |
| Actionable connection errors | Y | P | done | `live_server.rs::connection_errors_are_actionable` |
| Read preference, replica set name, app name | Y | Y | partial | `profile.rs` ReadPreference; no live test |
| Multiple simultaneous connections | Y | Y | implemented, unverified | `crates/mongo-raabit/src/tree.rs` (sessions → databases) |

## Databases and collections

| Feature | Compass | S3T | Mongo Raabit | Evidence |
|---|---|---|---|---|
| Browse databases (sizes) and collections (counts) | Y | Y | done (engine); tree UI implemented, unverified | `live_server.rs::metadata_and_server_info`; tree test `crates/mongo-raabit/tests/app_flow.rs::connect_browse_query_and_page` (live) |
| Views, time series, GridFS shown as such | Y | Y | partial | metadata live test above; no GridFS file browser |
| Create / drop database, collection | Y | Y | implemented, unverified | `connection.rs` `create_database`, `drop_database`, `create_collection`; no live test; no UI yet |
| Create view | Y | Y | implemented, unverified | `connection.rs::create_view`; no test, no UI |
| Rename collection | Y | Y | implemented, unverified | `connection.rs::rename_collection`; no test, no UI |
| Capped, clustered, time series, collation options on create | Y | Y | missing | none |
| Collection stats (size, count, avg doc) | Y | Y | partial | `connection.rs::collection_stats`; tooltip only |
| Tree filter and refresh | Y | Y | implemented, unverified | `app_flow.rs::connect_browse_query_and_page` (live) |

## Documents: CRUD and views

| Feature | Compass | S3T | Mongo Raabit | Evidence |
|---|---|---|---|---|
| Paging, total count, query time | Y | Y | done | `live_server.rs::query_paging_crud_and_explain`, `query.rs::pages_respect_skip_and_limit`; UI: `app_flow.rs::connect_browse_query_and_page`, `docs/screenshots/documents-table-dark.png` (1–50 of 1,200, page size, query time), `tests/documents_screens.rs::the_table_passes_the_audit` |
| Insert, replace, delete, clone documents | Y | Y | done | `live_server.rs::query_paging_crud_and_explain`; UI: `app_flow.rs::edit_insert_clone_delete_explain_and_code`, `tests/documents_screens.rs::the_document_editor_passes_the_audit`, `::the_delete_confirmation_passes_the_audit`, `docs/screenshots/documents-editor-dark.png`, `documents-delete-dark.png` |
| Inline field edit (`$set`) | Y | Y | done | `tests/documents_conformance.rs::editing_a_table_cell_updates_the_server`, `::editing_a_tree_value_updates_the_server` (live round trip), `::a_table_cell_edit_reports_the_set_update`, `::escape_cancels_a_table_cell_edit`, `docs/screenshots/documents-table-edit-dark.png`, `documents-tree-edit-dark.png` |
| Table view with nested drill-down | P (no table drill-down) | Y | done | `results.rs::tests::drill_down_expands_arrays_and_documents`, `tests/documents_screens.rs::the_table_with_a_selection_and_a_drill_path_passes_the_audit`, `docs/screenshots/documents-table-drill-dark.png`, `documents-table-light.png`; sort/reorder/resize via the kit `Table`, `documents-table-sort-dark.png` |
| Tree view | Y (list view) | Y | done | `results.rs::tests::tree_rows_follow_open_set`, `tests/documents_conformance.rs::tree_arrows_expand_and_collapse`, `tests/documents_screens.rs::the_tree_passes_the_audit`, `docs/screenshots/documents-tree-dark.png`, `documents-tree-light.png` |
| JSON view | Y | Y | done (read-only, highlighted; folding waits on a kit fix) | `tests/documents_screens.rs::the_json_view_passes_the_audit`, `docs/screenshots/documents-json-dark.png`, `documents-json-light.png` |
| Syntax-colored document editor with live validation | Y | Y | done | `tests/documents_screens.rs::the_document_editor_passes_the_audit`, `::the_document_editor_shows_a_parse_error`, `docs/screenshots/documents-editor-dark.png`, `documents-editor-light.png` |
| Shell literal syntax (ObjectId, ISODate, regex, Extended JSON) | Y | Y | done | `crates/mongo-raabit-core/src/shell.rs::parses_shell_syntax`, `::accepts_extended_json`, `::round_trips_through_printer`, `::reports_errors_with_offsets` |
| Multi-row selection, bulk edit / delete | P | Y | missing | none |
| Array / subdocument element editing (add/remove field in place) | Y | Y | missing | edit via whole-document editor only |
| Column chooser, column reorder | N | Y | missing | none |
| Transactions / multi-document ops | N | P | missing | none |
| GridFS file browse / upload | N | Y | missing | none |
| Geo / map view of documents | Y (geo query) | N | missing | none |

## Query bar: filter, project, sort

| Feature | Compass | S3T | Mongo Raabit | Evidence |
|---|---|---|---|---|
| Filter, project, sort, skip, limit | Y | Y | done (engine); bar implemented, unverified | `query.rs::parses_query_bar`, `live_server.rs::query_paging_crud_and_explain` |
| Inline parse errors with offsets | Y | Y | done | `shell.rs::reports_errors_with_offsets` |
| Visual query builder (conditions, groups, sort, projection) | P | Y | done (model); UI implemented, unverified | `crates/mongo-raabit-core/src/builder.rs::every_operator_round_trips`, `::all_and_any_nesting`, `::takes_over_supported_queries`, `::invalid_conditions_are_reported_and_skipped`; `crates/mongo-raabit/src/query_builder.rs` tests; `app_flow.rs::visual_query_builder_runs_its_query` (live) |
| Field-name autocomplete in the bar | Y | Y | missing | none |
| Query history and favorites | Y | Y | missing | none |
| Collation, maxTimeMS, hint options | Y | Y | missing | none |
| Distinct values / count shortcut | P | Y | missing | `query.rs::count` exists; no distinct |

## Aggregation

| Feature | Compass | S3T | Mongo Raabit | Evidence |
|---|---|---|---|---|
| Stage list: add, reorder, enable/disable, remove | Y | Y | implemented, unverified (model done) | `crates/mongo-raabit-core/src/aggregate.rs::parses_and_renders_pipelines`, `::stage_errors_name_the_stage` |
| Per-stage input/output preview | Y | Y | done (engine) | `crates/mongo-raabit-core/tests/live_analysis.rs::aggregation_stage_preview`; writing stages (`$out`, `$merge`) never run in previews |
| Operator templates and completion | Y | Y | partial: templates done, in-editor operator autocomplete missing | `aggregate.rs::templates_parse_and_completion_works` |
| Visual stage forms ($match, $group, $sort, $project, $unwind, $lookup, $addFields) | P | Y | done (model); UI implemented, unverified | `crates/mongo-raabit-core/src/stage_form.rs::match_forms_round_trip`, `::group_forms_round_trip`, `::unwind_lookup_and_add_fields_round_trip`; `crates/mongo-raabit/tests/stage_forms.rs::form_built_pipelines_run_on_a_live_server` (live) |
| Full run, results in table/tree/JSON | Y | Y | implemented, unverified | `app_flow.rs::aggregation_sql_schema_and_index_tabs` |
| `$out` / `$merge` run | Y | Y | partial | `live_analysis.rs::aggregation_stage_preview` covers guard; no explicit run test |
| Save / load pipelines, pipeline library | Y | Y | missing | none (see "Beyond: pipelines as files") |
| Import / export pipeline as text | Y | Y | partial | export via code generation only |
| Atlas Search / Vector Search stages | Y | N | missing | none |

## Schema analysis

| Feature | Compass | S3T | Mongo Raabit | Evidence |
|---|---|---|---|---|
| Sampling (random / first / last) | Y (random) | Y | done (engine) | `crates/mongo-raabit-core/tests/live_analysis.rs::schema_sampling`, `crates/mongo-raabit-core/src/schema.rs::analyzes_types_frequencies_and_nesting` |
| Field presence, type mix, nested fields | Y | Y | done (engine); UI implemented, unverified | same |
| Value stats (min/mean/max, histogram, top values, date range) | Y | Y | implemented, unverified | `crates/mongo-raabit/src/schema_view.rs`; no test asserts the histograms |
| Geo coordinate map | Y | N | missing | none |
| Schema export / JSON Schema generation | P | Y | missing | none |

## Explain plan

| Feature | Compass | S3T | Mongo Raabit | Evidence |
|---|---|---|---|---|
| Explain a find (stats, winning plan, COLLSCAN flag) | Y | Y | done | `crates/mongo-raabit-core/src/query.rs::summarizes_explain_reply`, `live_server.rs::query_paging_crud_and_explain`, `tests/documents_screens.rs::the_explain_plan_passes_the_audit` (COLLSCAN banner), `docs/screenshots/documents-explain-dark.png`, `documents-explain-light.png` |
| Visual stage tree with per-stage counts | Y | Y | done | `tests/documents_screens.rs::the_explain_plan_passes_the_audit`, `docs/screenshots/documents-explain-dark.png` |
| Explain an aggregation | Y | Y | missing | find only |
| Raw explain output | Y | Y | implemented, unverified | `crates/mongo-raabit/src/dialogs.rs` |
| Rejected plans, execution time breakdown by shard | Y | P | missing | none |

## Indexes

| Feature | Compass | S3T | Mongo Raabit | Evidence |
|---|---|---|---|---|
| List with keys, size, properties (unique/sparse/TTL/partial) | Y | Y | done (engine); UI implemented, unverified | `live_server.rs::index_manager` |
| Usage stats (`$indexStats`) | Y | Y | done (engine) | `live_server.rs::index_manager` |
| Create (form), drop | Y | Y | done (engine) | `live_server.rs::index_manager` |
| Hide / unhide | Y | Y | done (engine) | `live_server.rs::index_manager` |
| Text, geo, wildcard, hashed, collation options in the create form | Y | Y | partial | verify per type: only key-pattern creation is tested |
| Atlas Search indexes | Y | N | missing | none |

## Import and export

| Feature | Compass | S3T | Mongo Raabit | Evidence |
|---|---|---|---|---|
| Export JSON (array / lines; relaxed, canonical, shell) | Y | Y | done (engine) | `crates/mongo-raabit-core/src/transfer.rs::json_round_trips_in_every_mode`, `tests/live_transfer.rs::export_then_import_round_trip` |
| Export CSV with column discovery | Y | Y | done (engine) | `transfer.rs::csv_export_import_with_mapping`, `::headerless_csv_and_cells` |
| Import JSON / CSV with field types and mapping | Y | Y | done (engine) | same, plus `live_transfer.rs::export_then_import_round_trip` |
| BSON (mongodump layout) export / import | N (via mongodump) | Y | done (engine) | `transfer.rs::bson_round_trip_and_corruption` |
| SQL `INSERT` export / import | N | Y | done (engine) | `transfer.rs::sql_inserts` |
| Insert / upsert / drop-first, per-document failure report, cancel | P | Y | partial | live round-trip test covers insert; upsert/cancel untested |
| Import/export wizard UI | Y | Y | missing | engine only |
| Export query results, export from aggregation | Y | Y | missing | none |
| Task scheduler / saved jobs | N | Y | missing | none |
| Copy collection between connections | N | Y | missing | none |

## Shell

| Feature | Compass | S3T | Mongo Raabit | Evidence |
|---|---|---|---|---|
| Embedded shell (mongosh) | Y | Y | partial | `live_server.rs::builtin_shell_and_mongosh`: runs the installed mongosh, one process per run, no session state, no interactive `it` |
| Built-in evaluator (no mongosh installed) | N | N | partial | `live_server.rs::builtin_shell_and_mongosh`, `shell_eval.rs::splits_statements`, `::parses_call_chains`; `show`, `use`, `db.<coll>.*`, `runCommand`; no arbitrary JavaScript |
| Completion (collections, methods, cursor methods) | Y | Y | done (engine) | `shell_eval.rs::completes_names_and_methods` |
| Shell UI (editor, console, result tree) | Y | Y | implemented, unverified | `app_flow.rs::shell_tab_runs_scripts` (live) |
| Credentials kept off the command line | Y | n/a | done | `mongosh.rs` passes the URI in the environment; `mongosh.rs::rewrites_database` |
| Persistent history, multiple tabs | Y | Y | missing | none |

## Performance and monitoring

| Feature | Compass | S3T | Mongo Raabit | Evidence |
|---|---|---|---|---|
| Real-time server stats (ops, memory, network) | Y | P | partial | `connection.rs::server_status`; no UI, no test |
| Current operations, kill op | Y | Y | partial | `connection.rs::current_ops`, `::kill_op`; no UI, no test |
| Slow queries / profiler view | Y | N | missing | none |
| Performance advisor (index suggestions) | Y (Atlas) | N | missing | none |
| Users and roles manager | N | Y | partial | `live_server.rs::users_and_roles` (list/create/drop users, list roles); no UI |
| Replica set / sharding status view | N | Y | missing | none |

## Validation

| Feature | Compass | S3T | Mongo Raabit | Evidence |
|---|---|---|---|---|
| Edit JSON Schema validator, level and action | Y | Y | missing | none |
| Validate-as-you-type documents against the validator | Y | Y | missing | none |
| Show failing documents | Y | Y | missing | none |

## AI

| Feature | Compass | S3T | Mongo Raabit | Evidence |
|---|---|---|---|---|
| Natural-language to query / pipeline | Y | N | missing | none (see roadmap; opt-in, no document data sent without consent) |
| AI explain / fix of an error | Y | N | missing | none |

## Code generation

| Feature | Compass | S3T | Mongo Raabit | Evidence |
|---|---|---|---|---|
| Export find to driver code (JS, Python, Java, C#, Go, Rust, PHP, Ruby, shell) | Y (fewer languages) | Y | done | `tests/documents_screens.rs::the_query_code_dialog_passes_the_audit`, `docs/screenshots/documents-code-python-dark.png`; engine: | `crates/mongo-raabit-core/src/codegen.rs::every_language_renders_find_and_aggregate`, `::language_specific_literals`, `::strings_are_escaped` |
| Export aggregation to driver code | Y | Y | done (engine); UI entry for aggregation implemented, unverified | `codegen.rs::every_language_renders_find_and_aggregate` |
| Generated snippets compile / run in CI | P | n/a | missing | syntax was checked by hand during development only |
| Password masked in snippets | n/a | n/a | implemented, unverified | `app_flow.rs::edit_insert_clone_delete_explain_and_code` |

## Misc

| Feature | Compass | S3T | Mongo Raabit | Evidence |
|---|---|---|---|---|
| SQL → MQL (SELECT, JOIN → `$lookup`, GROUP BY, HAVING) | N (Atlas SQL separate) | Y | done (engine); tab implemented, unverified | `crates/mongo-raabit-core/src/sql.rs::simple_select_becomes_find`, `::joins_become_lookup`, `::group_by_with_having_and_order`, `::rejects_unsupported_sql`, `tests/live_analysis.rs::sql_translations_run_on_the_server` |
| Light / dark theme following the host | Y | Y | done | kit `Theme` from the host; `tests/documents_screens.rs` (both themes), `docs/screenshots/*-light.png` |
| Standalone desktop app | Y | Y | done | `crates/raabit` (eframe), `docs/screenshots/01-welcome-dark.png`, `23-light-main-window.png` |
| Runs as IDE plugin | N | N | done (archipel-sdk) | `tests/plugin.rs::manifest_is_valid_and_declares_its_permissions`, `::main_screen_passes_the_kit_audit_in_both_themes` |
| Keyboard navigation, accessibility audit | P | P | missing | `testing::audit` tests required by CLAUDE.md, none yet |
| Auto-update, telemetry opt-out | Y | Y | missing | release tooling in `reference/ide/` pending port |
| Data Modeling / ER diagrams | N | Y (reverse engineering) | missing | none |
| Compare / sync collections | N | Y | missing | none |

## Beyond Compass

Things Compass does not do. Status is `planned`, `partial` or `done` by the same evidence rule.

| Feature | Status | Evidence / note |
|---|---|---|
| Live per-stage previews in the pipeline builder (sampled input and output of every stage, writes guarded) | partial: engine done, UI unverified | `live_analysis.rs::aggregation_stage_preview`. Compass previews the stage you edit; the goal is every stage visible at once, updating as you type |
| Explain-plan visualizer with index advice | partial: visualizer implemented, advice planned | `query.rs::summarizes_explain_reply` flags COLLSCAN; no index suggestion yet |
| Schema analysis with drift detection (compare two samples or time points) | planned | analyzer exists (`schema.rs`); no diff |
| Queries and pipelines as files in git (diffable, reviewable, version controlled) | planned | none |
| Driver code generation for Rust, TypeScript, Java and Python | partial | Rust, Java, Python and JavaScript done in `codegen.rs`; TypeScript output is not distinct from JavaScript; no compile check in CI |
| IDE integration: open the query under the caret, jump to its explain plan | planned | plugin entry exists (`plugin.rs`) but no editor hook |
| SQL → MQL with live translation beside the editor | partial: engine done | `sql.rs` tests above; Compass has no SQL mode |
| SSH tunnel through the system `ssh` (agent, hardware keys, `~/.ssh/config` honoured) | partial | `tunnel.rs` tests; no live sshd test |
| Built-in shell evaluator that works without mongosh | partial | `live_server.rs::builtin_shell_and_mongosh` |
| One code base as standalone app and IDE plugin | partial | plugin done on the old API, `raabit` app planned |
| Table view with nested drill-down (one row per array element) | done | `results.rs::tests::drill_down_expands_arrays_and_documents`, `docs/screenshots/documents-table-drill-dark.png` |
| BSON and SQL-INSERT import/export | done (engine) | `transfer.rs` tests above |

## Sources

Merged from the earlier Studio 3T-oriented write-up that lived under `reference/ide/` (22 items,
MR-001 to MR-022, written for the Jetthing plugin). Corrections made while merging:
the visual query builder is no longer missing (it exists in the model and the UI); the claim that
status was "Implemented" has been downgraded to "implemented, unverified" wherever the only evidence is a
UI test in a crate that is excluded from the workspace, or a screenshot that is not in this repo; no
screenshot paths from the old evidence list are cited.

Compass feature list: public Compass documentation (connections, CRUD, query bar, aggregation
pipeline builder, schema analysis, explain plan, indexes, import/export, embedded shell, real-time
performance, validation, AI assistance). Studio 3T column: public documentation and observable
behaviour only.
