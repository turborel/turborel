# Versioning

How Turborel's releases are numbered and named, and what each kind of
release may change. The files a database is kept in and the wire protocol
have version numbers of their own; this file lists them.

[Release numbers](#release-numbers) · [Alpha](#alpha) ·
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

Turborel is in alpha, version **0.0.0**, until its first release. While
its version is 0.y.z nothing is promised. SQL's behaviour, settings, the
Rust API, the protocol and the files a database is kept in may change in
any release. A renamed setting is renamed in place, with no alias. A database made by
one may not open with another, and is made again from its SQL.

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
`Turborel 1.0.0 (Sprout)`, and so is what the server tells a client and
what the command-line client prints as a typed session begins.

## Format and protocol versions

Each format is written with its version where it is kept, and checked when
read. A version a release doesn't know is refused, never guessed at.
During alpha every version is 1, and a format may change without its
version changing, so a database made by one build may not open with
another. From 1.0.0
a change that a reader of the version before cannot read takes a new
version, and comes with a major release (see
[Compatibility](#compatibility)).

| Format | Version | Where it is written | Defined in |
|---|---|---|---|
| Block file (the log's segments, the catalog file, the replication journal) | 1 | the file's header | `disk/block/format.rs`, `FORMAT_VERSION` |
| Page file (tables and indexes) | 1 | the file's header | `disk/pager/format.rs`, `FORMAT_VERSION` |
| Catalog and checkpoint image | 1 | the image's first byte | `db/image.rs`, `IMAGE_FORMAT` |
| Partitioning descriptors, keys, bounds and index templates | 1 | the catalog image's, as part of it | `catalog/codec/partitions.rs` |
| Log record | 1 | each record's first byte | `log/record.rs`, `RECORD_VERSION` |
| Row store table | 1 | its meta page | `engine/rows/meta.rs`, `TABLE_FORMAT` |
| Column store row group, its chunks among it (an ALP chunk's factors marked by its first byte's bit 0x80, RULES.md D353) | 1 | its header's first byte | `engine/columns/group.rs`, `GROUP_FORMAT`; `engine/columns/alp.rs`, `FACTORED` |
| B-link tree | 1 | its meta page | `engine/tree.rs`, `TREE_FORMAT` |
| Hash index | 1 | its meta page | `engine/hash.rs`, `FORMAT` |
| R-link tree, its meta page and nodes | 1 | each page | `engine/rtree.rs`, `META_VERSION`; `engine/rtree/node.rs`, `FORMAT` |
| `rtree` index locator, numeric or geometry | 1 | the locator's first byte | `index/rtree.rs`, `LOCATOR_VERSION` |
| `labels` and `reachability` index locators | 1 | the locator's first byte | `index/counted.rs` |
| `adjacency` index locator | 1 | the locator's first byte | `index/adjacency/options.rs` |
| `fulltext` index locator | 1 | the locator's first byte | `index/fulltext/options.rs` |
| `json` index locator | 1 | the locator's first byte | `index/json/options.rs` |
| Vector index locators (`diskann`, `hnsw`, `ivf`) | 1 | the locator's first byte | `index/vector/index.rs` |
| `sparse` and hybrid index locators | 1 | the locator's first byte | `index/sparse/options.rs`, `index/vector/hybrid/options.rs` |
| DiskANN meta page | 1 | its meta page | `engine/diskann/meta.rs` |
| HNSW meta page and chain | 1 | each page | `engine/hnsw.rs`, `engine/hnsw/chain.rs` |
| IVF meta page and lists | 1 | each page | `engine/ivf.rs`, `engine/ivf/lists.rs` |
| Sparse index blocks and summaries | 1 | each block | `engine/sparse/block.rs`, `engine/sparse/keys.rs` |
| VECTOR value and sparse vector image | 1 | the value's first byte | `vector.rs`, `VERSION`; `engine/sparse/vector.rs`, `IMAGE_VERSION` |
| Vector parameters and quantizer codebooks | 1 | their first byte | `vector/params.rs`, `vector/quantizer.rs` |
| JSON binary value | 1 | the value's first byte | `json/binary.rs` |
| XML stored value | 1 | the value's first byte | `xml/stored.rs` |
| FULLTEXT value and FT_PATTERN | 1 | the value's first byte | `fulltext/value.rs`, `fulltext/types/ft_pattern.rs` |
| GEOMETRY payload, type descriptor and TIN element | 1 | the value's first byte | `geometry/model/payload.rs`, `geometry/reference.rs`, `geometry/tin_element.rs` |
| UUID value, record and index key, 16 octets (RULES.md D180) | 1 | not written; a column's type names UUID | `value/types/uuid.rs` |
| Linear referencing values | 1 | the value's first byte | `spatial/lr/codec.rs` |
| Encrypted page sealing | 1 | the sealed header | `aes/src/sealer.rs` |
| Replication state in the journal | 1 | its first byte | `replication/src/vr/frame.rs`, `STATE_FORMAT` |
| Replication connection | 1 | its hello, `TBREPL\0\x01` | `server/src/replication/net.rs` |
| Native protocol capabilities, and a message's tail of tagged fields | 1, the protocol's | Hello asks by name, Ready grants as bits | `protocol/src/capability.rs`, `protocol/src/tail.rs` |
| Native compressed frame (`z`), a zlib stream | 1, the protocol's; RFC 1950 | the stream's header | `protocol/src/compress.rs`, `protocol/src/zlib.rs` |
| Native export file | 1, the protocol's | none, its frames being the protocol's | `client/src/export.rs` |
| Native protocol, a UUID's field (tag 27) among it | 1 | the client's Hello | `protocol/src/lib.rs`, `VERSION`; `protocol/src/fields.rs` |
| Plug-in protocol | 1 | HELLO and WELCOME's first field | `plugin.rs`, `PROTOCOL` |
| Plug-in payloads of kinds 10 to 19 (rankings to sinks of events) | 1, as the protocol | the protocol's version | `plugin/payload/` |
| Plug-in payloads of host kinds 20 to 31 (encodings to background jobs) | 1, as the protocol | the protocol's version | `plugin/payload/hosted.rs` |
| A plug-in type's stored value (key, binary and text forms) | 1, as the protocol | the protocol's version | `plugin/payload/types.rs` |
| Plug-in manifest (`turborel-plugin.manifest`) | 1 | its PROTOCOL item | `plugin/manifest.rs` |
| Plan form of an optimizer rule (kind 37) | 1 | its first field | `plugin/plan.rs`, `FORMAT` |
| Plug-in statistics (engine and index operations 8 and 9, STATISTICS and STATISTICSDONE) | 1, as the protocol | the protocol's version | `plugin/payload/statistics.rs` |
| Extension package file | 1 | the byte after its magic `TBX\0` | `plugin/package/archive.rs`, `plugin/package.rs` `FORMAT` |
| Installed package digests file (`turborel-package.digests`) | 1 | its first line | `plugin/package/install.rs` |
| Extension lock file (`turborel-extensions.lock`), each package's license among it | 1 | its first line | `plugin/package/lock.rs` |
| Backup manifest | 1 | the manifest's first byte | `backup/manifest.rs`, `MANIFEST_FORMAT` |
| Archive layout and timeline history | 1 | the history's first byte | `backup/timeline.rs`, `ARCHIVE_FORMAT` |
| Backup history entry | 1 | each entry's first byte | `backup/history.rs`, `HISTORY_FORMAT` |

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

Every crate carries the license's text. The library's is the repository's
`LICENSE`, and each other crate's folder links to it, which `cargo package`
follows. The release's number is set in each crate's
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
how many new crates an account publishes in a while. If the limit stops
you, publish the rest once it allows, each with `-p` and the crate's name,
in the order above. A published version can be yanked, so no new dependency
takes it, but it can't be deleted or published again. A fix is a new
release.
