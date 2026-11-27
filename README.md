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

## What it has

- Standard SQL as one language, from SQL-86 through SQL:2023.
  [CONFORMANCE.md](CONFORMANCE.md) lists every claimed feature.
- Transactions from READ COMMITTED to SERIALIZABLE on many threads, with
  multi-version concurrency control, savepoints, a write-ahead log and
  crash recovery.
- Row and column storage chosen per table,
  [partitioned tables](doc/sql/23-partitioning.md) and large objects.
- A cost-based optimizer and
  [parallel](doc/operator/04-settings.md#parallel-queries), vectorized
  execution within one memory budget, spilling to disk past it, with
  [EXPLAIN ANALYZE](doc/sql/statements/explain.md#explain-analyze) showing
  what each operator did.
- Indexes by B-link trees and hash tables, over expressions too, and
  R-trees over coordinates and geometries.
- Analytic SQL with window functions, grouping sets, recursive queries and
  row pattern recognition.
- Exact numbers of up to 38 digits, DECFLOAT, arrays, multisets, row,
  distinct and structured types, [UUIDs](doc/sql/02-data-types.md#uuids),
  and application-time and system-versioned tables.
- SQL/JSON with a JSON type, and [SQL/XML](doc/sql/19-xml.md) with XQuery
  and XML Schema.
- [SQL/MM Spatial](doc/sql/10-spatial.md) with
  [topologies](doc/sql/16-topology.md),
  [spatial networks](doc/sql/18-spatial-networks.md) and
  [linear referencing](doc/sql/17-linear-referencing.md).
- [Full-text search](doc/data/01-fulltext.md) in 43 languages, and
  collations from binary order to the Unicode Collation Algorithm.
- [Vectors](doc/data/04-vectors.md) with nearest-neighbour and
  [hybrid search](doc/sql/15-hybrid-search.md), and embedding models.
- [Property graphs](doc/sql/20-property-graphs.md) queried by GRAPH_TABLE
  and [GQL](doc/sql/statements/execute-gql.md), with
  [graph algorithms](doc/sql/21-graph-algorithms.md).
- Stored procedures and functions in SQL/PSM, triggers, and
  [a routine debugger](doc/user/11-debugging.md).
- Replication over mutual TLS, online
  [backups](doc/operator/10-backups.md) and recovery to a point in time.
- Compression and encryption at rest, and roles with privileges.
- External [plug-ins](doc/client/04-plugins.md) of many kinds, storage
  engines, index methods, data types and functions among them, run as
  processes of their own and installed from signed packages.
- A [native protocol](doc/client/05-protocol.md) over TCP with TLS, Unix
  sockets or Windows named pipes, and a
  [client library](doc/client/02-library-connections.md) that pools
  connections and chooses among several servers.
- [Monitoring](doc/operator/11-monitoring.md) of connections, statements,
  tables and indexes.

## Quick start

```
cargo build --release -p turboreld -p turborel-cli
target/release/turboreld --dir DB user add ann --owner               # make the database DB and its owner ANN, asking a password
target/release/turboreld --dir DB --set SERVER.SOCKET=turborel.sock  # serve it
target/release/turborel --socket turborel.sock -U ann -W             # from another terminal, connect and type SQL
```

A password is proved over a Unix socket, a named pipe or TLS, never over
plain TCP, so a server of this machine is reached at its socket.
[Getting Started](doc/user/02-getting-started.md) goes on from here, and
[Connections](doc/operator/06-network.md) covers TCP and TLS.

```
cargo test --workspace   # the tests
cargo xtask verify       # every check a change must pass
```

## Documents

- [doc/](doc/README.md) holds the manuals, a User's Guide, the SQL
  Reference, an Operator's Guide, the Client Reference and Specialized Data.
- [VERSIONING.md](VERSIONING.md) covers releases, alpha, and the versions
  of the file formats and the protocol.
- [CONFORMANCE.md](CONFORMANCE.md) says what is implemented of the
  standards.
- [RULES.md](RULES.md) says how Turborel is designed, written and verified.
- [TODO.md](TODO.md) lists open work.
- [CONTRIBUTING.md](CONTRIBUTING.md) and [SECURITY.md](SECURITY.md) cover
  contributing, and reporting an issue or a vulnerability.
- [LICENSE](LICENSE) and [TRADEMARK.md](TRADEMARK.md). The code is under
  the BSD-2-Clause Plus Patent License. The name Turborel and its logos
  are not, and TRADEMARK.md says how they may be used.
