# Mongo Raabit: agent guide

Mongo Raabit is the MongoDB GUI of the Archipel suite, measured against MongoDB Compass and
Studio 3T. It runs standalone (`raabit`) and as an Archipel IDE plugin from the same code.

## Layout

| Crate | Path | Role |
|---|---|---|
| `mongo-raabit-core` | `crates/mongo-raabit-core` | headless engine on the `mongodb` driver. No egui. |
| `mongo-raabit` | `crates/mongo-raabit` | the views and the plugin, on `archipel-sdk` |
| `raabit` | `crates/raabit` | standalone desktop app (eframe) hosting the same views |

`reference/` holds temporary copies of the IDE's release tooling; delete files there once replaced.

## Rules

- Conventional Commits for commits and PR titles (they drive release-please). Never bump versions by hand.
- Merge, never rebase. Squash-merge PRs.
- UI only from archipel-kit components, parts and behaviours, reached through
  `archipel_sdk::kit` (one kit version: the SDK's). No ad-hoc egui painting, no private
  colors/spacing. Missing kit pieces are requested from the platform owner, never copied in.
- Every interactive element gets a `kit::testing::conformance` test; every screen a
  `testing::audit` test.
- Mouse-only verification under Xvfb (xdotool) before a PR; screenshots in the PR.
- Clean-room: public docs and observable behaviour only. No MongoDB Inc. code, icons,
  artwork or trademarks in names.
- Checks before a PR: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`.

## Live server tests

Core tests that need a server read `MONGO_RAABIT_TEST_URI` (default `mongodb://127.0.0.1:27017`)
and skip with a message when it is unreachable.
