# TODO

Open work, grouped by area. Each item says what is missing and why it
matters. How the system is built is in [RULES.md](RULES.md) (the decisions,
D1…); what is implemented of the standards is in
[CONFORMANCE.md](CONFORMANCE.md). Finished work leaves this file: git keeps
its history.

Code never cites this file (R-DOC-5): an item moves, or goes, when it is
done.

Contents: [Next](#next) · [Server](#server) · [SQL](#sql) ·
[Query processing](#query-processing) ·
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

- **Replication between nodes** (D57): the driver's events to its database's
  diagnostics (D40; its watch tells nothing now), and its job asked for as
  frames arrive, which the transport does; a following database's crash in
  the simulator losing the writes its files had not synced (their file
  systems keep them now); reads on backups (a following checkpoint is
  refused while a reader holds versions); state transfer (`Standby`'s export
  and import, whole files, the first `StateTransfer`, and its contract
  tests), log retention (the replicated log and its journal keep every
  record), VR's RECOVERY protocol for a journal lost, and DOVIEWCHANGE and
  STARTVIEW carrying a suffix of the log (VRR §5.3) rather than all of it,
  as they do today; the transport (mTLS, settings, events,
  INFORMATION_SCHEMA.REPLICAS, commands); reconfiguration. Also `cargo xtask
  simulate --seeds N`, for sweeps longer than `verify` runs, and mutations
  of VR for `cargo xtask mutate`, which mutates the core's package only. A
  replicated log's journal keeps the durability of its `FileLogOptions`, not
  LOG.DURABILITY (D25): changing it while a replica runs bears on VR's
  safety and needs a decision.

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
- **Triggers** (ISO/IEC 9075:1999), then **stored procedures** (SQL/PSM):
  CREATE PROCEDURE and FUNCTION, CALL, and the control statements, running
  as their module's authorization identifier, as SQL:2023 defines them
  (Parts 2 and 4).
- **Roles**, after SQL-92, which has none (ROLE is only a potential
  reserved word [ISO/IEC 9075:1992 §5.2]), from a later standard's text.
  Users are D43's.

---

## Query processing

- **Plans kept, further** (RULES.md D45): the access paths kept too, for
  values that would choose the same ones. Each execution rewrites and
  plans the kept bound plan anew: of a prepared key lookup's 5.4 µs,
  planning takes about 2.6 µs (the scan's zone share about 1.4, the
  index's estimate 0.9) and rewriting 0.35. The choice reads the values
  through rewrites (folding, transitivity), probes (`coerce`, ranges
  merged, a LIKE's prefix) and plug-ins' answers for them (`scan_share`,
  `Index::estimate`), so no test short of planning tells it is the same.
  Left as is for now: a generic plan planned with slots, kept beside the
  values' own while it costs no more (the planner and executor taking
  `Expr::Dynamic`), is the way if a benchmark on a quiet machine shows
  planning worth saving.
- **Joins by batches** (RULES.md D31): a join's right operand is held as
  rows, so its columns leave the join as values (`Column::Values`), as do
  a partitioned join's left columns, read back from runs; only the batch
  probing keeps its typed vectors. An outer join whose right operand
  outgrows the work area still joins row by row, partitioned or from a
  run, its rows gathered into batches after.
- **Operators over vectors, further** (D31, D38): a grouping of a set
  function that writes no state (a plug-in's, SUM or AVG of approximate
  numbers) still writes every row of a group not held. Groups are made
  partly only of a partition that outgrows the work area again, as a
  piece costs about as much as 6 rows written by batch (a group made at
  each level, states through the value codec, a merge a call each): of
  50,000 keys of 8 rows each past 4 MiB, a grouping makes none partly
  (1% more instructions than before), past 64 KiB 14% fewer. Pieces
  written by batch, their states as vectors, would make them pay sooner.
  A spilled sort is bound by its
  runs (`ORDER BY C` of 200,000 rows of the column store, 49 ms, 50
  before it sorted vectors): records made through the value codec (an
  integer 21 bytes) and pages written one by one; runs of vectors,
  integers at their width, would be fewer bytes. A spilled run's pages
  are freed through the pager's free list, each costing more than the
  record it held.
- **Typed vectors, further** (D31): a LIKE whose pattern is another
  expression (a CASE) leaves its filter judged whole a row at a time,
  each pattern split once (`S LIKE CASE ...` of 200,000 rows of the
  column store, 28.6 ms); a row store table's cursor still reads a row
  at a time (`reads_by_batch`), though its batches no longer make rows.
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
- **Read-ahead, further:** on the job `turborel-read-ahead` (D30, D55),
  measured on a quiet machine (2026-10-31, load 4 to 6, three runs): a
  row store scan of a table ten times its pool 3.96M to 4.35M rows/s
  (+10%), a column store one 5.73M to 6.15M (+7%); a tree in a small
  pool scanned forward, read ahead by a reader on a thread of its own,
  19.8M to 29.6M entries/s on one thread (+49%), 97M to 106M on four
  (+9%). Left: a frozen group's header, a chain of pages each linking
  the one before it, read page by page; scans of groups in their order
  (`frozen/ordered.rs`) ask for none; a scan asks for a group's chunks
  only as it takes the group up, not the next group's.
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
