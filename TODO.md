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
[Storage and durability](#storage-and-durability) ·
[Index structures](#index-structures) ·
[Other standards](#other-standards) · [XML](#xml) ·
[Full text](#full-text) · [Spatial](#spatial) · [Graph](#graph) ·
[Vectors](#vectors)

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

Nothing is open.

### Changing data

Nothing is open.

### Triggers, routines and roles

Nothing is open.

### Types

Nothing is open.

### Periods and temporal tables


### JSON

Nothing is open.

---

## Query processing

Nothing is open.

---

## Storage and durability

Nothing is open.

---

## Index structures

The access methods the other standards stand on. Each one is built, made
durable and tested on its own before the extension that reads it. Each
is kept consistent under concurrent change and recovery as the B-tree is.

- Extend nearest ordering beyond ascending ST_DISTANCE with FETCH FIRST ONLY;
  consider distribution statistics beyond bounded R-tree sampling.
  Price geometry predicate evaluation and clustered row fetches when
  choosing between a scan and a spatial probe. Measure large-result row
  materialization and wire encoding separately from tree traversal.
  Extend adaptive joins to builds larger than their memory budget through
  partitioning and spill. Cover large inputs on
  both sides, real polygon and line data, disk pressure and sustained
  updates in complete-query comparisons, including tail latency.
  Let SQL joins traverse two indexed trees directly,
  beyond probing one tree for each outer row. Add bulk packing
  and reclaim empty nodes without relocating entries under active scans;
  measure overlap and space after sustained updates as well as fresh loads.

---

## Other standards

Standards beside ISO/IEC 9075-2, each implemented from its own text as
SQL:2023 is (RULES.md D32). Each waits for its index structure above. The groups below are claimed in
CONFORMANCE.md one by one as they are done.

### XML

Nothing is open.

### Full text

### Spatial

Nothing is open.

### Graph

Nothing is open.

### Vectors

Nothing is open.
