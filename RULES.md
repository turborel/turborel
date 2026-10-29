# turborel rules

How turborel is designed, written and verified. Every change follows these
rules; `cargo xtask verify` checks each one a tool can check, and review
checks the rest.

- **Rules** (R-…) say how code and tests are written. **shall** is
  mandatory: a departure needs a recorded deviation (§6). **should** is
  expected: a reviewer may accept a reasoned exception.
- **Decisions** (D1…) record how the system is built and why. Change one
  only by changing this file in a reviewed commit.
- Cite either by its ID, in code, tests, reviews and commit messages:
  `RULES.md D29`, `R-ERR-2`.

**Before changing code**, read §3 (code rules) and §4 (test rules), and the
decisions that cover the area you are changing (§1). What turborel
implements of the standards is in [CONFORMANCE.md](CONFORMANCE.md); open
work is in [TODO.md](TODO.md).

The rules are written to serve as the *Software Code Standards* and the
core of the verification approach that DO-178C expects (§8).

Contents: [1 Decisions](#1-decisions) · [2 Layout](#2-project-layout) ·
[3 Code](#3-code-rules) · [4 Tests](#4-test-rules) ·
[5 Coverage](#5-structural-coverage-and-mcdc) · [6 Deviations](#6-deviations) ·
[7 Process](#7-process-and-tools) · [8 DO-178C](#8-do-178c) ·
[9 References](#9-references)

---

## 1. Decisions

| # | Decision |
|---|---|
| **The project** | |
| D1 | The project is turborel; SQL-86 is its first dialect |
| D2 | DO-178C Level A objectives are the target |
| D3 | No third-party dependencies |
| D6 | Production code only |
| D23 | A manual of what is settled; no design documents yet |
| **Tooling and process** | |
| D4 | Tests in Rust's standard levels |
| D5 | One integration test binary |
| D7 | Automation in Rust (`cargo xtask`) |
| D8 | The toolchain is pinned |
| D9 | Formatting |
| D10 | Lints |
| D11 | Lint baselines only shrink |
| D12 | Overflow checks in release builds |
| D13 | `doc_markdown` is allowed |
| D14 | Coverage floors only rise |
| D16 | Three consuming idioms are allowed in conditions |
| D17 | `unnested_or_patterns` is allowed |
| D20 | Lint baselines are counted |
| D21 | `verify` runs in a pre-commit hook and in CI |
| D22 | No instrumented MC/DC |
| D24 | Format errors name tokens, not clauses |
| **SQL semantics** | |
| D15 | Run-time errors do not depend on the plan |
| D18 | A limit the standard does not set is not a syntax error |
| D19 | A failed undo damages its table |
| D32 | SQL-86 and SQL-92 are one language |
| D33 | Schema changes are transactional |
| D47 | Schema statements on their own |
| D49 | Temporary tables: each session's instance |
| D50 | Domains: their constraints copied onto their columns |
| D51 | Assertions: a query whose rows break it |
| D52 | A view's REFERENCES: from what its columns are drawn from |
| D54 | Datetimes and intervals: UTC, the session's zone, a clock |
| **Architecture** | |
| D25 | Durability: one log, owned by the database |
| D26 | Cryptography only from vetted crates, in plug-ins |
| D48 | Compression and encryption as encodings, in plug-ins, per file or per table |
| D27 | A thread-safe core over snapshots of the definitions |
| D28 | Transactions may wait for locks; deadlocks are refused |
| D29 | Engines own concurrency, at logical timestamps |
| D30 | A storage foundation of independent components |
| D31 | The built-in engines: a row store and a column store |
| D34 | Index methods |
| D35 | The optimizer and planner |
| D36 | Settings |
| D37 | System tables and the Information Schema |
| D38 | The memory budget, and spilling |
| D39 | Directions for extensibility |
| D40 | Diagnostics: typed events to a pluggable sink |
| D41 | Table settings: options an engine declares for its tables |
| D42 | The server: three modes over one core |
| D43 | Users, passwords and the database's owner |
| D44 | A running statement is cancelled from another thread |
| D45 | Protocol version 1, and prepared statements |
| D46 | The command-line client |
| D53 | Data types: one contract, standard types built in |

### The project

**D1. The project is turborel; SQL-86 is its first dialect.** The crate is
`turborel`. Its first dialect is ANSI X3.135-1986; later ISO SQL standards
extend it (D32).
*Why:* the engine is the product, and a standard is one stage of it.

**D2. DO-178C Level A objectives are the target,** where tooling allows. No
certification is claimed.
*Why:* Level A is the only level that requires MC/DC, and writing to it
from the start is cheaper than retrofitting.

**D3. No third-party dependencies,** in the library, the client or the
tools. The exceptions are cryptography in plug-ins (D26), which the
client takes only with its feature `tls`, compression in plug-ins
(D48), tokio in the server crate (D42), tokio's signals in the
command-line client, and rpassword in it and in `turboreld` (D46).
*Why:* every line can be reviewed, and there is no supply chain to
qualify.

**D6. Production code only.** The library is the core, and the server
serves it (D42). There is no demo program, no `examples/` and no golden-file test.
Built-in plug-ins are ones a production database would ship; a test that
needs another uses a test-local plug-in. `bench/` measures, it is a tool.
*Why:* a toy costs upkeep and hides what is real.

**D23. A manual of what is settled; no design documents yet.** The
README is a short introduction; RULES.md, CONFORMANCE.md and TODO.md are
the working documents. The manual is a set in `doc/` (`doc/README.md`),
laid out as a system's manuals were: a User's Guide, an SQL Reference
Manual with a page for each statement (NAME, SYNOPSIS, DESCRIPTION,
RULES, PRIVILEGES, ERRORS, EXAMPLES, STANDARD, SEE ALSO), an Operator's
Guide, a Client Reference, and appendices. It describes what users and
operators see, and only what is settled; a section not yet written says
so. An internals guide waits for the design to settle. Doc comments on
public items are required meanwhile (R-DOC-1).
*Why:* documents written before a design settles go stale; doc comments
live beside the code they describe. What a user sees settles first, and a
manual laid out whole from the start keeps each part in its place as it
is written.

### Tooling and process

**D4. Tests in Rust's standard levels:** unit, integration and doc
(R-TEST-1), as Cargo and the Rust Book lay them out.

**D5. One integration test binary for each crate tested from outside,**
with a module for each area: `test/integration/main.rs` for the library,
`server/test/main.rs` for the server and its client together, and
`turboreld/test/main.rs` for the server's program.
*Why:* one link step, and fixtures shared without `mod.rs` tricks.

**D7. Automation in Rust,** as `cargo xtask`, never shell scripts.
*Why:* portable to every platform Rust supports, and reviewed, linted and
tested like the product.

**D8. The toolchain is pinned** to 1.99.1 in `rust-toolchain.toml`.
*Why:* the compiler is a configuration item (R-CM-1).

**D9. Formatting:** `rustfmt.toml` sets `max_width = 140` and
`use_small_heuristics = "Max"`, the width that changed the existing code
least.

**D10. Lints:** clippy `pedantic` plus the restriction lints in
`Cargo.toml`, with thresholds in `clippy.toml`. They enforce R-ERR, R-NUM,
R-CF and R-SIZE mechanically.

**D11. Lint baselines only shrink.** Code that predates a rule may carry a
file-level `#![expect(...)]` listing what it still breaks. None does today.

**D12. Overflow checks in release builds** (`overflow-checks = true`), so
release builds behave like tested ones (R-NUM-1).

**D13. `doc_markdown` is allowed,** so citations such as `GR3a` are written
as the standard prints them (R-DOC-4).

**D14. Coverage floors only rise** (R-COV-2).

**D16. Three consuming idioms are allowed in conditions,** a recorded
deviation from R-CF-4: `if self.accept_*(..)` in the parser,
`while let Some(x) = q.pop_front()`, and `if x.take().is_none()` (R-DEV-5).
*Why:* each is a single call whose only effect is consuming the input
decided on; rewriting about 50 sites would hide the parser's structure.

**D17. `unnested_or_patterns` is allowed.**
*Why:* it contradicts R-CF-5, which keeps a decision's conditions visible
for MC/DC test design.

**D20. Lint baselines are counted.** `cargo xtask lint-audit` builds with
the baselines off and compares each file's count of each lint with
`xtask/lint-baseline.txt`.
*Why:* a list of lint names alone would let one more violation of a listed
lint pass.

**D21. `verify` runs in a pre-commit hook and in CI.** The hook
(`cargo xtask install-hooks`) runs it before each commit; CI runs it on
Linux, macOS and Windows on every push and pull request, and coverage on
Linux.
*Why:* a hook can be skipped or not installed, and checks one platform.

**D22. No instrumented MC/DC.** MC/DC is shown by test design (R-TEST-8)
and checked in review.
*Why:* stable Rust cannot measure it, and a rule that waits on a tool that
does not exist cannot be met.

**D24. Format errors name tokens, not clauses.** A violation of the BNF
(SQLCODE -102) says "expected *x*, found *y*" at its position; every other
error cites its rule (R-ERR-2).
*Why:* the parser's generic helpers cannot know the clause of the
production calling them, and the position already identifies it.

### SQL semantics

**D15. Run-time errors do not depend on the plan.** Every conjunct of a
WHERE or HAVING condition that can raise an error is evaluated on every row
or group the clause applies to, whatever access path or rewrite is chosen.
*Why:* §5.18 GR2 defines AND only by its truth table, and §5.21 GR2 and
§5.23 GR2 apply the condition to each row and group; nothing permits
leaving an operand unevaluated. So results never depend on the plan.

**D18. A limit the standard does not set is not a syntax error.** Valid SQL
beyond one of this implementation's limits gets SQLCODE -900 ("valid, but
not processed"), not -103. The limits are listed in CONFORMANCE.md. So
does valid SQL not implemented, never (D32) or not yet: the parser reads
its Format in full, and refuses it once the statement is read whole
(`Parser::refuse_later`), so SQL that breaks a Format is a syntax error
(-102) still.
*Why:* -103 claims the statement breaks a Syntax Rule, which it does not,
and §3.4 requires conforming SQL to be processed; -102 claims it is not
SQL.

**D19. A failed undo damages its table.** The statement or ROLLBACK
reports SQLCODE -914, naming each table and its failure, and every later
statement on that table but DROP TABLE is refused with -914;
`Database::damaged_tables` lists them. A transaction a statement of which
could not be undone can only roll back (-913). While a table is damaged
no checkpoint is taken, nor a clean close, so the last checkpoint and the
log still hold what it should be. `Database::repair_table` repairs one
whose index entries alone could not be undone, building its indexes again
from its rows; one whose rows could not be is repaired by opening the
database again, which recovers it, or, kept nowhere, only dropped.
*Why:* a silently half-undone table breaks §7.3 GR3b i and §8.9 GR1;
refusing it keeps a state no transaction produced from being read, and
the log is what can put it right.

**D32. SQL-86 and SQL-92 are one language.** ISO/IEC 9075:1992 extends
X3.135-1986; where they differ, the newer wins, and a restriction SQL-92
keeps only for Entry or Intermediate SQL is lifted. Later standards will be
added the same way. The texts are kept locally, untracked, and cited as
`[X3.135-1986 §…]` and `[ISO/IEC 9075:1992 §…]`. Not claimed: the module
language and embedded SQL. Never implemented: character sets other than the
one there is, and collations.
*Why:* one consistent language serves programs better than a choice of
dialects; one character set keeps comparison, keys and storage simple.

**D33. Schema changes are transactional.** CREATE SCHEMA run by a
session [ISO/IEC 9075:1992 §20.1 GR6b ii, iii], CREATE TABLE, CREATE VIEW,
GRANT, REVOKE (D47), DROP SCHEMA, TABLE and VIEW, and ALTER TABLE run
inside transactions. A transaction's changes of definitions are staged on a
copy, made again on a newer copy if others publish meanwhile, and published
with its commit; rollback forgets them. Each takes its table's exclusive
lock until the transaction ends; a drop takes those of all it drops, or
changes the definitions of, with CASCADE too, and of a schema it drops,
which a table or view defined in the schema takes shared; CREATE SCHEMA
takes its schema's name exclusive, as `Database::schema`, the operator's,
made at once, does while it runs. Each is logged
under its transaction, and is made by recovery at its commit record. ALTER TABLE gives a table a new generation
of its storage, rows copied there, so a rollback leaves the old one intact.
A transaction that began before a change it cannot see is refused when it
next uses the table (SQLCODE -911).
*Why:* SQL-92 lets schema and data statements share a transaction [§4.28],
and a schema change that cannot be rolled back is a trap.

**D47. Schema statements on their own.**
- CREATE TABLE [ISO/IEC 9075:1992 §11.3], CREATE VIEW [§11.19], GRANT
  [§11.36] and REVOKE [§11.37] are statements of a session, outside a
  `<schema definition>` [§20.1], in its transaction (D33).
- A table, view, domain or assertion is defined by the owner of its
  schema, which exists, in any schema that owner has. The names it writes
  without a schema are the session's default schema's [§5.4 SR8b], and so
  are an ALTER TABLE's. Each text the catalog keeps, a view's query and
  each CHECK (a table's, a domain's, an assertion's), keeps the schema its
  names are of beside it (`ViewDesc::names`, `ConstraintKind::Check`), and
  is read again in that schema, whatever the default schema then; the log
  record of an ALTER TABLE keeps it too. A constraint's own name is in its
  table's schema.
- Each takes the lock of the name it defines, or grants or revokes on,
  exclusive: a table's own lock once there is a table, so two transactions
  defining one name take turns (D28). It takes shared locks on the tables
  it reads, and exclusive ones on those whose definitions it changes, and
  is made on the definitions as they are once it holds them.
- It is logged as its text, with the authorization identifier and default
  schema, and a created table's engine and storage generation; recovery
  makes it again at the commit record. A table defined under the name of
  one dropped takes a later generation, so the log's rows of the one
  dropped never reach it. A transaction that began before a table was
  created is refused when it uses it (SQLCODE -911), as for ALTER TABLE.
- GRANT grants what its grantor holds with grant option, of what it names,
  and completes with a warning, "privilege not granted" (SQLSTATE 01007),
  for the rest [§11.36 GR14, GR15]; one who holds nothing on the object is
  refused [AR1]. So does a `<schema definition>`'s, where SQL-86 refused
  [X3.135-1986 §6.6 SR5]. A privilege granted twice is kept once,
  grantable if either grant was [§11.36 GR12, GR13].
- REVOKE destroys the privileges its authorization identifier granted, of
  those it names, or with GRANT OPTION FOR their grant option. A privilege
  granted stands while its grantor holds it with grant option: as owner,
  or by a privilege that stands. What stands is a fixed point found from
  the owners' privileges up, so privileges that hold each other up in a
  cycle fall together [§11.37 SR11]. A view whose owner would lose SELECT
  on a table it names, and a constraint whose owner would lose the
  REFERENCES it needs, are abandoned [SR13, SR14]. RESTRICT refuses to
  abandon anything [SR18], and with GRANT OPTION FOR a privilege granted
  by the option (SQLSTATE 2B000) [GR2b]; CASCADE destroys it all. What it
  names and did not grant is warned of (SQLSTATE 01006) [GR8, GR9]. A
  view owner's privileges on the view are found from its tables', not
  kept, so they follow them.
- A warning is a completion: SQLCODE 0 [§22.2, Table 24] and its
  SQLSTATE, carried by the protocol's Complete (D45).
- A privilege granted to PUBLIC WITH GRANT OPTION lets anyone grant it,
  as SQL-92's applicable privileges hold PUBLIC's [§10.3 GR2, §11.37
  SR7a ii]; SQL-86 took only a grant naming the grantor itself
  [X3.135-1986 §6.6 SR3b ii].
*Why:* an operator defines and secures tables over a connection, one
statement at a time; with transactions, a definition and the rows that
fill it commit together, or not at all.

### Architecture

**D25. Durability: one log, owned by the database.**
- One logical log per database holds row changes (table, row id, before-
  and after-image) and changes of definitions, in a versioned,
  engine-neutral format. A transaction commits when its commit record is
  durable; commit order is log order.
- The `Log` that stores the records is a plug-in, with a memory log and a
  segmented file log with group commit built in. A database kept in a
  directory makes or opens its log through a `LogStorage`, a plug-in too,
  as it does its catalog (`db/stores.rs`); built in, the file log in the
  directory's `log/`. Opened, the file log is checked from the position
  the last image holds it durable through: the segments before it were
  synced whole. A segment cut short by a crash ends the log, and the
  segments after it are removed: a sync seals the segments before its
  own, so none of their records was durable.
- Each engine declares its persistence: `Logged` (replayed from the log),
  `Forced` (makes its own writes durable at commit), `External` (keeps its
  own durability) or `None` (scratch data, never recovered).
- A database kept in a directory has a catalog, recording the definitions
  as they are, a snapshot of the catalog and the database's settings, not
  every one made, and where each table and index is; it is opened from the
  snapshot, and the definitions logged since replayed. Where the catalog's
  images are kept is a plug-in (`CatalogStorage`, `CatalogStore`), each
  image opaque to it: built in, two files of the directory, by turns. Checkpoints write every
  engine's state as of a log position, so the log can be truncated behind
  it; the row store's file is shadowed, so after a crash it is exactly as
  the last checkpoint left it. Recovery reopens the tables, then redoes and
  undoes from the log.
- A database is open in one place at a time: opening it takes an exclusive
  lock on its directory's `lock` file (`FileSystem::lock`, the system's
  file lock), before anything there is cleared or read, and closing it
  gives the lock up; a process that crashes gives it up with it.
- A failed sync of the log stops the database taking changes, as it may
  have lost what the log held, and a later sync may succeed without it:
  from then on, no row change, commit of a transaction that wrote,
  definition or checkpoint is accepted (SQLCODE -915), and it cannot be
  closed cleanly; reads and rollbacks go on. Opened again, it is recovered
  from what its log holds (`Database::log_failure`).
*Why:* one commit point makes transactions atomic across engines without
two-phase commit. Logical records keep engines independent, and a
pluggable log leaves room for replicated or remote ones.

**D26. Cryptography only from vetted crates, in plug-ins.** Password
hashing, TLS, encryption at rest and cryptographic hash functions may
depend on vetted crates, inside their plug-ins only;
the core stays dependency-free (D3). Each crate needs its own decision
under R-SAFE-4.
- TLS: rustls 0.23.46 (Apache-2.0, ISC or MIT), with tokio-rustls 0.26.6
  (MIT or Apache-2.0) in the server, its cryptography from aws-lc-rs
  (ISC and Apache-2.0 or ISC) only: maintained by AWS on AWS-LC, rustls's
  default provider, with a FIPS 140-3 build for whoever needs one. It
  builds with a C compiler alone, prebuilt assembly on Windows. `ring` is
  not taken: its own README calls it an experiment, not for third parties
  to depend on. In the client, only with its feature `tls`.
- Tests make their certificates with rcgen 0.14.10 (MIT or Apache-2.0),
  on aws-lc-rs, a development dependency of the server: none is kept in
  the repository to expire.
- Encryption at rest: AES-256-GCM from the same aws-lc-rs, in the crate
  `turborel-aes`, an encoding (D48), its subkeys by HKDF-SHA256.
- Passwords: PBKDF2-HMAC-SHA256 from the same aws-lc-rs (D43), at
  SERVER.PASSWORD_ITERATIONS, 600,000 by default: OWASP's choice where
  FIPS 140 is needed, and no dependency beyond TLS's. It is not
  memory-hard as Argon2id is, OWASP's first choice; but RustCrypto's
  `argon2`, its one implementation in Rust, says nothing of an audit or
  of being ready for production, and has no FIPS path. A verifier names
  its scheme, so Argon2id may come beside it.
- Cryptographic hash functions, as `HashFunction`s (D30) for checksums a
  forger cannot match: none yet. A digest holds 256 bits at most
  (`hash::MAX_DIGEST_BYTES`).
*Why:* cryptography written here would be unreviewed and likely wrong, and
a server needs it.

**D48. Compression and encryption as encodings, in plug-ins, per file or
per table.**
- An encoding (D30) is registered with the database. STORAGE.ENCODINGS
  names those every file passes through (D36); a table's option
  ENCODINGS, the database's own option of every table of any engine
  (D41), those its rows do. Each name may take an argument after a
  colon, `AES-256-GCM:PAYROLL`: the encoding is bound to it, and records
  in its output what its decoding needs, so a file's header names only
  ids. An encoding's id is never changed or reused: 1 is LZ4, 2 is
  AES-256-GCM; tests take 1000 and above.
- An encoding declares the most it adds to any input
  (`Encoding::overhead`): the pager keeps that room in every page, so an
  encoded page never spills to an overflow page, and the row store in
  every record. One that hides what it encodes says so
  (`Encoding::confidential`).
- LZ4 is the crate `turborel-lz4`, on lz4_flex 0.14.0 (MIT), its safe
  encoder and decoder only (features `safe-encode` and `safe-decode`,
  which forbid `unsafe` in both), and nothing else. Its own documents
  make no claim of readiness for production; it is taken on its use
  (about 160 million downloads; arrow, parquet, tantivy and the
  ClickHouse and Scylla clients depend on it) and its fuzzing. A block
  decoded must give the length it records, at most 255 times its own, so
  damage is refused before memory is taken for it. Input it would not
  shrink is kept as it is, behind one byte.
- AES-256-GCM is the crate `turborel-aes`, on the aws-lc-rs of D26. Its
  keys come from a `Keyring`, an extension point its host gives it, never
  from the database's files: the built-in `KeyDirectory` reads a
  directory an operator keeps (`turboreld --keys DIR`), a key a file of 64
  hexadecimal digits; a key service is a keyring of its own. A page is
  encrypted with a subkey, HKDF-SHA256 of the named key and a random
  salt drawn anew every 2^30 encryptions, and a nonce aws-lc-rs draws, as
  its FIPS rules require: random nonces bear 2^32 encryptions a key, and
  no subkey nears that. The salt, the nonce and the key's name are kept
  in what is encrypted; the place it is stored and the key's name are
  authenticated, so data changed, moved or of another key is refused
  (SQLCODE -916). A key is rotated by naming another: rows written before
  are read with their own.
- Per table, the engine seals what it stores: the data passed through a
  pipeline, headed by the pipeline's ids (`Pipeline::seal`,
  `Encodings::unseal`), every engine given the registry through
  `Host::encodings`; an engine that does not (`StorageEngine::seals`)
  takes no table naming encodings (SQLCODE -103). Sealed data names its
  encodings, so a change of the option is made in place: rows written
  after pass through the new encodings, those before are read through
  their own. The row store seals its records and values out of line,
  the row id the context; a table naming none stores its records as they
  are, at no cost. The table's row changes are sealed in the log too
  (`log/sealed.rs`). An index keeps its keys in order, in the clear, so a
  table whose encodings are confidential is refused an index (SQLCODE
  -103) unless the database's files are encrypted too, or kept nowhere.
- Spill areas' files (D38) are encrypted while the database encrypts
  anything, its files or a table: through the first confidential encoding
  in use, with no key named, its process's ephemeral key, drawn as it is
  first used and kept only in memory; what spills is never read by a
  later process, so no keyring is needed. The data file's shadows hold its
  pages as encoded, and the undo store its sealed records.
- Names are checked as the option is set (SQLCODE -103 if one is not
  registered; -910 if one refuses its argument, as a key the keyring
  lacks). A database opened without an encoding a table names still
  opens; that table's rows are refused (SQLCODE -910) until it is
  registered.
*Why:* LZ4 is fast enough for every read path, and a compressor or a
cipher written here would be unreviewed; aws-lc-rs is already vetted for
TLS, with a FIPS 140-3 build. What compresses well, or must be hidden,
differs by table. Tables share one data file whose pages keep their
slots whatever their encoded length, so the engine seals what it stores
rather than the pager its pages; and keys live outside the database, or
encrypting it would hide nothing.

**D49. Temporary tables: each session's instance.** A global or created
local temporary table [ISO/IEC 9075:1992 §4.9, §11.3] is defined as a
persistent one is: in the catalog, logged and kept by checkpoints, its
storage empty. Its rows are each session's.
- The definitions a session's statements see give each temporary table
  the session's own instance: its rows, its indexes and its lock
  (`db/temporary.rs`), so binding, constraints and indexes need nothing
  of their own for it.
- An instance is made in memory, of the database's budget, when the
  session first writes it, and is gone with the session. Its changes are
  not logged: they are never recovered [§4.9].
- ON COMMIT DELETE ROWS empties an instance as its transaction ends,
  committed or rolled back: as GR3 of §14.3 leaves it, or as it was.
- A session is one module, so a created local temporary table is, within
  a session, as a global one; and its declared local temporary tables
  [§13.11] are in a schema MODULE of its own definitions alone, owned by
  its authorization identifier (`db/declared.rs`), in no other session's
  and no view of the Information Schema.
- ALTER TABLE of one is made only while no session's instance holds a
  row, the altering session's included, else refused (SQLSTATE TB911);
  and while its transaction is open, no other writes an instance. The
  database's instances are noted for it (`temporary::Uses`). So no
  session keeps rows its new definition does not describe, and no added
  constraint goes unchecked of rows only another session sees.
- Not implemented: CREATE INDEX on one.
*Why:* the standard puts each session's rows in a schema of its own
[§4.9]; giving the session's view of the definitions its own storage does
so with no change to how a table is found, bound or checked.

**D50. Domains: their constraints copied onto their columns.** A domain
[ISO/IEC 9075:1992 §11.21] is described in the catalog with its data
type, default, constraints and USAGE grants (`DomainDesc`).
- A column of a domain takes its data type, and keeps the domain's name
  and a copy of its default, the column's when it has none of its own
  [§11.5 GR2b], changed as the domain's is.
- A domain constraint holds of every column of the domain [GR5, GR6]: each
  such column has a copy of it, a CHECK of its table, VALUE the column,
  naming the constraint it copies. ALTER DOMAIN adds copies, the rows
  there are checked, or drops them; DROP DOMAIN CASCADE leaves them as the
  table's own constraints [§11.27 GR1c]. The Information Schema shows the
  domain's constraint, not the copies.
- CAST to a domain takes its data type, and needs USAGE on it; §6.10 has
  it check none of its constraints, so none is checked. Whether a CHECK or
  a query CASTs to a domain is read from its text (`domains::casts_to`),
  in the schema its names are of: so no domain is in usage by its own
  constraints [§11.21 SR4], and what CASTs to one in a schema whose owner
  loses USAGE on it is abandoned [§11.37 SR13-SR16].
*Why:* a CHECK of a table is already bound, checked, deferred and logged;
copying a domain constraint onto each column makes it one, with no new
machinery to check it by.

**D51. Assertions: a query whose rows break it.** An assertion
[ISO/IEC 9075:1992 §11.34] is a constraint of its schema, on no table,
described in the catalog as a CHECK is (`ConstraintDesc`), with the tables
it reads and the columns of each it sees.
- It is bound as `SELECT 1 FROM (VALUES (1)) WHERE NOT (condition)`, as
  its schema's owner and as a constraint, so a CHECK's rules hold of it.
  A row of it is the assertion broken [GR2].
- It is checked as it is made, and at the end of each statement that
  changes a table it reads, its reads locked shared, or at commit if it is
  deferred. Its name is a constraint name of its schema, for SET
  CONSTRAINTS and for every other constraint's [SR2, SR7].
- One in a `<schema definition>`, made outside any session's
  transaction, is checked in one of its own, reading the newest committed
  rows; it is not checked again as the log is replayed.
*Why:* a CHECK with a subquery is already checked over all of what it
reads, deferred and logged; an assertion is one with no table of its own,
and needs no new machinery but its query.

**D52. A view's REFERENCES: from what its columns are drawn from.** The
privileges of a view's owner on it are found from its tables'
`applicable`); REFERENCES on its column CV is held if the owner holds
REFERENCES on every column of the leaf underlying tables CV is drawn
from, and on some column of each [ISO/IEC 9075:1992 §11.19 GR6b].
- What each column is drawn from is found as the view is defined, from the
  columns its select list item references, a subquery's too, and kept in
  its descriptor (`Underlying`), each by table and name.
- It is known of a `<query specification>` whose FROM clause names
  tables and views, joined without common columns, reading no view of
  which it is not known; of a `<table value constructor>`, whose columns
  are drawn from none [§7.10 SR16a]; and of a set operation by position
  of queries it is known of, each column drawn from both operands' i-th
  [SR16d]. Of any other, the owner holds no REFERENCES on the view.
*Why:* finding it again from the query at every privilege check would bind
the view each time; and where a column may be drawn from more than what
its item names (a set operator's other side, a derived table, a common
column), granting none is safe where granting by the item alone is not.

**D53. Data types: one contract, standard types built in.** What a data
type does is one trait, `value::SqlType`: how it is written, what it
compares with, store assignment and CAST to it, its value's bytes in the
log, the catalog and on the wire, in a record and in an index key (with
the anchor of a probe), its literal, its Information Schema descriptor,
and its position for a histogram. A `DataType` names its kind
(`DataType::kind`), and every type-level operation goes through it.
- SQL's standard types are built in: each a kind of its own module in
  `value/types/`, registered by default. The parser and binder know them,
  since their syntax (`DATE '...'`, `INTERVAL ... DAY TO HOUR`) and their
  rules are the language's.
- A built-in kind's values are variants of `Value`, so comparison and
  hashing (`value::compare`, `value::bucket`), the hot path, match them
  directly, with no virtual call; each kind's own `compare` and `bucket`
  are those same functions.
- Types beyond the standard are plug-ins through the same trait,
  registered on the database (`Database::register_type`) before it is
  opened from its files, under a regular identifier: its kind lives as
  long as the program, as code does, so `DataType` stays `Copy`, and
  engines and index methods, which take `&[DataType]`, encode by it with
  no registry at hand. Its values are `Value::Extension`, the kind with
  its bytes, so comparison and hashing need no registry either.
- A plug-in type is written `name` or `name(a [, b])`; a name not
  qualified is a domain's of the default schema first. It is stored by
  its name: the catalog, the log, spilled rows and undo read it back
  through the database's registry (`bytes::Reader::with_types`), and a
  database whose files name a type not registered is not opened.
- On the wire a plug-in value is its type's name, its text and its kind's
  bytes; a client without the plug-in holds it as `Unregistered`, shows
  its text and sends it back as it came, and the server reads it as its
  type again.
*Why:* a standard type is grammar as much as storage, and none is
replaced; one contract still keeps every type, built in or not, honest to
the same responsibilities, and a plug-in type stored, logged, indexed and
sent as a built-in one is.

**D54. Datetimes and intervals: UTC, the session's zone, a clock.**
- A time or timestamp is kept as its instant in UTC, to the microsecond,
  as ISO/IEC 9075:1992 §4.5.1 says, with its displacement if its type is
  WITH TIME ZONE. Without time zone, its displacement is the session's,
  implied [§6.1 SR5]: a value retrieved is shown in it, so it shows
  differently in a session of another zone. A date is its days.
- A session's displacement is the setting TIME_ZONE, the database's by
  default, `+00:00`, and its own if it sets one, by SET TIME ZONE [§16.5]
  or as any setting; SET TIME ZONE LOCAL takes the database's again.
- An interval is its months, of a year-month interval, or its
  microseconds, of a day-time one, in 128 bits, so a leading field of the
  greatest precision, 9 digits, and the sum of two never overflow what
  holds them; its qualifier is its type's. Arithmetic is on these units,
  a datetime's year and month moved by months as shown [§6.14 GR3].
- A statement takes the clock's instant and its session's displacement
  once, as it begins: every CURRENT_DATE, CURRENT_TIME and
  CURRENT_TIMESTAMP in it is that instant [§6.8 GR3], a constant of its
  plan, and so is the current date a CAST of a time to a timestamp reads.
  CAST to or from a datetime is bound with them; `SqlType::cast`, given
  none, takes UTC.
- A default's datetime literal without a displacement is of the
  defining session's [§5.3 SR18], or, of a schema the operator defines,
  of the database's TIME_ZONE. The displacement is logged with the
  definition (`Change::Schema`, `Defined`, `AlterTable`), so recovery
  makes the same default, no session being there.
- The clock is a plug-in (`Database::set_clock`), the system's by
  default; a test sets one that stands still (R-TEST-5).
*Why:* a server's results must not depend on the host's zone, nor a
statement's datetimes on when within it each is evaluated; a time
without time zone is kept as SQL-92, the standard implemented, says,
until a later standard's reading is taken.

**D27. A thread-safe core over snapshots of the definitions.**
- A `Database` is shared between threads, and every extension trait is
  `Send + Sync`; a plug-in guards its own state.
- The definitions (catalog, storage and index handles, plug-ins, settings)
  form an immutable snapshot. A statement keeps the one it started with, an
  updatable cursor the one of its OPEN, and no lock on the definitions is
  held while it runs. A change copies the snapshot, changes the copy, and
  publishes it; changes take turns.
- `create_index` on a versioned table waits for no transaction: the index
  is kept up to date while it is built, and a transaction that wrote the
  table before is refused at its next statement (SQLCODE -911).
- A writer whose snapshot predates a change to its table is refused
  (SQLCODE -911). A plug-in calling back into its own session is refused
  (SQLCODE -910).
*Why:* schema changes never wait for queries nor they for schema changes,
and no lock cycle can form through the definitions.

**D28. Transactions may wait for locks; deadlocks are refused.** The
setting LOCK_WAIT says whether a statement meeting a lock waits or is
refused at once (SQLCODE -911); LOCK_TIMEOUT, a session's setting in
milliseconds, bounds a wait, past which the statement is refused so too. A wait that would close a cycle is refused
at once, as is a wait on the waiting thread's own call. The refused
statement is undone; its transaction goes on.
*Why:* sessions on their own threads, as a server runs them, should wait;
a program driving several sessions from one thread must not.

**D29. Engines own concurrency, at logical timestamps.**
- The database hands out transaction ids and commit timestamps from
  counters that only increase; they are logical, never wall-clock time.
- An engine that isolates transactions itself keeps versions of its rows:
  a transaction reads as of its snapshot, the second writer of a row is
  refused (SQLCODE -911), and commit checks that nothing it read was
  changed meanwhile (serializable). Versions no snapshot can read are
  removed as transactions end: of the tables each touched, and of every
  table whose versions still awaited collection, as the horizon moves
  only then.
- Commit is atomic across engines: each validates at the commit timestamp,
  then each commits at it. A transaction that wrote nothing commits at its
  snapshot, unchecked.
- Index entries are counted per version, and a reader keeps a row found
  through an index only if the version it sees has that key.
- The coordinator's table locks (D28) remain for engines that leave
  concurrency to it. The built-in row store isolates transactions itself.
- Below SERIALIZABLE [ISO/IEC 9075:1992 §4.28, §14.1 GR5], a transaction
  reads at one snapshot (REPEATABLE READ), or at a new one as each
  statement begins while its session has no cursor open (READ COMMITTED,
  and READ UNCOMMITTED, which reads nothing uncommitted), and its reads are
  not checked at commit: write skew commits. Its writes are still refused
  over a row written after its snapshot, so no update is lost; and its
  reads are checked while constraints are checked and referential actions
  run, which are serializable at every level. Engines are told how it
  reads (`TableStorage::read_mode`); one that ignores it, or leaves
  concurrency to the coordinator, reads as SERIALIZABLE, which shows none
  of the phenomena any level would not.
*Why:* only the engine knows its versions and rows. Readers never block
writers, nor writers readers, and writers meet only on the same row.

**D30. A storage foundation of independent components.** Below every
engine, log and index, components that know nothing of what is above them:
portable file I/O (never mmap); a block manager of checksummed,
append-only blocks; a pager of checksummed pages whose content may be of
any length, owning allocation; a buffer pool with pluggable eviction; an
encoding pipeline (compression, encryption) for blocks and pages, and for
what an engine stores of a table (D48); and hash
functions as a plug-in (CRCs for checksums, MurmurHash3 for hash tables).
Hash functions and encodings are registered with the database; a file
names in its header those it was written with, and is read with them, and
a file added to others is written as they were. A buffer pool read that
hits writes one thing shared, its frame's latch: a table of hints, read
without a lock, finds the frame, and the latch keeps it from eviction,
which takes it exclusively; hits are counted on stripes by thread. Hot
pages contend on that latch still; a reader-biased latch (BRAVO) would
need a reviewed exception to R-SAFE-1. File formats are the same
on every platform. Each component counts its own
work for the heuristics above it.
*Why:* each layer can be tested, fault-injected and benchmarked alone, and
reused; mmap leaves the order of writes to the operating system, which
breaks write-ahead logging.

**D31. The built-in engines: a row store and a column store.**
Whole engines are the extension point. Two are built in, one code over
one data file, sharing one value codec, record format, statistics
module, buffer pool, MVCC and row ids: `rowstore` keeps each row's values
together; `columnstore` keeps rows written so, then freezes them into
groups of each column's values together (below), its tables' files the
row store's, which checkpoints them for both. A table chooses by USING,
else DEFAULT_ENGINE, and moves by ALTER TABLE ... USING. A hybrid (PAX),
each page holding a group's rows' columns apart, is the next store, and
the format of frozen groups becomes a trait when it is.
- A table is a B-link tree (Lehman and Yao, 1981) keyed by a logical 64-bit
  row id, given in blocks per thread. Long values are kept out of line.
  A leaf split keeps as its high key the shortest that parts its halves
  (suffix truncation, Bayer and Unterauer, 1977), so the levels above
  hold short keys. A leaf a delete empties is taken out (after Lanin and
  Shasha, 1986): its range given to its right neighbour, under its parent
  and its left neighbour's locks, a link left to that neighbour for
  whoever reaches it; its page freed once no operation that began before
  is left, operations counted by epoch on stripes by thread. A tree built whole, while nothing else reads or writes
  it, is loaded from its entries in key order: full leaves left to right,
  to nine tenths of a page, then the levels above. A value replaced by
  one of its length, as a version stamped committed is, is written over
  in place, nothing moved, so a leaf is not compacted again and again as
  a commit stamps its versions: a commit of 2048 rows went from 8.6 ms to
  1.8 ms.
- The newest version of a row is in the tree; older ones are in an undo
  store, a log: on disk for a database kept in a directory (`undo/`),
  never synced and begun anew when it opens, as no reader outlives the
  process; in memory for a database kept in memory. A long reader then
  holds disk, not memory, for what it may go back to. On disk, versions are
  gathered in batches written as one record each (`OpenOptions::undo_batch`),
  read from memory until written.
- Scans are given the columns needed, and the conjuncts of a filter that
  compare a column with a constant (a BETWEEN as two), test it IS NULL or
  IS NOT NULL, IN or NOT IN a list of constants (none null), OR those of
  one column, or LIKE a constant pattern, as the bounds of the characters
  it begins with, the last one less and one more: strings beginning with
  them are between whatever follows, padded with spaces or not
  (`TableStorage::scan_where`, `storage::Conjunction`,
  `logical::pushed_conjuncts`), unless the filter can raise an error,
  which a LIKE of a constant pattern and escape that partition cannot:
  each
  true of every row the filter keeps, so an engine may leave out rows
  they do not match, and count as read only those they all match; the
  filter still judges every row given. The row store skips the zones,
  the column store the frozen chunks, any of them cannot match, and each
  leaves the rows of the rest to the filter, as judging them twice cost
  more than it saved (`G NOT IN (1, 2)` of the row store's 400,000 rows:
  45.9 ms, 53.9 judged in the engine too; `G = 3` of the column
  store's: 6.4 ms, 11.1). Of the column store's 400,000 rows, `G = 3 AND
  B < 4000` ran in 1.1 ms where with one conjunct told it took 11.6; `B
  BETWEEN 100000 AND 103999` in 1.2 ms where it took 13.4; `B IN (5,
  200000, 399000)` in 1.1 ms where it took 9.0; `N IS NULL`, its nulls in
  one chunk, in 0.9 ms where it took 4.2; `S LIKE 'k01000%'`, S in the
  rows' order, in 1.4 ms where it took 53.7 (the row store's: 0.3, where
  94.7); `B < 1000 OR B > 399000` in 1.3 ms where it took 13.3 (the row
  store's: 0.4, where 50.8).
- Zone maps of the row store (`rows/zones.rs`): for each block of 1024
  row ids, of each column of a number, a datetime, an interval or a
  character string of at most 32, its least and greatest value and
  whether one is null; a scan reads only the blocks its conditions may
  match. By row id, as rows never move between ids, so leaves splitting
  touch none. A write widens its block's zone before it is made; the
  maintenance thread sums up again one an update or deletion left stale,
  from each version of its rows a reader at or after the horizon may
  read, an open transaction's too, and keeps it stale while such a
  version is not its row's newest; a write meanwhile throws its sum away.
  So a zone is never narrower than what a snapshot may read. Kept in
  memory, each zone locked alone; a table opened knows none until each is
  summed up. Of 400,000 rows: `B BETWEEN 100000 AND 103999`, B in the
  rows' order, 54.4 ms to 0.9 ms; `B IN (5, 200000, 399000)` 48.4 to 0.4;
  `N IS NULL`, its nulls in two blocks, 45.2 to 0.3; a scan nothing
  skips 2 to 3% slower; an insert 1% slower.
- The planner prices a scan by the share of the rows the conditions it
  will be told may match (`TableStorage::scan_share`, `logical::pushed_conjuncts`,
  the executor's same): the row store's zones' rows they may match, of
  256 zones at most, evenly apart, so planning costs no more as a table
  grows; the column store's frozen chunks and every row of its tree. Of
  400,000 rows, a tenth counted by a range of a column in the rows' order
  ran by a scan in 6.8 ms where the index took 19.8; a range of a
  column of scattered values keeps its index.
- The planner asks for statistics through a trait, every answer optional:
  rows, widths, distinct values, nulls, selectivity. The row store
  gathers them again, once a table has changed enough, as the database's
  maintenance (below), not in the commit that made it due: a commit of
  2048 rows into a growing table took 15.6 ms at the 95th percentile
  with the pass, 1.9 ms without.
- The column store: the tree is the table's delta, every row written
  there as in the row store. Once COLUMNSTORE.FREEZE_ROWS rows past the last group are committed
  before every snapshot (and COLUMNSTORE.FREEZE_DELAY seconds ago), the
  database's maintenance thread freezes up to COLUMNSTORE.GROUP_ROWS of them,
  2^16 by default, into a group (`engine/columns`): its row ids, and each column's values
  in chunks of a page each, each chunk's least and greatest values and
  nulls kept (zone maps), every page sealed by the table's ENCODINGS, its
  number the context. A chunk is written in whichever encoding takes
  fewest bytes, and holds as many values as its page does so: plain, a
  constant, a frame of reference of 8-byte fields (any such field read as
  a signed integer, so every type of that width), or a dictionary
  (`columns/encoded.rs`); COLUMNSTORE.COMPRESSION = NONE keeps them plain.
  Measured: rows of numbers of few values or near one another, and of
  texts of few values, freeze into 12 pages where they take 46 plain, and
  scans run as fast, a value read at a time either way. A group's rows are
  in row id order, or sorted by the columns COLUMNSTORE.ORDER names (at
  most 8, ascending, nulls as ORDER BY puts them, row ids breaking ties)
  as they are frozen or written again, its ids then kept in its rows'
  order and its rows' positions by id, so a row is found by its id: a
  hundredth of 200,000 rows counted by a scattered column ran 4.7 times
  as fast sorted by it (`bench/analytics.rs`), a column the rows were in
  order of a sixth slower, a fetch by key a twentieth. An option naming
  columns (`Kind::Columns`) names the table's, and a column it names is
  not dropped. Groups are immutable, their row ids' ranges apart,
  found through a directory the meta page names. A row's newest state is
  its entry in the tree if it has one, else its group's values, which
  every reader sees: so a frozen row changed is written as any row is,
  over a version holding its frozen values, and a deletion of one stays in
  the tree, until its group is written again: once the tree's entries in
  a group's range of row ids are COLUMNSTORE.REWRITE_FRACTION of its rows
  (looked at only after so many frozen rows are given entries), the group
  is written again with those of them committed before every snapshot,
  replaced in the directory, and the entries taken out as a freeze takes
  out its rows'. A scan merges the tree and the groups in row id order, a
  group read a vector (2048 rows) at a time, only the chunks each of
  the scan's conditions may match; a sorted group's rows read all as it is taken
  up, then put in row id order. The entries of rows frozen leave the tree once
  no reader holds the directory from before their group; a checkpoint is
  not captured during a freeze. A deletion collected while a freeze under
  way may freeze its row keeps its entry, so the row is not seen again; a
  write that finds the entry it read taken out is tried once more, its
  row then its group's.
- Maintenance (`db/maintenance.rs`): work an engine leaves for later
  (`TableStorage::maintenance_due`, `maintain`), statistics gathered
  again and the column store's freezes, rewrites and entries taken out, so no transaction waits for
  it: measured over 100 commits of 2048 rows each, in groups of 2^16,
  the slowest twentieth of commits went from 47 ms to 30 ms. A
  transaction's end notes a table with work due and asks the database's
  maintenance thread, started the first time; it has each table noted do
  some, one at a time, under the gate, so a checkpoint waits for at most
  a few groups, and goes round again while any did some; a table still
  due though it did none it looks at again a second later, so a freeze
  waiting out COLUMNSTORE.FREEZE_DELAY needs no later commit.
  `Database::maintain` does every table's now. The thread, like the
  checkpoint thread (`db/background.rs`), holds the database weakly, and
  ends as it is closed or dropped. The log is unchanged, of whole rows, and
  recovery writes them to the tree. The planner is told a scan's cost by
  the columns it reads: a row of the tree 0.8 and 0.2 more as all its
  columns are read, a frozen row 0.25 and 0.75 more; and a fetch by id
  4.4 of a table the buffer pool holds, more as it holds less of it, to
  14.6 of one it holds none of (its bytes of the table's, by its rows and
  their width), 10% more as all the rows are frozen, as measured
  (`bench/analytics.rs`; a scan of 400,000 rows of sixteen columns, 0.27
  us a row read for no column and 0.06 more for fourteen; in a database's
  files, a fetch through an index 1.28 us a row, a scan 0.29, with a pool
  of 256 MiB; 5.3 and 0.36 with one of 1 MiB. Priced at 3 a fetch, the
  planner chose the index up to a third of the rows, a fifth taking 308
  ms where a scan took 123; priced at 10, a scan of a tenth of a table
  the pool held, 98 ms where the index took 46).
- Batches (`TableStorage::scan_batches`): a scan's rows by vectors of up
  to 2048 of each column's values, in no particular order, by default its
  rows gathered. A column store table gives each frozen vector straight
  from its chunks, a condition's columns read first and the others only
  of the rows it keeps; the tree's entries in its group's range of row
  ids, read once as the group is taken up, in their rows' place; then the
  tree's other rows. A subtree of a plan gives its rows by batch (a
  stream, `exec/batch/stream.rs`) if it is a table's scan; a filter of
  one, told the simple conjuncts of its condition, or of another stream,
  each batch with the rows kept of it; or an inner hash join whose left
  operand is a stream: its right read first, row by row, and built as the
  row path builds it; held in the work area, each of the left's batches
  probes it, a batch of joined rows made of each, the left's columns then
  the right's; partitioned, the joined rows made row by row and gathered.
  Of 400,000 rows joined with 10, a COUNT(*) of the column store took
  16 ms where row by row it took 79 (the row store's, 50 where 110), and
  SUM grouped by a column of the 10 rows 36 where 123 (71 where 151). A
  LEFT outer join on keys, no other condition, of streams is one too,
  each left row no right row matches joined with nulls; its right
  operand, outgrowing the work area, moved to a table of the spill area,
  so every left row probes it in order: of the same rows, COUNT(*) 20.4
  ms where 77.8 (54.0 where 110), grouped 33.2 where 110 (67.5 where
  137). An
  aggregation whose set functions count rows or take a column, none
  DISTINCT, grouped by columns the stream holds or not at all, of a
  stream, feeds them from the vectors (`exec/batch.rs`), each a column's
  values, or an expression's evaluated as a row's is, of a row of only
  the values it reads, no subquery in it (`SUM(B * 2)` of 400,000 rows of
  the column store: 31.0 ms to 11.9; grouped, `SUM(B * 2 + 1)` 72.6 to
  24.8),
  of no GROUP BY a vector at a time (`Accumulator::update_many`,
  `update_rows`, each by default `update` a value at a time): COUNT
  counting, SUM adding exact values as decimals, nothing cloned, MAX and
  MIN cloning only the best (of the column store, COUNT(*) 142M to 206M
  rows a second, SUM 47.8M to 62.3M, `bench/analytics.rs`). A
  filter that cannot raise an error has its conjuncts comparing a column
  with a constant, testing it IS NULL or IS NOT NULL, IN or NOT IN a
  list of constants, or LIKE a constant pattern, split once and matched
  by bytes if the string is ASCII, judged over the vectors, the LIKE last
  (`exec/batch/judge.rs`): of 400,000 rows, `S LIKE '%5%'` ran in 11.4 ms
  of the column store where it took 47.8, 52.6 of the row store where it
  took 85.2. The rest of it is judged of
  the rows they keep, each made a row; one that can raise is judged whole
  a row at a time, as row by row. A projection of a stream whose
  expressions read only its columns, no subquery among them, every table
  of which reads by batch (`reads_by_batch`, a column store table's), makes
  each row it gives of the vectors, a column or a constant as it is, any
  other expression evaluated as a row's is of a row of only the values it
  reads, kept and written over (`SELECT K + 1, S` of 400,000 rows: 80.8
  ms to 69.1; `SELECT B * 2, UPPER(S) ... WHERE G = 3`: 44.4 to 25.6), a
  batch read ahead of what a cursor is given; of any other table it reads
  a row at a time, as reading ahead would gain nothing
  (`exec/batch/project.rs`). Groups are held in the work area, given in the order
  their first rows came; past it, the rows of every group not yet held
  are written, numbered, as a pass of `exec/group.rs` writes them, grouped
  as it groups them, and merged with the groups held by number. Measured against the row store row by row (`bench/analytics.rs`): COUNT(*)
  of a column store table some 50 times as fast, a SUM of one column 17, a
  filter of 1% 10, a GROUP BY of 10 groups 7.9, each row's grouping key
  made in a buffer kept (`spill::put_norm`). A projection of columns of a
  stream is a stream, its batches those columns' vectors, moved; so are
  the operands of a UNION ALL, each holding every column of its rows
  alike, read in turn, a chain of them gathered without a frame each. A
  DISTINCT of a stream holding every column of its rows is that grouping
  by every column, no set function (`batch::distinct_grouping`): its groups
  are DISTINCT's rows, in the order their first came, past the work area
  too (of 400,000 rows of the column store, `SELECT DISTINCT G` 41.5 ms to
  9.0, of `G, S` 60.4 to 19.3; of the row store, 88.0 to 46.0); a UNION,
  a DISTINCT of its operands appended, too (`G UNION G` of the column
  store 39.9 to 18.7, of the row store 131 to 91.4; a COUNT(*) of a UNION
  ALL 77.4 to 7.2). The setting
  EXECUTOR.BATCHES, a session's too, ON by default, reads by batches
  where a query can; OFF, a row at a time throughout, as to compare the
  two, as the tests of batches do, each answer read both ways.
*Why:* a row store suits SQL's point reads, updates and cursors, and is
the base the other layouts build on. Logical row ids stay stable, so
indexes never need rewriting for them, frozen or not. Columns suit scans
of a few columns of many rows; keeping new rows as rows keeps writes and
key lookups as fast as in the row store, the design of SQL Server's deltastore,
HyPer's Data Blocks and TiFlash's delta tree. Measured on 200,000 rows of
16 columns in memory, against the row store (`bench/analytics.rs`): COUNT(*) 3.1
times as fast, a SUM of one column 3.0, a filter of 1% by a column in the
rows' order 4.8; one row fetched by key 7% slower, as each column is a
page of its own.

**D34. Index methods.** An index method is a plug-in that creates indexes
and says which probes they answer. Two are built in, both in pages through
the buffer pool, so an index larger than memory lives in its file:
- `btree`, a B-link tree keyed by the encoded key then the row id: ordered,
  for equality and ranges. A lower and an upper bound of one column, as
  BETWEEN gives, are one probe (`Probe::Range`), so the keys past the
  upper are not read: 100 keys of 200,000 by BETWEEN went from 28 to some
  7,400 queries a second (`bench/cursors.rs`, 2026-10-28). A method says
  it answers ranges by `IndexMethod::supports_range`.
- `hash`, a linear hash table (Litwin, 1980) keyed by the encoded key:
  equality only, growing and shrinking a bucket at a time, readers locking
  nothing.
A probe value not of the column's type is placed by its anchor and each
entry found checked, so an index finds exactly what SQL compares.
An index built from its table while nothing else writes it (rebuilt as a
database opens, or for an engine the coordinator locks) is loaded whole
(`Index::load`): `btree` sorts its entries in a work area of WORK_MEMORY,
spilling past it, and loads its tree from them; a method that does not
inserts them one by one.
Index-only scans are not done: a versioned engine has an entry for each
version, and a reader must read the version it sees.
In SQL, as an extension: `CREATE INDEX name ON table (columns) [USING
method] [WITH (...)]`, `DROP INDEX name` and `ALTER INDEX name SET (...)`,
by the owner of the table's schema. The index is built while transactions
write the table, so these run outside a transaction, each made at once (as
PostgreSQL's CREATE INDEX CONCURRENTLY). An index takes no UNIQUE: a UNIQUE
constraint makes its own. INFORMATION_SCHEMA.INDEXES lists them. Inserting
into a unique index, a method says whether the entry is its key's only one
(`Index::insert_alone`), by default by `rows` after `insert`; the B-link
tree's, under the leaf's lock, of the entries beside the new one, not when
the leaf before may hold one, nor when the leaf after may, its high key
beginning with the key. A UNIQUE constraint at a statement's end does not
read a row whose entry was so: a row of its key inserted after was not,
and is read itself. An INSERT of 2048 rows with a PRIMARY KEY went from
6.9 ms to 5.7.
*Why:* each kind of index excels at different probes, and the planner
picks by estimate, so a new method needs no change to the planner.

**D35. The optimizer and planner.** Rules rewrite the logical plan; the
physical planner chooses access paths and join order by estimated cost
from the engines' statistics. Built in: hash joins on equalities, join
ordering of chains of products, transitive predicates, sort elimination
for ordered indexes and GROUP BY, filters pushed into views and outer
joins, and decorrelation of repeated subqueries. A rewrite is made only
where it cannot change a result or an error (D15). Rules are plug-ins.
An index path is costed from the index method's own answers where it
gives them: the rows a probe selects (`Index::estimate`), the pages it
reads to find them (`Index::probe_pages`, a hash table's one bucket, a
B-link tree's levels), and its distinct keys (`Index::distinct_keys`, for
an equality on all its columns), counted by a sketch kept as the index is
written, of no more keys than it holds, and kept with the index's locator,
so a checkpoint records it with the pages it commits and recovery's mending
changes it as it changes the entries; an index whose locator holds none
counts its keys once, when first asked, in a pass that streams them. Else
from the table's statistics.
*Why:* the General Rules define results, not plans; any plan that gives
the same results and errors is the optimizer's to choose.

**D36. Settings.**
- One registry: each setting declared once, with its kind, default, whether
  a session may set it, and when a change takes effect: for each statement,
  each transaction, or when what reads it starts, as a server's addresses.
  Plug-ins declare theirs the same way, the server too (D42): every
  component is configured by settings, never by a configuration of its own.
- Scopes: a built-in default, the database's value, a session's own value.
- A number a subsystem is tuned by (a threshold, a weight, a guess, a work
  area's size) is a setting, the constant it was named by its default.
  Only these stay constants: formats on disk, the standard's
  implementation-defined values (CONFORMANCE.md), and the shapes of
  structures fixed when they are made (stripes and shards).
- The database's values are definitions: logged and replayed like schema
  changes. A session's values never persist.
- Some are fixed when a database is created in its directory: its page
  size, checksum (STORAGE.CHECKSUM, a registered hash function), encodings
  (STORAGE.ENCODINGS) and log segment size. They are given before it is
  created, recorded in its first image, and refused after; given again as
  it opens, they must be the ones it has. So a database opens with no
  options. Those read as it opens (STORAGE.POOL_BYTES), given before it
  opens, hold from then on. `OpenOptions` keeps only what its files do not
  fix: the log's durability and the log of older row versions.
- Only the database's owner (D43) may change the database's values; a
  session may set its own. The Rust API is unrestricted: the host program
  holds the database.
- In SQL: SET TRANSACTION and SET SCHEMA from the standard;
  `SET name = value` for a session's own values; and `SET DATABASE name =
  value` for the database's, by its owner, outside a transaction, kept as
  `Database::set_setting` keeps them, so a running server is configured
  from a client. ISO/IEC 9075:1992 has neither: its SET statements of a
  session [§16.1–§16.5] set no setting (`parser/session.rs` gives their
  BNF). INFORMATION_SCHEMA.SETTINGS lists every setting with its value and
  where the value comes from.
*Why:* a server's behaviour must be visible and changeable without
rebuilding it, by the people entitled to change it.

**D37. System tables and the Information Schema.** A system table's rows
are made from the definitions as a statement runs, as its user sees them.
System tables are a plug-in. The Information Schema's views are the
standard's own definitions [ISO/IEC 9075:1992 §21.2], over Definition Schema
tables that only those views may read [§21.3]; SELECT on them is PUBLIC's,
and nothing else is granted, so they cannot be changed.
*Why:* the standard's own views, over tables simulating its Definition
Schema, give the standard's answers with no second description of the
catalog to keep in step.

**D38. The memory budget, and spilling.**
- A database has one budget, in bytes (MEMORY_BUDGET). Every buffer pool
  reserves its pages of it when made and gives them back when dropped; a
  reservation past the budget is refused (SQLCODE -918).
- Plug-ins are given the budget when they are registered
  (`StorageEngine::attach`, `IndexMethod::attach`), so their caches draw on
  the same limit. Each owner keeps its own pool and eviction policy. A
  database in memory gives its engine and index methods one pool to
  share, as one in a directory shares its data file's, of
  STORAGE.POOL_BYTES as its first table is made.
- Each reservation says what holds it; INFORMATION_SCHEMA.MEMORY_RESERVATIONS
  shows the database's owner each one held and its bytes, and anyone else
  none, as a spill area tells what others' statements do.
- An operator that holds rows (a sort, grouping, DISTINCT, a hash join's
  right operand, EXCEPT and INTERSECT) holds up to WORK_MEMORY of them in
  memory, a setting a session may set for itself: its statements read the
  settings it set over the database's (D36). Past that it spills to an area of as many bytes, reserved of the
  budget: a temporary pager file whose buffer pool is the area, so
  eviction writes it to disk; its creation not made durable
  (`Pager::create_scratch`), as nothing reads it after a crash, which
  saved 7 to 9 ms of each sort that spilled. In the area it makes tables (byte keys to
  values, in the order added) and runs (records read back in order).
  Areas come from a provider, an extension point (`Spill`), given to
  plug-ins through the `Host` too, so one may give partitioned or parallel
  areas.
- Spilling keeps the orders the planner relies on: a sort is an external
  merge, stable; the rows held, or each run, sorted by a prefix of the
  first key, a number in the key's order, beside each row's place, those
  of equal prefixes compared whole, or by the other keys only if every
  prefix holds its value whole (`exec/sort/prefix.rs`): of 400,000 rows
  in memory, by a column of random integers, 95 ms to 40.6, as the rows
  themselves, each in memory of its own, are no longer read for every
  comparison; grouping and DISTINCT give their rows in the order their
  first rows came. A hash join is partitioned (Grace), its partitions
  partitioned again while they outgrow the work area, its left rows
  numbered so the joined rows are merged back into the left operand's
  order. Grouping, DISTINCT, EXCEPT and INTERSECT partition so too, by
  their keys, once what they hold outgrows the work area: DISTINCT gives
  the rows it held as they came, then the rest's first of each key, merged
  by number; EXCEPT and INTERSECT partition both operands alike, the
  second's rows as counts; grouping numbers each group by its first row.
  Each partitions a partition again while it outgrows the work area, as
  many times as SPILL.PARTITION_DEPTH says; past that, DISTINCT, EXCEPT
  and INTERSECT keep its keys in a table of the spill area.
- Runs are written in batches of SPILL_BATCH bytes; an operator reads or
  writes as many runs at once as WORK_MEMORY holds two batches of. A run
  read once gives its pages back as it is read (`SpillRun::into_records`),
  so a merge holds no more than it has still to read; one read again, a
  product's right operand or a subquery's rows, keeps them. A plug-in's
  accumulator is opaque and is never written out: past
  SPILL.PARTITION_DEPTH, grouping makes a partition in passes, the rows of
  groups a pass has no room for left to the next.
- An outer join on keys is partitioned as the hash join is, both operands
  numbered: each left row with its matches or nulls, merged back into the
  left's order, then the right's rows that matched none, in the right's.
  A product, and an outer join on no keys, read their right operand again
  for each left row, from a run of the spill area once it outgrows the
  work area.
- A subquery's rows spill as a product's right operand does, and a
  decorrelated subquery's, kept by key, as a hash join's; the rows kept
  for each set of outer values are kept while they fit the work area. The
  values a DISTINCT set function took, of every group of a grouping pass,
  are keys of one bag, which spills. IN, NOT IN, ALL and SOME of a row,
  nulls in it or in the subquery's rows or not, are answered from what is
  made once of those rows: for each pattern of nulls they have, a hash of
  them on the columns neither they nor the row has null; each column's
  least and greatest value; and, by the place of a row's first null, the
  least and greatest of the values before it. Never by comparing the row
  with every row.
- A database kept in a directory keeps its areas' files in `tmp/` there,
  cleared when it opens; one in memory, in the system's temporary
  directory.
- The coordinator's undo records stay apart from the write-ahead log,
  which is only written while the database runs.
- What a transaction holds is bounded as an operator's is. The
  coordinator's undo entries, for indexes and for engines that do not undo
  their own changes, are held up to WORK_MEMORY, then written to runs of a
  spill area, and read back a run at a time to undo a statement or the
  transaction. The row store tracks up to TRANSACTIONS.TRACKED_ROWS rows of
  a transaction's reads, or writes, of a table for the commit's check, and
  past that counts every row, which may refuse a commit that need not be.
  Its changes, and the garbage its commit makes, are held up to
  TRANSACTIONS.HELD_BYTES, then written to a spill area of the database's
  work areas; a change keeps the version it replaced only if it was the
  transaction's own, a committed one being in the undo store already. A
  committed transaction's changed rows, which others' reads are checked
  against, are kept up to as many bytes, then as each column's least and
  greatest value. Each of these lists spills alike (`spill/list.rs`).
  Collection hands each version it removes to the coordinator as it goes,
  for its index entries to go (`TableStorage::gc`), and an index build
  reads the garbage it needs entries for a segment at a time. Recovery
  keeps the row changes it reads in such a list too, undoing them newest
  first and redoing them oldest first, and mends the indexes in two passes
  over it; a checkpoint logs the index entries of versions awaiting
  collection as it reads them, after its gate opens, of the commits it
  captured. While an index is built, or a checkpoint logs those entries,
  collection of the table is held back; what awaits it is read a segment
  at a time, its lock taken for each, so commits adding to it wait for one
  segment at most.
- A spill area's files pass through the encodings its temporary files are
  given (`TempFiles::pipeline`): encrypted while the database encrypts
  anything (D48).
*Why:* a server must bound its memory, and one budget over every cache
bounds it without binding plug-ins to one page cache.

**D39. Directions for extensibility.** Beyond today's plug-ins, the server's
wire protocol is a plug-in (`server::Protocol`, D42), so more than one
protocol can serve the same database; and data types share one contract,
`SqlType` (D53): SQL's standard types are built in behind it, and types
beyond the standard are plug-ins through it.
*Why:* the same principle as the engines and index methods: the core
fixes the protocols between components, and implementations are
replaceable.

**D40. Diagnostics: typed events to a pluggable sink.** The database tells
whoever runs it of its life and of failures an operator must know of, as
typed events (`diagnostics::Event`) given to a `Diagnostics` plug-in set
with `Database::set_diagnostics`; by default they are dropped.
- Events: the database opened (created, reopened, or recovered with
  recovery's report) and closed; a checkpoint taken; a setting changed; a
  plug-in registered; users and the owner changed (D43); a failure of the
  database's making (a damaged table, corrupt data, an I/O error, a failed
  commit, memory refused), with its error; and a statement whose work took
  DIAGNOSTICS.SLOW_STATEMENT or longer, with who ran it and its text, as
  written, values in it included. A statement's work is its call; a
  cursor's, its opening and its fetches, told once it is done with, not
  the time between, which is its client's.
- Not events: a failure of a statement's own SQL (syntax, privileges,
  constraints, conflicts), which is returned to its client; and what is
  counted (I/O, pool hits), which counters keep.
- An event is given with no lock held, and carries no row data, but for a
  slow statement's text, and no wall-clock time: the sink stamps it.
  `Event` is non-exhaustive, so a sink takes events added later. A sink never calls back into the
  database and returns soon.
- Production code never prints (R-STYLE-7): diagnostics are how it tells.
*Why:* a server's operators need to know what the database did and what
went wrong, in a form a program can route and filter; a library's host
needs the same without being made to read text.

**D41. Table settings: options an engine declares for its tables.**
- Each storage engine declares the options its tables take
  (`StorageEngine::table_options`): a name, a kind and a default, as a
  setting has, and whether a change is made in place or by a rebuild. A
  plug-in engine declares its own the same way.
- Any other plug-in declares options of every table, of any engine
  (`Database::declare_table_option`), named under its own name ("NOTED.FILL"):
  an index method's indexes are made with their table's descriptor
  (`IndexMethod::create_on`, `open_on`), and others read the options from
  the catalog as they run. An option is declared once, by an engine or by a
  plug-in. A table keeps a plug-in's options when it moves to another
  engine.
- A table opened again with an option no plug-in declares yet keeps it as
  written, listed as NOT DECLARED, and takes it of its kind once declared:
  a plug-in registered late, or no more, never stops a database opening.
- A table's descriptor keeps the values it sets (`TableDesc::options`);
  the rest are their defaults. `create_table` and `open_table` read them
  there.
- In SQL: `CREATE TABLE t (...) USING engine WITH (name = value, ...)`,
  `ALTER TABLE t SET (name = value | DEFAULT, ...)`, and `ALTER TABLE t
  USING engine [WITH (...)]`, which moves the table, its options reset to
  those given. They are definitions: logged as written and replayed.
- A change made in place is given to the table's storage and its indexes
  once it is published (`TableStorage::reconfigure`, `Index::reconfigure`),
  never before its transaction commits. A rebuild, or a change of engine, gives the table a new
  generation of its storage, its rows copied there, as ALTER TABLE does for
  a change of columns (D33).
- An index method declares the options of its indexes the same way
  (`IndexMethod::index_options`), set by `CREATE INDEX ... WITH (...)` and
  kept in the index's descriptor (`IndexDesc::options`). ALTER INDEX tells
  the index of a change made in place (`Index::reconfigure`); for one that
  needs a rebuild it builds the index again, while transactions write its
  table, as CREATE INDEX does, under a working name; once built it takes
  the old one's place and name in one change, the old read on by earlier
  snapshots. Replayed, the index is made anew of its table's rows. Any other
  plug-in declares options of every index, of any method
  (`Database::declare_index_option`), as it does of every table; an index
  opened again with one not declared yet keeps it, as for tables.
- Option names are the plug-ins' own, so a part of one may be an SQL key
  word ("AUDIT.LEVEL").
- INFORMATION_SCHEMA.TABLE_OPTIONS lists every option of every table the
  user holds a privilege on, with its value and where it comes from;
  INDEX_OPTIONS, every option of their indexes.
*Why:* what suits one table (its sampling, its layout, its engine) does not
suit every table, and an engine knows best which of its choices a table may
make.

**D42. The server: three modes over one core.**
- The database runs embedded (the `turborel` crate in a program), served
  (the program `turboreld`), or both at once: a program embeds the
  database and serves it with the server's library, its own sessions
  beside its clients'.
- One crate for each part, each depending only on those below it:
  `turborel`, the core, std only and synchronous; `turborel-protocol`, the
  native wire protocol's messages and codec, without I/O, on the core's
  types; `turborel-client`, a blocking connection over the protocol, std
  only but for TLS with its feature `tls`, for programs that connect rather
  than embed; `turborel-server`, the server as a library, for any program
  to serve a database with; and `turboreld`, the server's program: its
  command line, its signals, the database opened and closed. The server
  does not depend on the client, nor the library on the program.
  `turborel-cli`, the command-line client `turborel` (D46), is a crate of
  its own on the client. The plug-ins `turborel-lz4` and `turborel-aes`
  (D48) are crates of their own on the core.
  `turborel-test-support` holds fixtures the crates' tests share
  (R-TEST-10): tests alone depend on it.
- The server depends on tokio 1.54.1 (MIT), for network I/O only, and on
  TLS's crates (D26), with the features rt-multi-thread, net, io-util, sync,
  macros and time; the program adds signal, LZ4 and AES-256-GCM (D48), and rpassword for
  a password typed at a terminal (D46). Chosen as the most used and most reviewed async
  runtime of Rust; its `unsafe` is its own, and none is written here
  (R-SAFE-1). The core never sees it.
- Threads. Tokio's tasks do I/O only. Statements, the steps of an
  authentication and the end of a session run on blocking workers, at most
  SERVER.WORKERS at once. A query's rows go from its worker to its
  connection through a bounded channel, in batches of about
  SERVER.BATCH_BYTES, so a client reading slowly holds a worker but never
  more than a few batches of memory. A connection runs one request at a
  time, so no more wait for a worker than there are connections.
- A client's time is bounded: it has SERVER.LOGIN_TIMEOUT (a minute by
  default) to begin TLS and authenticate, and once the server has waited
  on it SERVER.IDLE_TIMEOUT (no limit by default) for a request, or to take
  what it is sent, it is closed, its session rolled back (SQLSTATE 08006).
  A client that stops reading frees its worker so, as one that stops
  sending frees its connection.
- Extension points (R-SAFE-3): `auth::Authenticator`, a mechanism of
  authentication as an exchange of as many steps as it needs, built in
  `password` (D43) and `trust`, which takes only clients of the server's
  own machine, and only while SERVER.TRUST is LOCAL;
  `transport::Listener`, built in TCP and Unix sockets; and
  `server::Protocol`, built in the native protocol.
- TLS (D26) secures an endpoint's TCP connections, as SERVER.TLS says:
  OFF, ON (offered) or REQUIRED, with the certificate chain and key in the
  PEM files SERVER.TLS_CERTIFICATE and SERVER.TLS_KEY. A client may speak
  TLS or not on the same port: the first byte tells (a TLS handshake
  begins with 22, RFC 8446 §5.1). Required, a client in the clear is
  refused (SQLSTATE 08004). The handshake runs in the connection's own
  task. A Unix socket needs none. An authenticator knows whether its peer
  is secure (`Peer::secure`), so one taking a secret refuses it in the
  clear.
- The native protocol, version 1 (`turborel_protocol`, D45): framed
  messages, a kind and a little-endian length; Hello with the version, the
  user and the mechanism; every error with its SQLCODE, SQLSTATE, message,
  rule and position; values exactly as stored. Port 7422 by default.
- A connection has one session, of the authorization identifier its
  authenticator accepts. Each statement is a `<direct SQL statement>`
  [ISO/IEC 9075:1992 §20.1] (`Session::direct`); a client creates only its
  own schema [§11.1 AR1]. A session ends as SESSION_END says when its
  client terminates it; otherwise, lost, failed or cut by the server
  stopping, it is rolled back [§20.1 GR2a].
- Configuration is the database's settings (D36), declared by the server
  on the database it serves: SERVER.HOST, SERVER.PORT, SERVER.SOCKET and
  the TLS settings when it starts; SERVER.WORKERS, SERVER.MAX_CONNECTIONS,
  SERVER.MAX_MESSAGE_BYTES, SERVER.BATCH_BYTES and the timeouts as
  statements and connections begin, so a change needs no restart.
  `turboreld --set NAME=VALUE` sets one as it starts.
- Diagnostics (D40): `log::Lines`, a sink writing each event as a line of
  JSON stamped with the time in UTC, to standard error or appended to a
  file; `turboreld --log FILE` names the file, the sink set before the
  database opens, so its opening is told too.
- The server's own errors are SQLCODE -919, a connection exception: SQLSTATE
  08004 for a connection refused, 08006 for one failed, 28000 for
  credentials refused [ISO/IEC 9075:1992 §22.1, Table 23].
- Portable: TCP everywhere; Unix sockets on Unix, a setting of one refused
  elsewhere; Ctrl-C everywhere, SIGTERM on Unix.
- Told to stop, the server takes no connection, lets each finish its
  statement, rolls its session back and closes it; then the database is
  closed.
*Why:* one core serves every mode, so the embedded and the served database
are the same database; I/O that waits must not hold a thread, and a
statement that waits must not hold an I/O thread; and a server's behaviour
is configured, seen and changed like the database's.

**D43. Users, passwords and the database's owner.**
- Which identifiers are authorization identifiers is implementation-defined
  [ISO/IEC 9075:1992 §5.4; Annex B, item 41]. Any is, for a session the
  host program starts. A user is one a server's clients may prove they
  are: kept in the catalog with a verifier, logged and replayed as a
  definition, in the checkpoint's snapshot, made outside any transaction.
  A name a client gives is read as an `<identifier>`, so `ann` is ANN, as
  in SQL.
- A verifier is opaque to the core. A `Credentials` plug-in makes one of a
  password (`Database::set_credentials`); the authenticator of the same
  scheme checks a password against it. The server's is PBKDF2-HMAC-SHA256
  (D26), its verifier `$pbkdf2-sha256$i=N$salt$hash`, naming its scheme
  and iterations, so verifiers made before a change of either still check.
- The authenticator `password` takes a password only over a secure
  connection (`Peer::secure`: TLS, or a Unix socket), as the password
  travels whole. A user not there is refused as a wrong password is, after
  as long, the check made against a decoy verifier.
- The authenticator `trust` takes a client as the user it names, of the
  server's own machine alone, and only if SERVER.TRUST is LOCAL. It is
  OFF by default, so every client proves who it is unless the operator
  says otherwise: on a machine of many accounts, any of them could
  otherwise connect as the owner.
- The database's owner is a user: the first one created, so a database
  never has users and no owner; or another the host program or `turboreld
  user owner` makes it; it cannot be dropped until another owns the database. In
  SQL, the owner alone manages users: `CREATE USER`, `ALTER USER` and
  `DROP USER` (`parser/users.rs` gives their BNF, in the standard's
  notation). INFORMATION_SCHEMA.USERS shows the owner every user, any
  other only itself, never a verifier.
- An operator sets a database up with `turboreld --dir PATH user ...`
  while no server holds it: its directory is locked by the one that does.
  A password is typed without its echo at a terminal (rpassword, as the
  command-line client reads one, D46), or else read from standard input;
  never from the command line.
- SET SESSION AUTHORIZATION [ISO/IEC 9075:1992 §16.4]: a session begun
  by the database's owner sets any authorization identifier, a user or
  not; any other session
  sets only the one it began with (SQLSTATE 28000) [GR4, GR5]. The default
  schema stays as it was [§4.30]. A Failure's 28000 ends the connection
  only of a connection exception's SQLCODE, as a server refusing
  credentials sends it; a statement's has another.
- Users and the owner are told to the diagnostics (`Event::UserChanged`,
  `OwnerChanged`), never a verifier, for an audit of who may connect.
*Why:* a server reached from other machines needs clients to prove who
they are; the core keeps what is proven against without knowing how it
was made, so mechanisms are plug-ins, as everything else is.

**D44. A running statement is cancelled from another thread.**
- A session's statements are cancelled through its `Stop`
  (`Session::canceller`), which any thread may hold. Each call into the
  session, and each fetch of its cursors, is a window: the first opened
  while none is takes a new generation, and a cancel marks the generation
  running then, so one that arrives as a statement ends never stops the
  next, and one made while nothing runs does nothing.
- A statement looks as it reads each row of a table or an index, before
  and after the read, and while it waits for a lock, which wakes every 20
  ms to look. A cancelled statement fails (SQLCODE -920, SQLSTATE TB920)
  and is undone as any failed statement is [X3.135-1986 §7.3 GR3b i]; its
  transaction goes on. A cursor's rows cut short by a cancel stay so:
  every later fetch fails too, none seeming the last.
- STATEMENT_TIMEOUT, a session's setting in milliseconds (no limit by
  default), cancels a statement as a cancel does once it has run that
  long: each call and each fetch alone, its deadline set as its window
  opens. A row read looks at the clock every 64th time, so a row costs a
  count; a wait for a lock looks at every wake.
*Why:* a server must stop a statement its client gives up on without
ending the client's session, and must never stop the wrong one; and an
operator must be able to bound what any statement takes.

**D45. Protocol version 1, and prepared statements.**
- Prepared statements are SQL-92's dynamic SQL [ISO/IEC 9075:1992 §17]:
  `Session::prepare` checks a `<preparable statement>` once [§17.6],
  deduces each `<dynamic parameter specification>`'s type from where it
  stands [§17.6 GR5] and describes its result [§17.8, §17.9 GR3, GR4];
  `execute_prepared` [§17.10] and `open_prepared` [§17.13, §17.14] run it
  with values, each cast to its parameter's type [§17.9 GR5]. A `?` where
  GR5 gives none is refused (SQLSTATE 37000) [GR4], and so is every
  Format, Syntax or Access Rule broken while preparing [GR4a]. A scalar
  function plug-in says what type a `?` takes as each of its arguments
  (`ScalarFunction::parameter_type`), refused by default.
- Each execution binds and plans the statement again, its values as
  constants, so the planner chooses indexes by them; the definitions it
  was prepared against may have changed [§17.6 GR10].
- The protocol: Prepare, Prepared with the parameters' types and the
  columns; Execute with values and a row limit, a query's cursor opened
  and its rows sent up to the limit, then Suspended if it has more, which
  Fetch sends, or Complete once it has none, the cursor then closed; Close
  [§17.16] and Deallocate [§17.7]. Statements are numbered by the client,
  SERVER.MAX_STATEMENTS at most. Errors: 24000 for a cursor open or not as
  it must be [§13.2-§13.4 GR1, §17.7 GR2], 26000 for a statement not
  prepared [§17.10 GR1], 07001 for values not one for each parameter
  [§17.9 GR5a].
- A Complete gives the statement's SQLCODE and its SQLSTATE, so a
  completion with a warning reaches the client (D47). A Complete and an
  Error end with a byte saying whether the session's transaction is
  active after the statement: an SQL-agent refuses to leave a connection
  whose transaction is active, or to end it (D46).
- Requests may be sent before the last is answered: each is answered, in
  order.
- Ready gives the server's product, version and codename ("turborel
  0.0.0 (alpha)", `turborel::product`, VERSIONING.md), to an
  authenticated client alone, and a
  random 128-bit key. A connection whose first and only
  message is Cancel with it cancels what that session runs (D44), over
  TCP, TLS or a Unix socket alike; the server closes it, saying nothing.
- Hello and Ready exchange capabilities; version 1 defines none, so later
  ones are added without a new version.
*Why:* drivers expect SQL-92's dynamic SQL; a client must read a large
result in parts, and must be able to stop what it started. A cancel on its
own connection needs no second writer on the first, which a TLS stream
cannot have.

**D46. The command-line client.**
- `turborel`, of the crate `turborel-cli`, on `turborel-client`: a
  library, its program's `main.rs` its one caller, as `turboreld` is.
- Statements come from `-c`, from files (`-f`), or from standard input,
  prompted for when it is typed at a terminal. Text splits into
  statements at each `;` outside a `<character string literal>`, a
  `<delimited identifier>` and a `<comment>` [ISO/IEC 9075:1992 §5.2,
  §5.3]; the last of a text needs none. Each runs as a `<direct SQL
  statement>` [§20.1], one at a time.
- A query's rows are shown as a table as they come, never held: a
  column is as wide as its type's values, at most 60 characters, or its
  name; numbers align right. A null is shown as `--null` says, nothing by
  default. Another statement says OK, OK with a warning and its SQLSTATE,
  or no data.
- A typed session begins with a banner on standard output: the client's
  version, its copyright and licence, the server's product and version as
  Ready gives them, where it is reached and as whom, and how to end. A run
  not typed prints none, so its output is its statements' alone.
- A failure is told on standard error and the run goes on; a run not
  typed stops at a cancelled statement, or with `--stop-on-error` at the
  first failure; any run stops once the current connection is lost. The
  program exits with 1 if any statement failed.
- The client library is the SQL-agent [ISO/IEC 9075:1992 §15]
  (`turborel_client::agent`): it runs CONNECT, SET CONNECTION and
  DISCONNECT itself, every other statement on the current connection, and
  a server that is sent one refuses it (SQLCODE -900). A `Connector` makes
  its connections; the command-line client's (`cli/src/dial.rs`) takes an
  `<SQL-server name>` as an address: 'host:port', or on Unix a socket's
  path beginning '/'; DEFAULT is the server the command line names, the
  connection made as the program starts. A connection is proved as the
  command line says: a password typed is asked for again; one from
  TURBOREL_PASSWORD is sent again. No transaction spans servers, so
  CONNECT and SET CONNECTION are refused in an active one (0A001)
  [§15.1 GR1, §15.2 GR1], and DISCONNECT of a connection whose
  transaction is active (25000) [§15.3 GR6].
- Where: `--host`/`--port` (127.0.0.1, 7422), `--tls FILE` trusting that
  PEM file's certificates with `--server-name`, or `--socket` on Unix,
  refused clearly elsewhere. Every address of a host is tried until one
  answers; a refusal is not retried.
- Who: `--user`, else the environment's USER or USERNAME. A password is
  typed without its echo (`-W`), or taken from TURBOREL_PASSWORD; never
  from the command line, where other users of the machine may see it.
  Without either, the server is asked to trust the user.
- Ctrl-C cancels the statement running (D44) by tokio's signals, on a
  thread of their own with a one-thread runtime: SIGINT on Unix, the
  console's Ctrl-C on Windows. It cancels on the current connection, as
  SET CONNECTION and DISCONNECT change it; before a connection is made,
  or while none is current, it ends the program, as it would have; the
  password is typed before it is taken.
- Dependencies (R-SAFE-4): tokio 1.54.1 (MIT), already the server's, with
  the features rt and signal only; rpassword 7.5.4 (Apache-2.0), a
  terminal's input without its echo on Unix and Windows, small enough to
  review, maintained since 2015.
*Why:* an operator needs a client to run SQL on a server without writing
a program. Its rows streamed keep its memory bounded whatever the result;
a password kept off the command line and out of the terminal's echo is
not seen by others.

---

## 2. Project layout

```
Cargo.toml              workspace and package; lint levels (D10)
clippy.toml             lint thresholds
rustfmt.toml            formatting (D9)
rust-toolchain.toml     pinned toolchain (D8)
.cargo/config.toml      the `cargo xtask` alias
README.md               what turborel is, and how to build and test it
RULES.md                this file
CONFORMANCE.md          what is implemented of the standards, and the
                        choices they leave to an implementation
TODO.md                 open work
CONTRIBUTING.md         how to contribute: changes, commit messages, pull
                        requests and issues
SECURITY.md             how to report a vulnerability
AGENTS.md               the guide for coding agents
CLAUDE.md               points Claude Code to AGENTS.md
.github/                CI (workflows/), the issue forms and the pull request
                        template
src/                    the library (lib.rs); one concept per module
protocol/src/           the native wire protocol: messages and codec (D42)
client/src/             the client: a connection over the protocol (D42)
server/src/             the server, as a library (D42)
server/test/            the server's integration test binary, with its client
turboreld/src/          the server's program: command line, signals (D42)
turboreld/test/         the program's integration test binary
cli/src/                the command-line client `turborel` (D46)
cli/test/               its integration test binary
lz4/src/                LZ4 compression, an encoding plug-in (D48)
aes/src/                AES-256-GCM encryption, an encoding plug-in (D26, D48)
support/src/            fixtures shared by the crates' tests (R-TEST-10)
test/integration/       the integration test binary: public API only
  main.rs               crate root and module list
  common.rs             shared fixtures and helpers
  <area>.rs             one module per area
bench/                  benchmarks, run by `cargo xtask bench`
  harness.rs            the harness every benchmark uses
xtask/                  the project's tools: verify, coverage, trace, bench
```

`Cargo.toml` declares each test and benchmark target with its path and
turns Cargo's discovery off, so the directories are `test/` and `bench/`.
Every other file under a target's directory is a module of that target.

---

## 3. Code rules

### R-STYLE: formatting and idiom

| ID | Rule | Checked by |
|---|---|---|
| R-STYLE-1 | Code **shall** be formatted by `rustfmt`. | `cargo fmt --check` |
| R-STYLE-2 | Code **shall** pass clippy `pedantic` and the restriction lints with `-D warnings`. | clippy |
| R-STYLE-3 | Names **shall** follow the Rust API Guidelines' casing. Code implementing a production is named after it: `QuerySpecification`, `query_specification`. | review |
| R-STYLE-4 | Modules are `foo.rs`, submodules in `foo/`; no `mod.rs`. | `mod_module_files` |
| R-STYLE-5 | No wildcard imports, except `use super::*` in unit tests. | `wildcard_imports` |
| R-STYLE-6 | A name **shall not** shadow an unrelated binding. | `shadow_unrelated` |
| R-STYLE-7 | No `dbg!`; production code **shall not** print. | `dbg_macro`, `print_stdout`, `print_stderr` |

### R-SIZE: short functions and files

| ID | Rule | Checked by |
|---|---|---|
| R-SIZE-1 | A function **shall** have at most 50 lines of code. | `too_many_lines` |
| R-SIZE-2 | A source file **shall** have at most 600 lines. Split along the standard's clauses or the pipeline's stages. | `cargo xtask file-length` |
| R-SIZE-3 | A function **shall** take at most 5 parameters, at most 1 of them a `bool`. Use a struct, a tuple or an enum. | `too_many_arguments`, `fn_params_excessive_bools` |
| R-SIZE-4 | A module **should** hold one concept, stated in its `//!` doc. | review |

### R-DOC: comments and documentation

| ID | Rule | Checked by |
|---|---|---|
| R-DOC-1 | Every public item **shall** have a doc comment; a function returning `Result` an `# Errors` section, one that can panic a `# Panics` section. | `missing_docs`, `missing_errors_doc`, `missing_panics_doc` |
| R-DOC-2 | Comments **shall** say *why*: the rule implemented, the choice made, the invariant relied on. | review |
| R-DOC-3 | Documentation **shall** build without warnings. | `cargo doc` with `-D warnings` |
| R-DOC-4 | Behaviour the standard governs **shall** cite it as printed: `[X3.135-1986 §8.6 GR2]`, `[ISO/IEC 9075:1992 §11.19 SR13]`. A choice left to the implementation says so and states the choice. Facilities beyond the standards are marked `[EXTENSION]`, internals the standards do not govern `[DESIGN]`. BNF in doc comments goes in backticks. | review |
| R-DOC-5 | No commented-out code and no `TODO` or `FIXME` comments: open work goes in TODO.md, and code never cites TODO.md. | review; `cargo xtask verify` |
| R-DOC-6 | Every file **shall** start with a `//!` doc stating its responsibility and the clauses it implements. | review |
| R-DOC-7 | Each implementation-defined choice **shall** be listed in CONFORMANCE.md with the file implementing it. | review |
| R-DOC-8 | A change of what a user, an operator or a program sees **shall** update the manual, `doc/`, in the same commit: each page that states what changed, in its own format (a statement page's NAME, SYNOPSIS, DESCRIPTION, RULES, PRIVILEGES, ERRORS, EXAMPLES, STANDARD and SEE ALSO; a chapter's sections); a new statement, setting, program option or event gets its page or its entry. Every claim is checked against the code, and every example run as shown, its output pasted as the change gives it. | review |

### R-CONST: no magic numbers or strings

| ID | Rule | Checked by |
|---|---|---|
| R-CONST-1 | A numeric literal other than `0` or `1` **shall** be a named `const` in production code, whose doc says where the value comes from. | review; `default_numeric_fallback` |
| R-CONST-2 | A string the program matches on or looks up (key words, plug-in names, settings, paths) **shall** be a named `const`, defined once. Messages for people are not, but cite their rule (R-ERR-2). | review |
| R-CONST-3 | Related constants **shall** live together in the module owning the concept, under a header naming their source. | review |
| R-CONST-4 | Tests may use literal fixture data; expected values restating a constant **should** use the constant. | review |

### R-ERR: errors, not panics

| ID | Rule | Checked by |
|---|---|---|
| R-ERR-1 | Production code **shall not** panic: no `unwrap`, `expect`, `panic!`, `unreachable!`, `todo!`, `unimplemented!`, indexing, slicing or `process::exit`. | clippy restriction lints |
| R-ERR-2 | Every failure **shall** be a `SqlError` with a SQLCODE, and its message **shall** say what happened in plain words. In the source, a message begins with the rule of a standard it breaks, `"[X3.135-1986 §8.3 SR6] ..."`, which the error keeps apart as its `rule`; or with `[DESIGN]` or `[EXTENSION]` when no standard governs it, which the error drops. Format errors name tokens instead (D24). | review; `error.rs` tests |
| R-ERR-3 | A `Result` **shall not** be discarded silently: `let _ = fallible()` needs a comment saying why the error cannot matter. | `unused_must_use`; review |
| R-ERR-4 | A `RefCell` borrow or lock guard **shall** be scoped to the block that needs it, and **shall not** be held across a call into a plug-in or any call that could take it again. A poisoned lock is reported (SQLCODE -910), never unwrapped. | review |
| R-ERR-5 | A failed statement **shall** leave no effect [X3.135-1986 §3.3]; write paths go through the transaction coordinator's undo. | tests |

### R-NUM: arithmetic and conversions

| ID | Rule | Checked by |
|---|---|---|
| R-NUM-1 | Integer arithmetic **shall** be explicit about overflow: `checked_*`, `wrapping_*`, `saturating_*` or `overflowing_*`. | `arithmetic_side_effects` |
| R-NUM-2 | `as` **shall not** be used on numbers; use `From`, `TryFrom` or a checked helper. | `as_conversions`, `cast_*` |
| R-NUM-3 | A divisor **shall** be shown non-zero, and `MIN / -1` handled. | `integer_division`; review |
| R-NUM-4 | Floating-point values **shall not** be compared for equality except through the standard's comparison rules in `value.rs`. | `float_cmp`; review |
| R-NUM-5 | A numeric literal whose type would fall back to the default **shall** carry a suffix or a typed binding. | `default_numeric_fallback` |

### R-CF: control flow testable to MC/DC

| ID | Rule | Checked by |
|---|---|---|
| R-CF-1 | A boolean decision **shall** have at most 4 conditions; beyond that, extract a named predicate. | review |
| R-CF-2 | Cognitive complexity **shall** be at most 15, block nesting at most 5. | `cognitive_complexity`, `excessive_nesting` |
| R-CF-3 | A `match` on an enum **shall** list every variant, with no `_`. | `wildcard_enum_match_arm` |
| R-CF-4 | Conditions **shall** have no side effects, except the consuming idioms of R-DEV-5. | review |
| R-CF-5 | Or-patterns **should** appear only at the root of a pattern, and match guards **should** be avoided: they hide conditions from MC/DC test design. | review |
| R-CF-6 | Recursion **shall** be bounded by a named depth constant, and fail with a `SqlError` past it. | review; tests |

### R-SAFE: language subset and resources

| ID | Rule | Checked by |
|---|---|---|
| R-SAFE-1 | No `unsafe` code. | `unsafe_code = "forbid"` |
| R-SAFE-2 | No `mem::forget`, `process::exit` or global mutable state. | `mem_forget`, `exit`; review |
| R-SAFE-3 | `dyn Trait` **shall** be used only at the extension points listed at the top of `src/db.rs`, the traits they hand out, the storage foundation's traits (D30), and streams of rows. Each extension trait **shall** document its contract, and every built-in implementation **shall** pass the same contract tests. | review; tests |
| R-SAFE-4 | No dependency **shall** be added without a decision in §1 covering rationale, version, license and review; only cryptography and compression plug-ins, the server crate and the command-line client may have any (D3, D26, D42, D46, D48). | review |

---

## 4. Test rules

### R-TEST: levels and layout

| ID | Rule | Checked by |
|---|---|---|
| R-TEST-1 | Each test **shall** go at the lowest level that can show its requirement: unit (`#[cfg(test)] mod tests`, private logic), integration (`test/integration/`, the public API only) or doc (examples in `///` docs). | review |
| R-TEST-2 | Tests **shall** be derived from requirements, not from the code, with normal-range cases (boundaries included) and robustness cases (invalid inputs, with the exact SQLCODE expected). | review |
| R-TEST-3 | A test's name **shall** say the behaviour checked; an integration test may end in its clause: `fetch_targets_8_6`. | review |
| R-TEST-4 | Test code may `unwrap`, index and panic: a panic is a failure. Every other rule applies. | lints |
| R-TEST-5 | Tests **shall** be deterministic and independent: no clock, randomness, network beyond the machine's own loopback (the server's tests, D42) or order dependence, and each test builds its own `Database`. | review |
| R-TEST-7 | Every defect fixed **shall** get a regression test that fails without the fix, saying so in its doc comment. | review |
| R-TEST-8 | For each decision of two or more conditions in new code, the tests **shall** show MC/DC, their doc comments naming the condition pairs. | review |
| R-TEST-9 | The tests **should** be checked by mutation now and then: break a mechanism, and expect the named tests to fail. | periodic |
| R-TEST-10 | A fixture or helper used by more than one module **shall** be written once: in its test binary's `common.rs`, or, used by the tests of more than one crate, in `turborel-test-support` (`support/`). Tests use `#[test]` and no other framework. | review |

R-TEST-6 was withdrawn with the golden-file tests (D6).

### R-TRACE: traceability

| ID | Rule | Checked by |
|---|---|---|
| R-TRACE-1 | Every test **shall** have a `/// Verifies:` paragraph starting with one of the tags `[X3.135-1986 §…]`, `[EXTENSION]`, `[DESIGN]` or `[TOOL] R-…`. | `cargo xtask verify` |
| R-TRACE-2 | Code implementing a requirement **shall** cite it (R-DOC-4). `cargo xtask trace '§8.6'` lists the tests for a clause. | review; `xtask trace` |
| R-TRACE-3 | CONFORMANCE.md **shall** be updated in the change that implements or changes a clause. | review |

### R-BENCH: benchmarks

| ID | Rule | Checked by |
|---|---|---|
| R-BENCH-1 | Every benchmark **shall** run on `bench/harness.rs`: the benchmark holds its workloads, the harness times, reports and keeps them. What benchmarks share belongs to the harness. | review |
| R-BENCH-2 | A benchmark measures and does not test: no assertions on timings, and `verify` does not run it. A claim about performance **shall** cite a `cargo xtask bench --compare` against the code before. | review |

---

## 5. Structural coverage and MC/DC

| ID | Rule | Checked by |
|---|---|---|
| R-COV-1 | Coverage **shall** be measured by `cargo xtask coverage`, over unit and integration tests, for the code under `src/`. On the pinned toolchain, doc tests are not run under coverage and unit-test modules count as covered code. | `xtask coverage` |
| R-COV-2 | Coverage **shall not** fall below the floors in `xtask/src/baseline.rs`; a change that raises it **should** raise them. | `xtask coverage` |
| R-COV-3 | The target is 100 % statement, decision and MC/DC coverage (D2). A gap is closed by a requirements-based test, by removing dead code, or by documenting deactivated code; never by a test without a requirement. | review |
| R-COV-4 | MC/DC **shall** be shown by test design (R-TEST-8) and checked in review (D22). | review |
| R-COV-5 | A coverage tool used for credit **shall** first be qualified (DO-330, TQL-5). | process |

Stable rustc gives region, line and function coverage, which approximates
statement coverage. Branch coverage needs nightly; MC/DC cannot be measured
(D22). Masking MC/DC is acceptable (CAST-6), which matters for Rust's
short-circuit `&&` and `||`.

---

## 6. Deviations

| ID | Rule | Checked by |
|---|---|---|
| R-DEV-1 | Every `#[allow]` and `#[expect]` **shall** carry a `reason` citing the rule and the justification. | `allow_attributes_without_reason` |
| R-DEV-2 | A lint baseline's count **shall never** rise, nor a lint be added to one; when violations are removed, the lower count is recorded with `cargo xtask lint-audit --write` (D11, D20). | `cargo xtask lint-audit` |
| R-DEV-3 | Permanent deviations, each with its reason in the code: printing in `xtask`; test-code allowances (R-TEST-4); `as` in the two float roundings std has no function for (`value::round_to_single`, `value::decimal::nearest_f64`); and `Code::sqlcode`, where an enum's discriminant is its SQLCODE. | lints |
| R-DEV-4 | Production code **should** use `#[expect]` rather than `#[allow]`, so a deviation no longer needed fails the build. | review |
| R-DEV-5 | A condition may consume input when it is a single call, alone or under `!`, whose only effect is consuming the input decided on: the parser's `accept_*`, `pop_front`, `pop`, `Option::take` (D16). Never inside `&&`, `\|\|` or a match guard. | review |

---

## 7. Process and tools

| ID | Rule | Checked by |
|---|---|---|
| R-PROC-1 | `cargo xtask verify` **shall** pass before every commit: format, lints, lint baselines, docs, file length, traceability and citations, every test. `--coverage` adds coverage. | pre-commit hook; CI |
| R-PROC-2 | Every change **shall** be reviewed against the checklist below. | review |
| R-PROC-3 | Commits **should** be small and about one thing, the message in the form CONTRIBUTING.md gives (`area: summary`, then why), citing the rules it applies or changes. | review |
| R-PROC-4 | A change to a threshold **shall** change RULES.md, `clippy.toml` and `xtask` together. | review |
| R-PROC-5 | A release's crates **shall** be published together at its number, from a pushed commit that passed `cargo xtask verify --coverage`, a dry run first, as VERSIONING.md says. | review |
| R-TOOL-1 | Project automation **shall** be Rust in `xtask/`, never shell scripts (D7). | review |
| R-TOOL-2 | `xtask` **shall** follow these rules, use only std, and test its own decisions (`[TOOL]` requirements). | `xtask verify` |
| R-CM-1 | The toolchain is pinned (D8); changing it **shall** be a commit of its own that passes `cargo xtask verify --coverage`. | review |
| R-CM-2 | `Cargo.lock` and the baselines **shall** be under version control. | review |

The review checklist:

- the requirement is cited in the code (R-DOC-4) and in the tests (R-TRACE-1);
- normal-range and robustness tests (R-TEST-2), and MC/DC pairs for new decisions (R-TEST-8);
- no magic numbers or strings (R-CONST);
- comments explain why (R-DOC-2);
- conditions have no side effects (R-CF-4), and borrows and locks are scoped (R-ERR-4);
- CONFORMANCE.md updated (R-TRACE-3), and TODO.md where open work changed.

---

## 8. DO-178C

These rules make the code and its verification acceptable inputs to a
DO-178C process. They do not make the software certifiable on their own:
certification is a property of a process, run under plans agreed with the
certification authority. Meeting DO-178C as written here also meets
DO-178B.

| DO-178C item | Covered by |
|---|---|
| Software Code Standards (11.8) | §3 |
| High-level requirements | The standards' clauses; CONFORMANCE.md is their status |
| Derived requirements | `[EXTENSION]` and `[DESIGN]` tags |
| Requirements-based testing (6.4.2) | R-TEST-2 |
| Traceability, requirements ↔ code ↔ tests | R-DOC-4, R-TRACE |
| Structural coverage (6.4.4.2) | R-COV; statement coverage approximated by regions, MC/DC by design |
| Dead and deactivated code (6.4.4.3) | R-COV-3, R-DOC-5 |
| Configuration management of tools and baselines | R-CM |
| DO-332, dynamic dispatch | R-SAFE-3 |

Outside these rules:

1. **Plans** (PSAC, SDP, SVP, SCMP, SQAP) are not written; this file feeds
   the SDP and SVP.
2. **Low-level requirements** have no document of their own: doc comments
   and unit tests stand in for them.
3. **Data and control coupling analysis** has not been done.
4. **Source to object code traceability** (Level A) has not been done.
5. **Tool qualification:** the compiler and the coverage tools are not
   qualified (R-COV-5).
6. **Independence of verification** needs an organisation, not a rule.
7. **Dynamic memory:** the engine allocates on the heap throughout, a DO-332
   vulnerability area; an airborne use would need bounded allocation or an
   analysis of it.

The rules were adopted on 2026-10-23, when the code was audited against
them; every finding was fixed, and the lint baselines and over-long files
were paid down, the same week.

---

## 9. References

- ANSI X3.135-1986, *Database Language SQL* (FIPS PUB 127), and ISO/IEC
  9075:1992, *Database Language SQL*: the requirements.
- RTCA DO-178C, *Software Considerations in Airborne Systems and Equipment
  Certification*, and DO-178B; supplements DO-330 (tool qualification) and
  DO-332 (object-oriented technologies). CAST-6, *Rationale for Accepting
  Masking MC/DC in Certification Projects*.
- [Safety-Critical Rust Coding Guidelines](https://coding-guidelines.arewesafetycriticalyet.org/):
  gui_dCquvqE1csI3 (overflow), gui_ADHABsmK9FXz (`as`), gui_kMbiWbn8Z6g5
  (division by zero).
- [What does it take to ship Rust in safety-critical?](https://blog.rust-lang.org/2026/01/14/what-does-it-take-to-ship-rust-in-safety-critical/),
  Rust blog, 2026-01-14. [Ferrocene](https://ferrocene.dev/en), a
  qualified Rust toolchain.
- *The Rust Programming Language*, ch. 11; *The Cargo Book*; the Rust API
  Guidelines; the Rust Reference on integer overflow.
- P. L. Lehman and S. B. Yao, "Efficient Locking for Concurrent Operations
  on B-Trees", ACM TODS 6(4), 1981: the B-link tree.
- W. Litwin, "Linear Hashing: A New Tool for File and Table Addressing",
  VLDB 1980: the hash table.
- R. N. Williams, *A Painless Guide to CRC Error Detection Algorithms*
  (1993); G. Cook, [CRC RevEng catalogue](https://reveng.sourceforge.io/crc-catalogue/all.htm);
  M. E. Kounavis and F. L. Berry, *A Systematic Approach to Building High
  Performance Software-based CRC Generators* (2005); D. V. Sarwate,
  *Computation of Cyclic Redundancy Checks via Table Look-Up* (1988).
