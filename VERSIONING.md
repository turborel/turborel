# Versioning

How turborel's releases are numbered and named, and what each kind of
release may change. The files a database is kept in and the wire protocol
have version numbers of their own; this file lists them.

Contents: [Release numbers](#release-numbers) · [Alpha](#alpha) ·
[Compatibility](#compatibility) · [Codenames](#codenames) ·
[Format and protocol versions](#format-and-protocol-versions) ·
[Where the version is](#where-the-version-is) · [Publishing](#publishing)

---

## Release numbers

A release is numbered **MAJOR.MINOR.PATCH**, as in
[Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html). One
number is the version of every part of turborel at once: the library, the
protocol, the client, the server, `turboreld`, the command-line client and
the crates they rest on. They are released together, and a client and a
server of the same release always work together.

## Alpha

turborel is in alpha, version **0.0.0**, until its first release. While
its version is 0.y.z nothing is promised: SQL's behaviour, the Rust API,
the protocol and the files a database is kept in may change in any
release. A database made by one may not open with another, and is made
again from its SQL.

The first stable release is **1.0.0**.

## Compatibility

From 1.0.0, what a release may change follows from its number:

- **PATCH** (1.0.0 to 1.0.1) fixes what is wrong. It adds nothing, and
  changes no file format, no protocol version, nothing the Rust API
  offers, and no SQL that ran correctly before.
- **MINOR** (1.0.1 to 1.1.0) adds, and keeps what was: SQL, settings, API
  items, protocol capabilities (exchanged in Hello and Ready, so a client
  and a server agree on the ones both have). A database kept by one
  release of a major version opens with every later release of that major
  version.
- **MAJOR** (1.4.2 to 2.0.0) may change anything: a file format, the
  protocol's version, the Rust API, how a statement behaves. Its release
  says how a database is brought from the major version before to it.

## Codenames

Every major version has a codename. A minor or patch release that marks a
large milestone may have a codename of its own. Any other release carries
the codename of the last named release before it in its major version, so
every release has one.

| Release | Codename |
|---|---|
| 0.y.z | alpha |
| 1.0.0 | Sprout |

Each later codename is chosen when its release is planned, and added here.
There is no fixed theme. A release is named with its codename, as
`turborel 1.0.0 (Sprout)`, and so is what the server tells a client and
what the command-line client prints as a typed session begins.

## Format and protocol versions

Each format is written with its version where it is kept, and checked when
read: a version a release does not know is refused, never guessed at.
During alpha a format may change without its version changing. From 1.0.0
a change that a reader of the version before cannot read takes a new
version, and comes with a major release (see
[Compatibility](#compatibility)).

| Format | Version | Where it is written | Defined in |
|---|---|---|---|
| Block file (the log's segments) | 1 | the file's header | `disk/block/format.rs`, `FORMAT_VERSION` |
| Page file (tables and indexes) | 1 | the file's header | `disk/pager/format.rs`, `FORMAT_VERSION` |
| Catalog image | 1 | the image's first byte | `db/image.rs`, `IMAGE_FORMAT` |
| Log record | 1 | each record's first byte | `log/record.rs`, `RECORD_VERSION` |
| Row store table | 1 | its meta page | `engine/rows/meta.rs`, `TABLE_FORMAT` |
| B-link tree | 1 | its meta page | `engine/tree.rs`, `TREE_FORMAT` |
| Hash index | 1 | its meta page | `engine/hash.rs`, `FORMAT` |
| Native protocol | 1 | the client's Hello | `protocol/src/lib.rs`, `VERSION` |

A server refuses a Hello of a protocol version it does not speak, saying
which it does (SQLSTATE 08004).

## Where the version is

- Each crate's `Cargo.toml` gives the release's number, the same in all.
- `turborel::VERSION` is the number, `turborel::CODENAME` the codename,
  and `turborel::product()` both, as `turborel 0.0.0 (alpha)`.

## Publishing

A release's crates are published to [crates.io](https://crates.io) together,
all at its number:

| Crate | Depends on |
|---|---|
| `turborel` | |
| `turborel-lz4`, `turborel-aes`, `turborel-protocol` | `turborel` |
| `turborel-client` | `turborel`, `turborel-protocol` |
| `turborel-server` | `turborel`, `turborel-protocol` |
| `turboreld` | `turborel`, `turborel-server`, `turborel-lz4`, `turborel-aes` |
| `turborel-cli` | `turborel`, `turborel-client` |

`turborel-test-support` and `xtask` are not published (`publish = false`).

Every crate carries the license's text: the library's is the repository's
`LICENSE`, and each other crate's folder links to it, a link `cargo
package` follows. The release's number is set in each crate's
`Cargo.toml` and in every dependency of one crate on another, which gives
its version beside its path, as crates.io requires.

A release is published from a clean checkout of a pushed commit that has
passed `cargo xtask verify --coverage`, first as a trial, then for good:

```
cargo publish --workspace --exclude xtask --exclude turborel-test-support --dry-run
cargo publish --workspace --exclude xtask --exclude turborel-test-support
```

Cargo publishes the crates in the order of their dependencies and waits
for each to be available before the next that needs it. crates.io limits
how many new crates an account publishes in a while; stopped by the
limit, the rest are published once it allows, each with `-p` and the
crate's name in the order above. A version published can be yanked, so no
new dependency takes it, but never deleted or published again: a fix is
a new release.
