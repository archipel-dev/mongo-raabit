# Testing

```bash
cargo test --workspace        # unit tests; live tests skip without a server
cargo test -p mongo-raabit-core   # includes the live tests when a server is reachable
```

## Live server tests

`crates/mongo-raabit-core/tests/live_*.rs` run against a real `mongod`. They use
`MONGO_RAABIT_TEST_URI` (default `mongodb://127.0.0.1:27017`), create databases named
`raabit_test_*` and drop them first, so use a throwaway server. `live_write_paths.rs` runs
every `Write` variant on its own scratch database (`raabit_wp_<test>_<pid>`), dropped at
the end; see [safety.md](safety.md).

When the server is unreachable the tests print `skipping …` and pass. Set
`MONGO_RAABIT_REQUIRE_SERVER=1` to make that a failure instead (CI does); `0` or empty
keeps the skip. `mongosh` on `PATH` additionally enables the mongosh half of
`builtin_shell_and_mongosh`.

### Option 1: tarball (no Docker)

```bash
mkdir -p ~/mongo && cd ~/mongo
curl -fLO https://fastdl.mongodb.org/linux/mongodb-linux-x86_64-ubuntu2404-8.0.4.tgz
tar xzf mongodb-linux-*.tgz && mkdir data
./mongodb-linux-*/bin/mongod --dbpath data --bind_ip 127.0.0.1 --fork --logpath mongod.log
cd /path/to/mongo-raabit
MONGO_RAABIT_REQUIRE_SERVER=1 cargo test -p mongo-raabit-core
```

Pick the tarball for your OS from <https://www.mongodb.com/try/download/community>
(`ubuntu2204`, `macos-arm64`, …). Stop it with `kill $(pgrep mongod)`.

### Option 2: Docker

```bash
docker run -d --name raabit-mongo -p 27017:27017 mongo:8.0
MONGO_RAABIT_REQUIRE_SERVER=1 cargo test -p mongo-raabit-core
docker rm -f raabit-mongo
```

Use `MONGO_RAABIT_TEST_URI` for a server elsewhere, e.g.
`MONGO_RAABIT_TEST_URI='mongodb://user:pass@host:27017/?authSource=admin'`.

## CI

`.github/workflows/ci.yml` runs on pull requests and pushes to main:

- **checks**: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`
  and `cargo test --workspace` on Ubuntu with the Linux libraries egui/eframe need and
  `Swatinem/rust-cache`. No server is available there, so live tests skip.
- **live**: a matrix over MongoDB 7.0 and 8.0 `mongo` service containers
  (`MONGO_RAABIT_TEST_URI=mongodb://127.0.0.1:27017`, `MONGO_RAABIT_REQUIRE_SERVER=1`)
  running `cargo test -p mongo-raabit-core`. A server that is not reachable fails the job.

## Isolated desktop UX checks

Run `raabit --dark --data-dir /tmp/raabit-ux-profile` (or `--light`) to keep
connection profiles and host settings separate from the normal installation.
Use a dedicated local MongoDB port for test data.

The Query sidebar follows the Studio 3T visual-builder workflow: collection results
stay in the center while Filter, Projection and Sort are edited on the right. Run
applies the builder of the active collection; changing collection tabs preserves each
builder. Visual and Text modes share the same query. This is an adaptation, not a claim
of complete Studio 3T feature parity (result-cell dragging remains to be added).

Standalone Search commands reveal the embedded Connections/Collection islands and
return from a host editor to the app without creating duplicate panels. Settings are
a modal SDK dialog and do not create another tab strip. In an IDE,
the same commands continue to open host tool windows. Query-island tests cover both
themes at 1280×800 and 1024×700. The Connections filter has an inset below the island
header divider; collection metadata occupies its own row to avoid covering view controls.

Manual macOS check with the updated SDK/Kit: Search reveals hidden Connections and
Collection islands; field double-click focuses the filter value; `city = Berlin` + Run
returns Ada and Grace from the isolated three-document fixture; Reset + Run restores
all three. Native drag through Computer Use did not produce a drop and remains to be
verified independently; the whole-window pointer-driven drag tests pass.

The fixed-coordinate perf driver now targets the three-island welcome center and the
inset Connections rows. `standalone_layout` guards the welcome, dialog opener and Close
coordinates at the driver's 1280×800 size. Perf runs use `--data-dir` rather than replacing
HOME/XDG variables. The live CI perf job must still confirm the database/tree targets.
