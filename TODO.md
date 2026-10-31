# TODO

Open work, grouped by area. Each item says what is missing and why it
matters. How the system is built is in [RULES.md](RULES.md) (the decisions,
D1…); what is implemented of the standards is in
[CONFORMANCE.md](CONFORMANCE.md). Finished work leaves this file: git keeps
its history.

Code never cites this file (R-DOC-5): an item moves, or goes, when it is
done.

Contents: [Next](#next) · [Project](#project) · [Server](#server) · [SQL](#sql) ·
[Configuration](#configuration) · [Query processing](#query-processing) ·
[Storage and durability](#storage-and-durability)

---

## Next

Each item becomes a decision in RULES.md when done:

- Nothing is chosen yet: the open work is in the areas below.

---

## Project

- **Documentation for alpha** (D23), to be published and read on the
  project's website: the manual in `doc/` is laid out whole, its sections
  to be written as each settles; then an internals guide, from the doc
  comments and RULES.md, once the design settles.

---

## Server

The server (RULES.md D42) serves sessions over its native protocol, over
TCP with TLS or a Unix socket, its clients authenticated by password
(D43), or trusted on its own machine. Each item becomes a decision in
RULES.md when its design settles.

- **TLS, further:** client certificates, as an authenticator, and the
  client presenting one; a certificate and key taken again without a
  restart; the client trusting the platform's authorities, beside the
  files it is given.
- **Passwords, further:** a challenge-and-response mechanism (SCRAM), so a
  password never travels, even inside TLS; Argon2id beside PBKDF2 (D26),
  once a vetted crate is.
- **Plans kept between executions** of a prepared statement, planned again
  only when the definitions change or its values would choose another
  plan: today each execution plans anew (D45).
- **Stored, named modules** [X3.135-1986 §7]: their procedures run with
  the module's identifier, so at first only that identifier may associate
  with one. With them, [ISO/IEC 9075:1992 §12.1 SR5]: exactly one
  procedure of a module opens each cursor it declares, which a session,
  playing the module's part, does not check.
- **Dynamic SQL in SQL text** [ISO/IEC 9075:1992 §17]: PREPARE, EXECUTE,
  DESCRIBE and the descriptor areas, for a module or an embedded program;
  the protocol and the Rust API prepare without them.
- **The database's owner given in SQL**, by the owner, so it is handed on
  without stopping the server: today `Database::set_owner` and `turboreld
  user owner` give it.
- **Logins refused before an authenticator, told** (D40): a Hello of a
  protocol version not served, of a user name that is no identifier, or
  of a mechanism not served is refused by the native protocol before
  any authenticator runs, so no `login_failed` is told of it.
- **TCP keepalive's times as settings** (idle, interval, probes), so a
  client gone is found out sooner than the system's default: tokio sets
  keepalive on or off only; the times need a socket option of the
  platform's, by a crate such as socket2 (D3).
- **Import**, on the client: rows read from a source and inserted into a
  table over a connection, the source pluggable (a trait any program may
  implement, built-in ones for common formats), so the command-line client
  imports from any source a plug-in reads. Its design, and which sources
  are built in, become a decision in RULES.md when it settles.
- **Named pipes** as the local transport on Windows, where Unix sockets
  are not served.
- **Configuration files** in a subset of TOML, read by our own parser, the
  source of the database's settings at startup.
- **Other systems' wire protocols**, each a `server::Protocol` plug-in in
  a crate of its own (RULES.md D39), served by a listener a setting
  names, so their clients and drivers connect: PostgreSQL's
  frontend/backend protocol, version 3, first (startup and SSLRequest;
  SCRAM-SHA-256, or a password over TLS; simple and extended queries;
  cancel by key; type OIDs and text formats of the standard types; the
  server parameters drivers read); then MySQL's client/server protocol
  (handshake v10, caching_sha2_password, COM_QUERY, COM_STMT_*). A client
  still speaks turborel's SQL: what it sends in its own system's dialect
  is refused, saying why; how far its tools' catalog queries
  (`pg_catalog`, `SHOW`) are answered is a decision to make.
- **Replication**, pluggable, Viewstamped Replication first. Most likely a
  `Log` whose `sync` returns once a quorum holds the record, so commit needs
  no change. To settle: whether replication is a kind of `Log` or a plug-in
  of its own; how replicas agree on the order of records; view changes;
  state transfer from checkpoints; reads on backups.

---

## SQL

- **SQL-92 in full** comes first, but for what CONFORMANCE.md lists as
  never implemented or not claimed (RULES.md D32): character sets other
  than the one there is, collations and translations, the module language
  and embedded SQL. What came after it, through SQL:2023 (triggers,
  SQL/PSM, and the rest), follows, written from the standards' own text,
  never from memory or other products.
- **Temporary tables, further** (RULES.md D49): CREATE INDEX on one; ALTER
  TABLE of one while sessions hold rows in it. An instance reserves
  in-memory pools of the database's budget (2 MiB for its rows, and for
  each index) when first written, and its pages are files in memory of
  MEMORY_BUDGET, so an instance larger than the budget leaves is refused
  (SQLCODE -918): it should spill to the session's temporary files past
  WORK_MEMORY instead.
- **Default warnings** [§11.5 GR1b v]: a BIT column's default shorter
  than the column is kept padded with 0 bits, and a statement that takes
  it raises no warning 01008, implicit zero-bit padding.
- **Plug-in data types, further** (RULES.md D53): no privilege is needed
  to write one, SQL-92 granting none on a data type; a character string is
  stored in one only through CAST, not by store assignment; a function's
  arguments of one are typed only as its `return_type` says.
- **Assertions, further** (RULES.md D51): an assertion is checked whole
  after each statement that changes a table it reads, however little it
  changed.
- **Triggers** (ISO/IEC 9075:1999), then **stored procedures** (SQL/PSM):
  CREATE PROCEDURE and FUNCTION, CALL, and the control statements, running
  as their module's authorization identifier, as SQL:2023 defines them.
- **Roles**, which SQL-92 leaves to the implementation, as it does users
  (RULES.md D43).

---

## Configuration

The registry, scopes and SET statements exist (D36). Still to do:

- **Constants still to move onto the registry**, each keeping today's
  value as its default:
  - log durability, once the log can change it while open;
  - turning the optimizer's rules on and off, one setting each: today
    `Database::with_optimizer` does, and what it sets is lost when the
    database is opened again;
  - each to be put on the registry or documented as fixed: the undo log's
    batch and segment sizes (`UNDO_BATCH`, `UNDO_SEGMENT_SIZE`), the
    largest block (`DEFAULT_MAX_BLOCK`), an engine's default fetch cost
    (`DEFAULT_FETCH_COST`), the entries an index counts for an estimate
    (`COUNTED`), the precision of distinct counts (`REGISTER_BITS`), and
    the server's batches of replies in flight on a connection
    (`IN_FLIGHT`) and its pause after a failed accept (`ACCEPT_BACKOFF`),
    and the bytes of frozen groups' headers a store holds
    (`HEADER_CACHE_BYTES`).
- **Session settings to add:** commit durability, optimizer overrides, and
  the function call syntax (today `Session::enable_extensions`).

---

## Query processing

- **Conditions that can raise an error, of an outer join's preserved
  operand** (D15): of an outer join's ON, a conjunct that can raise an
  error and reads the preserved operand's columns alone keeps the join
  from its equalities, every pair judged: it is evaluated on each of that
  operand's rows when the other has rows, which a join on keys would
  need to evaluate once a row, apart from its matches.
- **Index nested-loop joins, further** (D35): RIGHT outer joins, the
  right operand preserved, probing the left's table; a right operand that
  reads its table through a projection, as a view's does; and the
  planner's estimate of a join's left operand when it is itself a join,
  now its product's rows by the default selectivity of each equality.
- **Batch execution, further** (RULES.md D31): RIGHT and FULL outer
  joins, and LEFT ones with a condition beyond their keys; products and
  a join's right operand built from batches; a sort, a DISTINCT of
  expressions, EXCEPT and INTERSECT over streams; typed vectors further:
  a filter's comparisons of expressions over vectors of integers
  (`B * 2 > 400000` 9.5 ms, `B > 200000` 2.3), quotients of them, the
  row store's batches as integers, and of text; CHARACTER columns as
  text, padded as LIKE sees them; grouping of many groups (`GROUP BY K`
  of 400,000 keys, 382 ms, past the work area); a LIKE whose pattern is
  not a constant judged over vectors, its patterns split once each.
- **Index-only scans** need what is visible kept beside the index (as
  PostgreSQL's visibility map): a design of its own (D34).
- **Statistics:** a fetch's cost does not tell a page the buffer pool
  misses but the system's cache holds from one read from a disk: each is
  priced as read from the disk. The pool's share of its file is taken as
  even among the file's tables and indexes, not as each is read.

---

## Storage and durability

- **Opening a table counts its rows:** a table opened makes a pass of
  its rows to count those of its newest versions (`count_newest`), as its
  zones no longer need: the count could be kept with its pages as the
  zones are.
- **Zones saved, a failure midway:** a checkpoint whose pool write fails
  while it saves a table's zones fails, and marks every segment to be
  saved again; no test drives that path.
- **Freeing a page the caller pins:** `free_when_released` waits for a
  page's pins without limit, so a caller that itself holds a pin on the
  page it frees waits for ever; only a page latched by a hinted read is
  waited for a bounded while.
- **Plug-ins' files, shadowed** (D56): a plug-in's directory, given
  through `Host`, is kept as the plug-in writes it, which makes its files
  durable itself. A directory it declares shadowed would have its writes
  kept aside and committed at each checkpoint with the data file, as the
  built-in engines' are, which needs a checkpoint contract for it.
- **Hash functions:** SHA-512 beside SHA-256 and HMAC-SHA-256 (D26),
  which needs a digest of more than 256 bits (`MAX_DIGEST_BYTES`
  raised). HMAC-SHA-256's key
  is fixed for a database's life: a file records the function's id
  alone, so changing the key means checksumming every file again.
- **Encryption, further** (D48): keyrings of key services (a cloud KMS,
  Vault) beside the key directory; a database's files re-encrypted with
  another key, which today means naming it at creation, so a key
  STORAGE.ENCODINGS names cannot be retired.
- **Spills, further:** compressed, through the database's compression,
  where the disk is slower than the CPU; and a spill file made before a
  table is encrypted stays as it was until its operator ends. A
  referential action gathers the rows that match each row it acts for in
  memory (`State::matching_rows`, `db/integrity/references.rs`), as MATCH
  PARTIAL does the referenced table as it was before the statement, and
  the values a statement gave each row of a table an action may change
  [ISO/IEC 9075:1992 §11.8 GR7]: they should spill as the statement's
  effects do.
- **Buffer pool:** a benchmark of the B-link tree in a small pool, and
  read-ahead, to make misses cheaper. Hits with one shared write measured
  87.6M/s to 151M/s on one thread, 49.4M/s to 107.5M/s on four (2026-10-27):
  four still read less than one, on the frame's latch; a reader-biased
  latch (BRAVO) needs a reviewed exception to R-SAFE-1, a decision to make.
- **Layouts** (D31): a hybrid (PAX), the format of frozen groups then made
  a trait. For the column store:
  - a sorted group's rows read backwards, to answer an ORDER BY of its
    order's columns, all descending, without a sort, as ascending ones
    are (`frozen/ordered.rs`); COLUMNSTORE.ORDER needs no descending
    columns for it (Vertica keeps ascending only, ClickHouse sorting keys
    took DESC in 24.12 for the same reads); and a clause limiting the rows
    an ordered read gives, once a later standard's text is on hand;
  - a table option of columns (COLUMNSTORE.ORDER) following a column
    renamed, once ALTER TABLE renames columns;
  - conditions judged of runs, a run at a time, as a vector of runs
    through the filter and the set functions: of 393,216 rows of a sorted
    column `COUNT(*) WHERE S >= 10` takes 1.43 ms where a `COUNT(*)`
    reading none takes 0.48 (of a frame, decoding is a hundredth of a
    scan's time, so nothing to gain);
