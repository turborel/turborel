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

- **Documentation for alpha** (D23): the manual in `doc/` is written
  whole; it is to be published on the project's website, and kept as the
  code changes. The internals guide, drafted from the doc comments and
  RULES.md, comes into the repository once the design settles.

---

## Server

The server (RULES.md D42) serves sessions over its native protocol, over
TCP with TLS or a Unix socket (a named pipe on Windows), its clients
authenticated by password, SCRAM-SHA-256 or certificate (D43), or trusted
on its own machine. Each item becomes a decision in RULES.md when its
design settles.

- **Argon2id beside PBKDF2** (D26), once a crate meets D26's bar (an
  audit, or a claim of readiness of its own): a verifier names its
  scheme, so it comes beside the one there is, with a setting choosing
  the scheme new verifiers are made with, and `password` checking by the
  scheme a verifier names (today any other is refused). SCRAM-SHA-256
  still needs PBKDF2's keys, so a user of Argon2id proves its password by
  `password` alone.
- **SCRAM's binding `tls-exporter`** (RFC 9266), beside
  `tls-server-end-point` (D43): RFC 9266 §3 makes it the binding to
  implement for TLS 1.3, which rustls negotiates by default; it binds the
  TLS connection itself, not its certificate. rustls exports the keying
  material on both sides; the mechanism takes the client's `p=` type,
  and the client chooses it of TLS 1.3.
- **Named pipes, further** (D42): a security descriptor of the server's
  own, so only its user and the administrators may open the pipe at all,
  not others to read only; and a client's reads and writes of a pipe
  bounded by its timeouts, as a socket's are. Both need the system's API
  (overlapped I/O, `SECURITY_ATTRIBUTES`), which std does not offer: it
  takes `unsafe` (R-SAFE-1) or, for the client, an async runtime it does
  not take (D42). Parked while R-SAFE-1 stands.
- **Other systems' wire protocols**, each a `server::Protocol` plug-in in
  a crate of its own (RULES.md D39), so their clients and drivers
  connect. The library already serves endpoints of several protocols
  (`Endpoint`); turboreld serves the native one alone, so a setting
  naming a listener for each protocol is missing, and the protocols:
  PostgreSQL's frontend/backend protocol, version 3, first (startup and
  SSLRequest; SCRAM-SHA-256 as its clients do it, with SASLprep of a
  password, RFC 4013, and -PLUS; simple and extended queries; cancel by
  key; type OIDs and text formats of the standard types; the server
  parameters drivers read); then MySQL's client/server protocol. A client
  still speaks turborel's SQL: what it sends in its own system's dialect
  is refused, saying why; how far its tools' catalog queries
  (`pg_catalog`, `SHOW`) are answered is a decision to make. A protocol
  crate's dependencies are a decision of R-SAFE-4.
- **Replication on one node** (D57), with nothing visible to users: the
  `Standby` seam (the core's `StateSource` is there); the `node` property
  of settings; a replicated `Log` of one node passing the log,
  durability, recovery and reopen tests.
- **Replication between nodes** (D57): real databases in the simulator,
  a backup compared with the primary, through the driver, a job of the
  thread manager (D55) joining a consensus, a transport, a journal and
  `Standby`, which tells VR where its database recovered to rather than
  having committed records delivered again from the start; continuous
  replay and reads on backups; state transfer (whole files, the first
  `StateTransfer`, and its contract tests), log retention, VR's RECOVERY
  protocol for a journal lost, and DOVIEWCHANGE and STARTVIEW carrying a
  suffix of the log (VRR §5.3) rather than all of it, as they do today;
  the transport (mTLS, settings, events, INFORMATION_SCHEMA.REPLICAS,
  commands); reconfiguration. Also `cargo xtask simulate --seeds N`, for
  sweeps longer than `verify` runs, and mutations of VR for `cargo xtask
  mutate`, which mutates the core's package only.

---

## SQL

What came after SQL-92 follows it, written from the standards' own text
(RULES.md D32).

- **Temporary tables, further** (RULES.md D49): an instance's rows and
  entries spill to a file, but the older versions its transactions keep
  (committed rows a transaction updates or deletes) go to an undo store
  over a log in memory, outside MEMORY.BUDGET_BYTES, until the
  transaction ends: an UPDATE of every row of a large instance holds a
  copy of it in memory. They should spill too, as the database's undo
  log does, to files of `tmp/` (which holds files alone today).
- **ALTER TABLE of a temporary table in use**, refused where it need not
  be: ADD or DROP COLUMN while another session holds rows and a
  constraint of the table reads another table, as the copy checks
  constraints against the altering session's instances alone; any other
  action, SET or DROP DEFAULT and DROP CONSTRAINT too, which change no
  row, while any instance holds a row.
- **Plug-in data types, further** (RULES.md D53): CREATE DOMAIN takes only
  a standard type, never a plug-in one; and one scalar function is
  registered for each name, none chosen among several by its arguments'
  types, as D53 decides today. The second is shared with SQL-invoked
  functions (stored procedures, below).
- **Assertions, further** (RULES.md D51): more forms checked from a
  statement's change than `NOT EXISTS` and COUNT(*) of one table's
  filtered rows: COUNT of a column (its warning of nulls eliminated
  depends on every row), SUM, AVG, MIN and MAX (the value before is not
  kept, and a SUM's nulls and overflow depend on every row); a condition
  through a view or a join; and a deferred assertion from the changes of
  its transaction.
- **Triggers** (ISO/IEC 9075:1999), then **stored procedures** (SQL/PSM):
  CREATE PROCEDURE and FUNCTION, CALL, and the control statements, running
  as their module's authorization identifier, as SQL:2023 defines them
  (Parts 2 and 4).
- **Roles**, after SQL-92, which has none (ROLE is only a potential
  reserved word [ISO/IEC 9075:1992 §5.2]), from a later standard's text.
  Users are D43's.

---

## Configuration

The registry, scopes and SET statements exist (D36). Still to do:

- **The log's durability as a setting** (D36), today
  `OpenOptions::durability`, by default Data: the file log fixes it at
  open for every segment (`log/file.rs`). It must change while the log is
  open, each new segment, and a `Log` plug-in, fixing it as made, and
  `Log::capabilities`, which the Strict policy reads at each write (D25),
  telling the value now.
- **Read-ahead's pages as a setting** (D36): `BufferPool::set_read_ahead`
  is called by a benchmark alone.
- **Plug-ins' settings beyond engines' own** (D36): index methods, and
  every other kind of plug-in, declaring and told theirs as an engine's
  are (`StorageEngine::settings`, `configure`), so a plug-in reads its
  own without calling back into the database.

---

## Query processing

- **Plans kept, further** (RULES.md D45): the access paths kept too, for
  values that would choose the same ones (each execution now rewrites and
  plans the kept bound plan anew); positioned UPDATE and DELETE kept as
  the searched ones are; a dynamic cursor's query (RULES.md D58), bound
  anew at each OPEN; and a session's declared local temporary tables,
  which make its catalog anew at each call, so no plan of that session is
  kept, not only theirs: kept by what they are rather than by the
  catalog's identity.
- **Joins by batches** (RULES.md D31): FULL outer joins, RIGHT and LEFT
  ones whose condition can raise or whose operand has a condition of its
  own (a RIGHT one too if an operand can raise, or its left has more
  rows), and products: their operands are read by batches, but their rows
  joined (a product's paired) row by row and gathered into batches after,
  as a probe by batches needs a flag of each right row held and the
  right's rows of a null key kept; so too an inner join's partitions past
  the work area. A join's batches lose their typed vectors (`Column::
  Values`).
- **Operators over vectors** (D31): a sort, EXCEPT and INTERSECT over
  streams, of their operands' vectors, not of rows made of them; grouping
  of many groups: past the work area each row of a group not held is
  written whole and grouped again row by row (`group::Spilling`), where
  partial groups made first, or partitions written by batches, would do
  (`GROUP BY K` of 400,000 keys, 382 ms).
- **Typed vectors, further** (D31): a filter's comparisons that cannot
  raise (`A > B`) over vectors of integers, as raising ones are
  (`B * 2 > 400000` 6.2 ms, `B > 200000` 2.3); quotients of them; the row
  store's batches, and the column store's rows not yet frozen, as
  integers and text; CHARACTER columns as text, padded as LIKE sees them;
  a LIKE whose pattern is not a constant, over vectors and row by row
  alike, its patterns split once each.
- **Index-only scans** need what is visible kept beside the index (as
  PostgreSQL's visibility map): a design of its own (D34).
- **Statistics:** a fetch's cost does not tell a page the system's cache
  holds from one read from a disk: a page the buffer pool misses is
  priced as the system's cache gave it, so a read from a disk is priced
  too cheap. Telling them apart takes no `unsafe` (R-SAFE-1): a setting
  of the cache's size, say. The pool's share of a table is taken as the
  pool over its file's pages in use, the same for each of the file's
  tables and indexes, not as each is read.

---

## Storage and durability

- **A rollback's garbage kept in tmp/:** a ROLLBACK of changes to
  committed rows leaves garbage of each (`Dead::Undone`), keyed past the
  rolled-back transaction's snapshot, which spills to an area of its own
  past TRANSACTIONS.HELD_BYTES and stays until a later commit moves the
  horizon past it: on a database at rest, until it closes. It could be
  collected at once when no other transaction is open in the table.
- **Plug-ins' files, shadowed** (D56): a plug-in's directory, given
  through `Host` (`PluginFiles::directory`), is kept as the plug-in writes
  it, which makes its files durable itself. A directory it declares
  shadowed would be wrapped in `ShadowFileSystem`, its writes kept aside,
  sealed and committed in the checkpoint's two phases beside the data
  file's, and recovered as the database opens.
- **Keys of key services** (D48): keyrings of key services (a cloud KMS,
  Vault) beside the key directory; a host can give one through `Keyring`,
  but turboreld takes the key directory alone.
- **A database's files written again under another key** (D48):
  STORAGE.ENCODINGS, STORAGE.CHECKSUM and its HMAC key are fixed at
  creation, a file recording the function's id alone; so a key of either
  cannot be retired, and changing one means encrypting or checksumming
  every file again.
- **Referential checks and actions, bounded:** what they gather is held
  in memory, unbounded and not of the budget: the rows that match each
  row an action is for (`State::matching_rows`,
  `db/integrity/references.rs`), the referenced table as it was before
  the statement for MATCH PARTIAL, read whole for each change, and the
  values a statement gave each row of a table an action may change
  [ISO/IEC 9075:1992 §11.8 GR7]. They should spill as the statement's
  effects do. NO ACTION, and whether a row has a match, gather every
  matching row only to ask whether there is one: the first would do.
- **Spills, further:** SPILL.ENCODINGS compresses spill files (LZ4), off
  by default: on 2026-10-31 it ran from 17% slower to 45% faster, most
  within 10%, on an SSD the system's cache kept up with; a disk slower
  than the CPU may make it pay, to be measured there. An operator's spill
  area made before a table is encrypted stays as it was until the
  operator ends (a list that spills makes a new one), as D48 allows.
- **Buffer pool's latch:** hits with one shared write measured 87.6M/s to
  151M/s on one thread, 49.4M/s to 107.5M/s on four (2026-10-27): four
  still read less than one, on the frame's latch. A reader-biased latch
  (BRAVO) needs `unsafe` (R-SAFE-1), and a crate for it breaches R-SAFE-4;
  safe ways are reader locks striped by thread on hot frames, or copies
  of a tree's inner nodes checked by a version.
- **Read-ahead, further:** it is synchronous: reading ahead on a job of
  the thread manager (D55) would overlap the reads with the scan; the
  column store's scans of frozen groups read none ahead.
- **Layouts** (D31), paused: a hybrid (PAX), the format of frozen groups
  then made a trait. For the column store:
  - a sorted group's rows read backwards, to answer an ORDER BY of its
    order's columns, all descending, without a sort, as ascending ones
    are (`frozen/ordered.rs`); COLUMNSTORE.ORDER needs no descending
    columns for it (Vertica keeps ascending only, ClickHouse sorting keys
    took DESC in 24.12 for the same reads); and a clause limiting the rows
    an ordered read gives, from a later standard's text;
  - a table option of columns (COLUMNSTORE.ORDER) following a column
    renamed, once ALTER TABLE renames columns;
  - conditions judged of runs, a run at a time, as a vector of runs
    through the filter and the set functions: of 393,216 rows of a sorted
    column `COUNT(*) WHERE S >= 10` takes 1.43 ms where a `COUNT(*)`
    reading none takes 0.48. Frames are left out: decoding one is a
    hundredth of a scan's time.
