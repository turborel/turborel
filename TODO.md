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
  with one.
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
- **Replication**, pluggable, Viewstamped Replication first. Most likely a
  `Log` whose `sync` returns once a quorum holds the record, so commit needs
  no change. To settle: whether replication is a kind of `Log` or a plug-in
  of its own; how replicas agree on the order of records; view changes;
  state transfer from checkpoints; reads on backups.

---

## SQL

- **SQL-92 in full** comes first. What came after it, through SQL:2023
  (triggers, SQL/PSM, and the rest), follows, written from the standards'
  own text, never from memory or other products.
- **Temporary tables, further** (RULES.md D49): CREATE INDEX on one; ALTER
  TABLE of one while sessions hold rows in it. An instance reserves
  in-memory pools of the database's budget (2 MiB for its rows, and for
  each index) when first written, and should spill to the session's
  temporary files past WORK_MEMORY instead; a PRESERVE ROWS instance's old
  row versions are not collected during a long session.
- **GET DIAGNOSTICS, further** [§18.1 GR3g]: a syntax error or access
  rule violation names its table only where a privilege is refused, a
  table is not defined, or a named table has no column it names; and its
  column only in the last case, or for a privilege on a column.
- **CAST warnings** [§6.10 GR5c ii, GR5d, GR6c ii, GR6d, GR7, GR8]: a
  CAST that cuts a string, or pads a bit string with 0 bits, raises no
  warning, SQLSTATE 01004 or 01008, where SQL-92 raises one; a warning
  of an expression needs a way to reach the statement's completion.
- **Constraint names outside a `<schema definition>`** (RULES.md D47): a
  constraint's name written without a schema is taken as its table's
  schema's, where §5.4 SR8b would take the session's default schema's,
  which may be another; the constraint would then be in another schema
  than its table, which the catalog does not keep.
- **Plug-in data types, further** (RULES.md D53): no privilege is needed
  to write one, SQL-92 granting none on a data type; a character string is
  stored in one only through CAST, not by store assignment; a function's
  arguments of one are typed only as its `return_type` says.
- **Assertions, further** (RULES.md D51): an assertion is checked whole
  after each statement that changes a table it reads, however little it
  changed.
- **Privileges, further:** a view's owner holds REFERENCES on it only
  where what its columns are drawn from is known (RULES.md D52): not of a
  view with a derived table, a join with common columns, or a set
  operation with CORRESPONDING.
- **Triggers** (ISO/IEC 9075:1999), then **stored procedures** (SQL/PSM):
  CREATE PROCEDURE and FUNCTION, CALL, and the control statements, running
  as their module's authorization identifier, as SQL:2023 defines them.
- **Roles**, which SQL-92 leaves to the implementation, as it does users
  (RULES.md D43).
- **[X3.135-1986 §7.1 SR1]** (exactly one procedure opens each cursor) is
  not checked.

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
    pause after a failed accept (`ACCEPT_BACKOFF`).
- **Session settings to add:** commit durability, optimizer overrides, and
  the function call syntax (today `Session::enable_extensions`).

---

## Query processing

- **Scan filters** (D31): an index probe of a LIKE's prefix, and of an
  OR of one column's ranges, as the scan is told them.
- **Batch execution, further** (RULES.md D31): outer joins, products and
  a join's right operand built from batches; a sort, DISTINCT and set
  operations over streams; expressions evaluated a vector at a time, not
  a row's; groups' set functions fed a vector
  at a time, each group's rows gathered first; a LIKE whose pattern is
  not a constant judged over vectors, its patterns split once each.
- **Index-only scans** need what is visible kept beside the index (as
  PostgreSQL's visibility map): a design of its own (D34).
- **Statistics:** the row store's fetch and scan costs, measured: it
  keeps the defaults of `TableStorage`, and its scan cost ignores the
  columns read; wide columns left out of histograms. A scan's share
  (D31) is taken whole, the filter's estimate of the rows kept not told
  how the rows read lie.
- **Zone maps, further** (D31): a zone a rolled-back insert widened is
  never narrowed; zones are summed up from scratch after a table is
  opened, a pass of the whole table, rather than kept with its pages.

---

## Storage and durability

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
  table is encrypted stays as it was until its operator ends. A read-only
  cursor's rows held at once, a SCROLL cursor's as it reads them, or any
  one's read before its transaction changes a table it reads, are held in
  memory, and should spill past WORK_MEMORY as an operator's do.
- **Buffer pool:** a benchmark of the B-link tree in a small pool, and
  read-ahead, to make misses cheaper. Hits with one shared write measured
  87.6M/s to 151M/s on one thread, 49.4M/s to 107.5M/s on four (2026-10-27):
  four still read less than one, on the frame's latch; a reader-biased
  latch (BRAVO) needs a reviewed exception to R-SAFE-1, a decision to make.
- **Layouts** (D31): a hybrid (PAX), the format of frozen groups then made
  a trait. For the column store:
  - COLUMNSTORE.ORDER changed: the groups frozen before sorted again in
    the background; and descending columns;
  - a table option of columns (COLUMNSTORE.ORDER) following a column
    renamed, once ALTER TABLE renames columns;
  - chunks compressed further: run-length, bit-packing below a byte,
    FSST for texts, ALP for approximate numbers; and conditions judged on
    the codes of a dictionary or a frame without decoding each value;
  - bloom filters, for an equality on a column of many values;
  - groups' headers read as they are needed, not held in memory.

---

## Tests

- Rules implemented but not tested: §5.3 SR2's `1A`, FLOAT(p) up to 24
  taken as REAL and its boundary (FLOAT(24) against FLOAT(25)), REAL
  rounded on assignment, UNION over nulls giving one null row (run, but
  its result not checked), a cursor declared, opened and fetched in SQL on
  an empty table (only the Rust API's is), §5.25 GR3 together with
  DISTINCT.
- Random queries for the comparison with every optimizer rule on and off:
  `planning.rs` compares the two on queries written by hand only.
- Run the mutation check regularly: break a mechanism, and expect the named
  tests to fail (R-TEST-9).
