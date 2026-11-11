# turborel

> turborel is in alpha, meaning, version 0.0.0. Anything may change
> from one build to the next, SQL, API, protocol and files alike. See
> [VERSIONING.md](VERSIONING.md).

turborel (TUR-boh-rel) is a relational database system written in
Rust. Embed it as a library, or run it as a server, `turboreld`, for
many clients at once, and query it in SQL with `turborel`, its
command-line client.

Nearly every part of turborel is a plug-in point. Storage engines, index
methods, data types, functions, optimizer rules, system tables, the log,
the catalog's store, file systems, encodings, authentication, transports
and wire protocols are each an interface with a built-in implementation
behind it, and each can be replaced or added to.

## What it has

- Standard SQL implementing SQL-86 in full, SQL-92 being taken in full, and each
  later standard in turn through SQL:2023, as one language. See
  [CONFORMANCE.md](CONFORMANCE.md).
- Transactions at READ COMMITTED by default, or up to SERIALIZABLE, that run
  on many threads at once and survive a crash with multi-version concurrency
  control, a write-ahead log, checkpoints and recovery.
- Two storage engines over one data file, chosen per table. The row store
  suits point reads and updates. The column store freezes rows into
  compressed column groups for scans of a few columns of many rows, and
  keeps new rows in a delta so writes stay fast. A table moves between them
  with ALTER TABLE.
- Multiple Indexes such as B-link trees and linear hash tables, chosen by a
  cost-based planner.
- Collations, from binary order through case-insensitive to the Unicode
  Collation Algorithm and CLDR's tailorings for 48 languages, so text sorts
  and compares as its readers expect.
- Compression and encryption at rest, LZ4 and AES-256-GCM or
  ChaCha20-Poly1305, for a whole database or one table.
- Replication over a group of servers. A change commits once a majority
  holds it, the backups serve reads, and they choose a new primary within
  seconds when one is lost.
- Domains, assertions, temporary tables, scrollable cursors and dynamic SQL.
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
  Reference, an Operator's Guide and the Client Reference.
- [VERSIONING.md](VERSIONING.md): releases, alpha, and the versions of
  the file formats and the protocol.
- [CONFORMANCE.md](CONFORMANCE.md): what is implemented of the
  standards.
- [RULES.md](RULES.md): how turborel is designed, written and verified.
- [TODO.md](TODO.md): open work.
- [CONTRIBUTING.md](CONTRIBUTING.md) and [SECURITY.md](SECURITY.md):
  contributing, and reporting an issue or a vulnerability.
- [LICENSE](LICENSE) and [TRADEMARK.md](TRADEMARK.md): the code is
  under the BSD 2-Clause License; the name turborel and its logos are not,
  and TRADEMARK.md says how they may be used.
