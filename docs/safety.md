# Safety contract

People point Mongo Raabit at live databases. The app must be deterministic: it changes
exactly what it showed, only after it was shown (when confirmation is on), never on a
read-only connection, never over data that changed since it was loaded, and a new version
of the app never breaks or destroys what users saved. This page says how, and where it is
enforced.

## One write path

Every change Mongo Raabit can make is a value of `Write`
(`crates/mongo-raabit-core/src/write.rs`): the namespace and the exact arguments, as data.
`Connection::execute` is the only function that changes data or structure; the low-level
write helpers of the connection are private to the core crate. The views, the built-in
shell, imports, the Index Manager, `$out`/`$merge` pipelines and unknown database commands
(`Connection::run_command`) all build a `Write` and hand it to `execute`.

## Read-only connections

A connection is read-only when its profile says so or the "open connections read-only"
setting was on when it opened. `execute` refuses every `Write` on it with
`RaabitError::ReadOnly` before anything is sent to the server; the views also disable the
write actions and refuse in `request_write`. Commands only run unguarded when they are in
`write::READ_COMMANDS` (and an `aggregate` has no `$out`/`$merge`).

## Confirmation: what is shown is what runs

With "Confirm changes" on (the default) every write opens a confirmation first. It shows:

- the title ("Update 1 document in shop.people");
- the exact mongosh statement, `Write::shell()`, guard filters included;
- what it affects, counted by `Connection::preview` with a 4 s limit ("2 documents will be
  modified", "No document matches: nothing will change", or that it could not be counted).

The dialog holds the `Write` value itself. Run hands that value to `execute`; nothing is
rebuilt from the views, so editing the grid, reloading the page or anything else happening
while the dialog is open cannot change what runs (`tests/confirm.rs`,
`what_is_confirmed_is_what_runs`). Deletes always ask, even with confirmation off.

Shell scripts that call write methods are confirmed as a whole script.

## Optimistic concurrency for single documents

An inline cell edit (`Write::SetField`) carries the value the field had when the page was
loaded, and its filter requires it still (`write::set_field_filter`):

```js
db.getCollection("people").updateOne({
  _id: 1,
  name: { $eq: "Ada", $type: "string", $not: { $type: "array" } }
}, { $set: { name: "Ada L." } })
```

`$eq` compares the value (a document is a value, not operators; a regular expression a
value, not a pattern), `$type` its BSON type (so `1` that became `1.0`, or a `null` that
became missing, is a change) and `$not: { $type: "array" }` that it did not become an array
that merely contains it. A field that was missing must still be missing
(`{ $exists: false }`).

The document editor's Save (`Write::ReplaceDocument`) carries the whole document as it was
loaded and only replaces it while the stored document is still exactly that
(`write::replace_filter`):

```js
db.getCollection("people").replaceOne({
  _id: 1,
  $expr: { $and: [
    { $eq: [ "$$ROOT", { $literal: { _id: 1, name: "Ada" } } ] },
    { $eq: [ { $bsonSize: "$$ROOT" }, 28 ] }
  ] }
}, { _id: 1, name: "Ada L." })
```

Comparing the whole document is the simplest complete check: a filter on the loaded fields
would miss a field someone else added, which the replacement would silently drop.
`$literal` keeps `$`-strings in the document from being read as expressions; `$bsonSize`
catches numbers whose stored type changed width (`$eq` compares numbers by value). Editing a
projected document is refused (it is not the whole document).

When the guard matches nothing, `execute` returns `RaabitError::Conflict`: "The document
changed (or was deleted) since it was loaded. Reload and try again." Nothing was written.
The preview already shows "No document matches" in that case, and the confirmation text is
the guarded statement itself, so what is shown is what runs.

Deleting selected documents is by `_id` (deleting what the user selected); when fewer are
deleted than were selected the app says so.

Known limits: an `Int64` and a `Double` of the same value have the same width, so a Save
does not notice that one became the other; an edit of a field whose loaded value was an
array does not notice that the array became an element of a larger array; a Save of a
document over about 8 MB can exceed the server's command size (it fails, nothing is
written); `$bsonSize` needs MongoDB 4.4 or newer.

## Outcome checked against the preview

`WriteOutcome` reports what ran (matched/modified, deleted, inserted). After a confirmed
write, `Write::check_outcome` compares it with the count the confirmation showed and the
app adds a warning to the status line when they differ: "Expected 12 documents, 14 were
matched — the data changed between preview and run". A guarded single-document write
matches exactly one document or fails with the conflict above.

## Round trip: the text shown reads back as the write

For every variant, `Write::shell()` read by the built-in shell without running anything
(`shell_eval::statement_write` / `script_plan`) gives back the same write in its canonical
form (`Write::canonical`: the general mongosh call a write runs — an inline edit is its
guarded `updateOne`, a Save its guarded `replaceOne`, a delete of selected documents a
`deleteOne`/`deleteMany` by `_id`, a clone an `insertOne` without `_id`). The built-in
shell executes statements through the same function, so a statement typed in the shell
runs exactly as it reads. It refuses what it cannot run as typed (a missing filter,
options such as `upsert` it does not apply, unknown methods) instead of ignoring parts of
it.

`tests/write_contract.rs` checks every variant's sample, and a property test (2,000 cases)
generates random documents and values — strings with quotes, backslashes, control
characters and Unicode, nested documents and arrays, ObjectId, dates in and out of the
ISO range, decimals, int32/int64/double edge values (`-0.0`, `NaN`, infinities, whole
doubles from `1e15`), regular expressions, binary, timestamps, null and booleans —
through `SetField`, `ReplaceDocument`, `UpdateMany`, `DeleteMany` and `InsertDocuments`.

Two variants are exempt, by design: `CreateUser` (the password is never shown; the text
says `passwordPrompt()`) and `Import` (a file import shows its first documents and how
many follow). An insert of more than three documents likewise shows the first three and
the count.

## Reads are bounded

Every query, count and aggregation carries the "Query timeout" setting to the server as
`maxTimeMS` (`Connection::set_query_timeout`, kept in sync with the setting), so the server
stops it instead of it running on after the view gave up; the views wait 5 s longer as a
backstop for a server that does not answer. A `$out`/`$merge` pipeline is bounded the same
way, so it stops on the server rather than finishing after the app reported a timeout.
Result sizes are bounded: pages are at most 10,000 documents, aggregations from the views
at most 10,000 (`aggregate::MAX_RESULTS`), the built-in shell's `find` at most 1,000 and
its `aggregate` reads only the 200 it shows. Previews count with a 4 s limit. All of it
runs in the background, never on the UI thread. Exports and imports are long by nature:
they are not time-limited but can be cancelled.

## Persisted data

The app saves one file: the connection profiles (`connections.json` in the host's data
directory). Settings are stored by the host and read leniently (an unknown or mistyped
value falls back to its default); passwords live in the secret store, not in the file.

- The file has a `version` (`profile::STORE_VERSION`). Files from before versions load as
  version 0 and are migrated on load (`ProfileStore::migrate`). A format change raises the
  version and adds a migration step; every released format has a fixture in
  `crates/mongo-raabit-core/tests/fixtures/` that must keep loading.
- Fields the app does not know (written by a newer version) are kept and saved back, at
  the top level and in profiles and their auth, TLS and SSH settings.
- A file that cannot be read (corrupt, or a format this version does not understand) is
  never saved over: it is copied to `connections.json.bak-<timestamp>`, the status line
  says so, and connections changed during that run are not saved.
- Saving is atomic: the new content goes to `connections.json.tmp`, is flushed to disk, the
  previous file is kept as `connections.json.bak`, then the new file is renamed into place
  (`persist::write_atomic`).

`tests/persisted_data.rs` (core) and `tests/persisted.rs` (app) cover each rule.

## Rule for contributors

Any new mutation must be a `Write` variant, with:

1. `shell()` that prints the exact statement, and a `preview` that counts what it touches;
2. a sample in `write::samples` and an entry in `write::KINDS`;
3. a live test: an arm in `expected_after` of `tests/live_write_paths.rs` asserting the
   exact database state it leaves (the read-only test runs every sample automatically);
4. a round trip: its `shell()` must read back through `shell_eval::statement_write`, or it
   must be listed in `NOT_ROUND_TRIPPED` of `tests/write_contract.rs` with the reason.

`tests/write_contract.rs` reads the variants of `enum Write` from the source and fails
when one is missing from `KINDS`, from `samples`, or from the live test, so CI fails until
all of it exists. New files the app saves follow the persisted-data rules above.
