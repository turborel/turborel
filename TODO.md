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
- **After SQL-92, for the column store** (RULES.md D31): a clause
  limiting the rows an ordered read gives, from a later standard's text;
  and a table option of columns (COLUMNSTORE.ORDER) following a column
  renamed, once ALTER TABLE renames columns.

---

## Query processing

- Nothing is open: what is left as is, and why, is in RULES.md (D31,
  D35, D38, D45).

---

## Storage and durability

- **A database's files written again under another key** (D48):
  STORAGE.ENCODINGS and STORAGE.CHECKSUM_KEY change at once, every file
  writing in the new formats from then on and reading what it wrote
  before as it was; what is missing is the sweep that writes the data
  file's pages still under old formats again, paced, on the thread
  manager (`turborel-rekey`), resumable across opens, and the word
  (a diagnostics event, and INFORMATION_SCHEMA) that none is left, so
  the old key may be retired; `turboreld rekey` as a convenience.
