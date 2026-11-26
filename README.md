<p align="center">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="artwork/turborel-logo-white.svg">
      <img src="artwork/turborel-logo.svg" alt="turborel" width="360">
    </picture>
</p>

> Turborel is in alpha, meaning, version 0.0.0. Anything may change
> from one build to the next, SQL, API, protocol and files alike. See
> [VERSIONING.md](VERSIONING.md). The latest alpha is built for Linux,
> macOS and Windows as the pre-release
> [nightly](https://github.com/turborel/turborel/releases/tag/nightly),
> and as the image `ghcr.io/turborel/turborel:nightly`.

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
  [CONFORMANCE.md](CONFORMANCE.md), whose claimed features SQL_FEATURES shows.
- Transactions at READ COMMITTED by default, or up to SERIALIZABLE, that run
  on many threads at once and survive a crash with multi-version concurrency
  control, a write-ahead log written once per transaction at its commit,
  checkpoints and recovery.
- Savepoints, with SAVEPOINT, RELEASE SAVEPOINT and ROLLBACK TO SAVEPOINT
  at the savepoint levels of routines and ATOMIC blocks.
- [Data change delta tables](doc/sql/24-delta-tables.md), OLD, NEW and
  FINAL TABLE of INSERT, UPDATE, DELETE and MERGE.
- Row and columnar storage, chosen per table, the columnar store's filters
  judged in place and its other columns decoded of the rows kept alone.
- DISTINCT set functions computed by two groupings, read by batches.
- Grouping columns a key decides carried, not grouped by, and read after
  FETCH FIRST for the rows kept.
- Groupings and DISTINCT streamed over rows in their keys' order, a group
  at a time, and FETCH FIRST's row goal passed through filters, joins and
  groupings to an ordered read.
- Batches of vectors with null bitmaps, dates and truth values among them,
  filtered by pluggable kernels, and parallel morsels read by batch.
- Vector kernels on SSE2, AVX2, AVX-512 and NEON, chosen by the processor
  and each bit for bit the portable body: filters, sums, extremes,
  decimals, grouping hashes, vector distances and CRC-32C.
- One table of groups with typed keys and columns of set functions'
  states, merged in parallel in waves, of the column store too, its hash
  keyed past long probes; EXCEPT and INTERSECT counted in it.
- Hash joins probed a batch at a time in one tagged table, shared by
  parallel morsels, telling the left's scan a range and Bloom filter;
  dense keys found by their place, their filter an exact bitmap.
- Sets of a few keys probed in the index, from IN, EXISTS, a subquery of
  one row, a LEFT JOIN or MERGE from host arrays, and a join's bounds.
- Expressions compiled once and evaluated a vector at a time, CASE by the
  rows each WHEN keeps.
- Window functions over batches, partitions found of the sort's keys by a
  kernel, moving frames by prefix sums and segment trees.
- Sorts by normalized keys, radix sorted, spilled runs merged by a tree of
  losers, and FETCH FIRST that makes only the rows it gives, reading its
  other columns late and skipping groups past its bound.
- [Partitioned tables](doc/sql/23-partitioning.md) by RANGE, LIST, HASH
  or a strategy a plug-in registers, many levels deep, attached and
  detached, rows routed and moved, reads pruned and merged in order, joins
  routed by each probe, global indexes holding keys across partitions,
  and every index method made on each partition.
- Large objects up to 4G, stored, logged, read and sent a chunk at a
  time, and reached by locators.
- Indexes as plug-ins. B-link trees, with prefix-truncated nodes, key heads
  and leaf hints to their rows, linear hash tables and R-trees over
  points and rectangles are built in, R-trees packed when built and their
  emptied nodes reclaimed up to the root. Any other method registers with its
  own options, and a cost-based planner chooses among them for filters
  and inner joins. Zone maps and Bloom filters skip what a scan cannot
  match, and a row-store COUNT(*) is summed from its settled zones.
- Column store skipping that survives writes: shadow bits and zones over
  the delta, optional column imprints, column sketches of compressed
  texts, and sorted dictionaries judged by codes. COUNT(*), MIN and MAX
  come from the groups' directory and summaries.
- Bulk loads by INSERT or copy that build empty indexes whole and write
  column store rows straight into frozen groups, sent by column and
  parsed into each column's type with no row of values made.
- [Join ordering](doc/sql/06-queries.md#how-joins-are-planned-extension)
  by dynamic programming, DPccp and DPhyp, outer joins reordered where
  sound, hash, merge and index joins chosen with the order, adaptive to
  queries of a hundred tables, on statistics of most common values, a
  sample of rows for correlated conditions and indexes' joint key counts,
  joins estimated by keys and bounded, index joins chosen robustly.
- [Column statistics](doc/sql/07-information-schema.md#column-statistics)
  in a view: what the planner holds of each column, and each index's keys.
- [Range joins](doc/sql/06-queries.md#how-joins-are-planned-extension) of
  bands and overlapping periods, one table sorted and searched by the other.
- [Predicate transfer](doc/sql/06-queries.md#how-joins-are-planned-extension)
  shrinks a chain's tables by Bloom filters of each other's keys before
  they are joined, from the smaller tables to the largest and back.
- [EXPLAIN ANALYZE](doc/sql/statements/explain.md#explain-analyze) runs a
  query or a data change and shows each operator's rows beside its
  estimate, its batches, spills and index reads, and a program may give
  the planner true row counts to test its plans.
- [EXPLAIN of a data change](doc/sql/statements/explain.md#explain-of-a-data-change)
  shows its plan, indexes, cascades, triggers and point path, running nothing.
- Correlated subqueries of every form unnested into joins, and SQL
  functions that return one value inlined, so each is computed once
  rather than once for each row.
- [Lateral derived tables](doc/sql/06-queries.md#lateral-derived-tables),
  whose query reads the tables before them in the FROM clause.
- [Semi joins and antijoins](doc/sql/06-queries.md#correlated-subqueries)
  for EXISTS, NOT EXISTS, IN and NOT IN, ordered with the other joins,
  and joins under MIN and MAX reduced to semi joins; mark joins for
  those under OR, in CASE and in select lists.
- Joins removed on unique and foreign keys, filters and FETCH FIRST moved
  into set operations and below windows, and parts of conditions that can
  raise computed on the table they read, so joins over them are reordered.
- [SQL/XML](doc/sql/19-xml.md): an XML type kept in tables, in arrays,
  multisets, rows and distinct and structured types too, its values parsed
  from XML 1.0 and 1.1 text and serialized back, XMLDOCUMENT, XMLCAST, IS
  DOCUMENT and IS CONTENT, and XML published from SQL values, arrays and
  multisets too, by
  XMLELEMENT, XMLFOREST, XMLAGG, XMLCONCAT, XMLCOMMENT, XMLPI and XMLTEXT
  with namespaces and XMLBINARY of a query, a statement or a definition,
  tables, schemas and the catalog mapped to XML and XML Schema by
  XMLMAP, identifiers and XML Names mapped by XMLNAME_TO_IDENTIFIER and
  IDENTIFIER_TO_XMLNAME, and queried in XQuery 1.0 by XMLQUERY, XMLEXISTS
  and XMLTABLE, statically typed on request, importing library modules
  registered by CREATE XQUERY MODULE, written in-house with no
  dependency, its modifiers shown in the
  Information Schema, XML indexes of its paths and values, and XML
  Schemas registered by CREATE XML SCHEMA, values validated by them with
  XMLVALIDATE, IS VALID and XQuery's validate and kept in columns of
  their XMLSCHEMA types, USAGE on them and their Information Schema views.
- [SQL/MM Spatial](doc/sql/10-spatial.md), its 27 geometry types in 2D
  and 3D with every constructor and mutator, exact relationships and
  set operations, reference systems, angles, directions and vectors,
  CHECK constraints over geometries,
  and [R-tree spatial queries](doc/data/02-spatial.md), distance searches
  and joins, outer joins too, priced by geometry statistics, spilled past
  memory or through two trees at once,
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
- [Full text in 43 languages](doc/sql/08-fulltext.md#languages), stemmed by
  Snowball's own stemmers, with CJK and Thai read as character bigrams,
  and more served by plug-ins, as dictionary segmenters.
- Collations, from binary order through case-insensitive to the Unicode
  Collation Algorithm and CLDR's tailorings for 48 languages, so text sorts
  and compares as its readers expect.
- Compression and encryption at rest, LZ4 and AES-256-GCM or
  ChaCha20-Poly1305, for a whole database or one table.
- Replication by Viewstamped Replication over mutual TLS. A change commits
  once a majority of servers holds it, so three servers survive losing one.
  Backups serve reads and elect a new primary within seconds, and a lost
  server catches up when it rejoins.
- [Backups](doc/operator/10-backups.md), full and incremental, taken
  online by BACKUP DATABASE, verified and restored offline, and recovery
  to a point in time through an archive of the log, to pluggable targets.
- [Monitoring](doc/operator/11-monitoring.md): each connection's running
  statement and what it waits on, statistics of every statement run, and
  each table's size and last change and each index's searches.
- Analytic SQL, with window functions, grouping sets, recursive queries
  with SEARCH and CYCLE, row pattern recognition (MATCH_RECOGNIZE), and
  the statistical set functions and math functions of T621.
- Exact numbers of up to 38 digits, in arithmetic and in columns of
  either store.
- Arrays, row types, distinct types, DECFLOAT, regular expressions,
  sequences and identity columns.
- [UUIDs](doc/sql/02-data-types.md#uuids), with UUID_V4() and time-ordered
  UUID_V7().
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
- [External plug-ins](doc/client/04-plugins.md): functions, metrics,
  models and more from programs of other parties, run as processes of their
  own, installed from signed package files, with a starter project and
  `pack` for authors and licenses told.
- [Every plug-in kind external](doc/client/04-plugins-kinds.md), types,
  collations, storage engines and index methods among them, with an SDK.
- [Host plug-ins](doc/client/04-plugins-host.md) beneath the database,
  its encodings, log, clock, files, keys and listeners among them.
- [Replication and archive plug-ins](doc/client/04-plugins-replication.md):
  a replica's transport, journal, state transfer and consensus, and a
  backup's archive, served by host plug-ins that fail stop.
- [Plug-in settings](doc/client/04-plugins.md#settings) declared by
  manifests and set as built-in ones, and a buffer eviction policy a
  host plug-in programs.
- [Optimizer rules and statistics as plug-ins](doc/client/04-plugins-planner.md):
  rules that rewrite a stable plan form, and statistics both ways.
- [Property graphs](doc/sql/20-property-graphs.md) over tables or
  queries' rows, queried by GRAPH_TABLE's path patterns through adjacency
  indexes, LINE_GRAPH among them.
- Native graph tables, CREATE TABLE ... AS NODE and AS EDGE, with hidden
  identities, CONNECTION constraints and each schema's SYS_GRAPH.
- [Graph compatibility syntax](doc/sql/22-graph-compatibility.md):
  MATCH in a WHERE clause, identity functions and hints, PGQL and
  Cypher queries, as GRAPH_TABLE.
- [GQL](doc/sql/statements/execute-gql.md) by EXECUTE GQL, GQL_TABLE or
  a GQL session, over property graphs and typed or open graphs.
- [Graph algorithms](doc/sql/21-graph-algorithms.md) such as PAGERANK,
  WCC, LOUVAIN and SSSP over a property graph or a table of edges, in
  parallel, with INTO PROPERTY.
- Graph plans that join rigid patterns, Expand edges and take a
  worst-case optimal join of cycles, with label and reachability
  indexes and graph statistics.
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
- [A routine debugger](doc/user/11-debugging.md): one connection attaches
  to another's session and steps through its procedures, functions and
  triggers by breakpoints, reading their stack and variables.
  A procedure's body is compiled as it is first called, its key reads
  and its changes run straight by the plans kept of them.
- Triggers before, after and instead of a change, for each row or
  statement, and roles with privileges granted and revoked.
- Domains, assertions, temporary tables, scrollable cursors and dynamic SQL.
- Columns renamed in place, without copying their rows, and tables renamed.
- A column's NOT NULL set or dropped and its data type widened, the rows
  checked or copied a row at a time.
- Import and export of CSV and tab-separated files through the client, large objects as their values,
  one file imported over several connections in contiguous ranges.
- Prepared CALLs that describe each `?` with its parameter's mode, through the API, DESCRIBE and the protocol.
- Procedure parameters with default values, taken by DEFAULT, by arguments left out or by names not named.
- A command-line client with line editing, completion, describe commands,
  variables, scripting and seven output formats ([guide](doc/client/05-turborel-commands.md)).
- Library, embedded modes
- A server over TCP, Unix sockets or Windows named pipes, with TLS,
  users and passwords, prepared statements, pipelined requests whose
  replies share a write, and cancellation.
- A [native protocol](doc/client/05-protocol.md) that grows by
  negotiated capabilities: structured errors and notices, row counts,
  generated keys and server time in each completion, bulk copy,
  compression, session status, columns' declared types, and reads
  after a commit on replicas, with sample frames for every message.
- A [client library](doc/client/02-library-connections.md) that pipelines
  requests, pools connections, fails over across servers, and takes its
  settings from a URI, the environment, a service file or a password file.
- [CALL](doc/sql/statements/call.md) prepared with `?` for OUT and INOUT
  parameters, their values returned as a row over the protocol.
- Bounded memory with one budget over every cache, with sorts, joins and
  grouping granted their share of it and spilling to disk past it.
- [Parallel queries](doc/operator/04-settings.md#parallel-queries) run by
  morsels on a shared pool of workers, the same rows on any number of threads.
- [xxHash](doc/operator/07-encryption.md#hash-functions) for every checksum,
  hash table and filter, XXH3 vectorized on SSE2, AVX2, AVX-512 and NEON.

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
