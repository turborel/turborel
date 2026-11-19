<p align="center">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="artwork/turborel-logo-white.svg">
      <img src="artwork/turborel-logo.svg" alt="turborel" width="360">
    </picture>
</p>

> Turborel is in alpha, meaning, version 0.0.0. Anything may change
> from one build to the next, SQL, API, protocol and files alike. See
> [VERSIONING.md](VERSIONING.md).

Turborel (TUR-boh-rel) is a relational database system written in
Rust. Embed it as a library, or run it as a server, `turboreld`, for
many clients at once, and query it in SQL with `turborel`, its
command-line client.

Nearly every part of Turborel is a plug-in point. Storage engines, index
methods, data types, functions, optimizer rules, system tables, the log,
the catalog's store, file systems, encodings, authentication, transports
and wire protocols are each an interface with a built-in implementation
behind it, and each can be replaced or added to.

## What it has

- Standard SQL as one language, from SQL-86 through SQL:2023. See
  [CONFORMANCE.md](CONFORMANCE.md).
- Transactions at READ COMMITTED by default, or up to SERIALIZABLE, that run
  on many threads at once and survive a crash with multi-version concurrency
  control, a write-ahead log, checkpoints and recovery.
- Savepoints, with SAVEPOINT, RELEASE SAVEPOINT and ROLLBACK TO SAVEPOINT
  at the savepoint levels of routines and ATOMIC blocks.
- Row and columnar storage, chosen per table.
- Large objects up to 4G, stored, logged, read and sent a chunk at a
  time, and reached by locators.
- Indexes as plug-ins. B-link trees, linear hash tables and R-trees over
  points and rectangles are built in. Any other method registers with its
  own options, and a cost-based planner chooses among them for filters
  and inner joins. Zone maps and Bloom filters skip what a scan cannot
  match.
- [SQL/MM Spatial](doc/sql/10-spatial.md), its 27 geometry types in 2D
  and 3D with every constructor and mutator, exact relationships and
  set operations, reference systems, angles, directions and vectors,
  CHECK constraints over geometries,
  and [R-tree spatial queries](doc/data/02-spatial.md), distance searches
  and joins, outer joins too,
  serializable over the windows they read, and Simple feature access's
  SQL option with GEOMETRY_COLUMNS, SPATIAL_REF_SYS, unprefixed names and
  annotation text.
- [Topologies](doc/sql/16-topology.md) of nodes, edges and faces, and
  [spatial networks](doc/sql/18-spatial-networks.md) with shortest paths.
- [Linear referencing](doc/sql/17-linear-referencing.md), locating along
  roads, rails and other linear features by measure and offset.
- [Full-text search](doc/data/01-fulltext.md) by a text's words,
  whatever their case and accents, through an inverted index of
  block-compressed postings.
- [SQL/MM Full-Text](doc/sql/08-fulltext.md), its FULLTEXT type, CONTAINS
  patterns and thesauri, ranked by BM25 or a ranking of your own.
- Collations, from binary order through case-insensitive to the Unicode
  Collation Algorithm and CLDR's tailorings for 48 languages, so text sorts
  and compares as its readers expect.
- Compression and encryption at rest, LZ4 and AES-256-GCM or
  ChaCha20-Poly1305, for a whole database or one table.
- Replication by Viewstamped Replication over mutual TLS. A change commits
  once a majority of servers holds it, so three servers survive losing one.
  Backups serve reads and elect a new primary within seconds, and a lost
  server catches up when it rejoins.
- Analytic SQL, with window functions, grouping sets, recursive queries
  with SEARCH and CYCLE, row pattern recognition (MATCH_RECOGNIZE), and
  the statistical set functions and math functions of T621.
- Arrays, row types, distinct types, DECFLOAT, regular expressions,
  sequences and identity columns.
- SQL/JSON's JSON_VALUE, JSON_QUERY and JSON_EXISTS, with the SQL/JSON
  path language, strict and lax, and its filters; IS JSON, JSON_OBJECT,
  JSON_ARRAY, JSON_OBJECTAGG and JSON_ARRAYAGG.
- A JSON type stored parsed, with `doc.items[0].name` accessors, and
  JSON in comparisons, grouping and ordering.
- JSON_TABLE, with nested columns, plans and PASSING.
- A json index method answering JSON_EXISTS, JSON_VALUE and
  JSON_CONTAINS by an inverted index of paths and values.
- Indexes of expressions, such as `UPPER(name)`, `a + b` or a JSON_VALUE
  that cannot raise, used for lookups and ORDER BY.
- [Vectors](doc/data/04-vectors.md), a VECTOR type of dense or sparse
  embeddings with distances, operators and nearest-neighbour search,
  exact or approximate.
- Vector indexes by DiskANN, HNSW, IVF and an inverted index of sparse
  vectors, kept up to date as rows change.
- [Hybrid search](doc/sql/15-hybrid-search.md), fusing BM25 and vector
  top-k by RRF or weighted scores.
- Embedding models, pipelines that chunk and embed text, and columns
  embedded after commit.
- Multisets, with MULTISET UNION, EXCEPT and INTERSECT, MEMBER,
  SUBMULTISET and IS A SET, and COLLECT, FUSION and INTERSECTION.
- Structured types with attributes, methods and subtypes, typed tables
  in hierarchies, and references that are dereferenced.
- Application-time periods, PERIOD FOR two columns of a table, its start
  before its end, the period predicates OVERLAPS, EQUALS, CONTAINS,
  PRECEDES, SUCCEEDS and IMMEDIATELY PRECEDES and SUCCEEDS, and UPDATE and
  DELETE FOR PORTION OF a period, which keep the parts of a row outside it,
  keys WITHOUT OVERLAPS of it, one row of a key at each point in time, and
  foreign keys of a period, whose rows' periods their references cover.
- System-versioned tables, PERIOD FOR SYSTEM_TIME of ROW START and ROW END
  columns the database stamps, WITH SYSTEM VERSIONING keeping each row an
  update or delete ends as a historical row, read FOR SYSTEM_TIME AS OF,
  BETWEEN or FROM … TO, and bitemporal with an application-time period.
- Stored procedures and functions in SQL/PSM, with variables, cursors,
  handlers, loops and SIGNAL, grouped into modules if wished, and
  polymorphic table functions that take whole tables as arguments.
- Triggers before, after and instead of a change, for each row or
  statement, and roles with privileges granted and revoked.
- Domains, assertions, temporary tables, scrollable cursors and dynamic SQL.
- Columns renamed in place, without copying their rows.
- Import of CSV and tab-separated files through the client.
- Library, embedded modes
- A server over TCP, Unix sockets or Windows named pipes, with TLS,
  users and passwords, prepared statements and cancellation.
- Bounded memory with one budget over every cache, with sorts, joins and
  grouping that spill to disk.

## Quick start

```
cargo run -p turboreld -- --dir DB     # serve the database in DB on 127.0.0.1:7422
cargo run -p turborel-cli -- --help    # the command-line client
cargo test --workspace                 # the tests
cargo xtask verify                     # every check a change must pass
```

## Documents

- [doc/](doc/README.md): the manuals: a User's Guide, the SQL
  Reference, an Operator's Guide, the Client Reference and Specialized Data.
- [VERSIONING.md](VERSIONING.md): releases, alpha, and the versions of
  the file formats and the protocol.
- [CONFORMANCE.md](CONFORMANCE.md): what is implemented of the
  standards.
- [RULES.md](RULES.md): how Turborel is designed, written and verified.
- [TODO.md](TODO.md): open work.
- [CONTRIBUTING.md](CONTRIBUTING.md) and [SECURITY.md](SECURITY.md):
  contributing, and reporting an issue or a vulnerability.
- [LICENSE](LICENSE) and [TRADEMARK.md](TRADEMARK.md): the code is
  under the BSD-2-Clause Plus Patent License; the name Turborel and its
  logos are not, and TRADEMARK.md says how they may be used.
