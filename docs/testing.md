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
