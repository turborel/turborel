# TODO

Open work, grouped by area. Each item says what is missing and why it
matters. How the system is built is in [RULES.md](RULES.md) (the decisions,
D1…); what is implemented of the standards is in
[CONFORMANCE.md](CONFORMANCE.md). Finished work leaves this file: git keeps
its history.

Code never cites this file (R-DOC-5): an item moves, or goes, when it is
done.

Contents: [Next](#next) · [Server](#server) · [SQL](#sql) ·
[Configuration](#configuration) · [Query processing](#query-processing) ·
[Storage and durability](#storage-and-durability)

---

## Next

Each item becomes a decision in RULES.md when done:

- Nothing is chosen yet: the open work is in the areas below.

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
- **Replication between nodes** (D57): the driver, a job of the thread
  manager (D55) joining a consensus, a transport, a journal and
  `Standby`, which tells VR where its database recovered to rather than
  having committed records delivered again from the start (the
  simulator models a deposed or crashed database as one new, given the
  log from its start); a following database's records of open
  transactions spilled past MEMORY.WORK_BYTES (held in memory now);
  reads on backups, and checkpoints of a following database; state
  transfer (`Standby`'s export and import,
  whole files, the first `StateTransfer`, and its contract tests), log
  retention (the replicated log and its journal keep every record), VR's
  RECOVERY protocol for a journal lost, and DOVIEWCHANGE and STARTVIEW
  carrying a suffix of the log (VRR §5.3) rather than all of it, as they
  do today;
  the transport (mTLS, settings, events, INFORMATION_SCHEMA.REPLICAS,
  commands); reconfiguration. Also `cargo xtask simulate --seeds N`, for
  sweeps longer than `verify` runs, and mutations of VR for `cargo xtask
  mutate`, which mutates the core's package only.
  A replicated log's journal keeps the durability of its
  `FileLogOptions`, not LOG.DURABILITY (D25): changing it while a
  replica runs bears on VR's safety and needs a decision.

---

## SQL

What came after SQL-92 follows it, written from the standards' own text
(RULES.md D32).

- **Function overloading** (RULES.md D53): one scalar function is
  registered for each name, none chosen among several by its arguments'
  types. Overloading is shared with SQL-invoked functions (stored
  procedures, below), and should follow their resolution: candidates by
  number of arguments and store assignment [ISO/IEC 9075:1992 §9.2 SR2]
  to their declared types, the most specific chosen, an ambiguous call
  refused.
- **Assertions, further** (RULES.md D51): forms still checked whole,
  each needing a design the owner settles first:
  - A join or a grouping: from a query of the change against the other
    tables, or of the changed groups alone; no kept state, but a plan run
    over the changed rows and its reads checked as D29's.
  - SUM and AVG: the sum before is unknown, and `SumAcc` raises on the
    running sum, so in the order read, where §6.5 GR2b iv judges the sum.
    A sum kept per assertion and snapshot would serve only once overflow
    is of the final sum.
  - COUNT, MAX or MIN of a value that may be null, the change having no
    null: whether the warning of a null eliminated is raised needs a
    count of nulls kept per assertion and snapshot.
  - A deferred count, greatest or least reached, or MAX or MIN unequal:
    each row's image as it was deferred, kept to commit. And deferred
    forms whose steps may raise, or `x` null: a failure held back to the
    last row, as `KeyCheck` does.
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

---

## Query processing

- **Plans kept, further** (RULES.md D45): the access paths kept too, for
  values that would choose the same ones. Each execution rewrites and
  plans the kept bound plan anew: of a prepared key lookup's 5.4 µs,
  planning takes about 2.6 µs (the scan's zone share about 1.4, the
  index's estimate 0.9) and rewriting 0.35. The choice reads the values through rewrites
  (folding, transitivity), probes (`coerce`, ranges merged, a LIKE's
  prefix) and plug-ins' answers for them (`scan_share`,
  `Index::estimate`), so no test short of planning tells it is the
  same: which plan to keep, and when to plan again, is a decision to
  make (a generic plan beside the values' own, kept while it costs no
  more, is one way).
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
- **Statistics, further** (D35): an index's probe is priced by its pages,
  PLANNER.INDEX_DESCENT_COST each, whether the pool holds them or not;
  only a table's fetches are priced by what its lookups missed (`Tally`),
  as an index's probes, ranges of its tree, are not counted. The system's
  cache is taken as STORAGE.SYSTEM_CACHE_BYTES says, not as it is.

---

## Storage and durability

- **Keys of key services** (D48): keyrings of key services (a cloud KMS,
  Vault) beside the key directory; a host can give one through `Keyring`,
  but turboreld takes the key directory alone.
- **A database's files written again under another key** (D48):
  STORAGE.ENCODINGS, STORAGE.CHECKSUM and its HMAC key are fixed at
  creation, a file recording the function's id alone; so a key of either
  cannot be retired, and changing one means encrypting or checksumming
  every file again.
- **Hash index, a key's rows:** a lookup of one key gathers its
  bucket's entries for that key (`Index::rows`, and `Index::key_rows` by
  default), in memory and not of the budget: a key of millions of rows
  holds them all. Reading its chain a page at a time
  (`HashTable::values`) fails if the table's shape changes meanwhile,
  as writers may make it; and a pair inserted twice must be given once.
- **Spills, further:** SPILL.ENCODINGS compresses spill files (LZ4), off
  by default: on 2026-10-31 it ran from 17% slower to 45% faster, most
  within 10%, on an SSD the system's cache kept up with. With each spill
  write delayed 200 us (the faults file system, a temporary bench; reads
  not delayed; the machine busy, spreads 10% to 180%), LZ4 ran 15% to
  101% faster at 64 KiB (sort 24K to 43K rows/s, DISTINCT 44K to 88K,
  hash join 24K to 42K) and 9% to 97% at 1 MiB; at 50 us, from 35%
  slower (hash join, 64 KiB) to 84% faster. Whether to make it the
  default, or a setting's choice by the disk, is to decide; a quiet
  machine and a real slow disk would confirm the figures.
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
