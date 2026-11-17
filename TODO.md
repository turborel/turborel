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

- A row a statement holds past its work area (`exec/held.rs`) is written
  to its spill area in the log's form of values, so a large object or an
  XML value a table keeps apart is written as that table's object id and
  read back as a reference no table resolves: an UPDATE of such a row
  fails (SQLCODE -916), and an INSERT of rows read from another table may
  name that table's object. Held rows should keep such values beside
  their records.

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
- An inverted index of terms to the rows and positions they appear at,
  with its tokenizer, for full text search. Full text needs it.
- HNSW, a hierarchical graph of vectors for approximate nearest neighbour
  search. Vectors need it, and a structure kept on disk past what memory
  holds may follow it.
- Adjacency held for a graph's edges, so a path query follows an edge
  without a join's search. Graph queries need it.
- An index of XML documents' paths and values, so XQuery and XPath read
  only the nodes they ask for. XML needs it.

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

- Publishing: WITH XMLNAMESPACES and XMLBINARY of a
  query expression, and their scope in DML, DDL and compound statements
  (X081 to X084, X131 to X135); XMLNAMESPACES in XMLTABLE (X086), with
  XQuery; arrays, multisets and rows mapped to XML [§9.8 GR9i, GR9j], as
  XMLELEMENT's and XMLFOREST's content; and the mapping of XML Names to
  identifiers (X400).
- XQuery: XMLQUERY, XMLEXISTS and XMLTABLE over the XQuery 1.0 data
  model, its functions and operators, passed BY VALUE or BY REF (X096,
  X200 to X206, X221, X222, X300 to X305). An index of documents' paths
  and values follows ([Index structures](#index-structures)). The parser
  exists (`src/xquery.rs`): the whole XQuery 1.0 grammar into a tree,
  written back as text, with the static errors the text alone shows.
  Left: the static context (namespaces, names, types, the boundary-space
  policy applied), evaluation, and the imports, which are parsed and
  refused.
- XML Schema: registered XML schemas, XML(DOCUMENT(XMLSCHEMA)) and
  XML(CONTENT(XMLSCHEMA)), XMLVALIDATE and IS VALID, and their
  Information Schema (X141 to X161, X191, X192, X256, X257, X260 to X286).
- The mappings of tables, schemas and catalogs to XML and XML Schema
  (X040 to X059).

### Full text

- SQL/MM Full-Text (ISO/IEC 13249-2). The FULLTEXT type and its methods,
  CONTAINS and its search patterns of words, phrases, proximity,
  stemming and thesaurus, and ranking by score, over the inverted index.

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

- A VECTOR type and its search [EXTENSION]. No ISO standard defines one,
  so it follows what Oracle and Microsoft SQL Server ship, from their
  documentation. It covers a VECTOR column of a dimension and an element
  type, VECTOR_DISTANCE by cosine, Euclidean, dot product, Manhattan and
  Hamming metrics, norms and normalizing, and CREATE VECTOR INDEX over
  HNSW. An approximate top-k query, ORDER BY a distance then FETCH APPROX
  FIRST, reads the index, and an exact one sorts as ORDER BY does today.
