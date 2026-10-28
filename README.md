# turborel

> **ALPHA.** turborel is version 0.0.0, in alpha: anything may change
> from one build to the next, SQL, API, protocol and files alike. See
> [VERSIONING.md](VERSIONING.md).

turborel (TUR-boh-rel) is a relational database system written in
Rust. Embed it as a library, or run it as a server, `turboreld`, for
many clients at once, and query it in SQL with `turborel`, its
command-line client.

## Built to be extended

Nearly every part of turborel is a plug-in point. Storage engines, index
methods, data types, functions, optimizer rules, system tables, the log,
the catalog's store, file systems, encodings, authentication, transports
and wire protocols are each an interface with a built-in implementation
behind it, and each can be replaced or added to.

Its built-in engine keeps rows in a row store today. Its table layout is
built to be pluggable, and a column store and a hybrid of the two are
next, for analytical (OLAP) and mixed workloads beside transactional
ones.

## What it has

- **Standard SQL.** SQL-86 in full, SQL-92 being taken in full, and each
  later standard in turn through SQL:2023, as one language. See
  [CONFORMANCE.md](CONFORMANCE.md).
- **Transactions** that are serializable by default, run on many
  threads at once, and survive a crash: multi-version concurrency
  control, a write-ahead log, checkpoints and recovery.
- **Indexes** as B-link trees and linear hash tables, chosen by a
  cost-based planner.
- **A server** over TCP or Unix sockets, with TLS, users and passwords,
  prepared statements and cancellation.
- **Bounded memory:** one budget over every cache, with sorts, joins and
  grouping that spill to disk.

## Quick start

```
cargo run -p turboreld -- --dir DB     # serve the database in DB on 127.0.0.1:7422
cargo run -p turborel-cli -- --help    # the command-line client
cargo test --workspace                 # the tests
cargo xtask verify                     # every check a change must pass
```

## Documents

- **[doc/](doc/README.md):** the manuals: a User's Guide, the SQL
  Reference, an Operator's Guide and the Client Reference.
- **[VERSIONING.md](VERSIONING.md):** releases, alpha, and the versions of
  the file formats and the protocol.
- **[CONFORMANCE.md](CONFORMANCE.md):** what is implemented of the
  standards.
- **[RULES.md](RULES.md):** how turborel is designed, written and verified.
- **[TODO.md](TODO.md):** open work.
- **[CONTRIBUTING.md](CONTRIBUTING.md)** and **[SECURITY.md](SECURITY.md):**
  contributing, and reporting an issue or a vulnerability.
- **[LICENSE](LICENSE)** and **[TRADEMARK.md](TRADEMARK.md):** the code is
  under the BSD 2-Clause License; the name turborel and its logos are not,
  and TRADEMARK.md says how they may be used.
