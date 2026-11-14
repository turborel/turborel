# TODO

Open work, grouped by area. Each item says what is missing and why it
matters. How the system is built is in [RULES.md](RULES.md), and what is
implemented of the standards is in [CONFORMANCE.md](CONFORMANCE.md).
Finished work leaves this file, and git keeps its history.

Code never cites this file (R-DOC-5).

[Next](#next) · [SQL](#sql) · [Queries](#queries) ·
[Changing data](#changing-data) ·
[Triggers, routines and roles](#triggers-routines-and-roles) ·
[Types](#types) ·
[Periods and temporal tables](#periods-and-temporal-tables) · [JSON](#json) ·
[Query processing](#query-processing) ·
[Storage and durability](#storage-and-durability)

---

## Next

Nothing is chosen yet. Each item becomes a decision in RULES.md when done.

---

## SQL

What came after SQL-92 follows it, written from the standards' own text
(RULES.md D32). Each item names its features of ISO/IEC 9075-2:2023 Annex
D and the clauses that define them, so a bare `§` here is SQL:2023. The
groups run in the order they build on each other, and so do the items in
each group.

### Queries

- The rest of T612 (Advanced OLAP operations) [§10.9]. The inverse
  distribution functions.
- A hypothetical set function whose sort key holds an outer reference
  [§10.9 SR14, §6.9 SR6]. It is the enclosing query's, its values over
  that query's rows, which the outer set functions of
  `binder/subquery.rs` would carry.
- Framed COUNT DISTINCT (T627) [§9.23 SR8d]. A moving frame's distinct
  values counted within the work area, which a spilled count of each
  value would allow.
- ORDER BY in a grouped query of what the select list leaves out (F868)
  [§7.17]. It needs functional dependencies (T301) [§4.26], which also
  decide what a grouped select list may name.
- FETCH FIRST and OFFSET (F850 to F867) [§7.17]. For the column store (D31)
  this is the clause limiting the rows an ordered read gives. Once ALTER
  TABLE renames columns, COLUMNSTORE.ORDER follows a renamed column.
- Row pattern recognition (R010, R020, R030) [§4.19 to §4.23, §6.27, §7.7
  to §7.9, §9.41]. MATCH_RECOGNIZE in FROM and in a window, for finding
  runs of rows such as a price falling then rising. It builds on windows.

### Changing data

- MERGE (F312, F313, F314) [§14.12]. Inserts, updates and deletes from one
  source in one statement, with several WHEN clauses and a condition on
  each. Today an upsert takes two statements and a retry.
- GET DIAGNOSTICS into targets of another type (F122), ALL (F123) and the
  transaction counts (T511) [§23.1]. Each target must be of its item's
  type today.

### Triggers, routines and roles

- Triggers (T200, T211 to T218) [§4.46, §11.49, §11.50], BEFORE and AFTER,
  per row and per statement, with transition tables, then INSTEAD OF
  triggers on views (T213). They follow MERGE so every data change fires
  them through one path.
- Function overloading (D53). Each name registers one scalar function today.
  Overloading is shared with SQL-invoked functions, and should follow their
  resolution, with candidates by number of arguments and store assignment
  [ISO/IEC 9075:1992 §9.2 SR2], the most specific chosen and an ambiguous
  call refused.
- Stored procedures (SQL/PSM). This covers CREATE PROCEDURE and FUNCTION,
  CALL and the control statements, running as their module's
  authorization identifier, as SQL:2023 Parts 2 and 4 define them. Part
  4's text is not yet local.
- Array element assignment (S097) [§6.4 SR7, SR8; §14.5 GR; §14.7 GR].
  A FETCH INTO or SELECT INTO target `A[i]`, which writes one element of
  an array. Its array is an SQL parameter or a new transition variable's
  column, never a host parameter, so it waits for triggers or SQL-invoked
  routines.
- Enhanced derived column names (T285) [§7.16 SR17c, CR6]. A select list
  item that is a single SQL parameter reference names its column for the
  parameter. Any other item without an `<as clause>` that is not a column
  reference is unnamed with or without it [SR17d] (UV087), so it waits for
  SQL-invoked routines.
- Polymorphic table functions (B200 to B209) [§4.35.4, §7.6, §9.24, §9.25].
  A function in FROM whose result's columns depend on its arguments. It
  needs SQL-invoked routines.
- Roles (T331, T332) [§12.4 to §12.6]. SQL-92 has none, and ROLE is only a
  potential reserved word [ISO/IEC 9075:1992 §5.2].

### Types

- Large objects (T041, T042, T046, T040, T048, T047, T050) [§4.3, §4.4,
  §6.1]. CHARACTER LARGE OBJECT and BINARY LARGE OBJECT, strings past the
  32,767 a row holds today, kept apart from the row, with concatenation
  and the string functions over them. Their locators (T039, T049)
  [§4.37.5] are for host languages and follow the client protocol.
- Multisets (S271, S275, S272, S274, S281, S402, S202) [§4.12.3, §6.50 to
  §6.52, §8.16 to §8.18]. MULTISET types and constructors, MEMBER, SUBMULTISET
  and IS A SET, MULTISET UNION, EXCEPT and INTERSECT, and SET, FUSION and
  COLLECT. Arrays already give a collection type to build on.
- Structured types (S023 to S026, S051, S151, S161, S261) [§4.9.3,
  §11.51]. CREATE TYPE AS a list of attributes, methods, subtypes under
  their supertypes, typed tables, IS OF and TREAT. They need SQL-invoked
  routines for their methods.
- Reference types (S041, S043, S162, S094) [§4.11]. REF of a structured
  type, a typed table's self-referencing column and dereference. They
  need structured types and typed tables.

### Periods and temporal tables

- Periods (T181) [§4.16, §11.3, §11.27, §11.28]. PERIOD FOR names two
  columns as a period from its start, included, to its end, excluded.
  Every temporal item builds on it.
- Period predicates (T502) [§8.20]. CONTAINS, OVERLAPS, EQUALS, PRECEDES,
  SUCCEEDS, IMMEDIATELY PRECEDES and IMMEDIATELY SUCCEEDS. OVERLAPS of two
  row values exists today (F053) [§8.14].
- UPDATE and DELETE FOR PORTION OF [§14.9, §14.14] of an application-time
  period table. A row that spans more than the portion is split, its parts
  outside kept as they were.
- Temporal primary and unique keys, WITHOUT OVERLAPS [§11.7], so that no
  two rows of one key hold for one point in time.
- Temporal foreign keys, PERIOD in a referential constraint [§4.25.3,
  §11.8]. A child row's period must be covered by its parents' periods.
- System-versioned tables (T180) [§4.17.2.3, §11.3], PERIOD FOR
  SYSTEM_TIME and WITH SYSTEM VERSIONING. The engine keeps every version a
  change ends as a historical row, which garbage collection must not
  remove, and constraints judge only the current rows [§4.25.3].
- FOR SYSTEM_TIME AS OF, BETWEEN and FROM … TO [§4.17.10, §7.6], reading
  the rows current at a time or within one.
- Bitemporal tables, an application-time period and system versioning on
  one table. No feature names them, and each part above composes once
  both exist.

### JSON

- SQL/JSON's path language (T831 to T838, T840) [§4.48.5, §9.45, §9.46],
  strict and lax, with filters, item methods, starts with, like_regex and
  hex integer literals. Every JSON function reads it, so it comes first.
- SQL/JSON functions over character strings (T803, T811 to T830) [§4.48,
  §6.28, §6.34, §6.35, §7.11, §8.22, §8.23, §10.11]. JSON_OBJECT,
  JSON_ARRAY and their aggregates, JSON_VALUE, JSON_QUERY, JSON_EXISTS, IS
  JSON and JSON_TABLE, with PASSING, ON EMPTY and ON ERROR.
- The JSON type (T801, T802) [§4.8, §6.1, §6.37, §6.40, §6.41], stored
  parsed so a query does not parse text again.
- The simplified accessor (T860 to T864) [§6.36], as `doc.items[0].name`.
  It needs the JSON type.
- Item methods (T865 to T878) [§9.46], bigint() to timestamp_tz().
- JSON in equality, grouping and ordering (T879 to T882) [§4.8.2].

---

## Query processing

Nothing is open.

---

## Storage and durability

Nothing is open.
