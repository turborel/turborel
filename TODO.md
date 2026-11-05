# TODO

Open work, grouped by area. Each item says what is missing and why it
matters. How the system is built is in [RULES.md](RULES.md), and what is
implemented of the standards is in [CONFORMANCE.md](CONFORMANCE.md).
Finished work leaves this file, and git keeps its history.

Code never cites this file (R-DOC-5).

[Next](#next) · [Server](#server) · [SQL](#sql) ·
[Query processing](#query-processing) ·
[Storage and durability](#storage-and-durability)

---

## Next

Nothing is chosen yet. Each item becomes a decision in RULES.md when done.

---

## SQL

What came after SQL-92 follows it, written from the standards' own text
(RULES.md D32).

- Function overloading (D53). Each name registers one scalar function today.
  Overloading is shared with SQL-invoked functions, and should follow their
  resolution, with candidates by number of arguments and store assignment
  [ISO/IEC 9075:1992 §9.2 SR2], the most specific chosen and an ambiguous
  call refused.
- Triggers (ISO/IEC 9075:1999), then stored procedures (SQL/PSM). This
  covers CREATE PROCEDURE and FUNCTION, CALL and the control statements,
  running as their module's authorization identifier, as SQL:2023 Parts 2
  and 4 define them.
- Roles, from a later standard's text. SQL-92 has none, and ROLE is only a
  potential reserved word [ISO/IEC 9075:1992 §5.2].
- For the column store (D31), a clause limiting the rows an ordered read
  gives, and COLUMNSTORE.ORDER following a renamed column once ALTER TABLE
  renames columns.

---

## Query processing

Nothing is open.

---

## Storage and durability

Nothing is open.
