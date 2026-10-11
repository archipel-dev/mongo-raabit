# mongo-raabit evidence

Captured under Xvfb (1600×1000, `JETTHING_RENDERER=glow`) from the standalone example
window (1440×900) against MongoDB Community 8.0.32, seeded with
`plugins/mongo-raabit/scripts/seed-sample-data.js` (`raabit_shop`: 240 customers,
1,200 orders, products, a view, a time series collection and a GridFS bucket).
The example will be replaced by `jetthing --app dev.jetthing.mongo-raabit` once the
plugin API (#48) lands.

```bash
cargo run -p mongo-raabit --example standalone -- --uri mongodb://127.0.0.1:27017 \
    --open raabit_shop.customers [--view tree|json] [--drill items] [--light]
cargo run -p mongo-raabit --example standalone -- --demo-profiles --manager
```

There is no Studio 3T reference capture (clean-room: behavior is taken from public
documentation); the look follows the Jetthing Islands theme instead.

| File | Scene | Notes |
|---|---|---|
| `welcome-dark.png` | Welcome tab | quick actions, saved connections with masked URIs |
| `manager-dark.png`, `manager-light.png` | Connection Manager | folders, filter, Server/Auth/TLS/SSH/Advanced pages, live masked connection string |
| `table-dark.png`, `table-light.png` | Collection tab, Table view | connection tree with sizes/counts, view/time series/GridFS icons, query bar, paging, count, query time, type-colored cells, nested values as drill-down pills |
| `drilldown-dark.png` | Table drill-down | `orders` filtered on status/total, drilled into `items` (one row per array element, `#` keeps the source document) |
| `tree-dark.png` | Tree view | expandable documents with key/value/type columns |
| `json-dark.png` | JSON view | syntax-colored mongo shell syntax per document |
| `document-editor-dark.png` | Edit Document | syntax-colored editor with live validation (errors give line and column), Format, Save (`_id`-safe replace) |
| `inline-edit-dark.png` | Inline cell edit | double-click a scalar cell; Enter/focus loss saves with `$set`, Esc cancels; strings stay strings |
| `delete-confirm-light.png` | Delete confirmation | lists the `_id`s to delete |
| `explain-collscan-dark.png`, `explain-index-dark.png` | Explain plan | stats cards, COLLSCAN warning, winning plan as a left-to-right stage flow, raw output toggle |
| `aggregation-dark.png` | Aggregation Editor | stage list (enable/disable, reorder, remove, add from 31 operator templates), stage editor, input/output preview of stage 3 on the first 1000 docs, full-run results |
| `sql-dark.png` | SQL Query | SQL with a JOIN and GROUP BY, the generated `$lookup`/`$unwind`/`$group` pipeline, results |
| `schema-dark.png`, `schema-light.png` | Schema Explorer | presence bars, type-mix bars (validated categorical palette, legend + labels), histogram and top values of the selected field |
| `indexes-dark.png` | Index Manager | keys, properties (unique/sparse/TTL/partial/hidden), sizes, usage, new-index form |
| `shell-mongosh-dark.png` | Shell (mongosh 2.13.0) | multi-line JavaScript run in the installed mongosh; `print` output and the result in the console, parsed result as a tree |
| `shell-builtin-light.png` | Shell (built-in) | the fallback evaluator: `use` and a find with projection/sort/limit, per-statement results |
| `query-code-dark.png` | Query Code | mongo shell plus 8 driver languages, copy; passwords are replaced by `<password>` |

```bash
cargo run -p mongo-raabit --example standalone -- --uri mongodb://127.0.0.1:27017 \
    --open raabit_shop.orders --dialog edit|insert|delete|explain|code [--edit-cell tier]
```

```bash
cargo run -p mongo-raabit --example standalone -- --uri mongodb://127.0.0.1:27017 \
    --open raabit_shop.orders --tool aggregate|sql|schema|indexes [--text …] [--stage N]
```

Chart colors come from the dataviz reference palette, validated with its
`validate_palette.js` against the island surfaces (dark: all checks pass; light: three
slots below 3:1 contrast, so every chart carries text labels and a legend).

Known gaps: import/export wizard, server status and users UI are not in these
slices; credentials are kept in memory only (OS keychain pending).
