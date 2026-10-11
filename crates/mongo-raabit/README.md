# mongo-raabit (plugin crate)

The views and the Archipel plugin of Mongo Raabit, a clean-room, Studio 3T Pro-class MongoDB
GUI. The same code runs as an Archipel IDE plugin (`dev.archipel.mongo-raabit`: Tools ›
MongoDB, plus the Databases tool window) and as the standalone `raabit` app
(`crates/raabit`, `cargo run -p raabit`).

Three tool tabs per collection:

- **Query**: build the query visually (Builder), write it in shell syntax (Code) or as SQL.
- **Aggregation**: pipeline stages as forms or code, with live per-stage preview.
- **Structure**: Schema Explorer and Index Manager.

| Crate | Path | Purpose |
|---|---|---|
| `mongo-raabit` | `crates/mongo-raabit` | egui views and the `archipel-sdk` plugin (`src/plugin.rs`): Connection Manager, connection tree, Query / Aggregation / Structure tabs with Table/Tree/JSON results |
| `raabit` | `crates/raabit` | standalone desktop app (eframe) hosting the same views |
| `mongo-raabit-core` | `crates/mongo-raabit-core` | headless: connection profiles, SSH tunnels, secrets, metadata, query/paging/CRUD, explain, index/user management, aggregation stages with preview, schema sampler, SQL → MQL, query code generation, import/export |

The core is built on the official [`mongodb`](https://crates.io/crates/mongodb) driver.
External tools (`ssh`, later `mongosh`) are discovered on `PATH` at runtime and never
bundled.

## Tests

```bash
mongod --dbpath /tmp/mongo-data --fork --logpath /tmp/mongod.log   # optional
cargo test --workspace
```

Tests that need a server use `MONGO_RAABIT_TEST_URI` (default
`mongodb://127.0.0.1:27017`) and print a skip message when it is unreachable.
Seed the sample database used for screenshots with
`mongosh crates/mongo-raabit/scripts/seed-sample-data.js`.

View tests render through wgpu: on Linux install `mesa-vulkan-drivers`.
