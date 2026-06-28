## Phase 1: Fail-closed timestamp fallback

### Design decisions

- Changed `parse_dt(&modified).unwrap_or_else(Utc::now)` to `parse_dt(&modified).unwrap_or(DateTime::<Utc>::MIN_UTC)` in `map_record` - `sessions/src/db.rs:map_record` - the sentinel value `MIN_UTC` is chronologically earlier than any real session timestamp, so corrupt rows sink in any `modified DESC` sort instead of floating to the top as if freshly modified.
- Added a multi-line comment at the `modified` field assignment in `map_record` explaining the canonical-UTC invariant: the write path stores `to_rfc3339()` of a `DateTime<Utc>`, so lexicographic `TEXT DESC` equals chronological `DESC` and the SQL `ORDER BY s.modified DESC` is sound without a cast. The comment also states the fail-closed rationale.
- Added `UUID_C` constant to `sessions/src/db/tests.rs` to support a three-session test without reusing UUIDs from other tests.
- Test uses `include_archived: true` filter to retrieve all rows regardless of archive state, making the test independent of archive status of the inserted sessions.

### Deviations

- None. Phase 1 implements exactly the spec: change fallback to `MIN_UTC`, add the canonical-UTC comment, add the `map_record_corrupt_timestamp_sinks` test.

### Tradeoffs

- `unwrap_or(MIN_UTC)` vs `unwrap_or_else(|| MIN_UTC)` - used `unwrap_or` directly since `MIN_UTC` is a `Copy` constant with no allocation cost; no closure needed.
- Testing via `db.list` + field assertion vs SQL-level position assertion - `db.list` uses SQL `ORDER BY s.modified DESC` where the text sort can place a non-ISO string in an arbitrary position (e.g. `'NOT-A-TIMESTAMP'` sorts after `'2026...'` because 'N' > '2' in ASCII), so the SQL position of the corrupt row is not a reliable proxy for the Rust `modified` field value. The test asserts the Rust-level `modified` field is `MIN_UTC` and is strictly less than any valid session timestamp, which is the actual invariant that Phase 2's Rust-side re-sort depends on.

### Open questions

- None.
