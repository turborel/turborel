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
TCP with TLS or a Unix socket (a named pipe on Windows), its clients
authenticated by password, SCRAM-SHA-256 or certificate (D43), or trusted
on its own machine. Each item becomes a decision in RULES.md when its
design settles.

- **The client trusting the platform's authorities,** beside the files
  it is given: it needs rustls-native-certs or webpki-roots, neither in
  the tree, so a decision of its own under R-SAFE-4 (D26).
- **Argon2id beside PBKDF2** (D26), once a vetted crate is: a verifier
  names its scheme, so it comes beside the one there is. SCRAM-SHA-256
  would still need PBKDF2's keys, so a user of Argon2id proves its
  password by `password` alone.
- **SCRAM's channel binding** (`tls-server-end-point`, RFC 5929), so a
  SCRAM exchange is bound to its TLS connection and a server in the
  middle cannot relay it: SCRAM-SHA-256-PLUS. And SASLprep of a password
  (RFC 4013), for clients of other systems whose non-ASCII passwords it
  maps.
- **Plans kept, further** (RULES.md D45): the access paths kept too, for
  values that would choose the same ones, where each execution now
  rewrites and plans the kept bound plan; positioned UPDATE and DELETE
  kept as the searched ones are; a dynamic cursor's query (RULES.md D58),
  bound anew at each OPEN; and a session's declared local temporary
  tables, which make its catalog anew at each call, kept by what they
  are rather than by the catalog's identity.
- **Stored, named modules** [X3.135-1986 §7]: their procedures run with
  the module's identifier, so at first only that identifier may associate
  with one. With them, [ISO/IEC 9075:1992 §12.1 SR5]: exactly one
  procedure of a module opens each cursor it declares, which a session,
  playing the module's part, does not check.
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
- **Import, pipelined** (RULES.md D59): an import's Executes sent before
  the last is answered, as the protocol allows (D45), so a row costs no
  round trip; a row that fails is then known after more are sent, which
  the batch's rollback undoes.
- **Named pipes, further** (D42): a security descriptor of the server's
  own, so only its user and the administrators may open the pipe at all,
  not others to read only; and a client's reads and writes of a pipe
  bounded by its timeouts, as a socket's are. Both need the system's API
  (overlapped I/O, `SECURITY_ATTRIBUTES`), which std does not offer and
  `unsafe` would reach (R-SAFE-1).
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
- **Replication**, pluggable, Viewstamped Replication first, as RULES.md
  D57 decides. Done: the one replay, `record::KINDS` and its tests, the
  files' classes and the export manifest, a commit in doubt never
  stamped (D25). Next, with nothing visible to users: the `Standby` and
  `StateSource` seam; the `node` property of settings; a single-node replicated
  `Log` passing the log, durability, recovery and reopen tests. Then the
  `Consensus`, `Transport` and `StateTransfer` traits and VR as a pure
  state machine in a deterministic simulator; real databases in it, a
  backup compared with the primary; continuous replay and reads on
  backups; state transfer and log retention; the transport (mTLS,
  settings, events, INFORMATION_SCHEMA.REPLICAS, commands);
  reconfiguration.

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
- **Plug-in data types, further** (RULES.md D53): a domain is never of
  one, CREATE DOMAIN taking only a standard type's name; a function is
  one of its name, none chosen among several by its arguments' types.
- **Assertions, further** (RULES.md D51): more forms checked from a
  statement's change than `NOT EXISTS` and COUNT(*) of one table's rows:
  COUNT of a column, and SUM, AVG, MIN and MAX, which need the value
  before kept or bounded; a condition through a view or a join; and a
  deferred assertion from the changes of its transaction.
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
