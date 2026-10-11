# Roadmap

Each item is one line and becomes one issue. Status of every feature is tracked honestly in
[parity.md](parity.md). Version numbers are set by release-please, never by hand.

Priorities: daily-use blockers, then speed and consistency budgets, then remaining Compass parity, then beyond Compass.

## v0.1.0: foundation and kit port

Goal: everything that exists today runs on archipel-sdk and archipel-kit, as a standalone app and as an IDE plugin, with CI, release and evidence.

**Workspace and SDK port**
- [ ] Port `mongo-raabit` from the Jetthing plugin API to `archipel-sdk` and re-add it to the workspace (excluded today)
- [ ] Create the `raabit` crate: eframe standalone app hosting the same views
- [ ] Plugin entry point on `archipel-sdk`, built from the same code as `raabit`
- [ ] Replace Jetthing-specific style and palette code with `archipel_sdk::kit` tokens (no private colors or spacing)
- [ ] Request missing kit pieces from the platform owner instead of copying them in

**CI and release**
- [ ] CI: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`
- [ ] CI: live-server tests against a `mongod` service container
- [ ] PR title check (Conventional Commits)
- [ ] release-please config and release workflow (from `reference/ide/`), then delete the copies in `reference/`
- [ ] Release artifacts for Linux, macOS, Windows

**Views on the kit** (each interactive element gets a `kit::testing::conformance` test, each screen a `testing::audit` test)
- [ ] Connection dialog: server, auth, TLS, SSH, advanced pages, URI import, test connection
- [ ] Database / collection tree with filter and context menus
- [ ] Documents: table, tree and JSON views with paging and count
- [ ] Document editor, inline edit, delete confirmation
- [ ] Query bar and visual query builder
- [ ] Aggregation builder with stage forms and per-stage previews
- [ ] Shell tab (mongosh and built-in evaluator)
- [ ] Explain view
- [ ] Index manager and schema explorer
- [ ] SQL tab and Query Code dialog

**Evidence and docs**
- [ ] Mouse-only Xvfb (xdotool) verification of every view, screenshots under `docs/evidence/`
- [ ] Move parity rows from "implemented, unverified" to "done" as tests and screenshots land
- [ ] README, `docs/parity.md`, `docs/roadmap.md`

## v0.2: parity gaps

### 1. Daily-use blockers
- [ ] OS keychain secret store (memory only today)
- [ ] Verify SSH tunnel against a live sshd (password, key, agent)
- [ ] Verify TLS, SRV and SCRAM login against live servers; verify X.509, LDAP, AWS auth
- [ ] Create / drop / rename database, collection and view in the UI
- [ ] Multi-row selection with bulk delete and edit
- [ ] Field-name and operator autocomplete in the query bar and stage editor
- [ ] Query history and favorites
- [ ] Import / export wizard UI (engine exists)
- [ ] Export from query results and aggregation results
- [ ] Explain for aggregations
- [ ] Save / load pipelines
- [ ] Persistent shell session (state between runs) and history
- [ ] Collection options on create: capped, time series, clustered, collation

### 2. Speed and consistency budgets (measured in CI, build fails when exceeded)
- [ ] Cold start to first interactive frame < 400 ms
- [ ] Frame time ≤ 16 ms while scrolling a 10k-row table, resizing and typing
- [ ] Idle CPU < 1% with a connected session and an open collection tab
- [ ] Memory budget: bounded result cache, documented ceiling for browsing a 1M-document collection
- [ ] Audit every view so database work never blocks the UI thread
- [ ] Benchmarks in the repo, tracked per release

### 3. Remaining Compass parity
- [ ] Kerberos (GSSAPI) auth
- [ ] OIDC authentication
- [ ] SOCKS5 proxy
- [ ] Validation: JSON Schema validator editor, level/action, failing documents
- [ ] Server status UI (real-time stats), current operations and kill op
- [ ] Slow query / profiler view
- [ ] Users and roles UI
- [ ] Atlas Search and Vector Search indexes and stages
- [ ] Index form: text, geo, wildcard, hashed, collation options
- [ ] Geo view in schema analysis and queries
- [ ] Array / subdocument element editing
- [ ] Column chooser and reorder in the table view
- [ ] Query options: collation, maxTimeMS, hint
- [ ] GridFS browser
- [ ] AI: natural language to query / pipeline (opt-in, no document data sent without consent)
- [ ] Accessibility and keyboard navigation pass (audit tests clean)

## v0.3+: beyond Compass
- [ ] Live per-stage previews for every stage at once, updating as you type
- [ ] Explain-plan visualizer with index advice
- [ ] Schema analysis with drift detection (diff two samples)
- [ ] Queries and pipelines as files in git (diffable text format)
- [ ] Driver code generation: distinct TypeScript output, compile checks of all snippets in CI
- [ ] IDE integration: open the query under the caret in Raabit
- [ ] IDE integration: jump from a query in code to its explain plan
- [ ] SQL → MQL: subqueries and UNION
- [ ] Compare / sync collections; copy collections between connections
- [ ] Task scheduler for saved import / export jobs
