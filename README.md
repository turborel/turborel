# turborel

A relational database system in Rust, queried in SQL: the database as a
library to embed; a server, `turboreld`, serving it to many clients at
once; a client of the server; and `turborel`, a command-line client. Each
is a crate of its own, over a shared wire protocol. The library and the
client have no dependencies; the server has tokio, and rustls over
aws-lc-rs for TLS, which the client has too with its feature `tls`; the
command-line client adds tokio's signals and rpassword.

turborel follows the SQL standards production by production: **ANSI
X3.135-1986** ("SQL-86") first, extended by **ISO/IEC 9075:1992**
("SQL-92"), read as one language where the newer wins. It claims full
SQL-86 conformance at Level 2, by direct processing through its Rust
binding, and is taking SQL-92 in full. Each later standard extends the
same language in turn, through **ISO/IEC 9075:2023** ("SQL:2023"), beside
turborel's own extensions.

What it has:

- **Transactions** that are serializable, or at a weaker isolation level
  when asked, run on many threads at once, and
  survive a crash: a write-ahead log, checkpoints and recovery, for a
  database kept in a directory.
- **A row store** with multi-version concurrency control: readers never
  block writers, nor writers readers.
- **SQL-92 features** beyond SQL-86: joins, query expressions, CASE and
  CAST, constraints with referential actions, deferrable constraints,
  CREATE TABLE and VIEW, ALTER TABLE, DROP SCHEMA, TABLE and VIEW,
  GRANT and REVOKE as statements in transactions, and the Information
  Schema.
- **Indexes** as B-link trees and linear hash tables, in pages through a
  buffer pool, chosen by a cost-based planner.
- **Plug-ins** for storage engines, index methods, functions, optimizer
  rules, system tables, the log, the catalog's store, file systems,
  encodings, hash functions, diagnostics, and the areas operators spill to.
- **A server** of the same database, for many clients at once, over TCP or
  Unix sockets, with TLS, users and passwords, prepared statements with
  SQL-92's dynamic parameters, results read a batch at a time, statements
  cancelled, pluggable authentication, transports and wire protocols,
  configured by the database's settings. A program may embed
  the database and serve it too.
- **A command-line client**, `turborel`, to run SQL on a server: typed,
  given on its command line or read from files, a query's rows shown as a
  table as they come, Ctrl-C cancelling the statement running.
- **Bounded memory:** one budget over every cache, and sorts, grouping,
  DISTINCT, hash joins and set operations that spill to disk past their
  work area.

## Build and test

```
cargo test --workspace           # unit, integration and tool tests
cargo xtask verify               # every check a change must pass
cargo xtask install-hooks        # run verify before every commit (once per clone)
cargo xtask coverage             # structural coverage report
cargo xtask trace '§8.6'         # which tests verify a clause
cargo xtask bench [NAME]         # benchmarks, optimized (--save LABEL, --compare LABEL)
cargo run -p turboreld -- --dir DB   # serve the database in DB at 127.0.0.1:7422
cargo run -p turborel-cli -- --help  # the command-line client: SQL run on a server
```

## Documents

- **[CONFORMANCE.md](CONFORMANCE.md):** what is implemented of the
  standards, clause by clause, and the choices they leave to an
  implementation.
- **[RULES.md](RULES.md):** how turborel is designed (the decisions), and
  how its code is written and verified.
- **[TODO.md](TODO.md):** open work.
- **[CONTRIBUTING.md](CONTRIBUTING.md):** how to contribute, and how to
  report an issue; **[SECURITY.md](SECURITY.md)**, how to report a
  vulnerability.
