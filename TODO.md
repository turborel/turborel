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
[Storage and durability](#storage-and-durability) · [Tests](#tests)

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
- **Diagnostics, further** (D40): the log file reopened on a signal, or
  rotated, for an operator's log rotation; levels filtered.
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

- **An owner for a database with no users:** the first user created owns
  the database (D43), but until one is, no one may SET DATABASE; a
  database kept nowhere, served by `turboreld --memory`, never has one.
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
    the server's requests read ahead on a connection (`IN_FLIGHT`) and its
    pause after a failed accept (`ACCEPT_BACKOFF`), and the bytes of frozen
    groups' headers a store holds (`HEADER_CACHE_BYTES`).
- **Session settings to add:** commit durability, optimizer overrides, and
  the function call syntax (today `Session::enable_extensions`).

---

## Query processing

- **Index nested-loop joins** (D35): plans join by a product or a hash
  join only; a small outer operand probing an index of the inner one for
  each of its rows needs a plan of its own, the executor running a probe
  of each row's values.
- **IN lists judged by a set** (D31): an IN list of constants is bound as
  an OR of equalities and judged one equality after another, a row at a
  time and over vectors (`exec/batch/judge.rs`); a long list would be
  judged faster by a set of its values' buckets made once.
- **Conditions that can raise an error, of outer joins** (D15): of an
  outer join's ON, a conjunct that can raise an error and reads one
  operand's columns alone keeps the join from being on its equalities
  through a hash table, as a WHERE's no longer keeps a product from being
  a hash join.
- **Index probes of an OR's ranges, in order** (D34): their index scans
  give rows a range at a time, so an ORDER BY of the column is still a
  sort, though the ranges are apart and in order.
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
- **Spilled sorts**: of 400,000 rows past the default WORK_MEMORY a sort
  takes 149 ms where held whole it takes 41: each row written as a
  record, checksummed in its page, read back and allocated again for the
  merge. Runs of prefixes beside the records, and records read in place,
  would spare most of it.
- **Index-only scans** need what is visible kept beside the index (as
  PostgreSQL's visibility map): a design of its own (D34).
- **Statistics:** a fetch's cost of pages read from a disk, not the
  system's cache, and of a pool shared by many tables, not one; wide
  columns left out of histograms. A scan's share
  (D31) is taken whole, the filter's estimate of the rows kept not told
  how the rows read lie.
- **Zone maps, further** (D31): a zone a rolled-back insert widened is
  never narrowed; zones are summed up from scratch after a table is
  opened, a pass of the whole table, rather than kept with its pages.

---

## Storage and durability

- **Plug-ins' files, shadowed** (D56): a plug-in's directory, given
  through `Host`, is kept as the plug-in writes it, which makes its files
  durable itself. A directory it declares shadowed would have its writes
  kept aside and committed at each checkpoint with the data file, as the
  built-in engines' are, which needs a checkpoint contract for it.
- **Hash functions:** cryptographic ones (D26), as `HashFunction`
  plug-ins from vetted crates, for checksums a forger cannot match. A
  digest holds 256 bits at most (`MAX_DIGEST_BYTES`), so SHA-512 would
  need it raised.
- **Encryption, further** (D48): keyrings of key services (a cloud KMS,
  Vault) beside the key directory; a database's files re-encrypted with
  another key, which today means naming it at creation; a table's rows
  sealed again after its ENCODINGS change, since rows written before keep
  their encodings and key, so a key cannot yet be retired;
  ChaCha20-Poly1305 for machines without AES instructions.
- **Spills, further:** compressed, through the database's compression,
  where the disk is slower than the CPU; and a spill file made before a
  table is encrypted stays as it was until its operator ends. What a
  data-changing statement did, kept for its constraints' checks at its
  end (each row's id, and an UPDATE's or DELETE's old values,
  `db/integrity.rs`), is held in memory, and should spill past
  WORK_MEMORY as an operator's rows do.
  DISTINCT read row by row (`exec/distinct.rs`) is not told its statement,
  so its passes over partitions look for no cancel (D44), as the others'
  do: `exec.rs` should hand it the transaction, for `Work::of`. And it,
  EXCEPT and INTERSECT take a second work area for a bag beside what they
  hold (`Work::another`), up to twice WORK_MEMORY, which `Work::split`
  would bound as grouping's is.
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

---

## Tests

- Rules implemented but not tested: §5.3 SR2's `1A`, FLOAT(p) up to 24
  taken as REAL and its boundary (FLOAT(24) against FLOAT(25)), REAL
  rounded on assignment, UNION over nulls giving one null row (run, but
  its result not checked), a cursor declared, opened and fetched in SQL on
  an empty table (only the Rust API's is).
- Random queries for the comparison with every optimizer rule on and off:
  `planning.rs` compares the two on queries written by hand only.
- Run the mutation check regularly: break a mechanism, and expect the named
  tests to fail (R-TEST-9).
