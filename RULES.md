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
| D58 | Dynamic SQL in SQL text: a session plays the module |
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
| D59 | Import: rows from a pluggable source, over a connection |
| D53 | Data types: one contract, standard types built in |
| D55 | Threads: one manager, each thread labelled and shown |
| D56 | Files: one manager a database, its open files limited, each shown |
| D57 | Replicated state: the log, the catalog and the durable files, as opaque bytes |

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
client and the protocol take only with their features `tls` and `scram`,
and the platform's certificate authorities, the client's only with its
feature `system-roots` (D26), compression in plug-ins
(D48), tokio and socket2 in the server crate (D42), tokio's signals in the
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
Of a product, such a conjunct that reads one operand's columns alone is
evaluated on every row of that operand when every other operand has a
row, and on none when one has none: each row of the operand is in some
row of the product exactly then (§5.20 GR2b). So it is moved onto its
operand, over a guard of one row of no columns, an EXISTS of each other
operand (`optimizer/raising.rs`), and the rest of the condition, which
cannot raise, may be a hash join's keys and index probes. A table read
for a filter with such a conjunct is never probed, by a constant nor by
a join's rows (D35): its every row is. An outer join's ON condition is
applied to every row of its operands' product [ISO/IEC 9075:1992 §7.5
GR1c], so its conjunct that can raise and reads the columns of the
operand given nulls alone is moved onto that operand so too, over an
EXISTS of the other (`optimizer/outer.rs`), and the join is on its
equalities still. One of the preserved operand's columns stays in the
join, as that operand's rows are all kept, so no filter of them can be
its: a join on keys judges it of each of that operand's rows once the
other is known to have one, a row it is not true of matching none, and
so too any such conjunct of either operand that did not move
(`Plan::OuterJoin`'s own conditions); a conjunct that can raise and
reads both operands' columns has every pair judged, the join on no keys.
*Why:* §5.18 GR2 defines AND only by its truth table, and §5.21 GR2 and
§5.23 GR2 apply the condition to each row and group; nothing permits
leaving an operand unevaluated. So results never depend on the plan.
*Exception:* an EXISTS stops at the first row of its subquery that its
condition keeps, whatever the plan: read by batches, a batch whose
condition raises an error gives the rows before its failing row first,
as a read row by row would, so no error of a later row is raised.
ISO/IEC 9075:1992 §8.8 GR1 defines EXISTS by "the cardinality of T",
which its first row decides, and §3.3.4.4 makes it
"implementation-dependent whether expressions are actually evaluated
left to right, particularly when operands or operators might cause
conditions to be raised or if the results of the expressions can be
determined without completely evaluating all parts of the expression"
(Annex C item 6: whether the exception of an inessential part is raised
is implementation-dependent). So, likewise, a scalar or row subquery
stops at its second row, which makes it fail [§7.11 GR1]. IN, ANY, SOME
and ALL over a subquery read all of its rows, so they raise any error of
them.

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
An undo is never refused by the memory budget (D38).
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
  record of an ALTER TABLE keeps it too. A constraint's own name written
  without a schema is the default schema's too, which may be another than
  its table's: no Syntax Rule makes them one, and the Definition Schema
  keeps CONSTRAINT_SCHEMA beside TABLE_SCHEMA [§21.3.13]. It is then a
  constraint of that schema, which must exist [§5.4 GR18], among whose
  constraints its name is unique [§10.6 SR2]; ALTER TABLE DROP CONSTRAINT
  finds it there. So is a domain constraint's, of CREATE and ALTER DOMAIN.
  A name the implementation gives one is of its table's or domain's
  schema [§11.6 SR2]; a declared local temporary table's are all MODULE's.
- \[EXTENSION\] A constraint is named in a schema only by that schema's
  owner, who is the owner of its table's or domain's schema too; another's
  is refused as an access rule violation (SQLCODE -104), which SQL-92's
  text does not require: no one puts a name in another's schema, which
  would refuse that owner's own constraints of the name. DROP SCHEMA
  RESTRICT is refused while a constraint named in the schema is of a table
  or domain of another; CASCADE drops it, as ALTER TABLE or ALTER DOMAIN
  DROP CONSTRAINT CASCADE would, where SQL-92 leaves it [§11.2 GR1-GR8].
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
  durable; commit order is log order. A session whose LOG.COMMIT_SYNC is
  OFF (D36) is answered once its commit record is written, not synced:
  the transaction is seen at once, and becomes durable with the log's
  next sync, of a commit that waits, a definition or a checkpoint, each
  syncing every record before its own; a crash before then loses it, and
  every commit after it, so recovery still finds a prefix of the commits.
- The `Log` that stores the records is a plug-in, with a memory log and a
  segmented file log with group commit built in. A database kept in a
  directory makes or opens its log through a `LogStorage`, a plug-in too,
  as it does its catalog (`db/stores.rs`); built in, the file log in the
  directory's `log/`. Opened, the file log is checked from the position
  the last image holds it durable through: the segments before it were
  synced whole. A segment cut short by a crash ends the log, and the
  segments after it are removed: a sync seals the segments before its
  own, so none of their records was durable; the bytes cut are reported
  in the database's `RecoveryReport` and its `opened` event. Valid
  records after invalid bytes, in the segment or a later one, are damage
  unless the invalid bytes show a write the crash interrupted (a sector
  never written, reading as zeros; a zero length ending them; a record
  running past the end of the file): the log is then refused (SQLCODE
  -916), nothing cut, as cutting would lose records committed after the
  damage.
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
  undoes from the log, by the database's one replay (D57).
- A checkpoint is due after CHECKPOINT.AFTER_CHANGES row changes
  logged since the last taken, a failed one leaving them counted. The
  log is kept from the last checkpoint or the first record of the oldest
  open transaction: while checkpoints are refused, not taken, or one
  transaction stays open, it grows, and past four times what makes one
  due (`db::OVERDUE`) an event says so and why, at most once a minute
  (`Event::CheckpointOverdue`, D40).
- A database is open in one place at a time: opening it takes an exclusive
  lock on its directory's `lock` file (`FileSystem::lock`, the system's
  file lock), before anything there is cleared or read, and closing it
  gives the lock up, unlocked before the file is closed, so a process it
  started, sharing the open file until it runs its program, does not
  hold it on; a process that crashes gives it up with it.
- A failed sync of the log stops the database taking changes, as it may
  have lost what the log held, and a later sync may succeed without it.
  The file log keeps the failure, of a commit's sync or of a segment's as
  an append seals it, and fails every later sync with it, never trying
  the sync again; the database stops at the next: from then on, no row
  change, commit of a transaction that wrote, definition or checkpoint is
  accepted (SQLCODE -915), and it cannot be closed cleanly; reads and
  rollbacks go on. Opened again, it is recovered from what its log holds
  (`Database::log_failure`).
- A commit whose commit record is written but whose sync fails is in
  doubt: it is never stamped (its engines are not told it committed), so
  no session sees it, its own neither; its session is told SQLCODE -922,
  SQLSTATE 08007 (transaction resolution unknown); its locks and versions
  are held until the database is dropped, with no abort record, so
  nothing builds on it or undoes it. Opened again, recovery decides it by
  what the log holds: committed if the commit record reached the disk,
  else rolled back.
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
  on aws-lc-rs, a dependency of the test support's feature `tls`, which
  the server's tests take: none is kept in the repository to expire.
- Encryption at rest: AES-256-GCM and ChaCha20-Poly1305 from the same
  aws-lc-rs, in the crate `turborel-aes`, encodings (D48), their subkeys
  by HKDF-SHA256.
- Passwords: PBKDF2-HMAC-SHA256 from the same aws-lc-rs (D43), at
  SERVER.PASSWORD_ITERATIONS, 600,000 by default: OWASP's choice where
  FIPS 140 is needed, and no dependency beyond TLS's. It is not
  memory-hard as Argon2id is, OWASP's first choice; but RustCrypto's
  `argon2`, its one implementation in Rust, says nothing of an audit or
  of being ready for production, and has no FIPS path. A verifier names
  its scheme, so Argon2id may come beside it.
- SCRAM-SHA-256 (D43): its HMAC-SHA-256, SHA-256, PBKDF2 and randomness
  from the same aws-lc-rs, in `turborel-protocol` with its feature
  `scram`, which the server takes and the client with its own feature
  `scram`: both sides one implementation, checked against RFC 7677's
  example. Its base 64 is written here: an encoding, not cryptography.
- Client certificates (D42) are verified by rustls's
  `WebPkiClientVerifier`; the common name of a verified certificate's
  subject is read from its DER by the server's own walk of tags and
  lengths, as no vetted crate in the tree reads a subject.
- The client trusts the authorities of the files it is given, and the
  platform's only when asked (`tls::roots`; `turborel
  --tls-system-roots`): a platform trusts many authorities, any of which
  may certify a server of any name, where a server of one's own is
  usually certified by an authority of one's own; so a file stays the
  default. The platform's come from rustls-native-certs 0.8.5 (Apache-2.0,
  ISC or MIT), the client's feature `system-roots`, which the command-line
  client takes: of the rustls project, as rustls is, its README calling
  it "mature and widely used" and "still maintained, but mostly for use
  inside the platform verifier": it points new users to
  rustls-platform-verifier, which verifies by the platform's
  own verifier: a larger change of trust, not taken. No `unsafe` of its
  own and no advisory. It reads the keychain on macOS by
  security-framework 3.7.0 with security-framework-sys 2.17.0,
  core-foundation 0.10.1, core-foundation-sys 0.8.7 and bitflags 2.13.2
  (each MIT or Apache-2.0; kornelski's, servo's and the bitflags
  project's, bindings to the system's frameworks, their `unsafe` the
  calls into them); the system store on Windows by schannel 0.1.30
  (MIT), on the windows-sys already in the tree; the system's bundle
  elsewhere by openssl-probe 0.2.1 (MIT or Apache-2.0, now the rustls
  project's; its one `unsafe`, setting the environment's variables, is a
  function rustls-native-certs never calls: it calls `probe`, which only
  looks for files), held at 0.2.1 in Cargo.lock as 0.2.2 is not
  yet two weeks old. None of these has an advisory open (RUSTSEC-2017-0003
  of security-framework was fixed in 0.1.12). Certificates rustls cannot
  parse are left out; a store with none it can is refused, saying why;
  the environment's SSL_CERT_FILE and SSL_CERT_DIR, if set, are read
  instead of the store, as the crate does. webpki-roots, a list fixed at
  build time, is not taken: it goes stale with the program.
- Cryptographic hash functions, as `HashFunction`s (D30), from the same
  aws-lc-rs, in `turborel-aes`: SHA-256 (id 11), a checksum no accident
  matches, though one who can rewrite a file can rewrite its checksums;
  SHA-512 (id 13), the same of 512 bits; and HMAC-SHA-256 (id 12),
  keyed by a keyring's key through HKDF-SHA256, a checksum a forger
  without the key cannot match. A file records the function's id alone,
  never the key: opened with another key, its headers fail their
  checksums, and it is refused (SQLCODE -916). `turboreld` keys it by
  `--checksum-key`. None stops a block or page
  the database once wrote being put back where it was. A digest holds
  512 bits at most (`hash::MAX_DIGEST_BYTES`), which every format that
  stores one has room for: a block file's header, a pager file's header
  and meta slots; a block's or a page's trailer is the function's own
  size, and a page's body the page less it.
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
  AES-256-GCM, 3 is ChaCha20-Poly1305; tests take 1000 and above.
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
- AES-256-GCM and ChaCha20-Poly1305 [RFC 8439] are the crate
  `turborel-aes`, on the aws-lc-rs of D26: ChaCha20-Poly1305 for
  processors without AES instructions, sealed as AES-256-GCM is, its
  nonces drawn from aws-lc-rs's generator, its HKDF info its own, so one
  key named by both gives each subkeys of its own. Their
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
  are read with their own, until the table is sealed again: `ALTER TABLE
  ... RESEAL` [EXTENSION], RESEAL a `<non-reserved word>` (also
  `Session::reseal_table`, `turboreld ... reseal`), gives the table a new
  generation of its storage, as ALTER TABLE changing its engine gives it,
  its rows copied there through its encodings now, by its transaction's
  own writes, logged as any ALTER TABLE. Committed and checkpointed, nothing of the table is read with
  the old key, which may then be retired. A key of STORAGE.ENCODINGS
  serves the database's life.
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
  later process, so no keyring is needed. Before it, they pass through
  the encodings SPILL.ENCODINGS names, as LZ4, none by default, as LZ4
  was not clearly better where the disk keeps up. Should the two make more
  stages than a pipeline holds, the encryption alone is kept. An area
  keeps the encodings it was made with; a list that spills
  (`SpillList`), as a transaction's changes, makes a new area for what it
  spills once they change (`WorkAreas::generation`), so a table
  encrypted while a transaction runs has its later spills encrypted, the
  runs before as they were until read or dropped; an operator's area is
  as it was until the operator ends. The data file's shadows hold its
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
  Another, of PRESERVE ROWS, is collected then by its session, of every
  version superseded or deleted, its index entries with them: no other
  session reads it, and none of its own transactions is open, so the
  database's horizon, which another session's long transaction holds
  back, holds none of its versions. The database's collection
  (`txn/end.rs`) leaves temporary tables to their sessions.
- A session is one module, so a created local temporary table is, within
  a session, as a global one; and its declared local temporary tables
  [§13.11] are in a schema MODULE of its own definitions alone, owned by
  its authorization identifier (`db/declared.rs`), in no other session's
  and no view of the Information Schema.
- ALTER TABLE of one is refused (SQLSTATE TB911) while another
  transaction has a session's instance's rows open; while its
  transaction is open, no other begins on an instance. The database's
  instances are noted, by session (`temporary::Uses`). ADD and DROP
  COLUMN give each instance that holds rows a successor of the new
  generation, its rows copied there by the altering transaction, the
  table's constraints checked of them at once; each session takes its
  successor at the commit, and a statement or transaction that began
  before reads it no more (`TableLock::check_generation`). A session's
  rows are checked only against its own instances, so the copy is
  refused while another session holds rows and a constraint of the table
  reads another table. Any other action is made only while no instance
  holds a row.
- CREATE INDEX on one defines it, of the built-in methods alone. Each
  instance makes its entries in memory as a statement of its session
  first sees the index: empty if it has no rows, else of every version of
  them if no transaction has them open; else the session uses the index
  only once its transaction ends, a use meanwhile refused (SQLSTATE
  TB911). An index dropped is forgotten by each instance alike.
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
  deferred.
- Checked immediate at a statement's end, it held as the statement began,
  so two forms of a conjunct of its condition, each over one table T and
  a condition `p` of T's row alone (columns, constants, arithmetic,
  comparisons, IS NULL, LIKE, CASE: no function, CAST, subquery, outer
  reference, so the same truth or error each time, and no warning), are
  checked from the rows the statement changed (`Effects::changed_rows`,
  `db/assertions/changes.rs`): `NOT EXISTS (SELECT ... FROM T WHERE p)`
  from the rows it inserted or updated, as a CHECK is; `(SELECT COUNT(*)
  FROM T WHERE p) op k`, of a constant `k` and the standard COUNT, from
  the rows brought into the count less those taken out, held without
  reading T if that cannot move the count toward false and the
  transaction reads at one snapshot (D29), else checked whole. Every other
  form, and a deferred assertion, is checked whole. SUM, AVG, MIN and MAX
  are checked whole: a SUM's nulls and its overflow depend on every row,
  which the change does not show. Its name is a constraint name of its schema, for SET
  CONSTRAINTS and for every other constraint's [SR2, SR7].
- One in a `<schema definition>`, made outside any session's
  transaction, is checked in one of its own, reading the newest committed
  rows; it is not checked again as the log is replayed.
*Why:* a CHECK with a subquery is already checked over all of what it
reads, deferred and logged; an assertion is one with no table of its own,
and needs no new machinery but its query. Checking it whole costs a read
of its tables for each statement, however little it changed; the forms
checked from a change are those whose result provably equals the whole
check's, given that it held as the statement began.

**D52. A view's REFERENCES: from what its columns are drawn from.** The
privileges of a view's owner on it are found from its tables'
`applicable`); REFERENCES on its column CV is held if the owner holds
REFERENCES on every underlying column of CV of the view's leaf underlying
tables, and on some column of each [ISO/IEC 9075:1992 §11.19 GR6b].
- The leaf underlying tables are the tables and views the FROM clauses of
  its query name, through joined and derived tables and set operations'
  operands; a view it reads is a leaf, having no underlying table [§4.9].
- The underlying columns of a column of a `<query specification>` are
  those its select list item references, a subquery's too, and those its
  `<table expression>` references: its WHERE, GROUP BY and HAVING, a join
  condition, a derived table's query [§7.9 SR5, §7.3 SR2, §6.4 SR9]. A
  common column of a NATURAL or USING join is drawn from both its join
  columns, as `COALESCE ( TA.C, TB.C )` [§7.5 SR6d]. A `<table value
  constructor>`'s columns have none [§7.10 SR16a]; a set operation's are
  both operands' i-th [SR16d], or with CORRESPONDING, both operands'
  columns of its name [SR9a iv].
- They are found as the view is defined, those of its leaves kept in its
  descriptor (`Underlying`), each by table and name.
*Why:* finding them again from the query at every privilege check would
bind the view each time.

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
- Store assignment [ISO/IEC 9075:1992 §9.2 SR2] to or from a plug-in type
  takes a value comparable with the target, as the kind says; and a
  character string, into a type whose kind says it takes one
  (`SqlType::assigns_text`), made a value of it as its CAST makes one.
  By default a kind takes none, and a character string becomes one of its
  values only by CAST. A DEFAULT's character string literal is its CAST
  either way, a plug-in type having no literals of its own.
- A scalar function may declare the type of each argument
  (`ScalarFunction::argument_type`): an argument of another type is
  stored in it by store assignment before the call, refused as the
  statement is bound if it cannot be, and `return_type` is told the
  declared types; a `?` there takes the declared type. By default none is
  declared, and an argument is given as it is. One function is
  registered under a name: none is chosen among several by its
  arguments' types.
- No privilege is needed to use a plug-in type: SQL-92's privileges are
  on objects of a schema [§4.26, §11.36 SR1], USAGE on a domain,
  character set, collation or translation, each with an owner who grants
  it; a plug-in type is in no schema and owned by no authorization
  identifier, registered by the program as code is, as a built-in type
  is. A privilege on one would be an extension with no grantor.
*Why:* a standard type is grammar as much as storage, and none is
replaced; one contract still keeps every type, built in or not, honest to
the same responsibilities, and a plug-in type stored, logged, indexed and
sent as a built-in one is.

**D54. Datetimes and intervals: UTC, the session's zone, a clock.**
- A time or timestamp is kept as its instant in UTC, to the microsecond,
  as ISO/IEC 9075:1992 §4.5.1 says, with its displacement if its type is
  WITH TIME ZONE. Without time zone, its displacement is the session's,
  implied [§6.1 GR5]: a value retrieved is shown in it, so it shows
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

**D58. Dynamic SQL in SQL text: a session plays the module.**
- The statements of ISO/IEC 9075:1992 §17 run by `Session::execute`, as
  any `<SQL procedure statement>` does, the session in the `<module>`'s
  part (D32 claims no module language): PREPARE, EXECUTE, EXECUTE
  IMMEDIATE, DEALLOCATE PREPARE, DESCRIBE, ALLOCATE, DEALLOCATE, GET and
  SET DESCRIPTOR, ALLOCATE CURSOR, and the dynamic OPEN, FETCH, CLOSE and
  positioned DELETE and UPDATE. A `<dynamic declare cursor>` is declared
  through `Session::declare_cursor`, as any cursor is.
- Their values are the program's parameters (`Params`), written as
  SQL-86 writes them, an `<identifier>` declared, where §17 writes a
  `<parameter name>` or an embedded variable; an `<identifier>` that is a
  parameter's name, where a name of a statement or cursor stands, is the
  parameter, so the name is an extended one [§5.4]. Direct SQL, the
  client's, has no parameters [§20.1 SR1]: there only the statements whose
  operands may be `<literal>`s are of use (ALLOCATE, DEALLOCATE and SET
  DESCRIPTOR, EXECUTE IMMEDIATE); PREPARE's text and an extended statement
  or cursor name are no `<literal>` [§17.6 SR1, §5.4 SR13].
- Prepared statements, cursors allocated and descriptor areas are the
  session's, by name: a `<statement name>` or `<cursor name>`, and LOCAL
  and GLOBAL extended names, each apart [§5.4 GR12-GR16]; kept across
  transactions [§17.6 GR10] until deallocated or the session ends [§20.1
  GR2b]. A statement is prepared as `Session::prepare` prepares one (D45);
  EXECUTE and EXECUTE IMMEDIATE run it as `execute_prepared` does, a
  statement of its own, which begins a transaction as the statement
  prepared does [§4.22.6]; every other statement of §17 is transaction
  initiating.
- A positioned DELETE or UPDATE prepared finds its cursor as it is
  prepared, a declared cursor's or an allocated one's [§17.6 GR2], and
  one of no table takes that cursor's [§17.19 SR1, §17.20 SR1]. PREPARE
  and EXECUTE IMMEDIATE refuse a statement with a `<comment>` [§17.6
  GR4q] and one whose cursor is not there (SQLSTATE 34000); the API's
  prepare (D45), not §17's PREPARE, takes both, as it always has: a
  driver's SQL often carries comments, and a program may prepare a
  positioned statement before it declares its cursor, the cursor then
  found as the statement runs.
- Implementation-defined: a descriptor area allocated without WITH MAX
  has 100 occurrences, at most 1,000 [§17.2 SR2, GR3]; a session holds 64
  areas at once, more refused as beyond a limit (D18); a plug-in's type
  (D53) is described by the TYPE code -1 [§17.1 Table 18].
*Why:* SQL-92's dynamic SQL is defined for a module's procedures; a
session already plays that part for cursors and statements run with
parameters, so the same statements run there with the standard's
meaning, and a program, or a driver, needs nothing beyond SQL text.

**D55. Threads: one manager, each thread labelled and shown.**
- Every thread the database, the server or the command-line client runs,
  and every piece of background work, is started through one manager
  (`src/threads.rs`), never by `thread::spawn` elsewhere (R-SAFE-5). Each
  has a label, `turborel-<kind>[-<n>]`, a kind, its database, and a state:
  starting, idle, running, stopping, stopped or failed; when it started,
  when it last ran, how often, its last error, and its restarts.
- Background work (checkpoints, maintenance) is a job the manager runs as
  a policy says, a policy of each database: automatic, a thread of its
  own as now; manual, run only when the program asks
  (`Database::run_due`), as tests and an operator choosing the time do;
  or paused. A job's errors are noted and told as events (D40); a panic
  is caught, told, and the job started again as its policy says.
- Lifecycle: a job starts as its database opens, or when first due, told
  either way; none starts once its database is closing; a close that
  fails leaves its jobs as they were; every thread of a database is
  joined before its files and lock are released, and none drops its
  database last: the program's handles, its sessions' among them, are
  counted apart from those a job holds while it works, and the program's
  last, dropped, stops and joins the jobs, a job's work in progress
  waited for, so the lock is given up as that drop returns.
- The server's runtime is built by the manager: its threads named, their
  number and its pool of blocking threads from settings (D42); its
  statement workers, listeners and connections counted, refused
  connections bounded. The command-line client's interrupts are a job of
  its own, a cancel bounded in time (D46).
- Turns a job takes (a checkpoint's, maintenance's) refuse the thread that
  holds them, as the definitions' turn does (D27), so no lock is held
  across a plug-in's call (R-ERR-4).
- The manager is shown: a system table (INFORMATION_SCHEMA.JOBS), each
  thread and job with its state, to the database's owner; and events
  when one starts, stops, fails or restarts.
*Why:* threads started where they were needed, under names of their own
or none, were unseen by operators, outlived what they served, swallowed
their errors, and made tests depend on their timing. One manager makes
each visible, bounded and stoppable, and lets a test or an operator run
the work at a time of its own choosing.

**D56. Files: one manager a database, its open files limited, each shown.**
- Every file a database opens, creates, removes or locks is through its
  file manager (`src/files.rs`), which wraps the `FileSystem` the
  database was given, so shadowing and test file systems stay below it.
  Each file is registered: its owner (the database, an engine or index
  method by name, a spill, the log), its kind and path, its mode, its
  bytes, its class of durability, and its counts of reads, writes and
  syncs.
- Classes of durability: durable (the catalog, the data file, the log);
  durable at commit (the shadow files); unsynced (the undo log, begun
  anew at each open); scratch (spill files, removed when dropped, cleared
  at each open). A class says what a create, a remove and a sync do, and
  what a close or an open does to its files: a directory made or emptied
  is synced in its parent where durability needs it, and only there.
- Open file descriptors are limited a database, FILES.MAX_OPEN, 1024 by
  default, 0 for no limit: past it, the file used least recently is
  closed, to be opened again as it is next used (LRU). Log and undo
  segments, spill files and the rest share the limit; a file held open
  by a read or write in progress is not closed under it, and one that
  needs a descriptor while every one is so held waits for one to be
  given back. A file that cannot be opened again is an error of the
  statement or job that needs it, told as an event.
- A file is closed for another only with its writes synced: a file
  with writes not yet synced is closed after those without, and synced
  (`Durability::Data`) first, as a writeback error seen by no sync is
  lost with the descriptor (on Linux, a sync through a descriptor opened
  after the error need not report it). If that sync fails, the file
  stays open and the failure is told, and kept: the file's next sync of
  some durability returns it, so no later sync stands for the writes
  lost. At close, such a file is synced before it is closed, a failure
  told.
- Engines and index methods take the manager through `Host`, as they take
  the memory budget (D38): a directory of their own under the
  database's, shadowed or not as they declare, and scratch files; what
  they open is theirs to show, limit and close as the database's.
- Files of a database in memory are of `MemoryFileSystem`, their bytes
  reserved of MEMORY_BUDGET; its scratch files too, never the system's
  temporary directory.
- On Unix, files are made readable and writable by their owner alone
  (0600), directories 0700.
- Lifecycle: at open, scratch is cleared and the durable classes
  recovered; at close, after its threads are joined (D55), every file is
  synced as its class says and closed, scratch removed, and anything
  still open or not removed told, before the directory's lock is
  released.
- Shown: INFORMATION_SCHEMA.FILES, each file open or known, to the
  database's owner; and events when a file cannot be opened, removed or
  synced, and when the limit closes files faster than they are reused.
- The server's own files, its diagnostics log, socket, and TLS and key
  files, are registered as the server's, shown and counted, though not
  the database's to limit.
- While anything of the database is encrypted (D48), events carry no
  statement's text: a slow statement is told by its user and time
  alone, so no value meant for an encrypted table reaches a log in the
  clear.
*Why:* files were opened where they were needed, some through the
database's `FileSystem` and some around it; nothing bounded the
descriptors a server held, nor showed them; a failed rotation, a file
left behind, a directory not synced, went unseen. One manager makes
each file visible and bounded, and gives engines what the database's
own files have.

**D57. Replicated state: the log, the catalog and the durable files, as
opaque bytes.**
*Decision:* all durable state of a database is in its `Log`, its
`CatalogStore`, or the files its file manager registers as durable
(D56). A feature keeping durable state anywhere else declares an export
hook, or is refused in a replicated database (SQLCODE -910). Replication
orders, stores and moves three kinds of opaque bytes, log records,
checkpoint images (catalog records) and durable files, and never decodes
them. What a record means and how it is applied is the core's one replay,
`Database::replay`, which recovery runs and a standby will run: a new
kind of record, object or file is replicated once recovery handles it.
Replication's own extension points are traits of the crate
`turborel-replication` (std only): `Consensus`, a protocol as a
deterministic state machine, `Transport` and `StateTransfer`, which
moves an export (the core's `StateSource`); Viewstamped Replication,
TLS over TCP and whole files are their first implementations. VR is
written from Liskov and Cowling's "Viewstamped Replication Revisited",
departing from it for durability: a record durable before PREPAREOK;
the view-number written before STARTVIEWCHANGE, and with v′ and the
new log, at once, before a replica is normal in a view; a replica made
again from its journal rejoins its view, but one that was its primary,
which waits for the next; a new primary serves once the log it took
over is committed and delivered. Approved
with it: a `Log` for commit, beside a driver of those traits that knows
no content, and the core's `Standby` seam (replay, promote, depose,
export, import); a record durable before a backup acknowledges it; a
sync in doubt never stamped (D25), and when replicated resolved by
consensus or the node deposed;
a `node` property for settings never logged; reads on the primary
confirming its view; no at-most-once for SQL clients in alpha; stale
snapshot reads on backups, with a least position; state transfer by the
durable files the file manager knows; the traits and VR in a crate of
their own, the network in the server.
*Why:* most of SQL-92 and all after it is still to build, each with
new records, objects and files; none may need a change to replication.
*Consequences:* checks fail the build or `verify`: replay matches on
`Change` with no wildcard arm (R-CF-3), so a kind added does not
compile until replayed; `record::KINDS` lists every kind, and a test
fails until a kind added is listed, and another until a statement
writing it is shown recovered to the state it left; every durable file
the file manager registers is in the export manifest
(`Database::export_manifest`), or carried by the log or the catalog,
and a file in the directory of no known kind is refused; contract tests
of `Consensus` and `Transport` run against each implementation
(R-SAFE-3); VR runs, in the tests, hundreds of seeds of a deterministic
simulator (`turborel-test-support`) of frames delayed, lost, duplicated
and reordered, partitions, crashes and failing disks, which checks that
committed records are never lost or reordered across view changes, that
at most one primary serves a view, the one it names, and that the group
converges once the faults stop; later, there, a comparison of backups
with the primary.
*See:* D25, D36, D55, D56; `db/recovery.rs`, `db/export.rs`,
`files/classes.rs`, `log/record/kinds.rs`, `replication/src`.

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
which takes it exclusively: a frame so latched is busy for a moment, as
is an unpinned one the eviction policy declines (CLOCK spares frames used
again as it sweeps), so an eviction finding only such frames and pinned
ones waits a bounded while for the busy, then is refused as with every
frame pinned (D38), as the latch may be its own thread's: some 20 ms
while the pool does nothing else, so a holder starved of the CPU on a
busy server does not get it refused while other reads go on, and half a
second at most however busy the pool; hits are counted on stripes by
thread. A page freed once released, by a structure whose readers lock
nothing, waits a bounded while for its latch, longer for its pins, then
is refused (SQLCODE -910) naming the page, as the hold may be its own
thread's, which the pool does not track. Hot pages contend
on that latch still; a reader-biased latch (BRAVO) would
need a reviewed exception to R-SAFE-1. A forward scan of a B-link tree
reading a leaf that follows the last one in the file reads ahead
(`BufferPool::read_ahead`): on a miss, the pages after it, 32 unless the
pool is told otherwise, in one read of the file, into frames of their
own, never more than an eighth of the pool's frames; pages in the pool,
frames that cannot be had at once, and pages that cannot be taken whole
from what was read (overflowing, freed, torn) are left to the reads that
want them, so it never fails one; none during an undo. Synchronous, it
saves reads of the file, not their waits: reading ahead on a thread of
its own would be a job of the thread manager (D55). File formats are the same
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
  a commit stamps its versions, rather than writing them again.
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
  leaves the rows of the rest to the filter, as judging them twice, in
  the engine and again in the filter, cost more than it saved. Telling a
  scan every such conjunct, not only one, lets it skip the chunks and
  zones any of them rules out: a conjunction, a range, an IN list, IS
  NULL of nulls kept together, a LIKE prefix of a column in the rows'
  order, and an OR of ranges. An index is probed by the same comparisons,
  LIKE prefixes, IN lists and ORs of one column's ranges (D34).
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
  An insert undone, its row taken out, leaves its zone stale alike. So a
  zone is never narrower than what a snapshot may read. Kept in memory,
  each zone locked alone, and with the table's pages: each checkpoint,
  writers held, saves the zones changed since the last, by segments of
  64, each a page sealed as the table's rows are (`zones/saved.rs`), its
  meta page naming their directory; a table opened reads them, with no
  pass over its rows, and recovery widens them as it writes. A segment
  its encodings cannot seal or open, as they are not registered, and the
  zones of row ids none holds, are not known until summed up. A range or
  IN list of a column in the rows' order, or IS NULL of nulls kept
  together, reads only a few blocks; a scan nothing skips, and an insert,
  pay a little for keeping the zones.
- A row store table's count of rows, those whose newest version is not
  a deletion (`rows/counted.rs`), is kept with its pages too: each
  checkpoint, writers held, writes it into the table's meta page if it
  changed, so the pages captured hold the count of the versions they
  hold. A table opened from a shadowed file, which holds after a crash
  exactly what its last checkpoint captured, reads it with no pass over
  its rows; recovery moves it as it writes each row's newest version, by
  whether the version replaced was a row, so a change written again that
  the pages already hold moves it not at all, and the count is exact
  after a crash as after a close. A file not shadowed may hold pages
  written after its last checkpoint, so a table opened from one counts
  its rows by a pass.
- The planner prices a scan by the share of the rows the conditions it
  will be told may match (`TableStorage::scan_share`, `logical::pushed_conjuncts`,
  the executor's same): the row store's zones' rows they may match, of
  256 zones at most, evenly apart, so planning costs no more as a table
  grows; the column store's frozen chunks and every row of its tree. A
  range of a column in the rows' order then runs by a scan that skips
  what it cannot match, faster than by the index; a range of a column of
  scattered values keeps its index. The filter over such a scan keeps,
  as estimated, its conditions' share of the table's rows, but never more
  than the scan reads.
- The planner asks for statistics through a trait, every answer optional:
  rows, widths, distinct values, nulls, selectivity, of a sample of each
  column's values, a character string sampled by its first 64
  characters and a bit string its first 512 bits, so long values take no
  more memory than short ones. The row store
  gathers them again, once a table has changed enough, as the database's
  maintenance (below), not in the commit that made it due, as a pass in
  the commit made the commits that fell due for one far slower. A table
  of a database opened has none until first asked for; asking makes them
  due, and notes the table for maintenance, which gathers them, so no
  plan waits for a pass over a table nor makes one, and planners asking
  at once cause one pass.
- The column store: the tree is the table's delta, every row written
  there as in the row store. Once COLUMNSTORE.FREEZE_ROWS rows past the last group are committed
  before every snapshot (and COLUMNSTORE.FREEZE_DELAY seconds ago), the
  database's maintenance thread freezes up to COLUMNSTORE.GROUP_ROWS of them,
  2^16 by default, into a group (`engine/columns`): its row ids, and each column's values
  in chunks of a page each, each chunk's least and greatest values and
  nulls kept (zone maps), and those of each 1024 rows of it (a part,
  `group.rs` PART_ROWS), so a scan skips a compressed chunk's rows as
  finely as a plain one's, every page sealed by the table's ENCODINGS, its
  number the context. A chunk is written in whichever encoding takes
  fewest bytes, and holds as many values as its page does so: plain, a
  constant, a frame of reference of 8-byte fields (any such field read as
  a signed integer, so every type of that width), each difference
  bit-packed in the bits its span needs (`columns/packed.rs`), runs of
  one value of such fields, each run's value and end bit-packed, a null
  continuing the run before it (`columns/runs.rs`), or a dictionary, each
  value's code bit-packed in the bits its entries need
  (`columns/encoded.rs`: of 12 texts, 4 bits a code, not a byte), or, of
  approximate numbers, each as n / 10^e, n bit-packed as a frame's, e of
  the first 64 values the one leaving fewest exceptions, kept as they
  are, n / 10^e checked to be the value bit for bit (ALP, Afroozeh et
  al., SIGMOD 2023; `columns/alp.rs`: no cast, integers made exact up to
  2^53; read a run at a time as a scan's vector of approximate numbers,
  `Floats`, faster than a value at a time); or, of VARCHAR texts, each
  as one-byte codes of a table of at most 64 symbols of 1 to 8 bytes, a
  byte of none escaped (FSST, Boncz, Neumann and Leis, VLDB 2020;
  `columns/fsst.rs`): the table chosen once of a group's column, of 16
  runs of its texts taken evenly through it, as every n-th text misses a
  pattern's kinds, in FSST's five rounds, symbols counted packed in a
  word, no allocation, and held in each chunk, 576 bytes at most; decoded
  a symbol a word of 8 bytes at a time, a range checked UTF-8 at once,
  each end on a character's boundary, so texts take far fewer pages and
  scan about as fast, a load and freeze paying a little for choosing the
  table; COLUMNSTORE.COMPRESSION = NONE keeps them plain. Unpacked a
  value's 8 bytes at a time, a column of numbers held in memory scans a
  little slower than of whole bytes, as its pages are half as many or
  fewer. Rows of numbers of few values or near one another, and of texts
  of few values, freeze into far fewer pages than plain, and scans run
  as fast, a value read at a time either way. A group's rows are in row
  id order, or sorted by the columns COLUMNSTORE.ORDER names (at most 8,
  ascending, nulls as ORDER BY puts them, row ids breaking ties) as they
  are frozen or written again, its ids then kept in its rows' order and
  its rows' positions by id, so a row is found by its id: a few rows
  counted by a scattered column run much faster sorted by it, as the scan
  skips the rest (`bench/analytics.rs` compares them), a column the rows
  were in order of slower, a fetch by key a little slower. A group's
  header records the columns its rows are sorted by; once the option
  names others, the maintenance thread writes each group again sorted as
  it says, two a call, until none is left, after a restart too. An ORDER
  BY of the first columns the option names, each ascending, is answered
  by a scan in their order, no sort planned (`TableStorage::scan_ordered`,
  `frozen/ordered.rs`): each group's rows a vector at a time, only the
  runs a condition may match, those the tree has an entry for left out,
  and the tree's rows the reader sees sorted, merged by a heap, the least
  of their keys as ORDER BY sorts them first; a group sorted otherwise,
  as the option changed, read whole and sorted, and a scan the plan asked
  for that its table no longer gives in order read and sorted. So the
  sort, which cost more than the scan, is left out, all but the merge's
  work. Of the columns COLUMNSTORE.BLOOM names (at most 8), each part of
  each chunk keeps a Bloom filter (`columns/bloom.rs`): 1280 bytes, 10
  bits a row, each value not null setting 7 bits of its bucket
  (`value::bucket`, the form every two values that compare equal share)
  hashed by MurmurHash3 x64-128 (D30), the i-th h1 + i × h2 of its
  halves (Kirsch and Mitzenmacher, 2006), so 1.5, 1.50 and 1.5E0, and 'a'
  and 'a  ', set the same bits; filters are packed on pages of their own,
  read only by a scan of an equality or IN of the column, after its
  parts' bounds, so a lookup of a column of distinct values in no order,
  which bounds cannot narrow, reads only the parts that may hold it,
  values absent taken as present some 0.8% of the time, as 10 bits and 7
  hashes give. A group whose filters are of other columns than the option
  names is written again as for ORDER. An option naming
  columns (`Kind::Columns`) names the table's, and a column it names is
  not dropped. Groups are immutable, their row ids' ranges apart,
  found through a directory the meta page names, which keeps of each its
  summary (`group/summary.rs`): its row ids' range, rows, the columns it
  is sorted by and keeps Bloom filters of, and each column's nulls and
  least and greatest values; that is all a table holds of a group in
  memory, so a scan passes over a group no condition may match unread.
  A group's header is read as a scan or a fetch needs it, and held while
  it was read recently (`columns/cache.rs`): a store's tables hold at most
  `HEADER_CACHE_BYTES` (16 MiB) of headers, reserved of the memory budget
  a MiB at a time, the one read longest ago given up first, and one the
  budget has no room for read each time; so a header read often is not
  read and decoded again for each scan and fetch. A row's newest state
  is its entry in the tree if it has one, else its group's values, which
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
  again and the column store's freezes, rewrites and entries taken out,
  so no transaction waits for it, and the slowest commits are no longer
  those that fell due for such work. A transaction's end notes a table
  with work due and asks the database's maintenance thread, started the
  first time; it has each table noted do
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
  their width, or of its file's pages in use, if more: the tables and
  indexes of one file share its pool), 10% more as all the rows are
  frozen. The weights follow
  the costs `bench/analytics.rs` compares: a row costs a scan little
  more for each column read; a fetch through an index costs several
  times a row scanned, and far more once the pool no longer holds the
  table. A fetch priced too cheap had the planner take an index for so
  many rows that a scan was faster; priced too dear, a scan where the
  index was faster.
- Batches (`TableStorage::scan_batches`): a scan's rows by vectors of up
  to 2048 of each column's values, in no particular order, by default its
  rows gathered. A column's vector is of values, or, of an exact number a
  store keeps as an integer, of those integers, its nulls beside them
  (`storage::Column`, `Ints`), its values made only the first time work
  asks for them: a run of a frozen group, read whole or of the rows the
  tree has no entry for, gives an exact column's integers decoded
  straight from its chunks, each chunk's span asked of it at once, each
  whole byte's width of a frame of reference read by a loop of its own
  and other widths unpacked, a dictionary's entries of 8 bytes read once
  and its codes unpacked (`Body::ints`), and of a vector of integers
  COUNT, SUM, AVG, MIN and MAX are fed, and a column compared with
  constants, IS NULL and IN judged, as integers, scaled once to one
  scale, a running sum checked a value at a time as decimals check it,
  so no value is made of each row. An approximate column's vector, of
  such a run, is of its numbers as `f64`s (`storage::Floats`), each
  chunk's 8-byte fields read as an exact column's are, its bits the
  number's, and of ALP a run at a time: of it
  COUNT, SUM, AVG, MIN and MAX are fed, SUM adding in the order a row at
  a time does, so the same total, and checked as `arith` checks it, a
  sum out of range given again a value at a time so it raises alike;
  comparisons with constants, IN and IS NULL judged of it, an exact
  constant compared as `value::compare` compares it (`judge/floats.rs`),
  and grouping keys made of it, again no value made of each row. A
  VARCHAR column's vector, of a run of a frozen group, is its text, one
  buffer of every
  row's and where each ends (`storage::Texts`), decoded straight from
  varying, dictionary and constant chunks, each checked UTF-8: LIKE a
  constant pattern, comparisons with constant strings, padded as strings
  compare, IN, IS NULL and COUNT are judged and fed of the text, and
  grouping keys made of it (`judge/texts.rs`), no value made of each
  row. Of runs whose chunks are all dictionaries, it is their entries and
  each row's entry among them (`storage::Coded`), a condition then
  judged once of each entry and a row kept by its entry's, so a text is
  judged once, not once for each row that holds it; making a group's key
  of an entry once, not of each row, saved nothing, its hashing the
  cost. An expression of integers and exact constants, negated, summed,
  subtracted or multiplied, is evaluated as integers at the scale
  decimals give it, a result past 18 digits, or a quotient, evaluated of
  values, which raise of it (`vector/ints.rs`). Grouping makes an
  integer's key as its value's (`spill::put_norm_exact`), reads a batch's
  values in place (`storage::Cell`), and gathers each group's rows of a
  batch in order, a counting sort into one buffer, to feed its set
  functions a vector at a time. A column store table gives each frozen
  vector straight from its chunks, a condition's columns read first and
  the others only of the rows it keeps; the tree's entries in its
  group's range of row
  ids, read once as the group is taken up, in their rows' place; then the
  tree's other rows. A subtree of a plan gives its rows by batch (a
  stream, `exec/batch/stream.rs`) if it is a table's scan; a filter of
  one, told the simple conjuncts of its condition, or of another stream,
  each batch with the rows kept of it; or a join of streams, each column
  its keys and conditions read held by their batches
  (`exec/batch/joined.rs`): its right operand read first, of its batches'
  rows if its tables read by batch (a column store's), else row by row.
  An inner hash join, and a LEFT outer join on keys whose condition
  beyond them cannot raise an error and whose operands have no condition
  of their own (D15), build it as the row path builds it; held in the
  work area, each of the left's batches probes it, a batch of joined rows
  made of each, the left's columns then the right's, the condition judged
  of each pair of equal keys, of a row of the pair's values, a LEFT
  join's left row no right row matches joined with nulls; partitioned, an
  inner join's joined rows made row by row and gathered; a LEFT join's
  right operand, outgrowing the work area, moved to a table of the spill
  area, so every left row probes it in order. A join's count or grouped
  sum so runs much faster than row by row, of either store. Any other
  outer join (RIGHT, FULL, a LEFT one whose condition can raise or whose
  operand has a condition of its own), and a product, join their
  operands' batches' rows as the row path joins rows, and gather the
  joined rows into batches, so their operands are scanned and filtered
  by batch. An
  aggregation whose set functions count rows or take a column, none
  DISTINCT, grouped by columns the stream holds or not at all, of a
  stream, feeds them from the vectors (`exec/batch.rs`), each a column's
  values, or an expression's evaluated as a row's is, of a row of only
  the values it reads, no subquery in it; one of columns and constants,
  negated and in arithmetic, no deeper than 16, a vector at a time, each
  node once over the batch, a value at a time as a row's
  (`exec/batch/vector.rs`), or the batch's a row at a time if one raises
  an error, so the error is the row's; of no GROUP BY a vector at a time
  (`Accumulator::update_many`, `update_rows`, each by default `update` a
  value at a time): COUNT counting, SUM adding exact values as decimals,
  nothing cloned, MAX and MIN cloning only the best. A filter that
  cannot raise an error has its conjuncts comparing a column with a
  constant, testing it IS NULL or IS NOT NULL, IN or NOT IN a list of
  constants, or LIKE a constant pattern, split once and matched by bytes
  if the string is ASCII, judged over the vectors, the LIKE last
  (`exec/batch/judge.rs`), much faster than a row at a time, of either
  store. An IN list of IN_SET_LEAST (8) constants or more is judged by a
  set of their buckets (`value::bucket`), made once as the plan is made,
  over vectors and row by row alike (`logical/sets.rs`, `Cond::In`): a
  value is looked for in its bucket and compared as SQL compares, numbers
  by algebraic value and strings padded, a string's text looked for
  without its trailing spaces; true if one equals it, else unknown if it
  or one of them is null, else false, as the OR of their equalities is.
  A shorter list is compared one constant after another, which costs no
  more. The rest of it is judged of the rows they keep, each made a
  row; one that can raise is judged whole a row at a time, as row by
  row. A projection of a stream whose expressions read only its columns,
  no subquery among them, every table of which reads by batch
  (`reads_by_batch`, a column store table's), makes each row it gives of
  the vectors, a column or a constant as it is, any other expression
  evaluated as a row's is of a row of only the values it reads, kept and
  written over, a batch read ahead of what a cursor is given; of any
  other table it reads a row at a time, as reading ahead would gain
  nothing (`exec/batch/project.rs`). Groups are held in the work area,
  given in the order their first rows came; past it, the rows of every
  group not yet held are written, numbered, as a pass of `exec/group.rs`
  writes them, grouped as it groups them, and merged with the groups
  held by number. Each row's grouping key is made in a buffer kept
  (`spill::put_norm`). Against the row store read row by row, a column
  store table's counts, sums, filters and groupings by batch run far
  faster (`bench/analytics.rs` compares them). A projection of columns
  of a stream is a stream, its batches those columns' vectors, moved; so
  is one of other expressions of its columns, no subquery among them,
  each evaluated of the rows a batch keeps only, as integers or of
  values a vector at a time where it can be, else a row at a time
  (`project::computed`), a batch of which one raises an error evaluated
  again a row at a time, in order, so the rows before its failing one
  come first; so a DISTINCT of expressions is grouped by batches. So
  are the operands of a UNION ALL, each holding every column of its rows
  alike, read in turn, a chain of them gathered without a frame each. A
  DISTINCT of a stream holding every column of its rows is that grouping
  by every column, no set function (`batch::distinct_grouping`): its groups
  are DISTINCT's rows, in the order their first came, past the work area
  too; a UNION, a DISTINCT of its operands appended, too. The setting
  EXECUTOR.BATCHES, a session's too, ON by default, reads by batches
  where a query can; OFF, a row at a time throughout, as to compare the
  two, as the tests of batches do, each answer read both ways.
*Why:* a row store suits SQL's point reads, updates and cursors, and is
the base the other layouts build on. Logical row ids stay stable, so
indexes never need rewriting for them, frozen or not. Columns suit scans
of a few columns of many rows; keeping new rows as rows keeps writes and
key lookups as fast as in the row store, the design of SQL Server's
deltastore, HyPer's Data Blocks and TiFlash's delta tree. Against the
row store, a column store table counts, sums and filters by a column in
the rows' order much faster (`bench/analytics.rs` compares them); one
row fetched by key is a little slower, as each column is a page of its
own.

**D34. Index methods.** An index method is a plug-in that creates indexes
and says which probes they answer. Two are built in, both in pages through
the buffer pool, so an index larger than memory lives in its file:
- `btree`, a B-link tree keyed by the encoded key then the row id: ordered,
  for equality and ranges. A lower and an upper bound of one column, as
  BETWEEN gives, are one probe (`Probe::Range`), so the keys past the
  upper are not read, and a short range costs its own keys, not the rest
  of the index (`bench/cursors.rs` compares them). A method says it
  answers ranges by `IndexMethod::supports_range`.
- `hash`, a linear hash table (Litwin, 1980) keyed by the encoded key:
  equality only, growing and shrinking a bucket at a time, readers locking
  nothing.
A search gives its entries as it reads them (`Index::search`, an iterator
holding its index, so a cursor may outlive the call): the B-link tree's a
leaf at a time, backward for descending keys; the hash table's one key's,
or, for a probe of every entry, a bucket at a time, its shape held while
the search lives, so each entry is found once, a split or merge due
meanwhile left to the writers after. So a read of a whole index, as an
ORDER BY in its order is, holds a leaf or a bucket of it, not the index,
and the statement looks for a cancel as each entry is read (D44).
A probe value not of the column's type is placed by its anchor and each
entry found checked, so an index finds exactly what SQL compares.
The planner probes an index by the conjuncts of a filter over its table,
none of which can raise an error (D15), that compare one of its columns
with a constant, a character string, a number, a datetime, an interval
or a bit string, of the column's kind: its first columns' equalities,
then of the next column a comparison, a lower and an upper bound (as
BETWEEN gives), the bounds of a LIKE of a constant pattern (strings
beginning with the characters it begins with are between them: the
filter still judges the LIKE), or an OR of these, an IN list of
constants among them, as one probe of each of its ranges, those that
overlap or touch joined so that no row is found twice, their index
scans' rows appended in the order of their ranges, so in the index's
order, or, read backward, the last range's first: an ORDER BY of the
column sorts nothing. A number of the other kind than the column's is
placed among the column's values (`physical/probe.rs`): an approximate
number among an exact column's multiples of its scale, an exact one
among the `f64`s, the comparison turned into one with the value it
equals, or with the values on either side, so the probe finds exactly
the rows SQL's comparison by algebraic value keeps
[X3.135-1986 §5.11 GR4].
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
and is read itself. So an INSERT into a table with a PRIMARY KEY reads
again at its end only the rows whose entries were not found alone.
*Why:* each kind of index excels at different probes, and the planner
picks by estimate, so a new method needs no change to the planner.

**D35. The optimizer and planner.** Rules rewrite the logical plan; the
physical planner chooses access paths and join order by estimated cost
from the engines' statistics. Built in: hash joins on equalities, index
nested-loop joins, join
ordering of chains of products, transitive predicates, sort elimination
for ordered indexes and GROUP BY, filters pushed into views and outer
joins, conditions that can raise an error moved onto the operand of a
product they read (D15), and decorrelation of repeated subqueries. A
rewrite is made only where it cannot change a result or an error (D15).
Rules are plug-ins (`Database::with_optimizer`). Each rule, built in or
added, is turned on or off by a setting of its own (D36), OPTIMIZER and
its name upper case (`optimizer::switch_name`), ON by default, a
session's to set for itself: declared as the rule is added, a value
given before, as the database was opened again, kept until it is; a
rule whose setting would be another kind's is refused.
An index path is costed from the index method's own answers where it
gives them: the rows a probe selects (`Index::estimate`), the pages it
reads to find them (`Index::probe_pages`, a hash table's one bucket, a
B-link tree's levels), and its distinct keys (`Index::distinct_keys`, for
an equality on all its columns), counted by a sketch kept as the index is
written, of no more keys than it holds, and kept with the index's locator,
so a checkpoint records it with the pages it commits and recovery's mending
changes it as it changes the entries; an index whose locator holds none
has none until first asked: asking makes the count due, and the
database's maintenance counts its keys once, in a pass that streams
them (`Index::maintain`), planning meanwhile estimating without them, as
a table's statistics are gathered (D31): no plan waits for a pass. Else
from the table's statistics, if its engine has them; else by the
defaults PLANNER.EQUAL_SELECTIVITY, RANGE_SELECTIVITY and NULL_FRACTION
give, as a table of a database just opened is planned until maintenance
gathers its statistics (D31). A filter over a scan that skips the zones
or chunks its conditions cannot match (`TableStorage::scan_share`, D31)
is estimated to keep no more rows than the scan reads.
An inner or LEFT outer join on equalities whose right operand reads a
table, filtered by a condition that cannot raise an error or not, or
through a projection of its columns, as a view of them does (or a view
of such a view), is an index nested-loop join (`Plan::IndexJoin`,
`physical/indexed.rs`) when
an index of the table has first columns the equalities give values of,
of the indexed column's kind (so the index places them among its keys),
its later columns NOT NULL (an index holds no key with a null), and the
left operand's estimated rows times a probe's cost (its pages, a level
of a descent each, and the rows it finds, a fetch each) is less than the
cost of the read the hash join's right operand would be; so is a RIGHT
outer join whose left operand reads a table so, the right's rows probing
it, under a projection that puts the left's columns first. A projection
of another expression is not probed: its value of a row given nulls
would not be null, and it would be evaluated of fewer rows. The rows of
a left operand that is itself a join are estimated by the distinct
values of the columns each of its equalities compares, one pair in as
many as the more distinct has values (a table's rows, as if a key, when
its statistics do not count them), as join ordering takes them. Each left row
probes the index for its values as it is pulled, a null finding nothing,
nothing held; the engine is told each probe's rows are read
(`TableStorage::read_by_condition`); the rest of the condition, the
equalities not probed and the table's filter are judged of each pair
found, so none may raise an error (D15); of a LEFT join, a row none is
kept of is kept with nulls; the statement looks for a cancel before each
probe and as each entry is read (D44). Its rows are in the left's order,
each row's matches in the index's. Join ordering weighs a relation's
probes beside a hash join, so a few rows are joined first to the table
they probe.
*Why:* the General Rules define results, not plans; any plan that gives
the same results and errors is the optimizer's to choose.

**D36. Settings.**
- One registry: each setting declared once, with its kind, default, whether
  a session may set it, and when a change takes effect: for each statement,
  each transaction, or when what reads it starts, as a server's addresses.
  Plug-ins declare theirs the same way, the server too (D42): every
  component is configured by settings, never by a configuration of its own.
- A storage engine declares its own (`StorageEngine::settings`), under
  its name, as it declares its tables' options (D41), and is told the
  database's values (`StorageEngine::configure`) as it is registered, after
  each change and once the database is opened, its values replayed: from
  a snapshot, with no lock held, so it never calls back into the database
  to read them (R-ERR-4). The column store so bounds the headers of its
  frozen groups it holds (COLUMNSTORE.HEADER_CACHE_BYTES).
- A session turns for itself, over the database's value: whether its
  commits wait for the log (LOG.COMMIT_SYNC, D25), whether it takes the
  function call syntax (SQL.FUNCTION_CALLS), and each optimizer rule
  (D35).
- Scopes: a built-in default, the database's value, a session's own value.
- A number an operator tunes a subsystem by (a size of memory or a pool,
  a threshold, a pace of background work, a timeout, a durability) is a
  setting, the constant it was named by its default. These stay
  constants: formats on disk, the standard's implementation-defined
  values (CONFORMANCE.md), the shapes of structures fixed when they are
  made (stripes, shards, vectors), and the planner's weights an engine
  states of its own costs, measured by its benchmark.
- Names: each setting is of a group, `GROUP.NAME`, a plug-in's under its
  own name; a quantity names its unit (`_BYTES`, `_MS`); a word means one
  thing throughout; choices take one vocabulary (`ON`, `OFF`, and words of
  their own only where two will not do). During alpha a name is changed
  in place (VERSIONING.md).
- A setting's effect is the one declared: one taken for each transaction
  is taken as the transaction begins; one that waits for a restart says
  so where settings are shown. A database kept in memory refuses those
  fixed at creation.
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
  reservation past the budget is refused (SQLCODE -918). A page wanted
  while every frame of its pool is held is refused too, -918, waited
  for only a moment, while frames latched by a read let go (D30):
  the pool, below every engine (D30), knows no
  statement to bound a wait by, and threads each holding pages while
  waiting for one more would wait for each other; the pool is too small
  for the work.
- What undoes work is never refused by the budget: a failed statement's
  undo, and a ROLLBACK's, with the collection after it (`Undoing`),
  reserve what they need even past the limit, until the undo gives back
  what it held; a page they want while every frame of its pool is held
  is waited for, up to ten seconds, as every other holder is refused and
  lets go and an undo holds a few pages at most. A refusal would damage
  a table (D19). Nor does reading a spilled list back need the budget: a
  run's last batch, not yet written, is read where it is held, so a
  commit's check reads its changes without a page more.
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
  settings it set over the database's (D36); a row counts its values and
  what each holds apart from itself (a string's characters, a bit string's
  or a plug-in's value's bytes). Past that it spills to an area of as many bytes, reserved of the
  budget: a temporary pager file whose buffer pool is the area, so
  eviction writes it to disk; its creation not made durable
  (`Pager::create_scratch`), as nothing reads it after a crash, so a
  sort that spills waits for no sync. In the area it makes tables (byte
  keys to values, in the order added) and runs (records read back in
  order).
  Areas come from a provider, an extension point (`Spill`), given to
  plug-ins through the `Host` too, so one may give partitioned or parallel
  areas.
- Spilling keeps the orders the planner relies on: a sort is an external
  merge, stable; the rows held, or each run, sorted by a prefix of the
  first key, a number in the key's order (of a number, a string, a
  datetime by its instant in UTC, an interval or a bit string), beside
  each row's place, those of equal prefixes compared whole, or by the
  other keys only if every prefix holds its value whole
  (`exec/sort/prefix.rs`), faster, as the rows themselves, each in memory
  of its own, are no longer read for every comparison. A run writes each
  row beside its prefix, which the merge compares, reading each run a
  batch at a time (`SpillRun::into_batches`), each record where it lies,
  and decoding a row only when prefixes tie, or a value has none, and as
  it is given; a pass that writes a run again copies each record as it is
  (`exec/sort/merge.rs`). Grouping and DISTINCT give their rows in the order
  their first rows came. A hash join is partitioned (Grace), its partitions
  partitioned again while they outgrow the work area, its left rows
  numbered so the joined rows are merged back into the left operand's
  order. Grouping, DISTINCT, EXCEPT and INTERSECT partition so too, by
  their keys, once what they hold outgrows the work area: DISTINCT gives
  the rows it held as they came, then the rest's first of each key, merged
  by number; EXCEPT and INTERSECT partition both operands alike, the
  second's rows as counts; grouping numbers each group by its first row.
  Each partitions a partition again while it outgrows the work area, as
  many times as SPILL.PARTITION_DEPTH says; past that, DISTINCT, EXCEPT
  and INTERSECT keep its keys in a table of the spill area, and a hash
  join, as for a partition that does not shrink, its right rows, each probe
  reading its key's rows from the table as it joins them
  (`SpillTable::values`), never all at once. DISTINCT, once it partitions,
  and EXCEPT and INTERSECT but for ALL split their work area in two, as a
  grouping of DISTINCT set functions does: half for the partitions and the
  keys each holds, half for the bag beside them, of a partition's keys past
  SPILL.PARTITION_DEPTH, or of the rows given already.
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
  left's order, then the right's rows that matched none, in the right's;
  its partitions partitioned again as the hash join's are. One that does
  not shrink, or is past SPILL.PARTITION_DEPTH, is joined a part of the
  work area's size at a time, the left partition read again for each part,
  whether each left row matched kept in a run beside it. A right operand
  held in memory has a flag for each row, whether it matched, counted in
  the work area with it.
  A product, and an outer join on no keys, read their right operand again
  for each left row, from a run of the spill area once it outgrows the
  work area; then, of a preserved right, the outer join keeps no flags but
  the left rows, in a run, and tries each right row with them again.
- A subquery's rows spill as a product's right operand does, and a
  decorrelated subquery's, kept by key, as a hash join's; the rows kept
  for each set of outer values are kept while they fit the work area in
  all, each counted with its key and entry, rows held in memory twice, for
  what a comparison makes of them, and rows that spilled as their area, so
  they are kept only alone; a subquery's with no outer references always,
  as they are its only run. The values a DISTINCT set function took, of
  every group of a grouping pass, are keys of one bag, which spills: the
  grouping's work area split in two, half its groups', half the bag's. IN, NOT IN, ALL and SOME of a row,
  nulls in it or in the subquery's rows or not, are answered from what is
  made once of those rows: for each pattern of nulls they have, a hash of
  them on the columns neither they nor the row has null; each column's
  least and greatest value; and, by the place of a row's first null, the
  least and greatest of the values before it. Never by comparing the row
  with every row.
- The rows a statement holds past its operators are held up to
  WORK_MEMORY too, then written to runs of a spill area, in a list that
  spills (`spill/list.rs`, `exec/held.rs`): a read-only cursor's rows read
  at OPEN, its plan able to raise an error, or read before its
  transaction changes a table it reads, given back in order, each run
  read once, the first error read after the rows before it; a SCROLL
  cursor's rows as it reads them, read back by place a run at a time; an
  updatable cursor's row ids, in its order, its rows sorted as ORDER BY
  sorts them; the object rows of a searched UPDATE or DELETE, with an
  UPDATE's new values, and the candidate rows of an INSERT, each chosen
  before any row is changed; what a data-changing statement did, each
  row's id and an UPDATE's or DELETE's old values, and the checks of its
  constraints gathered of it, kept for its end, the rows of a UNIQUE
  constraint checked one at a time as they are read back; and the checks
  of deferred constraints, kept until they are made, the transaction's
  end freeing them. A scan planned in an order its table no
  longer gives is sorted as ORDER BY sorts.
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
  greatest value. Each of these lists spills alike (`spill/list.rs`),
  the segments it holds written together in one run, so the changes of
  many small statements take a run between them, not a page each.
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
  the time between, which is its client's. A server tells what it does
  itself through the database it serves (`Database::tell`): where it
  listens, that it stops and hurries, accepts that failed, TLS
  handshakes and logins that failed (the user, peer and mechanism, never
  the proof), those the native protocol refuses before any authenticator
  runs among them (a Hello of a protocol version not served, of a user
  name that is no identifier, told as written, or of a mechanism not
  served), each slowed and counted by its address as a proof refused is
  (D43), so they flood no faster; and connections refused past
  SERVER.MAX_CONNECTIONS; what may come in floods, at most once a minute,
  counted.
- DIAGNOSTICS.LEVEL, the database's, ERROR, WARNING or INFO (the
  default), is the least level told: an event below it reaches no sink.
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
  only but for TLS and SCRAM with its features `tls` and `scram`, for programs that connect rather
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
  macros and time; the program adds signal, LZ4, AES-256-GCM, ChaCha20-Poly1305,
  SHA-256, SHA-512 and HMAC-SHA-256 (D26, D48), and rpassword for
  a password typed at a terminal (D46). Chosen as the most used and most reviewed async
  runtime of Rust; its `unsafe` is its own, and none is written here
  (R-SAFE-1). The core never sees it.
- TCP keepalive's times are set by socket2 0.6.5 (MIT or Apache-2.0),
  with its feature `all`: the version and feature tokio already takes,
  so no code enters the tree with it. Of the Rust project
  (rust-lang/socket2), its crate owned by tokio's maintainers too, it is
  the socket layer of tokio; its `unsafe` is its own, the system calls a
  socket option needs, and none is written here. Its one advisory
  (RUSTSEC-2020-0079, an unsound layout of addresses) was fixed in
  0.3.16. Its README makes no claim of readiness beyond its support of
  platforms: Linux, macOS and Windows are tested with each change, and
  all it offers must work on them but what is behind `all`, which the
  probes' count and reading the times back are; so the server's tests
  read each time back from a socket on each of them that offers it.
- Threads. Tokio's tasks do I/O only. Statements and the end of a session
  run on blocking workers, at most SERVER.WORKERS at once; the checks of
  an authentication on blocking threads apart, at most SERVER.MAX_LOGINS
  at once, so clients logging in never keep statements waiting. A
  query's rows go from its worker to its connection through a bounded
  channel, in batches of about SERVER.BATCH_BYTES, so a client reading slowly holds a worker but never
  more than a few batches of memory. A connection runs one request at a
  time, so no more wait for a worker than there are connections.
- A client's time is bounded: it has SERVER.LOGIN_TIMEOUT (a minute by
  default), from when its connection is taken, to begin TLS and
  authenticate, the two together; and once the server has waited on it
  SERVER.IDLE_TIMEOUT (no limit by default) for a request, or to take
  what it is sent, or SERVER.IDLE_IN_TRANSACTION_TIMEOUT_MS (no limit by
  default) for a request while its transaction is active, its locks
  held, it is closed, its session rolled back (SQLSTATE 08006). A client
  that stops reading frees its worker so, as one that stops sending
  frees its connection. TCP keepalive is on for each connection taken,
  so the system finds out a client gone without closing: at its own
  times, unless SERVER.KEEPALIVE_IDLE_MS, SERVER.KEEPALIVE_INTERVAL_MS
  and SERVER.KEEPALIVE_PROBES say otherwise (0, the system's own, by
  default), read as each connection is taken (`server/src/keepalive.rs`),
  each time rounded up to whole seconds, as most systems count them,
  and bounded by Linux's limits (32767 seconds, 127 probes). Windows sets
  the idle time and the interval together, so one given, the other is
  Windows' own default (2 hours, 1 second); its probes are set from
  Windows 10 version 1703. A time a platform cannot set is refused,
  each connection taken failing, told (`accept_failed`).
- A client not yet authenticated holds little: its messages are of at
  most 16 KiB (`native::LOGIN_MESSAGE_BYTES`), and a message's memory is
  taken as its bytes arrive, never as its header declares.
- An operator ends a connection by its ID in
  INFORMATION_SCHEMA.CONNECTIONS (`Server::operator`, `Operator::end`):
  what it runs is cancelled (D44), again until it stops, and it is
  closed, told so (SQLSTATE 08006), its session rolled back. In SQL, by
  the database's owner alone (SQLCODE -104), in a transaction or not, as
  neither is a definition or logged: `END CONNECTION id` so ends one, and
  `CANCEL CONNECTION id` cancels what it runs, its session going on; an
  ID of none served is SQLSTATE 08003, which leaves the connection that
  ran it (`parser/served.rs` gives their BNF). The core reaches the server
  through an extension point, `ServedConnections`, which the server
  registers on the database it serves (R-SAFE-3).
- The client waits on its server as its `Timeouts` say: to connect,
  begin TLS and for each step of the login 30 seconds by default; for
  each reply after, no limit by default; for Terminate's close, the
  read limit, else 60 seconds, reading no more than a reply. Its replies
  are of at most `MAX_BODY` unless the program says fewer; its memory,
  too, taken as their bytes arrive. Over a named pipe only the connect
  is bounded (below).
- Extension points (R-SAFE-3): `auth::Authenticator`, a mechanism of
  authentication as an exchange of as many steps as it needs, built in
  `password`, `scram-sha-256` and `certificate` (D43) and `trust`, which
  takes only clients of the server's own machine, and only while
  SERVER.TRUST is LOCAL;
  `transport::Listener`, built in TCP, Unix sockets and named pipes; and
  `server::Protocol`, built in the native protocol.
- TLS (D26) secures an endpoint's TCP connections, as SERVER.TLS says:
  OFF, ON (offered) or REQUIRED, with the certificate chain and key in the
  PEM files SERVER.TLS_CERTIFICATE and SERVER.TLS_KEY. A client may speak
  TLS or not on the same port: the first byte tells (a TLS handshake
  begins with 22, RFC 8446 §5.1). Required, a client in the clear is
  refused (SQLSTATE 08004). The handshake runs in the connection's own
  task. A Unix socket or a named pipe needs none. An authenticator knows
  whether its peer is secure (`Peer::secure`), so one taking a secret
  refuses it in the clear. With SERVER.TLS_CLIENT_CA, the PEM file of
  authorities, each TLS client is asked for a certificate of theirs,
  verified by rustls; one presenting none is served, one presenting
  another's fails its handshake; the common name of a verified one's
  subject is known to the authenticators (`Peer::certificate`).
- TLS's files are taken again without a restart (`Tls::reload`): as the
  next connection begins TLS once SERVER.TLS_CERTIFICATE, SERVER.TLS_KEY
  or SERVER.TLS_CLIENT_CA has changed, and on SIGHUP (Unix) from the same
  files, for a certificate renewed in place. The connections after are
  secured by the new ones, those secured go on; files that cannot be
  taken leave those there were, told (`Event::TlsReloadFailed`) once a
  change, and new ones taken are told (`Event::TlsReloaded`). SERVER.TLS
  takes effect as the server starts.
- The native protocol, version 1 (`turborel_protocol`, D45): framed
  messages, a kind and a little-endian length; Hello with the version, the
  user and the mechanism; every error with its SQLCODE, SQLSTATE, message,
  rule and position; values exactly as stored. Port 7422 by default.
- A connection has one session, of the authorization identifier its
  authenticator accepts. Each statement is a `<direct SQL statement>`
  [ISO/IEC 9075:1992 §20.1] (`Session::direct`); a client creates only its
  own schema [§11.1 AR1]. A session ends as SESSION_END says when its
  client terminates it (`Session::end`), a commit that fails sent to the
  client as its error before the connection closes; otherwise, lost,
  failed or cut by the server stopping, it is rolled back [§20.1 GR2a].
- Configuration is the database's settings (D36), declared by the server
  on the database it serves: SERVER.HOST, SERVER.PORT, SERVER.SOCKET and
  SERVER.TLS when it starts, TLS's files as connections begin TLS;
  SERVER.WORKERS, SERVER.MAX_CONNECTIONS,
  SERVER.MAX_MESSAGE_BYTES, SERVER.BATCH_BYTES and the timeouts as
  statements and connections begin, so a change needs no restart.
  `turboreld --set NAME=VALUE` sets one as it starts.
- Diagnostics (D40): `log::Lines`, a sink writing each event as a line of
  JSON stamped with the time in UTC, to standard error or appended to a
  file; `turboreld --log FILE` names the file, the sink set before the
  database opens, so its opening is told too. The lines are queued,
  bounded, and written by a thread of their own, `turborel-server-log`,
  made by the server's runtime module (D55): a slow disk or a blocked
  pipe holds up no statement; a line past the bound is dropped, counted,
  and the count told. SIGHUP (Unix) opens the file again, for a log
  rotation, and takes TLS's files again; off Unix a log is rotated with
  the server stopped.
- The server's own errors are SQLCODE -919, a connection exception: SQLSTATE
  08004 for a connection refused, 08006 for one failed, 28000 for
  credentials refused [ISO/IEC 9075:1992 §22.1, Table 23].
- Portable: TCP everywhere; SERVER.SOCKET a Unix socket on Unix, a named
  pipe on Windows, refused elsewhere; Ctrl-C everywhere, SIGTERM and
  SIGHUP on Unix.
- Named pipes, on Windows (tokio's `named_pipe`, of the feature net): a
  server makes its pipe as the first instance of the name, so it does not
  start where another process holds the name, nor can another take it
  while it listens, an instance always waiting, the next made before a
  connected one is handed over; clients of other machines are refused, so
  a client is local (`Peer::local`), secure as a Unix socket's. Who of the
  machine may connect is the system's default security for a pipe: its
  creator, the administrators and the system fully, any other user to
  read only, so able to send nothing, closed at SERVER.LOGIN_TIMEOUT. A
  narrower descriptor needs the system's API through `unsafe`, which is
  not written here (R-SAFE-1). The client opens the pipe as a file, of
  std alone, letting the server identify but never impersonate it, and
  tries it again while it is busy, within its connect timeout; std sets no
  limit of time on a file's reads and writes, so once open a pipe's are
  not bounded.
- Told to stop, the server takes no connection, lets each finish its
  statement, rolls its session back and closes it; then the database is
  closed. Past SERVER.SHUTDOWN_TIMEOUT (30 s by default, 0 for no
  limit), or told to stop again, it hurries: each session's statement is
  cancelled (D44), and every wait on a client ends, so each connection
  is closed, its session rolled back; the database is then closed as
  before.
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
  (D26) and SCRAM-SHA-256's keys of it, its verifier
  `$scram-sha-256$i=N$salt$StoredKey$ServerKey`, naming its scheme and
  iterations, so verifiers made before a change of either still check.
  It keeps StoredKey and ServerKey, never the salted password, which
  would let whoever read it log in by SCRAM; one verifier serves both
  mechanisms of a password.
- The authenticator `password` takes a password only over a secure
  connection (`Peer::secure`: TLS, a Unix socket or a named pipe), as the
  password travels whole. The client's side refuses it first: a
  connection to a server over plain TCP sends no Hello of a mechanism
  that says it is secret (`Mechanism::secret`), as `password` does. A
  user not there is refused as a wrong password is, after as long, the
  check made against a decoy verifier.
- The authenticator `scram-sha-256` (RFC 5802, RFC 7677) proves the
  password without its travelling, and proves the server to the client:
  client-first in Hello; server-first, client-final and server-final
  each a Challenge or an Answer; an empty Answer, the server's signature
  checked, then Ready, which the client refuses before it
  (`Mechanism::ready`). No channel binding: a header asking for it, or
  for another identity, is refused; Hello names the user, the `n=` of
  client-first left aside; a password is its UTF-8 bytes, without
  SASLprep. An exchange overheard lets its eavesdropper guess the
  password at leisure, and one relayed by a server in the middle cannot
  be told without binding: so it is taken over a secure connection, over
  plain TCP only while SERVER.SCRAM_WITHOUT_TLS is ON (OFF by default),
  and the client's side is secret unless its program says
  (`Scram::in_the_clear`). A user not there is answered with a salt made
  of its name and a secret of the server's, the same each time as a
  user's is, and refused as a wrong proof is. The command-line client
  proves a password so (D46).
- The authenticator `certificate` takes the client whose certificate,
  verified against SERVER.TLS_CLIENT_CA (D42), has a subject of exactly
  one common name, which, read as an `<identifier>`, is the user named,
  and the user is one: `CN=ann` is ANN. One name compared whole is the
  simplest mapping that cannot be stretched; the authorities answer for
  whom they certify.
- Failed logins slow their address, never their user: a user locked out
  after failures would let anyone lock out anyone. A Hello refused before
  any authenticator runs is a failure too, its address waiting its turn
  first. After a failure, the
  next login from the address waits 100 ms, doubled with each failure
  after, up to SERVER.LOGIN_DELAY_MS (ten seconds by default), its
  logins waiting taking turns a delay apart; one accepted, or ten minutes
  without a failure, forgets them. An IPv6 address counts by its /64, a
  Unix socket's or named pipe's clients as one; at most 10,000 addresses
  are kept.
- A session lasts while its user does: a user dropped, each session that
  logged in as it ends at its next request, refused (SQLSTATE 28000),
  rolled back and closed. A new password ends none: the session proved
  who it is as it logged in, and the operator may end one that should
  not go on (D42).
- The authenticator `trust` takes a client as the user it names, of the
  server's own machine alone, and only if SERVER.TRUST is LOCAL. It is
  OFF by default, so every client proves who it is unless the operator
  says otherwise: on a machine of many accounts, any of them could
  otherwise connect as the owner.
- The database's owner is a user: the first one created, so a database
  never has users and no owner; or another the host program or `turboreld
  user owner` makes it, or the owner hands it on, in SQL by `ALTER
  DATABASE OWNER TO name` (`Session::set_owner`): checked and made in one
  change of the definitions, outside a transaction (SQLCODE -917), by the
  owner alone (-104), to a user (-103), logged and replayed as a
  definition and told (`OwnerChanged`); no session is ended, the owner's
  going on as the owner no more. It cannot be dropped until another owns
  the database. In SQL, the owner alone manages users: `CREATE USER`,
  `ALTER USER` and `DROP USER` (`parser/users.rs` gives their BNF and
  ALTER DATABASE's, in the standard's notation). INFORMATION_SCHEMA.USERS shows the owner every user, any
  other only itself, never a verifier.
- An operator sets a database up with `turboreld --dir PATH user ...`
  while no server holds it: its directory is locked by the one that does.
  A database kept nowhere has no directory to set up: `turboreld --memory
  --owner NAME` makes NAME its first user, and so its owner, as the
  server starts, its password read as `user add` reads one; else it
  would have no owner, and no one could manage it in SQL.
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
  and after the read; as a spilled operator reads back each record of the
  runs of its work area, or each batch of them, as a sort's merge does, so
  passes over partitions and runs that read no table (grouping, DISTINCT,
  EXCEPT, INTERSECT, joins, products, sorts) stop too; and
  while it waits for a lock, which wakes every 20
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
  Format, Syntax or Access Rule broken while preparing [GR4a]. A
  `<comment>` is taken, as clients and drivers send them, though SQL's
  PREPARE refuses one [GR4q] (D58); a positioned DELETE or UPDATE finds
  its cursor as it is prepared [GR2], or, none of its name being there,
  as it runs, as before. SQL text prepares by name too (D58). A scalar
  function plug-in says what type a `?` takes as each of its arguments
  (`ScalarFunction::parameter_type`), refused by default.
- A query, an INSERT, and a searched UPDATE or DELETE keep their parsed
  text and bound plan between executions, each `?` its context types a
  slot (`Expr::Dynamic`); another statement keeps its parsed text. Each
  execution fills the slots with its values, cast to their types [§17.9
  GR5], then rewrites and plans the plan: the access paths are chosen by
  its values and the statistics of then, as for the statement written
  with them, so no generic plan is run for values it does not suit
  (`db/prepared/plans.rs`, `logical/dynamic.rs`).
- What is kept holds while what it was bound with does: the snapshot's
  generation (every published change of definitions, settings,
  functions, optimizer rules); the catalog it read, by identity, which
  every change makes anew, a transaction's uncommitted ones and GRANT
  and REVOKE among them; the authorization identifier, default schema,
  time zone and own settings of the executing session, whether it takes
  the function call syntax (SQL.FUNCTION_CALLS) among them. Else the
  statement is bound again, its Syntax and
  Access Rules checked against the definitions of then [§17.6 GR10]: a
  privilege revoked is missing at the next execution. A session's
  declared local temporary tables make its catalog anew at each call, so
  their statements are bound at each execution.
- A binding that reads a value, a `?`'s other than as a slot or the
  statement's instant (CURRENT_DATE, CURRENT_TIME, CURRENT_TIMESTAMP, a
  datetime CAST, a DEFAULT of them written, in the statement or a view it
  reads; `value::datetime::Clocked`), is not kept: it is made at each
  execution. An INSERT's columns not written take their defaults anew.
  `Prepared::plans_made` counts the plans made; its clones share them.
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
  completion with a warning reaches the client (D47); the Complete that
  ends a query's rows, the warning they raised, as a set function or a
  CAST may [ISO/IEC 9075:1992 §6.5 GR1b, §6.10]. A Complete and an
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
result in parts, and must be able to stop what it started. Parsing and
binding are what a statement run often repeats for nothing; planning per
execution keeps the plan its values choose, and invalidating by what the
binding read keeps the results those of binding anew. A cancel on its
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
  or no data; a query that completes with a warning says so, with its
  SQLSTATE, after its count of rows.
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
  path beginning '/', on Windows a named pipe's name beginning '\\';
  DEFAULT is the server the command line names, the connection made as
  the program starts. A connection is proved as the command line says: a
  password typed is asked for again; one from TURBOREL_PASSWORD is proved
  again. No transaction spans servers, so CONNECT and SET CONNECTION are
  refused in an active one (0A001) [§15.1 GR1, §15.2 GR1], and DISCONNECT
  of a connection whose transaction is active (25000) [§15.3 GR6].
- Where: `--host`/`--port` (127.0.0.1, 7422), `--tls FILE` trusting that
  PEM file's certificates, `--tls-system-roots` the platform's
  certificate authorities too, or alone (D26), with `--server-name`,
  presenting `--certificate` FILE and `--key` FILE if named, or
  `--socket`: a Unix socket on Unix, a named pipe on Windows, refused
  clearly elsewhere. Every address of a host is tried until one answers;
  a refusal is not retried.
- Who: `--user`, else the environment's USER or USERNAME. A password is
  typed without its echo (`-W`), or taken from TURBOREL_PASSWORD; never
  from the command line, where other users of the machine may see it.
  It is proved by SCRAM-SHA-256 (D43), never sent. Without one, a
  certificate presented proves the user (`certificate`); without either,
  the server is asked to trust the user.
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

**D59. Import: rows from a pluggable source, over a connection.**
- The client library imports (`turborel_client::import`): `import`
  inserts the records of a `Source` into a table over a connection; the
  command-line client's `--import` is one caller of it. `Source` is a
  trait any program may implement: its headings, names and types if it
  knows them, asked once; then its records, one at a time, each a value
  for each field and where it began, its record number and, of a source
  of lines, its line. A source holds a record at a time, and refuses a
  record past a bound of its own, so an import's memory is bounded
  whatever its input's size. `import` is generic over it, no `dyn`
  (R-SAFE-3).
- Built in: `Delimited`, of two dialects. CSV as RFC 4180 §2 writes it:
  a record a line, ended by CR LF or LF, the last one's optional; an
  optional header; a field enclosed in quotes to hold the delimiter, a
  quote, written twice, or a line break; a delimiter other than the
  comma, if named. Tab-separated text: fields split at tabs, never
  quoted. A field not in quotes written as the text for null, empty by
  default, is null; a quoted empty field is the empty string. Text is
  UTF-8, a byte order mark beginning it passed over. A quote in a field
  not quoted, or anything but the delimiter after a closing quote, is
  refused, saying the record, its line and the field. A record is at
  most the protocol's longest body, 16 MiB, by default. The reader is
  written in the tree (D3).
- Rows go in through the protocol's Prepare and Execute (D45), which
  need no change: the columns are those the source's headings name,
  each read as an `<identifier>`, or else the table's, in order, as the
  description of a prepared `SELECT *` of it gives them [ISO/IEC
  9075:1992 §17.9 GR3]; one INSERT is prepared, a `?` for each column,
  each taking its column's type [§17.6 GR5]. The server casts each value
  to it [§17.9 GR5f, g], but a string for a character string column,
  which the client assigns as INSERT does [§13.8 GR4b iii, §9.2 GR3b,
  GR3e], so one too long is refused (22001), not cut with a warning as
  CAST cuts it [§6.10 GR5c ii, GR6c ii]. The table's name is read as a
  `<table name>` and sent as delimited identifiers, so nothing else is
  taken into the SQL. A row is one Execute, answered before the next.
- An import begins with no transaction active, as it commits: one
  active is refused. Its rows are committed a batch at a time, 1000 by
  default, all at once for 0. A row refused, by the server or as too
  long, or a record of too many or too few fields, stops it: the batch
  under way is rolled back, and the error says the record, its line,
  why, and how many rows were committed. Told to skip bad rows, it reports such a row and goes
  on, if the row alone was undone: the transaction is still active, or
  had no rows. A connection lost, a statement cancelled (D44), or a
  record the source cannot read stops it all the same.
- The command-line client (D46): `--import FILE`, an input in turn with
  `-c` and `-f`; `--table`, `--format csv` or `tsv`, `--header`,
  `--delimiter`, `--batch`, `--skip-bad-rows`; `--null` gives the text
  both shown for null and read as null. A row skipped fails the run.
*Why:* operators load data that other systems write, CSV above all. A
source as a trait lets any program import any format without the client
knowing it; the built-in reader is short enough to write and review in
the tree. A prepared INSERT keeps the protocol as it is and puts the
server's casts and constraints on every row; batches bound what a
failure rolls back, and a record at a time bounds memory.

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
aes/src/                encryption (AES-256-GCM, ChaCha20-Poly1305) and cryptographic hash functions (D26, D48)
support/src/            fixtures shared by the crates' tests (R-TEST-10)
test/integration/       the integration test binary: public API only
  main.rs               crate root and module list
  common.rs             shared fixtures and helpers
  <area>.rs             one module per area
bench/                  benchmarks, run by `cargo xtask bench`
  harness.rs            the harness every benchmark uses
xtask/                  the project's tools: verify, coverage, trace, bench, mutate
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
| R-SAFE-5 | No thread **shall** be started but through the thread manager (`src/threads.rs`, D55), and none unnamed. | xtask; review |

---

## 4. Test rules

### R-TEST: levels and layout

| ID | Rule | Checked by |
|---|---|---|
| R-TEST-1 | Each test **shall** go at the lowest level that can show its requirement: unit (`#[cfg(test)] mod tests`, private logic), integration (`test/integration/`, the public API only) or doc (examples in `///` docs). | review |
| R-TEST-2 | Tests **shall** be derived from requirements, not from the code, with normal-range cases (boundaries included) and robustness cases (invalid inputs, with the exact SQLCODE expected). | review |
| R-TEST-3 | A test's name **shall** say the behaviour checked; an integration test may end in its clause: `fetch_targets_8_6`. | review |
| R-TEST-4 | Test code may `unwrap`, index and panic: a panic is a failure. Every other rule applies. | lints |
| R-TEST-5 | Tests **shall** be deterministic and independent: no clock, randomness, network beyond the machine's own loopback (the server's tests, D42) or order dependence, and each test builds its own `Database`. A generator of test cases is pseudo-random from a fixed seed, so each run makes the same cases (`test/integration/random_queries.rs`). | review |
| R-TEST-7 | Every defect fixed **shall** get a regression test that fails without the fix, saying so in its doc comment. | review |
| R-TEST-8 | For each decision of two or more conditions in new code, the tests **shall** show MC/DC, their doc comments naming the condition pairs. | review |
| R-TEST-9 | The tests **should** be checked by mutation now and then: break a mechanism, and expect the named tests to fail. `cargo xtask mutate [NAME]` applies each mutation of `xtask/src/mutate/list.rs` (a file, a snippet found in it once, its replacement, the tests that must fail) one at a time, runs its tests, writes the file back, and reports a mutation no test caught or a named test that passed; a change to a mechanism the list breaks runs it, and a new mechanism adds a mutation. | `cargo xtask mutate`, periodic; `verify` checks each mutation still applies |
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
