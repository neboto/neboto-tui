# Demo mode (`--demo`, `src/demo/`)

Moved out of the root `CLAUDE.md` so it loads only when needed.

An offline, made-up account (`acme-prod`, #47). `demo::enable()` runs before
the clients are built. `AwsClients::build` then installs `demo::http_client()`
on the `SdkConfig`, which answers every request from embedded fixtures, and
skips the profile and assumed-role credentials. Nothing above the HTTP layer
changes: lists, lazy sections and jumps all make real SDK calls. **Covering a
new service or section means adding fixtures, never demo branches in app
code.**

Fixtures live in `src/demo/fixtures/<service>/` and are registered in
`fixtures.rs`, whose header lists the story (what's planted, for which clip):
- **Lookup key:** the service (endpoint host label) and the operation (the
  `X-Amz-Target` suffix, the `Action` form field, or `METHOD /path` for
  restJson — a `*` path segment matches any one segment). An entry can also
  list `when` substrings the request URI + body must all contain; the first
  match wins, so narrow entries go first.
- **Wire format:** bodies are raw wire format (EC2 query XML, awsQuery XML,
  awsJson, restJson). The exception is RPC v2 CBOR (CloudWatch), which is
  binary: write those fixtures as JSON, with timestamps as
  `{"$timestamp": {{epoch:…}}}` and doubles with a decimal point (`5.0`).
  `src/demo/cbor.rs` encodes them on the way out, and decodes the request
  body to JSON, so `when` matches text like `"AlarmName":"x"`. A body starting `!status 404` fails with that status
  (with `__type` copied to `x-amzn-errortype`) — use it where the real API
  errors for "none", or the app reads an empty success as "configured".
- **Placeholders:** `{{account}}`, `{{region}}`, `{{iso:now-15m}}`,
  `{{epoch:now-2h}}` and `{{epochms:…}}` (CloudWatch Logs). Offsets chain
  (`now-26h+31s`) for events seconds apart. Times are always relative to now.
- **Id filtering:** EC2 describe calls that name ids (`GroupId.N`,
  `VolumeId.N`) get the fixture's list cut down to them (`narrow`); callers
  like the `N` lens merge whatever comes back. Extend `narrow` for another
  by-id call rather than adding one fixture per id.
- **Generated:** `FilterLogEvents` on a scripted group (`generate.rs`) is
  computed per request from a fixed clock, so a live tail keeps moving and a
  search finds the same lines; so is a `GetMetricData` made of plain
  `MetricStat` queries (the spec-charted kinds), so `m` draws moving
  series. Expression queries (SEARCH, math) still need fixtures. DynamoDB's
  `DescribeTable` / `Query` / `Scan` are generated too (`ddb.rs`): two
  tables built from a seed, with each read's expressions **evaluated** by
  the item browser's own parser — a stored Query body would answer every
  query with the same items. Add DynamoDB data there, not as fixtures.
- **No fixture:** an unmatched operation returns a protocol-correct *empty*
  success, so the service lists empty and `resource_list.rs` says "Not in the
  demo dataset yet". Except restXml (S3, Route 53, CloudFront): an empty
  body works for some operations, but most check a per-operation root
  element, so those calls need an empty-list fixture (see S3 `ListBuckets`).
- **Finding what to add:** `NEBOTO_DEMO_TRACE=<file>` logs every request with
  `fixture` / `generated` / `EMPTY` — open a view and read which calls it made.

The fixture tests in `src/demo/tests.rs` drive real SDK clients, so a fixture
that doesn't deserialize fails `cargo test`; another checks every fixture
renders to parseable JSON / balanced XML. The showcase clips are VHS tapes in
`demo/scenes/` recorded against `--demo`.
