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

SQL/XML ([XML](#xml)), series by series. Each item becomes a decision in
RULES.md when done.

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

- Connect more geometry predicates and geometry outer joins to `rtree`.
  Extend nearest ordering beyond ascending ST_DISTANCE with FETCH FIRST ONLY;
  consider geometry distance predicates and distribution statistics beyond
  bounded R-tree sampling.
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
- Adjacency held for a graph's edges, so a path query follows an edge
  without a join's search. Graph queries need it.
- XML indexes (RULES.md D106): `xml_path`, `xml_value`, `xml_property`
  and `xml_primary` are built, answer lookups of paths and values, and
  the primary answers queries with filters of an ancestor; the planner
  reads them for XMLEXISTS, XMLQUERY NULL ON EMPTY IS NOT NULL and
  XMLTABLE of a path, the primary answering exact queries with filters of
  an ancestor. Left: a column of XML(SEQUENCE), whose rows of a value of
  other than one node an index would mark so a lookup gives them;
  predicates of more than one step above, several paths joined by `and`,
  and paths of other axes, which no lookup takes yet; and an ON condition
  whose query may raise an error read by an index, which waits for the
  guard of D15 to be read below.

---

## Other standards

Standards beside ISO/IEC 9075-2, each implemented from its own text as
SQL:2023 is (RULES.md D32). Each waits for its index structure above. The groups below are claimed in
CONFORMANCE.md one by one as they are done.

### XML

SQL/XML, ISO/IEC 9075-14, written from its 2006 draft as SQL/PSM was,
and the W3C texts it names: XML 1.0 and 1.1, Namespaces, the XQuery 1.0
data model, XQuery 1.0, its functions and operators, its serialization,
and XML Schema 1.0. Each series below stands alone, by the draft's
features.

- XQuery: XMLQUERY, XMLEXISTS and XMLTABLE evaluate every expression of
  XQuery 1.0 (`src/xquery.rs`), and call every function of XQuery 1.0
  Functions and Operators (`src/xquery/library.rs`). Left: more of a
  query's shapes answered by the XML indexes ([Index
  structures](#index-structures)).
- XML Schema: registered XML Schemas, CREATE and DROP XML SCHEMA, the
  XMLSCHEMA types, XMLVALIDATE and IS VALID are done (`src/xml/schema.rs`,
  `db/definitions/xml_schemas.rs`, `binder/xml/valid.rs`), USAGE on one is
  granted and revoked by GRANT and REVOKE USAGE ON XML SCHEMA
  (`db/usage/xml_schemas.rs`), and the Information Schema shows them
  (X160, X161). Left: the short-name views XML_SCHEMAS_S,
  XML_SCH_NSPACES_S and XML_SCH_ELEMENTS_S, with short-name views of the
  Information Schema at all.

### Full text

### Spatial

- Geometry predicates over the registered XY GEOMETRY representation:
  topological validity, Contains/Within, Covers, Touches, Crosses, Overlaps,
  topological Equals and Relate. Add area and length measures and geometry
  component/vertex accessors.
  Extend intersection's envelope candidates to geometry outer joins and distance
  searches while preserving runtime errors. Track spatial serializable read
  regions instead of the current whole-table predicate read.
  Add SQL large objects for WKT/WKB beyond the current string limits,
  and Z/M representations.
  Add enforced geometry kind constraints. Registered function calls
  in stored CHECK conditions need their syntax permission retained across
  rebinding and recovery, together with determinism checks.
- SQL/MM Spatial (ISO/IEC 13249-3). The ST_Geometry type hierarchy of
  points, curves, surfaces and their collections, their constructors from
  well-known text and binary, measures, the spatial relationships of
  ST_Intersects to ST_Relate, ST_Buffer and the set operations, and
  spatial reference systems, over the R-tree.
- Simple feature access, SQL option (ISO 19125-2). The schema of geometry
  columns and spatial reference systems it defines, and its
  geometry functions where they differ from SQL/MM's, so tools that read
  either find both.

### Graph

- GQL (ISO/IEC 39075:2024). Property graphs of nodes and edges with labels
  and properties, their graph types, MATCH of graph patterns with
  quantified paths and path modes, and inserting, setting and removing
  nodes and edges, as a language beside SQL over the same storage and
  transactions.
- SQL/PGQ (ISO/IEC 9075-16:2023). CREATE PROPERTY GRAPH over existing
  tables, naming which are vertex tables and which edge tables and the
  keys that join them, and GRAPH_TABLE in FROM, whose MATCH shares GQL's
  graph patterns and gives its matches as rows to the rest of the query.
  It shares GQL's pattern matcher and the edges' adjacency, so the two
  are built together.

### Vectors

Nothing is open.
