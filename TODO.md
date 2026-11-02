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

- **Replication between nodes** (D57): reconfiguration (VRR §7).

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

- Nothing is open: what is left as is, and why, is in RULES.md (D25,
  D30, D48, D56).
