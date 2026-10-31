# turborel rules

How turborel is designed, written and verified. Every change follows these
rules; `cargo xtask verify` checks those a tool can check, and review
checks the rest.

- **Rules** (R-…) say how code and tests are written. **shall** is
  mandatory: a departure needs a recorded deviation (§6). **should** is
  expected: a reviewer may accept a reasoned exception.
- **Decisions** (D1…) record how the system is built and why. Change one
  only by changing this file in a reviewed commit.
- **Citing.** By ID, in code, tests, reviews and commit messages:
  `RULES.md D29`, `R-ERR-2`.

**Before changing code**, read §3 (code rules), §4 (test rules) and the
decisions covering your area (§1). What turborel implements of the
standards is in [CONFORMANCE.md](CONFORMANCE.md); open work is in
[TODO.md](TODO.md).

The rules serve as the *Software Code Standards* and the core of the
verification approach DO-178C expects (§8).

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
| D15 | Whether a run-time error is raised does not depend on the plan |
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
`turborel`. Its first dialect is ANSI X3.135-1986; later ISO SQL
standards extend it (D32).

*Why:* the engine is the product; a standard is one stage of it.

**D2. DO-178C Level A objectives are the target,** where tooling allows.
No certification is claimed.

*Why:* only Level A requires MC/DC; writing to it from the start beats
retrofitting.

**D3. No third-party dependencies,** in the library, client or tools,
except:
- cryptography in plug-ins (D26), in the client and protocol only with
  their features `tls` and `scram`;
- the platform's certificate authorities, in the client only with its
  feature `system-roots` (D26);
- compression in plug-ins (D48);
- tokio and socket2 in the server crate (D42);
- tokio's signals in the command-line client;
- rpassword in the command-line client and `turboreld` (D46).

*Why:* every line is reviewable; no supply chain needs qualifying.

**D6. Production code only.**
- **Scope.** The library is the core; the server serves it (D42). No demo
  program, `examples/` or golden-file test.
- **Plug-ins.** Built-in ones are what a production database ships; a
  test needing another uses a test-local one.
- **`bench/`** measures; it is a tool.

*Why:* a toy costs upkeep and hides what is real.

**D23. A manual of what is settled; no design documents yet.**
- **Documents.** The README introduces; RULES.md, CONFORMANCE.md and
  TODO.md are the working documents.
- **Manual.** `doc/` (`doc/README.md`), laid out as a system's manuals
  were: User's Guide; SQL Reference Manual, a page per statement (NAME,
  SYNOPSIS, DESCRIPTION, RULES, PRIVILEGES, ERRORS, EXAMPLES, STANDARD,
  SEE ALSO); Operator's Guide; Client Reference; appendices. Only
  settled, user- and operator-visible behavior; unwritten sections say
  so.
- **Internals.** That guide waits for the design to settle; meanwhile
  public items need doc comments (R-DOC-1).

*Why:* documents written before a design settles go stale; doc comments
live beside their code. What users see settles first, and a manual laid
out whole from the start keeps each part in place.

### Tooling and process

**D4. Tests in Rust's standard levels:** unit, integration and doc
(R-TEST-1), as Cargo and the Rust Book lay them out.

**D5. One integration test binary for each crate tested from outside,**
a module per area:
- `test/integration/main.rs`: the library;
- `server/test/main.rs`: the server and its client together;
- `turboreld/test/main.rs`: the server's program.

*Why:* one link step, and fixtures shared without `mod.rs` tricks.

**D7. Automation in Rust,** as `cargo xtask`, never shell scripts.

*Why:* portable to every platform Rust supports, and reviewed, linted and
tested like the product.

**D8. The toolchain is pinned** to 1.99.1 in `rust-toolchain.toml`.

*Why:* the compiler is a configuration item (R-CM-1).

**D9. Formatting:** `rustfmt.toml` sets `max_width = 140` and
`use_small_heuristics = "Max"`, the width that changed existing code
least.

**D10. Lints:** clippy `pedantic` plus the restriction lints in
`Cargo.toml`, thresholds in `clippy.toml`, enforcing R-ERR, R-NUM, R-CF
and R-SIZE mechanically.

**D11. Lint baselines only shrink.** Code predating a rule may carry a
file-level `#![expect(...)]` listing what it still breaks. None does
today.

**D12. Overflow checks in release builds** (`overflow-checks = true`), so
they behave like tested builds (R-NUM-1).

**D13. `doc_markdown` is allowed,** so citations such as `GR3a` appear
as the standard prints them (R-DOC-4).

**D14. Coverage floors only rise** (R-COV-2).

**D16. Three consuming idioms are allowed in conditions,** a recorded
deviation from R-CF-4 (R-DEV-5):
- `if self.accept_*(..)` in the parser;
- `while let Some(x) = q.pop_front()`;
- `if x.take().is_none()`.

*Why:* each is one call that only consumes the input decided on;
rewriting about 50 sites would hide the parser's structure.

**D17. `unnested_or_patterns` is allowed.**

*Why:* it contradicts R-CF-5, which keeps a decision's conditions visible
for MC/DC test design.

**D20. Lint baselines are counted.** `cargo xtask lint-audit` builds with
baselines off and compares each file's count of each lint with
`xtask/lint-baseline.txt`.

*Why:* a list of lint names alone would let another violation of a
listed lint pass.

**D21. `verify` runs in a pre-commit hook and in CI.**
- **Hook** (`cargo xtask install-hooks`): before each commit.
- **CI:** Linux, macOS and Windows on every push and pull request;
  coverage on Linux.

*Why:* a hook can be skipped or not installed, and checks one platform.

**D22. No instrumented MC/DC.** Test design shows MC/DC (R-TEST-8);
review checks it.

*Why:* stable Rust cannot measure it, and a rule awaiting a nonexistent
tool cannot be met.

**D24. Format errors name tokens, not clauses.** A BNF violation
(SQLCODE -102) says "expected *x*, found *y*" at its position; every
other error cites its rule (R-ERR-2).

*Why:* generic parser helpers cannot know their caller's clause, and the
position identifies it anyway.

### SQL semantics

**D15. Whether a run-time error is raised does not depend on the plan.**
- **Rule.** A WHERE or HAVING conjunct that can raise an error is
  evaluated on every row or group the clause covers, whatever the access
  path or rewrite.
- **Which error.** Where several rows or conjuncts would raise, the
  statement raises the first the plan meets, which may differ by plan:
  the operand a hash join builds, the join order, an OPTIMIZER setting.
  One of them is raised under every plan; with one alone, that one.
- **Products.** One over one operand's columns runs on all its rows if
  every other operand has a row, else on none, as the product holds
  them (§5.20 GR2b). It moves onto its operand over a guard (a row of no
  columns, an EXISTS of each other operand; `optimizer/raising.rs`); the
  non-raising rest may be hash join keys and index probes. A table read
  for such a filter is never probed, by a constant or a join's rows
  (D35): all its rows are read.
- **Outer joins.** ON applies to every row of the operands' product
  [ISO/IEC 9075:1992 §7.5 GR1c].
  - A raising conjunct over the null-supplying operand's columns alone
    likewise moves onto it, over an EXISTS of the other
    (`optimizer/outer.rs`), if every raising ON conjunct is such; the
    join keeps its equalities.
  - One over the preserved operand's columns alone stays in the join, as
    a filter would drop rows the join keeps. A join on keys judges it on
    each such row once the other side has a row, failing rows matching
    none; so too unmoved such conjuncts of either operand
    (`Plan::OuterJoin`'s own conditions).
  - A raising one over both operands' columns has every pair judged: the
    join has no keys.

*Why:* §5.18 GR2 defines AND only by its truth table, and §5.21 GR2 and
§5.23 GR2 apply the condition to each row and group; nothing permits
leaving an operand unevaluated. The order is not fixed: §3.3.4.4 says
"it is implementation-dependent whether expressions are actually
evaluated left to right, particularly when operands or operators might
cause conditions to be raised" (Annex C item 5).

*Exception:*
- **EXISTS** stops at the first row its subquery's condition keeps,
  whatever the plan; a batch that raises yields its earlier rows first,
  as a row-by-row read would, so no later row's error is raised.
  ISO/IEC 9075:1992 §8.8 GR1 defines EXISTS by "the cardinality of T",
  which its first row decides; §3.3.4.4 makes evaluation order
  implementation-dependent, notably when parts may raise or are not
  needed for the result (Annex C item 6: whether an inessential part's
  exception is raised is implementation-dependent).
- **Scalar and row subqueries** likewise stop at their second row, which
  makes them fail [§7.11 GR1]. **IN, ANY, SOME and ALL** read every
  subquery row, so raise any of their errors.

**D18. A limit the standard does not set is not a syntax error.**
- **Limits.** Valid SQL beyond an implementation limit (CONFORMANCE.md),
  or never (D32) or not yet implemented, gets SQLCODE -900 ("valid, but
  not processed"), not -103.
- **Parsing.** The parser reads the whole Format before refusing
  (`Parser::refuse_later`), so SQL breaking a Format still gets -102.

*Why:* -103 claims a broken Syntax Rule, falsely, and §3.4 requires
conforming SQL to be processed; -102 claims it is not SQL.

**D19. A failed undo damages its table.**
- **Report.** The statement or ROLLBACK gets SQLCODE -914 naming each
  table and failure; later statements on the table, except DROP TABLE,
  get -914. `Database::damaged_tables` lists them.
- **Transaction.** With a statement that could not be undone, it can
  only roll back (-913).
- **No checkpoint** or clean close while a table is damaged, so the last
  checkpoint and the log keep what it should be.
- **Repair.** Index entries alone: `Database::repair_table` rebuilds the
  indexes from the rows. Rows: reopening the database recovers it; kept
  nowhere, it can only be dropped.
- **Memory.** The memory budget never refuses an undo (D38).

*Why:* a silently half-undone table breaks §7.3 GR3b i and §8.9 GR1;
refusing it hides a state no transaction produced, and the log can put
it right.

**D32. SQL-86 and SQL-92 are one language.**
- **Precedence.** ISO/IEC 9075:1992 extends X3.135-1986; the newer wins
  where they differ, and restrictions SQL-92 keeps only for Entry or
  Intermediate SQL are lifted.
- **Order.** SQL-92 in full first, then later standards through
  SQL:2023, each from its own text, never memory or other products.
- **Texts.** Local, untracked; cited as `[X3.135-1986 §…]` and
  `[ISO/IEC 9075:1992 §…]`.
- **Excluded.** Not claimed: the module language, embedded SQL. Never
  implemented (CONFORMANCE.md): character sets other than the one there
  is, collations and translations, leap seconds.

*Why:* one consistent language serves programs better than a choice of
dialects; one character set keeps comparison, keys and storage simple.

**D33. Schema changes are transactional.**
- **Scope.** In transactions: CREATE SCHEMA run by a session [ISO/IEC
  9075:1992 §20.1 GR6b ii, iii]; CREATE TABLE, CREATE VIEW, GRANT,
  REVOKE (D47); DROP SCHEMA, TABLE and VIEW; ALTER TABLE.
- **Staging.** Definition changes are staged on a copy (redone on a
  newer one if others publish), published at commit, forgotten at
  rollback.
- **Locks.** Each holds its table's exclusive lock until the transaction
  ends. A drop locks all it drops or redefines, CASCADE included, and a
  dropped schema (defining a table or view in it takes that lock
  shared). CREATE SCHEMA holds its schema name exclusive, as the
  operator's immediate `Database::schema` does while running.
- **Log.** Logged under its transaction; recovery applies it at the
  commit record.
- **ALTER TABLE** copies rows to a new storage generation, so rollback
  leaves the old one intact.
- **Stale transactions.** One begun before a change it cannot see is
  refused on next using the table (SQLCODE -911).

*Why:* SQL-92 lets schema and data statements share a transaction
[§4.28], and an irreversible schema change is a trap.

**D47. Schema statements on their own.**
- **Session statements.** CREATE TABLE [ISO/IEC 9075:1992 §11.3], CREATE
  VIEW [§11.19], GRANT [§11.36] and REVOKE [§11.37] run outside a
  `<schema definition>` [§20.1], in the session's transaction (D33).
- **Names.** Only an existing schema's owner defines tables, views,
  domains and assertions in it. Unqualified names, ALTER TABLE's too,
  are in the session's default schema [§5.4 SR8b]. Kept texts (view
  queries; table, domain and assertion CHECKs) and ALTER TABLE log
  records store their names' schema (`ViewDesc::names`,
  `ConstraintKind::Check`) and are reread there, whatever the default.
- **Constraint names.** Unqualified, one is in the default schema, maybe
  not its table's: no Syntax Rule ties them, and the Definition Schema
  keeps CONSTRAINT_SCHEMA beside TABLE_SCHEMA [§21.3.13]. That schema
  must exist (SQLCODE -103, SQLSTATE 3F000) [§5.4 GR18]; the name is
  unique among its constraints [§10.6 SR2], where ALTER TABLE DROP
  CONSTRAINT finds it. Likewise domain constraints of CREATE and ALTER
  DOMAIN. Implementation-given names are in the table's or domain's
  schema [§11.6 SR2]; a declared local temporary table's are all
  MODULE's.
- **\[EXTENSION\] Owners.** A constraint's schema must belong to its
  definer, the owner of its table's or domain's schema; else
  SQLCODE -104 (access rule violation), beyond SQL-92's text: a name in
  another's schema would block that owner's own constraints of the name.
  DROP SCHEMA RESTRICT is refused (SQLCODE -103) while a constraint
  named in it belongs to another schema's table or domain; CASCADE
  drops it, as ALTER TABLE or ALTER DOMAIN DROP CONSTRAINT CASCADE
  would, where SQL-92 is silent [§11.2 GR1-GR8].
- **Locks.** Exclusive on the name it defines, grants or revokes on (the
  table's own lock once one exists), so two definers of one name take
  turns (D28); shared on tables read, exclusive on tables redefined. It
  works on the definitions as of acquiring them.
- **Log.** Its text, authorization identifier, default schema, and a
  created table's engine and storage generation; recovery reapplies it
  at the commit record. A table reusing a dropped name gets a later
  generation, so the dropped table's logged rows never reach it.
- **Stale transactions.** As for ALTER TABLE: one begun before a table's
  creation is refused on using it (SQLCODE -911).
- **GRANT** grants what its grantor holds with grant option among what
  it names, warning "privilege not granted" (SQLSTATE 01007) for the
  rest [§11.36 GR14, GR15]; a grantor holding nothing on the object is
  refused [AR1]. Likewise in a `<schema definition>`, where SQL-86
  refused [X3.135-1986 §6.6 SR5]. A double grant is kept once,
  grantable if either was [§11.36 GR12, GR13].
- **REVOKE** destroys the named privileges its authorization identifier
  granted, or with GRANT OPTION FOR, their grant option.
  - **Standing.** A privilege stands while its grantor holds it with
    grant option, as owner or by a standing privilege: a fixed point
    built up from owners' privileges, so mutually supporting cycles
    fall together [§11.37 SR11].
  - **Abandoned:** a view whose owner would lose SELECT on a table it
    names; a constraint whose owner would lose REFERENCES it needs
    [SR13, SR14].
  - **RESTRICT** refuses to abandon anything [SR18] or, with GRANT
    OPTION FOR, a privilege granted by the option (SQLSTATE 2B000)
    [GR2b]. **CASCADE** destroys it all.
  - **Not granted.** Named privileges it did not grant get a warning
    (SQLSTATE 01006) [GR8, GR9].
  - **View owners.** Their privileges on a view derive from its tables',
    unstored, so they follow them.
- **Warnings** are completions: SQLCODE 0 [§22.2, Table 24] plus their
  SQLSTATE, in the protocol's Complete (D45).
- **PUBLIC.** A privilege granted to PUBLIC WITH GRANT OPTION lets anyone
  grant it, as SQL-92's applicable privileges include PUBLIC's [§10.3
  GR2, §11.37 SR7a ii]; SQL-86 took only grants naming the grantor
  itself [X3.135-1986 §6.6 SR3b ii].

*Why:* operators define and secure tables over a connection, one
statement at a time; with transactions, a definition and the rows
filling it commit together or not at all.

### Architecture

**D25. Durability: one log, owned by the database.**
- **The log.** One logical log per database, in a versioned,
  engine-neutral format, holds:
  - row changes: table, row id, before-image and after-image;
  - changes of definitions.
- **Commit.** A transaction commits when its commit record is durable.
  Commit order is log order.
- **LOG.COMMIT_SYNC OFF** (D36). The session is answered once its commit
  record is written, not synced:
  - the transaction is visible at once;
  - it becomes durable at the log's next sync (a commit that waits, a
    definition, a checkpoint, a filled segment, close, or the log sync
    job), each sync covering every record before it;
  - a crash before then loses it and every commit after it, so recovery
    still finds a prefix of the commits.
- **Log sync job.** The first commit left unsynced asks for the job
  `turborel-log-sync` (D55) LOG.SYNC_AFTER_MS later (default 1000, 0 for
  none), so a database at rest loses at most that much time of them:
  - it syncs through the last commit left unsynced, unless another sync
    has, through `Log::sync`, so for every log, plug-ins too;
  - its failed sync stops the database taking changes, as any failed
    sync of the log;
  - close stops it, then syncs the log itself.
- **Log durability.** LOG.DURABILITY (D36): NONE, DATA (default) or
  FULL, changed while the log is open:
  - at once: each sync from then on syncs as it says, NONE not at all,
    and `Log::capabilities` tells it, which Strict reads at each write;
    not from the next segment, as capabilities must tell what a sync
    does now;
  - the file log makes each segment begun after as it says, its
    creation synced or not; a plug-in is told it (`Log::set_durability`)
    and takes it as far as it can; the memory log is never durable;
  - only a sync that syncs claims records durable, so the first after
    NONE first syncs the segments sealed unsynced and their directory:
    every record before it is then durable, as `Log::sync` promises;
  - a log opens with the value its last image records, and is told the
    one replayed once the database is open.
- **Plug-ins.** The `Log` is a plug-in: built in, a memory log and a
  segmented file log with group commit. A database in a directory opens
  its log, as its catalog, through a plug-in `LogStorage`
  (`db/stores.rs`): built in, the file log in `log/`. That log is checked
  from the position the last image holds durable; earlier segments were
  synced whole.
- **Torn segment.** One cut short by a crash ends the log; later segments
  are removed (a sync seals all earlier segments, so none of theirs was
  durable). Bytes cut are reported in `RecoveryReport` and the `opened`
  event.
- **Damage.** Valid records after invalid bytes, in that or a later
  segment, are damage, unless the invalid bytes show an interrupted
  write: an unwritten sector (zeros), a zero length ending them, or a
  record past the file's end. Damage refuses the log (SQLCODE -916),
  cutting nothing, lest later commits be lost.
- **Persistence.** Each engine declares `Logged` (replayed from the log),
  `Forced` (durable at commit by its own writes), `External` (its own
  durability) or `None` (scratch, never recovered).
- **Catalog.** Its images record the definitions as they are (a
  snapshot, not every one made), the database's settings, and where each
  table and index is. Opening loads the snapshot, then replays
  definitions logged since. Image storage is a plug-in (`CatalogStorage`,
  `CatalogStore`), images opaque to it: built in, two directory files, by
  turns.
- **Checkpoints** write every engine's state as of a log position, so the
  log can be truncated behind it. The row store's file is shadowed, so a
  crash leaves it as the last checkpoint did. Recovery reopens the
  tables, then redoes and undoes from the log by the database's one
  replay (D57).
- **Due** after CHECKPOINT.AFTER_CHANGES row changes logged since the
  last checkpoint; a failed one leaves them counted.
- **Retention.** The log is kept from the last checkpoint or the oldest
  open transaction's first record, so it grows while checkpoints fail or
  are not taken, or a transaction stays open. Past
  CHECKPOINT.OVERDUE_FACTOR (four) times the due count, an event says so
  and why, at most once a minute (`Event::CheckpointOverdue`, D40).
- **Lock.** Open in one place at a time: opening takes an exclusive lock
  on the directory's `lock` file (`FileSystem::lock`, the system's file
  lock) before clearing or reading anything. Closing unlocks before
  closing the file, so a child process sharing it until it runs its
  program does not keep the lock. A crash releases it.
- **Failed sync.** Changes stop: it may have lost logged data, and a
  later sync may succeed without it. The file log keeps the failure (of
  a commit's sync, or a segment's as an append seals it) and fails every
  later sync, never retrying. From the next sync, no row change, commit
  of a writing transaction, definition or checkpoint is accepted
  (SQLCODE -915), nor a clean close; reads and rollbacks go on.
  Reopening recovers from the log (`Database::log_failure`).
- **In doubt.** A commit whose record is written but whose sync fails:
  - is never stamped (engines are not told), so no session sees it, not
    even its own;
  - gets SQLCODE -922, SQLSTATE 08007 (transaction resolution unknown);
  - keeps its locks and versions, with no abort record, until the
    database is dropped, so nothing builds on or undoes it;
  - on reopening, commits if its commit record reached disk, else rolls
    back.
*Why:* one commit point makes transactions atomic across engines without
two-phase commit; logical records keep engines independent; a pluggable
log allows replicated or remote ones.

**D26. Cryptography only from vetted crates, in plug-ins.**
- **Scope.** Password hashing, TLS, encryption at rest and cryptographic
  hashes may use vetted crates, in their plug-ins only; the core stays
  dependency-free (D3). Each crate needs its own R-SAFE-4 decision.
- **TLS.** rustls 0.23.46 (Apache-2.0, ISC or MIT), plus tokio-rustls
  0.26.6 (MIT or Apache-2.0) in the server; in the client, only under
  feature `tls`. Cryptography only from aws-lc-rs (ISC AND (Apache-2.0
  OR ISC)): AWS-maintained on AWS-LC, rustls's default provider, with a
  FIPS 140-3 build; needs only a C compiler (prebuilt assembly on
  Windows). Not `ring`: its README calls it an experiment, not for third
  parties to depend on.
- **Test certificates** come from rcgen 0.14.10 (MIT or Apache-2.0), on
  aws-lc-rs, under the test support's feature `tls` (which the server's
  tests take); none sits in the repository to expire.
- **At rest.** AES-256-GCM and ChaCha20-Poly1305 from aws-lc-rs, in
  `turborel-aes`, as encodings (D48); subkeys by HKDF-SHA256.
- **Passwords.** PBKDF2-HMAC-SHA256 from aws-lc-rs (D43), at
  SERVER.PASSWORD_ITERATIONS, default 600,000: OWASP's FIPS 140 choice,
  no dependency beyond TLS's. Argon2id, OWASP's first choice, is
  memory-hard, but RustCrypto's `argon2`, its one Rust implementation,
  claims no audit or production readiness and has no FIPS path. A
  verifier names its scheme, so Argon2id may come beside it.
- **SCRAM-SHA-256** (D43): HMAC-SHA-256, SHA-256, PBKDF2 and randomness
  from aws-lc-rs, in `turborel-protocol` under feature `scram`, taken by
  the server and by the client's own feature `scram`: one implementation
  for both, checked against RFC 7677's example. Its base 64 is ours: an
  encoding, not cryptography. Its channel binding hashes with
  aws-lc-rs's SHA-256, -384 and -512, the certificate's signature
  algorithm read by the protocol's own DER walk (`der`).
- **Client certificates** (D42) are verified by rustls's
  `WebPkiClientVerifier`; the server reads the subject's common name from
  the DER by its own tag-and-length walk (`turborel_protocol::der`), as
  no vetted crate in the tree reads a subject.
- **Trust roots.** The client trusts authorities in files it is given,
  and the platform's only on request (`tls::roots`;
  `turborel --tls-system-roots`). Files stay the default: a platform
  trusts many authorities, any able to certify a server of any name,
  while one's own server usually has one's own authority.
- **rustls-native-certs** 0.8.5 (Apache-2.0, ISC or MIT) gives the
  platform's roots, as the client's feature `system-roots`, taken by the
  command-line client. It is the rustls project's; its README calls it
  "mature and widely used" and "still maintained, but mostly for use
  inside the platform verifier", pointing new users to
  rustls-platform-verifier, which uses the platform's own verifier: a
  larger change of trust, not taken. No `unsafe` of its own, no
  advisory. It reads:
  - macOS's keychain by security-framework 3.7.0 with
    security-framework-sys 2.17.0, core-foundation 0.10.1,
    core-foundation-sys 0.8.7 and bitflags 2.13.2 (each MIT or
    Apache-2.0; kornelski's, servo's and the bitflags project's; system
    framework bindings, their `unsafe` the calls into them);
  - Windows's system store by schannel 0.1.30 (MIT), on the windows-sys
    already in the tree;
  - elsewhere, the system bundle by openssl-probe 0.2.1 (MIT or
    Apache-2.0, now the rustls project's), whose one `unsafe` (setting
    environment variables) is in a function never called; only `probe`,
    which looks for files, is. Held at 0.2.1 in Cargo.lock, as 0.2.2 is
    under two weeks old.

  None has an open advisory (security-framework's RUSTSEC-2017-0003 was
  fixed in 0.1.12). Certificates rustls cannot parse are skipped; a store
  with none it can is refused, saying why. SSL_CERT_FILE and
  SSL_CERT_DIR, if set, replace the store, as the crate does. Not
  webpki-roots: a list fixed at build time goes stale with the program.
- **Hash functions** (`HashFunction`, D30), from aws-lc-rs in
  `turborel-aes`: SHA-256 (id 11), which no accident matches, though
  whoever can rewrite a file can rewrite its checksums; SHA-512 (id 13),
  likewise at 512 bits; HMAC-SHA-256 (id 12), keyed by a keyring's key
  through HKDF-SHA256, which a forger without the key cannot match.
- **Keyed checksums.** A file records the function's id, never the key;
  opened with another key, its headers fail their checksums and it is
  refused (SQLCODE -916). `turboreld` keys it by `--checksum-key`. None
  stops a block or page the database once wrote being put back in place.
- **Digests.** At most 512 bits (`hash::MAX_DIGEST_BYTES`), with room in
  every format storing one: block file headers, pager file headers and
  meta slots. A block's or page's trailer is the function's size; a
  page's body is the page less it.
*Why:* cryptography written here would be unreviewed and likely wrong,
and a server needs it.

**D48. Compression and encryption as encodings, in plug-ins, per file or
per table.**
- **Naming.** Encodings (D30) are registered with the database.
  STORAGE.ENCODINGS lists those every file passes through (D36); the
  table option ENCODINGS, the database's own on tables of any engine
  (D41), those its rows do. A name may take an argument after a colon
  (`AES-256-GCM:PAYROLL`), bound into the encoding, which records in its
  output what decoding needs; file headers name only ids.
- **Ids** are never changed or reused: 1 LZ4, 2 AES-256-GCM, 3
  ChaCha20-Poly1305; tests use 1000 and up.
- **Overhead.** An encoding declares its maximum growth
  (`Encoding::overhead`), reserved by the pager in every page (so an
  encoded page never spills to an overflow page) and by the row store in
  every record. One that hides its input says so
  (`Encoding::confidential`).
- **LZ4.** Crate `turborel-lz4`, on lz4_flex 0.14.0 (MIT): only its safe
  encoder and decoder (features `safe-encode` and `safe-decode`, which
  forbid `unsafe`). It claims no production readiness; taken on its use
  (about 160 million downloads; arrow, parquet, tantivy and the
  ClickHouse and Scylla clients depend on it) and fuzzing. A decoded
  block must match its recorded length, at most 255 times its own, so
  damage is refused before memory is taken. Incompressible input is kept
  as is, behind one byte.
- **Ciphers.** AES-256-GCM and ChaCha20-Poly1305 [RFC 8439]: crate
  `turborel-aes`, on D26's aws-lc-rs. ChaCha20-Poly1305, for processors
  without AES instructions, is sealed like AES-256-GCM, nonces from
  aws-lc-rs's generator, with its own HKDF info, so a key named by both
  gives each distinct subkeys.
- **Keys** come from a `Keyring`, an extension point its host supplies,
  never from database files. The built-in `KeyDirectory` reads an
  operator's directory (`turboreld --keys DIR`), one key per file of 64
  hex digits; a key service is another keyring.
- **Pages** are encrypted with a subkey (HKDF-SHA256 of the named key and
  a random salt redrawn every 2^30 encryptions) and a nonce aws-lc-rs
  draws, per its FIPS rules: random nonces bear 2^32 encryptions a key,
  and no subkey nears that. Salt, nonce and key name are kept in what is
  encrypted; storage place and key name are authenticated, so data
  changed, moved or of another key is refused (SQLCODE -916).
- **Rotation.** Name another key; older rows are read with their own
  until `ALTER TABLE ... RESEAL` [EXTENSION], RESEAL a
  `<non-reserved word>` (also `Session::reseal_table`,
  `turboreld ... reseal`), gives the table a new storage generation, as
  an engine change does, copying its rows through its current encodings
  by its transaction's own writes, logged as any ALTER TABLE. Once
  committed and checkpointed, nothing reads the old key, which may be
  retired. A STORAGE.ENCODINGS key serves the database's life.
- **Per table,** the engine seals what it stores: data through a
  pipeline, headed by its ids (`Pipeline::seal`, `Encodings::unseal`),
  with the registry from `Host::encodings`. An engine that does not
  (`StorageEngine::seals`) takes no table naming encodings (SQLCODE
  -103). Sealed data names its encodings, so the option changes in place:
  new rows use the new ones, old rows their own.
- **Row store.** Seals records and out-of-line values, the row id the
  context; a table naming none stores records plain, at no cost. Its row
  changes are sealed in the log too (`log/sealed.rs`).
- **Indexes** keep keys in order, in the clear, so a table with
  confidential encodings gets no index (SQLCODE -103) unless the
  database's files are encrypted too, or kept nowhere.
- **Spills.** Spill files (D38) are encrypted while the database
  encrypts anything (files or a table), by the first confidential
  encoding in use under the process's ephemeral key, not a named one:
  drawn on first use, held only in memory; no later process reads
  spills, so no keyring is needed. First they pass through
  SPILL.ENCODINGS, as LZ4: none by default, as LZ4 was not clearly better
  where the disk keeps up. If the two exceed a pipeline's stages, only
  the encryption is kept.
- **Spill changes.** A spill file's pages take the spill encodings in
  use as each is written (`LivePipeline`, `disk/pager/live.rs`), so a
  table encrypted while an area lives gets every later page encrypted:
  - every holder alike: sort runs, partitions, tables, held rows, lists,
    temporary tables' files and undo pages; no copy, no memory beyond
    the area's;
  - pages written before stay as they were until rewritten or freed;
  - a page's content leads with a byte naming its pipeline, at most 256
    a file, kept in memory: no later process reads spills;
  - a pipeline of the same ids replaces the one before (an encoding's
    output records what decoding needs);
  - a provider plug-in writes so too (`Spill`'s contract).
- **Shadows.** The data file's shadows hold pages as encoded, the undo
  store sealed records.
- **Checks.** Names are checked when the option is set: SQLCODE -103 if
  not registered, -910 if one refuses its argument (as a key the keyring
  lacks). A database missing an encoding a table names still opens; that
  table's rows are refused (SQLCODE -910) until it is registered.
*Why:* LZ4 is fast enough for every read path; a compressor or cipher
written here would be unreviewed; aws-lc-rs is already vetted for TLS,
with a FIPS 140-3 build. What compresses well, or must be hidden, differs
by table. Tables share one data file whose pages keep their slots
whatever their encoded length, so the engine seals, not the pager; keys
live outside the database, or encrypting it would hide nothing. Spill
files follow a change page by page: moving holders to new areas would
copy their tables and still write the pages left in a pool in the
clear; refusing to write would fail other sessions' statements.

**D49. Temporary tables: each session's instance.**
- **Definition.** A global or created local temporary table [ISO/IEC
  9075:1992 §4.9, §11.3] is defined as a persistent one: in the catalog,
  logged, kept by checkpoints, its storage empty. Its rows are per
  session.
- **Instance.** The definitions a session sees give each temporary table
  the session's own instance: rows, indexes and lock
  (`db/temporary.rs`), so binding, constraints and indexes need nothing
  special. It is made when a session's transaction first touches it, and
  dies with the session. Its changes are not logged: never recovered
  [§4.9].
- **Pages.** Rows, index entries and older versions of an instance
  share one spill file of the database's temporary files (D38), through
  a buffer pool of the session's MEMORY.WORK_BYTES reserved of the
  budget (`db/temporary/pages.rs`). What the pool does not hold is
  written to the file, so an instance may outgrow the budget.
  - The file takes the spill files' encodings (D48) as each page is
    written.
  - It is removed as DELETE ROWS empties the instance, or the instance
    goes; a crash leaves it to be cleared at the next open (D56).
  - Older versions a transaction keeps go to an undo store over a log
    of the file's pages, a batch to a page (`db/temporary/undo.rs`),
    freed as the transaction ends.
  - A database in memory keeps the file in memory of its budget (D56).
- **ON COMMIT.** DELETE ROWS empties an instance as its transaction ends,
  committed or rolled back: as GR3 of §14.3 leaves it, or as it was. A
  PRESERVE ROWS instance is then collected by its session (versions
  superseded or deleted, with their index entries): no other session
  reads it and none of its transactions is open, so the database's
  horizon, held back by others' long transactions, holds none of its
  versions. The database's collection (`txn/end.rs`) skips temporary
  tables.
- **Modules.** A session is one module, so a created local temporary
  table acts as a global one within it. Its declared local temporary
  tables [§13.11] live in a schema MODULE of its own definitions only,
  owned by its authorization identifier (`db/declared.rs`), invisible to
  other sessions and the Information Schema.
- **ALTER TABLE** is refused (SQLSTATE TB911) while another transaction
  has an instance's rows open, or wrote them and committed after the
  altering transaction's snapshot; while it runs, no other transaction
  begins on an instance. Instances are tracked by session
  (`temporary::Uses`).
  - **Open sessions.** A session mid-transaction on the table is never
    reshaped under it: the ALTER waits for it as for a lock, refused.
  - **Columns.** ADD and DROP COLUMN give each instance holding rows a
    successor of the new generation, its rows copied by the altering
    transaction. Each session switches at commit; a statement or
    transaction begun earlier no longer reads it
    (`TableLock::check_generation`).
  - **No row changed.** Any other action is every session's at once,
    instances kept in place.
  - **Checks.** Each session's rows are checked at once against the
    table's constraints, a constraint added among them, as that session
    sees the database: its own instance of each temporary table
    (`db/temporary/reshape.rs`). An index added gets entries of each
    instance checked.
- **CREATE INDEX** defines it, built-in methods only. An instance builds
  its entries in its pages when its session first sees the index: empty
  if no rows; from every version if no transaction has them open; else
  the session can use the index only after its transaction ends
  (SQLSTATE TB911 meanwhile). Instances forget a dropped index alike,
  its pages freed in the file.
*Why:* the standard gives each session's rows a schema of its own
[§4.9]; giving the session's view of the definitions its own storage
does so without changing how a table is found, bound or checked. One
definition serves every session, so ALTER TABLE changes each one's
rows; refusing it while another has them open keeps a transaction's
table as it began.

**D50. Domains: their constraints copied onto their columns.**
- **Catalog.** A domain [ISO/IEC 9075:1992 §11.21] is described with its
  data type, default, constraints and USAGE grants (`DomainDesc`).
- **Columns** of a domain take its data type and keep its name and a copy
  of its default, used when the column has none [§11.5 GR2b] and changed
  with the domain's.
- **Constraints.** A domain constraint holds for every column of the
  domain [GR5, GR6]: each has a copy, a CHECK of its table with VALUE the
  column, naming its original. ALTER DOMAIN adds copies (checking
  existing rows) or drops them; DROP DOMAIN CASCADE leaves them as table
  constraints [§11.27 GR1c]. The Information Schema shows the domain's
  constraint, not the copies.
- **CAST** to a domain takes its data type and needs USAGE on it; per
  §6.10 none of its constraints is checked.
- **Usage.** Whether a CHECK or query CASTs to a domain is read from its
  text (`domains::casts_to`), in its names' schema. So no domain is in
  usage by its own constraints [§11.21 SR4], and what CASTs to one in a
  schema whose owner loses USAGE on it is abandoned [§11.37 SR13-SR16].
*Why:* a table CHECK is already bound, checked, deferred and logged;
copying a domain constraint onto each column makes it one, needing no new
machinery.

**D51. Assertions: a query whose rows break it.**
- **Catalog.** An assertion [ISO/IEC 9075:1992 §11.34] is a schema
  constraint on no table, described as a CHECK (`ConstraintDesc`), with
  the tables it reads and the columns it sees of each.
- **Binding.** As `SELECT 1 FROM (VALUES (1)) WHERE NOT (condition)`, as
  the schema's owner and as a constraint, so a CHECK's rules apply. Any
  row means it is broken [GR2].
- **When.** Checked when made and at the end of each statement changing
  a table it reads, its reads locked shared; at commit if deferred.
- **Deferred, once.** A deferred assertion checked whole, or deferred
  CHECK reading other tables, waits once however many statements changed
  what it reads (`Deferral::wholes`), so is checked once at commit or SET
  CONSTRAINTS ... IMMEDIATE [§4.10.1]. A row's checks are each kept.
- **From a change.** Checked immediate at a statement's end, it held at
  the statement's start. So a conjunct of these forms is checked from the
  changed rows (`db/assertions/changes.rs`, `forms.rs`), each of one table
  T's rows passing its steps; `k` a constant, the set function the
  standard one:
  - `NOT EXISTS (SELECT ... FROM T WHERE p)`: from rows inserted or
    updated, as a CHECK;
  - `(SELECT COUNT(*) FROM T WHERE p) op k`: from rows entering the count
    less those leaving. It holds without reading T if that cannot move
    the count toward false and the transaction reads at one snapshot
    (D29); else whole. COUNT(x) is COUNT(*) of the rows `x IS NOT NULL`;
  - MAX(x) `<` or `<=` k, MIN(x) `>` or `>=` k: from rows inserted or
    updated, none past k [§6.5 GR2b iii];
  - MAX(x) `>` or `>=` k, MIN(x) `<` or `<=` k: holds if a row inserted
    or updated reaches k, or, at one snapshot, no row changed passes
    before or after; else whole.
- **Steps.** Of T's row alone (columns, constants, arithmetic,
  comparisons, IS NULL, LIKE, CASE; no function, CAST, subquery or outer
  reference), so the same truth or error each time, and no warning: the
  filters of the WHERE and of a view, merged or not, and a view's
  projection.
  - A condition above a projection is rewritten over T's row; its
    expressions that can raise are evaluated on each row it projects,
    as it evaluates them.
  - A row stops at the first filter not true of it, the lower first, as
    `MergeFilters` keeps.
- **Never null.** A set function's `x` is never null in those rows: a
  column of a NOT NULL or PRIMARY KEY constraint not deferrable, one a
  filter's conjunct is not true of when null, a constant, or arithmetic
  of those. Else whether a null is eliminated, warning 01003 [§6.5
  GR1b], depends on every row.
- **Errors.** Every form is evaluated on the change, as AND evaluates
  both operands, so an error any raises is raised (D15); NOT EXISTS
  stops at its first row passing, as EXISTS does.
- **Deferred rows.** A deferred assertion of one NOT EXISTS waits as a
  check of each row written (`Check::AssertedRow`), evaluated when it is
  made: it held as it was deferred, and rows are independent.
- **Whole.** Any other form; any other deferred assertion; SUM and AVG,
  whose value and overflow depend on every row [§6.5 GR2b iv]; a join, a
  grouping, COUNT DISTINCT; MAX or MIN `=` or `<>` k; an `x` that may be
  null.
- **Name.** A constraint name of its schema, for SET CONSTRAINTS and
  every other constraint's [SR2, SR7].
- **In a `<schema definition>`,** outside any session's transaction, it
  is checked in its own transaction against the newest committed rows,
  and not again during log replay.
*Why:* a CHECK with a subquery is already checked over all it reads,
deferred and logged; an assertion is one with no table, needing only its
query. A whole check reads its tables every statement, however small the
change; the change-based forms provably equal the whole check, given it
held when the statement began.

**D52. A view's REFERENCES: from what its columns are drawn from.**
- **Owner.** A view owner's privileges on it derive from its privileges
  on the view's tables (`applicable`). REFERENCES on column CV is held if
  the owner holds REFERENCES on every underlying column of CV in the
  view's leaf underlying tables, and on some column of each [ISO/IEC
  9075:1992 §11.19 GR6b].
- **Leaves.** The tables and views its query's FROM clauses name,
  through joined and derived tables and set operations' operands. A view
  it reads is a leaf, having no underlying table [§4.9].
- **Underlying columns** of a `<query specification>`'s column: those its
  select list item references (subqueries included) and those its
  `<table expression>` references: WHERE, GROUP BY, HAVING, join
  conditions, derived tables' queries [§7.9 SR5, §7.3 SR2, §6.4 SR9].
  - A NATURAL or USING join's common column draws from both join
    columns, as `COALESCE ( TA.C, TB.C )` [§7.5 SR6d].
  - A `<table value constructor>`'s columns have none [§7.10 SR16a].
  - A set operation's column draws from both operands' i-th [SR16d], or
    with CORRESPONDING, both operands' columns of its name [SR9a iv].
- **Recorded** at definition: the leaves', in the view's descriptor
  (`Underlying`), by table and name.
*Why:* re-deriving them at every privilege check would bind the view
each time.

**D53. Data types: one contract, standard types built in.**
- **Contract.** One trait, `value::SqlType`, defines a type: how it is
  written; what it compares with; store assignment and CAST to it; its
  value's bytes in the log, catalog, wire, records and index keys (with a
  probe's anchor); its literal; its Information Schema descriptor; its
  histogram position. A `DataType` names its kind (`DataType::kind`),
  through which every type-level operation goes.
- **Standard types** are built in, each a kind in its own module under
  `value/types/`, registered by default. The parser and binder know them:
  their syntax (`DATE '...'`, `INTERVAL ... DAY TO HOUR`) and rules are
  the language's.
- **Hot path.** Built-in values are `Value` variants, so comparison and
  hashing (`value::compare`, `value::bucket`) match them directly, no
  virtual call; each kind's `compare` and `bucket` are those functions.
- **Plug-in types** use the same trait, registered
  (`Database::register_type`) under a regular identifier before the
  database opens from its files. A kind lives as long as the program,
  like code, so `DataType` stays `Copy` and engines and index methods,
  taking `&[DataType]`, encode with no registry at hand. Values are
  `Value::Extension` (kind plus bytes), so comparison and hashing need no
  registry either.
- **Naming.** Written `name` or `name(a [, b])`:
  - where a `<domain name>` may be (a column definition, CAST), an
    unqualified name is a domain of the default schema if one exists,
    else the plug-in type;
  - in CREATE DOMAIN, in place of its `<data type>` [ISO/IEC 9075:1992
    §11.21], a name alone is the plug-in type; a domain's name, which
    §11.21 does not take there, is refused.
- **Storage** is by name: catalog, log, spilled rows and undo read it
  back through the registry (`bytes::Reader::with_types`); a database
  whose files name an unregistered type does not open.
- **Wire.** A value travels as type name, text and kind's bytes. A client
  without the plug-in holds it as `Unregistered`, shows its text and
  returns it unchanged; the server reads it as its type again.
- **Store assignment** [ISO/IEC 9075:1992 §9.2 SR2] to or from a plug-in
  type takes a value comparable with the target, per the kind, and a
  character string if the kind accepts one (`SqlType::assigns_text`),
  converted as its CAST would. By default it does not: a string becomes a
  value only by CAST. A DEFAULT's character string literal is its CAST
  either way, plug-in types having no literals.
- **Function arguments.** A scalar function may declare argument types
  (`ScalarFunction::argument_type`): other types are converted by store
  assignment before the call, or refused at binding; `return_type` gets
  the declared types, and a `?` takes the declared type. By default none
  is declared and arguments pass as is.
- **One per name.** One scalar function per name, never chosen among
  several by argument types: one function's `return_type` may accept
  several. Registering a name again, a standard function's too, is
  refused (SQLCODE -910), so no session's calls change under it on a
  running server. The five set functions alone are replaced by an
  aggregate of their name.
- **Privileges.** None is needed to use a plug-in type. SQL-92's
  privileges are on schema objects [§4.26, §11.36 SR1] (USAGE on a
  domain, character set, collation or translation), granted by an owner;
  a plug-in type has no schema and no owning authorization identifier,
  registered by the program like code, like a built-in type. A privilege
  on one would be an extension with no grantor.
*Why:* a standard type is grammar as much as storage, and none is
replaced; one contract still holds every type, built in or not, to the
same responsibilities, and plug-in types are stored, logged, indexed and
sent like built-in ones.

**D54. Datetimes and intervals: UTC, the session's zone, a clock.**
- **Times.** A time or timestamp is kept as its UTC instant, to the
  microsecond, as ISO/IEC 9075:1992 §4.5.1 says, plus its displacement if
  WITH TIME ZONE. Without, the session's displacement is implied [§6.1
  GR5]: retrieved values show in it, so differ between zones. A date is
  its days.
- **Session zone.** The setting SESSION.TIME_ZONE: the database's by
  default, `+00:00`, or the session's own, set by SET TIME ZONE [§16.5]
  or as any setting; SET TIME ZONE LOCAL restores the database's.
- **Intervals.** Months (year-month) or microseconds (day-time), in 128
  bits, so a leading field of maximum precision, 9 digits, and the sum
  of two never overflow. The qualifier is the type's. Arithmetic uses
  these units; a datetime's year and month move by months as shown
  [§6.14 GR3].
- **Statement time.** A statement reads the clock's instant and its
  session's displacement once, at its start: every CURRENT_DATE,
  CURRENT_TIME and CURRENT_TIMESTAMP in it is that instant [§6.8 GR3], a
  plan constant, as is the current date a CAST of a time to a timestamp
  reads. CAST to or from a datetime is bound with them; `SqlType::cast`,
  given none, uses UTC.
- **Defaults.** A default's datetime literal without a displacement takes
  the defining session's [§5.3 SR18], or, in a schema the operator
  defines, the database's SESSION.TIME_ZONE. It is logged with the
  definition (`Change::Schema`, `Defined`, `AlterTable`), so recovery,
  with no session, makes the same default.
- **Clock.** A plug-in (`Database::set_clock`), the system's by default;
  tests set one that stands still (R-TEST-5).
*Why:* a server's results must not depend on the host's zone, nor a
statement's datetimes on when within it each is evaluated; a time without
time zone is kept as SQL-92, the implemented standard, says, until a
later standard's reading is taken.
**D58. Dynamic SQL in SQL text: a session plays the module.**
- **Statements.** ISO/IEC 9075:1992 §17's statements run via
  `Session::execute` like any `<SQL procedure statement>`, the session
  as `<module>` (D32 claims no module language): PREPARE,
  EXECUTE, EXECUTE IMMEDIATE, DEALLOCATE PREPARE, DESCRIBE, ALLOCATE,
  DEALLOCATE, GET and SET DESCRIPTOR, ALLOCATE CURSOR, and the dynamic
  OPEN, FETCH, CLOSE and positioned DELETE and UPDATE. A
  `<dynamic declare cursor>` goes through `Session::declare_cursor`.
- **Values.** They are the program's parameters (`Params`), written as
  in SQL-86, a declared `<identifier>`, where §17 has a
  `<parameter name>` or embedded variable. A parameter's `<identifier>`
  in place of a statement or cursor name makes an extended name [§5.4].
- **Direct SQL.** No parameters [§20.1 SR1], so only ALLOCATE,
  DEALLOCATE and SET DESCRIPTOR, and EXECUTE IMMEDIATE are useful: their
  operands may be `<literal>`s. PREPARE's text and extended statement or
  cursor names may not [§17.6 SR1, §5.4 SR13].
- **Scope.** Prepared statements, allocated cursors and descriptor areas
  are the session's, by name; `<statement name>`, `<cursor name>`, and
  LOCAL and GLOBAL extended names are separate [§5.4 GR12-GR16]. They
  survive transactions [§17.6 GR10] until deallocated or session end
  [§20.1 GR2b].
- **Transactions.** PREPARE prepares as `Session::prepare` does (D45).
  EXECUTE and EXECUTE IMMEDIATE run the statement as `execute_prepared`
  does, as its own statement, beginning a transaction as the prepared
  one would [§4.22.6]. Other §17 statements are transaction initiating.
- **Positioned.** A prepared positioned DELETE or UPDATE finds its
  cursor, declared or allocated, when prepared [§17.6 GR2]; one naming
  no table takes its cursor's [§17.19 SR1, §17.20 SR1].
- **Refusals.** PREPARE and EXECUTE IMMEDIATE refuse statements with a
  `<comment>` [§17.6 GR4q] or a missing cursor (SQLSTATE 34000). The
  API's prepare (D45) accepts both: drivers' SQL often has comments,
  and a positioned statement may be prepared before its cursor, found
  when it runs.
- **Implementation-defined.** A descriptor area allocated without WITH
  MAX has 100 occurrences, at most 1,000 [§17.2 SR2, GR3]. A session
  holds 64 areas; more are refused as beyond a limit (D18). A plug-in's
  type (D53) has TYPE code -1 [§17.1 Table 18].
*Why:* SQL-92 defines dynamic SQL for module procedures; a session
already plays that part for cursors and parameterized statements, so
the standard's meaning holds and programs and drivers need only SQL.

**D55. Threads: one manager, each thread labelled and shown.**
- **One manager.** All threads of the database, server and command-line
  client, and all background work, start through `src/threads.rs`,
  never `thread::spawn` elsewhere (R-SAFE-5).
- **Per thread.** Label `turborel-<kind>[-<n>]`, kind, database, state
  (starting, idle, running, stopping, stopped, failed), start time, last
  run, run count, last error, restarts.
- **Jobs.** Background work (checkpoints, maintenance, the log's syncs,
  D25) runs as jobs under a per-database policy: automatic (own thread,
  at once or at the time asked); manual, only on `Database::run_due`
  (tests, operators choosing the time); or paused. Errors become events (D40); a panic is caught, reported, and
  the job restarted per policy.
- **Lifecycle.** A job starts, reported, when its database opens or it
  is first due; none starts once the database is closing. A failed
  close leaves jobs as they were. A database's threads are all joined
  before its files and lock are released.
- **Last handle.** No job drops its database last: the program's
  handles (sessions' included) are counted apart from jobs' working
  handles. Dropping the program's last one stops and joins the jobs,
  awaiting work in progress, so the lock is free when the drop returns.
- **Server.** The manager builds the server's runtime: named threads,
  thread count and blocking pool from settings (D42); statement workers,
  listeners and connections counted; refused connections bounded. The
  client's interrupts are their own job, its cancel time-bounded (D46).
- **Turns.** Job turns (checkpoint, maintenance) refuse a thread already
  holding them, like the definitions' turn (D27), so no lock is held
  across a plug-in's call (R-ERR-4).
- **Shown.** INFORMATION_SCHEMA.JOBS lists threads and jobs with state,
  to the database's owner; events report starts, stops, failures and
  restarts.
*Why:* ad hoc threads, oddly named or unnamed, were invisible to
operators, outlived their purpose, swallowed errors, and made tests
timing-dependent. One manager makes them visible, bounded and
stoppable, and lets tests and operators choose when work runs.

**D56. Files: one manager a database, its open files limited, each shown.**
- **One manager.** Every file a database opens, creates, removes or
  locks goes through its file manager (`src/files.rs`), which wraps the
  database's `FileSystem` (shadowing and test file systems stay below).
  Each file records its owner (the database, an engine or
  index method by name, a spill, the log), kind, path, mode, bytes,
  durability class, and read, write and sync counts.
- **Classes.** Durable (catalog, data file, log); durable at commit
  (shadow files); unsynced (undo log, begun anew at each open); scratch
  (spill files, removed when dropped, cleared at each open). The class
  decides what create, remove, sync, close and open do; a directory made
  or emptied is synced in its parent only where durability needs it.
- **Limit.** FILES.MAX_OPEN caps a database's open descriptors, shared
  by log and undo segments, spill files and the rest: 1024 by default,
  0 for none. Past it the least recently used file is closed, reopened
  on next use (LRU). Files mid-read or mid-write stay open; if all are,
  a file needing a descriptor waits. A failed reopen fails the
  statement or job and is reported as an event.
- **Sync first.** Files with unsynced writes are closed last, synced
  (`Durability::Data`) first: a writeback error no sync saw is lost with
  the descriptor (on Linux a later descriptor's sync need not report
  it). If that sync fails, the file stays open and the failure is
  reported and kept for its next sync, of any durability, to return, so
  no later sync covers the lost writes. At close such a file is synced
  first, failures reported.
- **Engines.** Engines and index methods get the manager through `Host`,
  like the memory budget (D38): their own directory under the
  database's, shadowed or not as they declare, and scratch files, all
  shown, limited and closed like the database's.
- **Shadowed.** A plug-in declares its directory as written (the
  default) or shadowed (`PluginFiles::declare`) as it is registered,
  before asking for it.
  - What it writes, makes and removes there reaches the disk only at a
    checkpoint (`disk/shadow.rs`): held while engines capture, sealed
    after them, committed before their `checkpoint_finish`, beside the
    data file.
  - Recovered as the database opens, before its tables: after a crash
    the directory is as the last checkpoint left it.
  - Each file commits on its own, through side files `NAME.shadow0` and
    `.shadow1` (names refused there), shown as SHADOW, durable at commit,
    never exported. A database in memory keeps it as written.
- **In memory.** An in-memory database's files, scratch too, live in
  `MemoryFileSystem`, bytes reserved from MEMORY.BUDGET_BYTES, never in
  the system's temporary directory.
- **Permissions.** On Unix, files 0600 (owner read/write only),
  directories 0700.
- **Lifecycle.** Open clears scratch and recovers durable classes.
  Close, after joining threads (D55) and before releasing the
  directory's lock, syncs each file per its class and closes it,
  removes scratch, and reports anything still open or not removed.
- **Shown.** INFORMATION_SCHEMA.FILES lists each file open or known, to
  the database's owner. Events report failed opens, removes and syncs,
  and the limit closing files faster than they are reused.
- **Server files.** The server's diagnostics log, socket, and TLS and
  key files are registered, shown and counted as the server's, outside
  the database's limit.
- **Encryption.** While anything is encrypted (D48), events carry no
  statement text (a slow statement shows only user and time), so no
  value meant for an encrypted table reaches a log in the clear.
*Why:* files were opened ad hoc, through the `FileSystem` or around it;
a server's descriptors were unbounded and unshown; failed rotations,
leftover files and unsynced directories went unseen. One manager makes
each file visible and bounded, and gives engines what the database's
own files have.

**D57. Replicated state: the log, the catalog and the durable files, as
opaque bytes.**

*Decision:*
- **State.** All durable state is in the `Log`, the `CatalogStore`, or
  files registered as durable (D56). A feature keeping it elsewhere
  declares an export hook or is refused when replicated (SQLCODE -910).
- **Opaque bytes.** Replication orders, stores and moves log records,
  checkpoint images (catalog records) and durable files undecoded. Only
  the core's one replay, `Database::replay` (run by recovery and by a
  standby), interprets and applies them, so a new kind of record,
  object or file is replicated once recovery handles it.
- **Traits.** Extension points, in crate `turborel-replication` (std
  only): `Consensus` (a protocol as a
  deterministic state machine), `Transport`, `StateTransfer`
  (moves an export, the core's `StateSource`) and `Journal` (keeps what
  a consensus persists). First implementations: Viewstamped
  Replication, TLS over TCP, whole files, a journal in a core `Log`.
- **Standby.** The core's seam, `Standby` (`db/standby.rs`), which
  `Database` implements; the core never depends on the replication
  crate:
  - `replay`: the one replay, as recovery runs it; the database then
    follows, until promoted;
  - continuous: records end anywhere, so a transaction's are held until
    the record ending it; the rest replay at once, as recovery's;
  - `promote`: it ends each transaction left open, an abort record
    appended and synced to its new log, then serves over that log
    ([VRR §4.2]: "Operations that had not committed might also
    survive"); `depose`: it stops until opened again;
  - following or deposed, it refuses what it would log (SQLCODE -917),
    as a failed sync does (D25), abort records too; reads go on;
  - export and import come with state transfer.
- **Replicated log.** `ReplicatedLog`, a `Log` over a `Consensus` and a
  `Journal`: its positions the consensus's, `sync(p)` returning once
  `p` is committed. Of one replica for now (f = 0, a quorum of one, as
  the paper's §2.2 counts them), driving its consensus itself;
  `ReplicatedLogs` keeps it as a database's log, its journal the file
  log in `log/`.
- **VR.** From Liskov and Cowling's "Viewstamped Replication
  Revisited", departing for durability:
  - a record is durable before PREPAREOK;
  - the view-number is written before STARTVIEWCHANGE, and with v′ and
    the new log, at once, before a replica is normal in a view;
  - a replica rebuilt from its journal rejoins its view, unless it was
    that view's primary, which waits for the next;
  - a new primary serves once the log it took over is committed and
    delivered.
- **Approved with it.**
  - a `Log` for commit beside a content-blind driver of the traits, and
    the core's `Standby` seam (replay, promote, depose, export, import);
  - a record durable before a backup acknowledges it;
  - a sync in doubt never stamped (D25); when replicated, resolved by
    consensus or the node deposed;
  - a `node` property for settings never logged (D36);
  - reads on the primary confirming its view;
  - no at-most-once for SQL clients in alpha;
  - stale snapshot reads on backups, with a least position;
  - state transfer by the durable files the file manager knows;
  - the traits and VR in their own crate, the network in the server.

*Why:* most of SQL-92 and all later SQL is still to build, with new
records, objects and files; none may need a replication change.

*Consequences:* checks fail the build or `verify`:
- replay matches on `Change` with no wildcard arm (R-CF-3), so a new
  kind does not compile until replayed;
- `record::KINDS` lists every kind; one test fails until a new kind is
  listed, another until a statement writing it is shown recovered to
  the state it left;
- every registered durable file is in the export manifest
  (`Database::export_manifest`) or carried by the log or catalog; a
  directory file of no known kind is refused;
- contract tests of `Consensus` and `Transport` run against each
  implementation (R-SAFE-3);
- the log suites (`support/src/logs.rs`: the `Log` contract,
  durability, recovery, reopening) run against every `Log`, built in or
  replicated;
- VR runs hundreds of seeds of a deterministic simulator
  (`turborel-test-support`: frames delayed, lost, duplicated, reordered;
  partitions, crashes, failing disks), checking that committed records
  survive view changes in order, at most one primary serves a view (the
  one it names), and the group converges once faults stop;
- the simulator also runs real databases, one a replica, through
  `Standby` alone, the primary's taking SQL: after each run every
  backup holds what the primary holds, rows and catalog.

*See:* D25, D36, D55, D56; `db/recovery.rs`, `db/export.rs`,
`db/standby.rs`, `files/classes.rs`, `log/record/kinds.rs`,
`replication/src`.

**D27. A thread-safe core over snapshots of the definitions.**
- **Sharing.** A `Database` is shared across threads; extension traits
  are `Send + Sync`; plug-ins guard their own state.
- **Snapshots.** The definitions (catalog, storage and index handles,
  plug-ins, settings) form an immutable snapshot. A statement keeps the
  one it started with, an updatable cursor its OPEN's, holding no
  definitions lock. A change copies, edits and publishes the snapshot;
  changes take turns.
- **`create_index`.** On a versioned table it waits for no transaction:
  the index is maintained while built; a transaction that wrote the
  table earlier is refused at its next statement (SQLCODE -911).
- **Refusals.** A writer whose snapshot predates a change to its table
  (SQLCODE -911); a plug-in calling into its own session (SQLCODE -910).
*Why:* schema changes and queries never wait for each other, and no
lock cycle can form through the definitions.

**D28. Transactions may wait for locks; deadlocks are refused.**
- **Waiting.** LOCKS.WAIT says whether a statement meeting a lock waits
  or is refused at once (SQLCODE -911). LOCKS.TIMEOUT_MS, a session
  setting in milliseconds, bounds the wait, then refuses likewise.
- **Deadlock.** A wait closing a cycle, or on the thread's own call, is
  refused at once. The statement is undone; its transaction goes on.
*Why:* sessions on their own threads, as in a server, should wait; a
program driving several sessions from one thread must not.

**D29. Engines own concurrency, at logical timestamps.**
- **Timestamps.** Transaction ids and commit timestamps come from
  increasing counters: logical, never wall-clock.
- **Versions.** An engine isolating transactions itself keeps row
  versions: reads are as of the snapshot, a row's second writer is
  refused (SQLCODE -911), and commit checks nothing read has changed
  (serializable). Unreadable versions are removed as transactions end,
  when alone the horizon moves: in tables each touched and any table
  with versions awaiting collection. A version an undone change kept
  waits only for the other transactions then open in its table.
- **Commit.** Atomic across engines: each validates at the commit
  timestamp, then each commits at it. A transaction that wrote nothing
  commits at its snapshot, unchecked.
- **Indexes.** Entries are counted per version; a row found through an
  index is kept only if its visible version has that key.
- **Table locks.** The coordinator's table locks (D28) remain for
  engines leaving concurrency to it; the built-in row store isolates
  transactions itself.
- **Lower levels.** Below SERIALIZABLE [ISO/IEC 9075:1992 §4.28, §14.1
  GR5] a transaction reads at one snapshot (REPEATABLE READ), or a new
  one per statement while its session has no open cursor (READ
  COMMITTED; READ UNCOMMITTED, which reads nothing uncommitted). Reads
  are unchecked at commit (write skew commits), but writes over rows
  written after the snapshot are refused (no lost update), and
  constraint checks and referential actions stay serializable, their
  reads checked.
- **Read mode.** Engines learn the read mode via
  `TableStorage::read_mode`; one ignoring it, or leaving concurrency to
  the coordinator, reads as SERIALIZABLE, showing no phenomenon any
  level forbids.
*Why:* only the engine knows its versions and rows. Readers and writers
never block each other; writers meet only on the same row.

**D30. A storage foundation of independent components.**
- **Components.** Below every engine, log and index, unaware of them:
  - portable file I/O (never mmap);
  - a block manager of checksummed, append-only blocks;
  - a pager of checksummed pages of any content length, owning
    allocation;
  - a buffer pool with pluggable eviction;
  - an encoding pipeline (compression, encryption) for blocks, pages,
    and what an engine stores of a table (D48);
  - hash functions as a plug-in (CRCs for checksums, MurmurHash3 for
    hash tables).
- **Codecs.** Hash functions and encodings are registered with the
  database. A file's header names those it was written and is read
  with; a file added to others is written as they were.
- **Pool hits.** A hit writes one shared thing, its frame's latch,
  found by a lock-free table of hints; the latch bars eviction, which
  takes it exclusively. Hits are counted on per-thread stripes.
- **Eviction.** A latched frame is busy briefly, as is an unpinned one
  the policy declines (CLOCK spares frames reused as it sweeps). Finding
  only busy and pinned frames, eviction waits a bounded while for the
  busy, then is refused as if all were pinned (D38): the latch may be
  its own thread's. Bound: some 20 ms of an otherwise idle pool, so a
  CPU-starved latch holder on a busy server does not get the eviction
  refused while other reads proceed; half a second at most overall.
- **Deferred frees.** A page freed once no one holds it, by a structure
  whose readers lock nothing, waits a bounded while for its latch,
  longer for its pins, then is refused (SQLCODE -910) naming the page:
  the hold may be the caller's own, which the pool does not track.
- **Hot pages.** They still contend on the latch; a reader-biased one
  (BRAVO) would need a reviewed exception to R-SAFE-1.
- **Read-ahead.** A forward B-link tree scan reading a leaf that follows
  the last one in the file reads ahead (`BufferPool::read_ahead`): on a
  miss, the next pages (STORAGE.READ_AHEAD_PAGES, 32 by default, a
  change taken at once) in one file read, into their own frames, at
  most an eighth of the pool's. It skips
  pooled pages, frames not free at once, and pages not whole in the
  read (overflowing, freed, torn), so it never fails a read; none during
  an undo. Synchronous, it saves file reads, not waits; a read-ahead
  thread would be a thread manager job (D55).
- **Portability.** File formats are the same on every platform.
- **Counters.** Each component counts its work for the heuristics above.
*Why:* each layer can be tested, fault-injected, benchmarked and reused
alone. mmap leaves write order to the operating system, breaking
write-ahead logging.
**D31. The built-in engines: a row store and a column store.**

- **Engines.** Whole engines are the extension point. Two are built in,
  one code base over one data file, sharing one value codec, record
  format, statistics module, buffer pool, MVCC and row ids:
  - `rowstore` keeps each row's values together;
  - `columnstore` keeps rows as written, then freezes them into groups
    of each column's values (below). Its files are the row store's,
    which checkpoints them for both.
- **Choosing.** A table chooses by USING, else TABLES.ENGINE, and moves
  by ALTER TABLE ... USING.
- **Next store.** A hybrid (PAX), each page holding a group's rows with
  their columns apart. The format of frozen groups becomes a trait then.

*Row store: tree and versions*

- **Tree.** A table is a B-link tree (Lehman and Yao, 1981) keyed by a
  logical 64-bit row id, handed out in blocks per thread. Long values
  are kept out of line.
- **Leaf split.** The high key kept is the shortest that separates the
  halves (suffix truncation, Bayer and Unterauer, 1977), so upper levels
  hold short keys.
- **Empty leaf.** Removed (after Lanin and Shasha, 1986): its range goes
  to its right neighbour, under its parent's and left neighbour's locks,
  leaving a link to that neighbour for whoever reaches it. Its page is
  freed once every operation that began before has ended, counted by
  epoch on stripes by thread.
- **Bulk load.** A tree built whole, nothing else using it, is loaded
  in key order: full leaves left to right, to nine tenths of a page,
  then the levels above.
- **In place.** A value replaced by one of the same length (as when a
  version is stamped committed) is overwritten, so a commit stamping its
  versions does not compact a leaf over and over.
- **Undo.** The newest version of a row is in the tree; older ones are
  in an undo store, a log. A database in a directory keeps it on disk
  (`undo/`), never synced and begun anew at open, as no reader outlives
  the process, so a long reader holds disk, not memory; a database in
  memory keeps it in memory. On disk, versions are written in batches
  of one record each (`OpenOptions::undo_batch`), read from memory until
  written.
- **Unpinned reads.** The maintenance thread reads older versions with
  no transaction open to pin them (`RowTable::unpinned`):
  - one released and truncated as it reads, by a rollback or a
    collection, makes it read the row's entry, and the table's latest
    collection, again, and try again while either changed;
  - a zone sum reads at that collection when it is past the horizon it
    was given, so it never follows a version released before it began;
  - so truncation never waits for it, and a commit takes space back at
    once.

*Scans, zones, counts and statistics*

- **Pushdown** (`TableStorage::scan_where`, `storage::Conjunction`,
  `logical::pushed_conjuncts`). A scan is given the columns needed and
  these conjuncts of a filter:
  - a column compared with a constant (a BETWEEN as two);
  - IS NULL or IS NOT NULL; IN or NOT IN constants, none null;
  - an OR of those, of one column;
  - LIKE a constant pattern, as bounds on its leading characters: those
    with the last one less, and one more. Strings beginning with them lie
    between, whatever follows, padded with spaces or not.
- **Safety.** Nothing is pushed if the filter can raise an error. A LIKE
  of a constant pattern and escape cannot when the pattern has a valid
  partitioning by the escape [X3.135-1986 §5.14 GR2a ii].
- **Contract.** Each conjunct holds for every row the filter keeps, so
  an engine may leave out rows they do not match, and count as read only
  rows matching all. The filter still judges every row given.
- **Skipping.** The row store skips zones, the column store frozen
  chunks, that any conjunct rules out, leaving other rows to the filter
  rather than judging them twice. So a scan skips by a conjunction, a
  range, an IN list, IS NULL where nulls are kept together, a LIKE
  prefix of a column in the rows' order, or an OR of ranges.
- **Indexes.** Probed by the same comparisons, LIKE prefixes, IN lists
  and ORs of one column's ranges (D34).
- **Zone maps** (`rows/zones.rs`). Per block of 1024 row ids, for each
  column that is a number, datetime, interval or character string of at
  most 32: least and greatest value, and whether one is null. A scan
  reads only blocks its conditions may match.
  - Keyed by row id, as rows never move between ids: leaf splits touch
    no zone. A write widens its zone before it is made.
  - An update, a deletion, or an undone insert (its row removed) leaves
    a zone stale. The maintenance thread sums it up again from each
    version of its rows a reader at or after the horizon may read, an
    open transaction's too, keeping it stale while such a version is not
    its row's newest; a write meanwhile discards the sum.
  - So a zone is never narrower than what a snapshot may read.
- **Zones saved.** Kept in memory, each locked alone, and with the
  table's pages: each checkpoint, writers held, saves zones changed
  since the last, in segments of 64, each a page sealed as the table's
  rows are (`zones/saved.rs`), listed in a directory the meta page names.
  - An opened table reads them with no pass over its rows; recovery
    widens them as it writes.
  - A segment whose encodings are not registered, and the zones of row
    ids no segment holds, are unknown until summed up.
  - A range or IN list of a column in the rows' order, or IS NULL where
    nulls are kept together, reads few blocks; a scan skipping nothing,
    and an insert, pay a little for the zones.
- **Row count** (`rows/counted.rs`). The count of rows whose newest
  version is not a deletion is kept with the pages: each checkpoint,
  writers held, writes it to the meta page if changed.
  - A shadowed file holds after a crash exactly what its last checkpoint
    captured, so the count is read with no pass. Recovery moves it by
    whether each newest version written replaced a row, so a change the
    pages already hold does not move it: exact after a crash as after a
    close.
  - A file not shadowed may hold pages written after its last
    checkpoint, so its tables count rows by a pass.
- **Scan pricing** (`TableStorage::scan_share`,
  `logical::pushed_conjuncts`, and the executor's same). By the share of
  rows the pushed conditions may match:
  - row store: rows of matching zones, from at most 256 zones evenly
    apart, so planning costs no more as a table grows;
  - column store: matching frozen chunks, plus every row of its tree.
  - So a range of a column in the rows' order runs by a skipping scan;
    one of scattered values keeps its index.
  - The filter over such a scan is estimated to keep its conditions'
    share of the table's rows, never more than the scan reads.
- **Statistics.** The planner asks through a trait, every answer
  optional: rows, widths, distinct values, nulls, selectivity, from a
  sample of each column. Character strings are sampled by their first
  64 characters, bit strings by their first 512 bits, so long values
  take no more memory than short ones.
  - The row store gathers them again once a table has changed enough, as
    maintenance (below), so no commit pays for a pass.
  - An opened database's table has none until asked; asking makes them
    due and notes the table for maintenance. So no plan waits for or
    makes a pass, and planners asking at once cause one pass.

*Column store*

- **Delta.** The tree is the table's delta, written as a row store.
- **Freezing** (`engine/columns`). Once COLUMNSTORE.FREEZE_ROWS rows past
  the last group are committed before every snapshot (and
  COLUMNSTORE.FREEZE_DELAY seconds ago), the maintenance thread freezes
  up to COLUMNSTORE.GROUP_ROWS of them (2^16 by default) into a group:
  its row ids, and each column's values in chunks of a page each.
  - Zone maps: least and greatest values and nulls of each chunk and of
    each 1024 rows (a part, `group.rs` PART_ROWS), so a compressed chunk
    is skipped as finely as a plain one.
  - Every page is sealed by the table's ENCODINGS, its number the
    context.
- **Encodings.** A chunk takes whichever encoding is fewest bytes, and
  holds as many values as fit its page that way:
  - plain, or a constant;
  - frame of reference of 8-byte fields (read as signed integers, so
    any type of that width), each difference bit-packed in the bits its
    span needs (`columns/packed.rs`);
  - runs of one value of such fields, each run's value and end
    bit-packed, a null continuing the run before it (`columns/runs.rs`);
  - dictionary, each code bit-packed in the bits its entries need
    (`columns/encoded.rs`: 12 texts take 4 bits a code, not a byte);
  - ALP for approximate numbers, FSST for VARCHAR texts (below).
  - COLUMNSTORE.COMPRESSION = NONE keeps them plain.
- **ALP** (Afroozeh et al., SIGMOD 2023; `columns/alp.rs`). Each value is
  n / 10^e, n bit-packed as in a frame of reference, e the one leaving
  fewest exceptions over the first 64 values; exceptions are kept as
  they are. n / 10^e must equal the value bit for bit; no cast; integers
  exact up to 2^53. Read a run at a time into a scan's vector of
  approximate numbers (`Floats`).
- **FSST** (Boncz, Neumann and Leis, VLDB 2020; `columns/fsst.rs`). Each
  text is one-byte codes into a table of at most 64 symbols of 1 to 8
  bytes; a byte matching none is escaped.
  - The table is chosen once per group's column from 16 runs of texts
    taken evenly through it (every n-th text misses kinds of pattern),
    in FSST's five rounds, symbol counts packed in a word, no
    allocation. Each chunk holds it, 576 bytes at most.
  - Decoding goes a symbol, a word of 8 bytes, at a time; a range is
    checked UTF-8 at once, each end on a character boundary.
  - Texts take fewer pages; loads and freezes pay to choose the table.
- **Sort order.** A group's rows are in row id order, or, as frozen or
  rewritten, sorted by the columns COLUMNSTORE.ORDER names: at most 8,
  ascending, nulls as ORDER BY puts them, row ids breaking ties.
  - A sorted group keeps its ids in its rows' order and its rows'
    positions by id, so a row is found by its id.
  - A scan by a sort column skips the rest of the group
    (`bench/analytics.rs` compares the orders).
  - The group header records its sort columns. Once the option names
    others, the maintenance thread rewrites each group to match, two a
    call, until none is left, after a restart too.
- **Ordered scans** (`TableStorage::scan_ordered`, `frozen/ordered.rs`).
  An ORDER BY of the option's first columns, each ascending, is answered
  by a scan in their order, no sort planned. A heap merges, least key
  (as ORDER BY sorts) first:
  - each group's rows a vector at a time, only runs a condition may
    match, less those with a tree entry;
  - the tree's visible rows, sorted.
  - A group sorted otherwise (option changed), and a scan the plan asked
    for that its table no longer gives in order, are read and sorted.
  - So only the merge's work remains of the sort.
- **Bloom filters** (`columns/bloom.rs`). For columns COLUMNSTORE.BLOOM
  names (at most 8), each part of each chunk keeps a filter of 1280
  bytes, 10 bits a row:
  - each non-null value sets 7 bits from its bucket (`value::bucket`,
    the form values that compare equal share), hashed by MurmurHash3
    x64-128 (D30), the i-th h1 + i × h2 of its halves (Kirsch and
    Mitzenmacher, 2006). So 1.5, 1.50 and 1.5E0, and 'a' and 'a  ', set
    the same bits;
  - filters sit on pages of their own, read only by a scan for an
    equality or IN on the column, after the parts' bounds. A lookup of
    unordered distinct values, which bounds cannot narrow, reads only
    parts that may hold it; 10 bits and 7 hashes pass an absent value
    about 0.8% of the time.
  - A group whose filter columns differ from the option is rewritten,
    as for ORDER.
- **Column options** (`Kind::Columns`). An option naming columns must
  name the table's, and a column it names cannot be dropped while it
  does.
- **Directory** (`group/summary.rs`). Groups are immutable, their row id
  ranges apart, found through a directory the meta page names. It keeps
  each group's summary: row id range, rows, sort and Bloom columns, and
  each column's nulls and least and greatest values. That is all a table
  holds of a group in memory; a scan passes unmatched groups unread.
- **Header cache** (`columns/cache.rs`). A group's header is read when
  needed and held while recently read, so a busy header is not decoded
  each time. A store's tables hold at most `HEADER_CACHE_BYTES`
  (16 MiB), reserved from the memory budget a MiB at a time; least
  recently read goes first; one the budget cannot fit is read each time.
- **Frozen rows changed.** A row's newest state is its tree entry if
  any, else its group's values, seen by every reader. So a changed
  frozen row is written as any row, over a version holding its frozen
  values; its deletion stays in the tree until the group is rewritten.
- **Rewrite.** Once the tree's entries in a group's row id range are
  COLUMNSTORE.REWRITE_FRACTION of its rows (checked only after enough
  frozen rows have entries), the group is rewritten with the entries
  committed before every snapshot, replaced in the directory, and the
  entries removed as a freeze removes its rows'.
- **Scan merge.** A scan merges tree and groups in row id order, a group
  a vector (2048 rows) at a time, only the chunks each condition may
  match. A sorted group is read whole as it is taken up, then put in row
  id order.
- **Concurrency.**
  - Frozen rows' entries leave the tree once no reader holds the
    directory from before their group.
  - A checkpoint is not captured during a freeze.
  - A deletion collected while a freeze under way may freeze its row
    keeps its entry, so the row is not seen again.
  - A write finding the entry it read removed is retried once, its row
    then its group's.

*Maintenance and costs*

- **Maintenance** (`db/maintenance.rs`, `TableStorage::maintenance_due`,
  `maintain`). Work an engine leaves for later: statistics, and the
  column store's freezes, rewrites and entry removals. No transaction
  waits for it.
  - A transaction's end notes a table with work due and asks the
    maintenance thread, started the first time.
  - The thread has each noted table do some, one at a time, under the
    gate, so a checkpoint waits for a few groups at most; it goes round
    again while any did some. A table due that did none is retried a
    second later, so a freeze waiting out COLUMNSTORE.FREEZE_DELAY needs
    no later commit.
  - `Database::maintain` does every table's now.
  - Like the checkpoint thread (`db/background.rs`), it holds the
    database weakly and ends when it is closed or dropped.
- **Log.** Unchanged, of whole rows; recovery writes them to the tree.
- **Cost model.** A scan is priced by the columns it reads:
  - a tree row 0.8, plus up to 0.2 as all its columns are read;
  - a frozen row 0.25, plus up to 0.75;
  - a fetch by id 4.4 for a table the buffer pool holds, rising as it
    holds less, to 14.6 for one it holds none of; 10% more when all rows
    are frozen. Held share is of the table's bytes (rows by width) or of
    its file's pages in use if more, as a file's tables and indexes
    share its pool.
  - The weights follow the costs `bench/analytics.rs` compares; a
    mispriced fetch picks an index where a scan costs less, or the
    reverse.

*Batch execution*

- **Batches** (`TableStorage::scan_batches`). A scan gives rows as
  vectors of up to 2048 values per column, in no set order; by default
  by gathering rows.
- **Integers** (`storage::Column`, `Ints`). An exact number a store
  keeps as an integer is a vector of integers, nulls beside; values are
  made only when first asked for.
  - A frozen run, whole or of rows with no tree entry, decodes them
    straight from chunks (`Body::ints`): each chunk's span at once;
    whole-byte frame-of-reference widths by loops of their own, other
    widths unpacked; a dictionary's 8-byte entries read once, codes
    unpacked.
  - COUNT, SUM, AVG, MIN and MAX are fed, and comparisons with
    constants, IS NULL and IN judged, as integers scaled once to one
    scale; a running sum is checked per value as decimals check it.
- **Floats** (`storage::Floats`). From such a run, an approximate column
  is `f64`s: 8-byte fields read as for integers, their bits the number;
  ALP a run at a time.
  - COUNT, SUM, AVG, MIN and MAX are fed. SUM adds in row order, so the
    same total, checked as `arith` checks it; a sum out of range is
    redone a value at a time so it raises alike.
  - Comparisons with constants, IN and IS NULL are judged on it, an
    exact constant compared as `value::compare` does
    (`judge/floats.rs`); grouping keys are made of it.
- **Texts** (`storage::Texts`, `judge/texts.rs`). From a frozen run, a
  VARCHAR vector is one buffer of every row's text and where each ends,
  decoded straight from varying, dictionary and constant chunks, each
  checked UTF-8. LIKE a constant pattern, comparisons with constant
  strings (padded), IN, IS NULL and COUNT run on the text, and grouping
  keys are made of it.
- **No row values.** Integer, float and text vectors make no per-row
  value.
- **Codes** (`storage::Coded`). For runs of only dictionary chunks: the
  entries and each row's entry. A condition is judged once per entry, a
  row kept by its entry's result, so a text is judged once, not per row.
  Group keys are still made per row, not per entry.
- **Integer expressions** (`vector/ints.rs`). Negation, sums,
  differences and products of integers and exact constants run as
  integers at the scale decimals give. A result past 18 digits, or a
  quotient, is computed on values, which raise for it.
- **Grouping.** An integer's key is made as its value's
  (`spill::put_norm_exact`); batch values are read in place
  (`storage::Cell`); each group's rows of a batch are gathered in order
  by a counting sort into one buffer, feeding its set functions a vector
  at a time.
- **Column store batches.** Each frozen vector straight from chunks, a
  condition's columns first and the rest only for rows it keeps; tree
  entries in the group's row id range, read once as the group is taken
  up, in their rows' place; then the tree's other rows.
- **Streams** (`exec/batch/stream.rs`). A plan subtree gives rows by
  batch (a stream) if it is:
  - a table's scan;
  - a filter of one (told its condition's simple conjuncts) or of
    another stream, each batch with the rows kept;
  - a join of streams (`exec/batch/joined.rs`), the columns its keys and
    conditions read held by their batches. The right operand is read
    first, by batch if its tables allow (a column store's), else by row.
- **Hash join.** An inner hash join, and a LEFT one on keys whose
  further condition cannot raise and whose operands have no condition of
  their own (D15), build as the row path does. Held in the work area,
  each left batch probes it, making a batch of joined rows, left columns
  then right. The condition is judged for each pair of equal keys, on a
  row of the pair's values; an unmatched LEFT row is joined with nulls.
  - Partitioned: an inner join's rows are made row by row and gathered;
    a LEFT join's right operand outgrowing the work area moves to a
    spill-area table, which every left row probes in order.
- **Other joins.** Other outer joins (FULL, a RIGHT one not planned as
  a LEFT one (D35), a LEFT one whose condition can raise or whose
  operand has its own condition) and
  products join batch rows as the row path does and gather the results
  into batches, so operands are still read by batch.
- **Aggregation** (`exec/batch.rs`). Of a stream, set functions that
  count rows or take a column, none DISTINCT, grouped by stream columns
  or not at all, are fed from the vectors:
  - each a column's values, or an expression's, evaluated as for a row
    on a row of only the values it reads, no subquery in it;
  - columns and constants, negated and in arithmetic, no deeper than
    16, evaluated a vector at a time, each node once over the batch, a
    value at a time as for a row (`exec/batch/vector.rs`); the batch a
    row at a time if one raises, so the error is the row's;
  - with no GROUP BY, a vector at a time (`Accumulator::update_many`,
    `update_rows`, by default calling `update` per value): COUNT
    counting, SUM adding exact values as decimals, nothing cloned, MAX
    and MIN cloning only the best.
- **Filters** (`exec/batch/judge.rs`). A filter that cannot raise judges
  over the vectors its conjuncts that compare a column with a constant,
  test IS NULL or IS NOT NULL, IN or NOT IN constants, or LIKE a
  constant pattern (split once, matched by bytes if ASCII), LIKE last.
  - IN lists (`logical/sets.rs`, `Cond::In`) of IN_SET_LEAST (8)
    constants or more, over vectors and row by row alike, use a set of
    their buckets (`value::bucket`) made at plan time. A value is looked
    up in its bucket (text without trailing spaces) and compared as SQL
    compares: numbers by algebraic value, strings padded. The result is
    that of the OR of equalities: true, else unknown if a null is
    involved, else false. Shorter lists compare constant by constant.
  - The rest is judged on the kept rows, each made a row. A filter that
    can raise is judged whole, a row at a time.
- **Projection** (`exec/batch/project.rs`). Over a stream whose tables
  all read by batch (`reads_by_batch`, a column store table's), a
  projection of only its columns, no subquery, makes each row from the
  vectors: a column or constant as is, another expression on a reused
  row of only its values; a batch is read ahead of the cursor. Over
  other tables it reads a row at a time, as reading ahead gains nothing.
- **Groups held.** In the work area, in order of first rows. Past it,
  rows of groups not yet held are written, numbered, as a pass of
  `exec/group.rs` writes them, grouped as it groups, and merged with the
  held groups by number. Each row's key is built in a kept buffer
  (`spill::put_norm`).
- **Projected streams** (`project::computed`). A projection of a
  stream's columns is a stream of those vectors, moved. So is one of
  other expressions of its columns, no subquery: each evaluated only on
  a batch's kept rows, as integers or values a vector at a time where it
  can, else a row at a time. A batch where one raises is redone a row at
  a time, in order, so rows before the failing one come first. So a
  DISTINCT of expressions groups by batch.
- **UNION ALL.** A stream of its operands read in turn, each holding
  every column of its rows alike; a chain gathers with no frame each.
- **DISTINCT** (`batch::distinct_grouping`). Over a stream holding every
  column of its rows: grouping by every column, no set function, its
  groups the rows in order of first, past the work area too. So is a
  UNION, as a DISTINCT of its operands appended.
- **EXECUTOR.BATCHES.** ON by default, per session too: read by batch
  where a query can. OFF reads a row at a time throughout, to compare
  the two; the batch tests read each answer both ways.

*Why:* a row store suits SQL's point reads, updates and cursors, and is the
base other layouts build on. Stable logical row ids mean indexes never need
rewriting, frozen or not. Columns suit scans of a few columns of many rows.
Keeping new rows as rows, as SQL Server's deltastore, HyPer's Data Blocks
and TiFlash's delta tree do, keeps writes and key lookups as fast as in the
row store. A column store reads only the columns a count, sum or filter
needs and skips by a column in the rows' order (`bench/analytics.rs`
compares them); a row fetched by key reads a page per column.
**D34. Index methods.**
- **Plug-in.** An index method creates indexes and says which probes
  they answer. Two are built in, paged through the buffer pool (an index
  larger than memory lives in its file):
  - `btree`: a B-link tree keyed by encoded key, then row id; ordered,
    for equality and ranges. Both bounds on one column (as BETWEEN
    gives) are one probe (`Probe::Range`): keys past the upper are not
    read, so a short range costs its own keys, not the rest of the index
    (`bench/cursors.rs` compares them). A method declares range support
    by `IndexMethod::supports_range`.
  - `hash`: a linear hash table (Litwin, 1980) keyed by encoded key;
    equality only; grows and shrinks a bucket at a time; readers lock
    nothing.
- **Search.** Entries are yielded as read (`Index::search`, an iterator
  holding its index, so a cursor may outlive the call): the B-link tree
  a leaf at a time, backward for descending keys; the hash table one
  key's entries, or, probing every entry, a bucket at a time, its shape
  held while the search lives so each entry is found once (a split or
  merge due meanwhile is left to later writers). So a whole-index read
  (as an ORDER BY in its order) holds a leaf or bucket, not the index,
  and the statement checks for a cancel at each entry (D44).
- **Other types.** A probe value not of the column's type is placed by
  its anchor and each entry found checked, so an index finds exactly
  what SQL compares.
- **Planner.** It probes an index by the conjuncts of a filter over its
  table that cannot raise an error (D15) and compare one of its columns
  with a constant of the column's kind (character string, number,
  datetime, interval, bit string): equalities on its first columns, then
  on the next column a comparison; both bounds (as BETWEEN gives); the
  bounds of a LIKE with a constant pattern (strings beginning with its
  leading characters lie between them; the filter still judges the
  LIKE); or an OR of these, IN lists of constants among them.
- **ORs.** One probe per range; ranges that overlap or touch are joined,
  so no row is found twice. Rows come in range order, the index's (read
  backward, last range first), so an ORDER BY of the column sorts
  nothing.
- **Mixed numbers.** A number of the other kind than the column's is
  placed among its values (`physical/probe.rs`): an approximate one
  among an exact column's multiples of its scale, an exact one among the
  `f64`s. The comparison becomes one with the value it equals or the
  values either side, so the probe finds exactly the rows SQL's
  comparison by algebraic value keeps [X3.135-1986 §5.11 GR4].
- **Bulk load.** An index built from its table while nothing else writes
  it (rebuilt as a database opens, or for an engine the coordinator
  locks) is loaded whole (`Index::load`): `btree` sorts its entries in a
  work area of MEMORY.WORK_BYTES, spilling past it, and loads its tree
  from them; other methods insert them one by one.
- **No index-only scans:** a versioned engine has an entry per version;
  a reader must read the version it sees.
- **Settings.** A method declares and reads its own as every plug-in
  does (`IndexMethod::configurable`, D36); its indexes' options are D41.
- **SQL.** An extension, by the owner of the table's schema:
  `CREATE INDEX name ON table (columns) [USING method] [WITH (...)]`,
  `DROP INDEX name`, `ALTER INDEX name SET (...)`. The index is built
  while transactions write the table, so these run outside a
  transaction, each effective at once (like PostgreSQL's CREATE INDEX
  CONCURRENTLY). An index takes no UNIQUE: a UNIQUE constraint makes its
  own. INFORMATION_SCHEMA.INDEXES lists them.
- **Unique inserts.** A method says whether the inserted entry is its
  key's only one (`Index::insert_alone`); by default, by `rows` after
  `insert`. The B-link tree checks the entries beside it under the
  leaf's lock, answering "not alone" if it is first in its leaf (the
  leaf before is out of reach) or last while the leaf's high key begins
  with the key (the leaf after may hold one).
- **End check.** A UNIQUE constraint at a statement's end skips rows
  whose entries were alone; a row of that key inserted after was not,
  and is read itself. So an INSERT into a table with a PRIMARY KEY
  rereads only rows whose entries were not alone.

*Why:* each kind of index excels at different probes, and the planner
picks by estimate, so a new method needs no change to the planner.

**D35. The optimizer and planner.**
- **Structure.** Rules rewrite the logical plan; the physical planner
  picks access paths and join order by estimated cost from the engines'
  statistics. A rewrite is made only where it cannot change a result or
  an error (D15).
- **Built in:** hash joins on equalities; index nested-loop joins; join
  ordering of chains of products; transitive predicates; sort
  elimination for ordered indexes and GROUP BY; filters pushed into
  views and outer joins; conditions that can raise an error moved onto
  the product operand they read (D15); decorrelation of repeated
  subqueries.
- **Rules.** Plug-ins (`Database::with_optimizer`). Each, built in or
  added, has its own setting (D36): OPTIMIZER and its name upper case
  (`optimizer::switch_name`), ON by default, a session's to set. It is
  declared as the rule is added; a database value replayed earlier (the
  database reopened) is held until then. If a setting of that name
  exists but is not a rule's ON/OFF kind, the rule is refused.
- **Index costs.** From the method's own answers where given: rows a
  probe selects (`Index::estimate`); pages it reads
  (`Index::probe_pages`: a hash table's one bucket, a B-link tree's
  levels); distinct keys (`Index::distinct_keys`, for an equality on all
  its columns).
- **Key sketch.** Distinct keys are counted by a sketch kept as the index
  is written, of no more keys than it holds, stored with the index's
  locator: a checkpoint records it with the pages it commits, and
  recovery's mending changes it with the entries. An index whose locator
  holds none has none until first asked; asking makes the count due, and
  maintenance counts its keys once in a streaming pass
  (`Index::maintain`). Meanwhile planning estimates without them, as for
  table statistics (D31): no plan waits for a pass.
- **Fallbacks.** Else the table's statistics, if its engine has them;
  else the defaults PLANNER.EQUAL_SELECTIVITY, RANGE_SELECTIVITY and
  NULL_FRACTION, as for a just-opened database's tables until
  maintenance gathers statistics (D31).
- **Skipping scans.** A filter over a scan that skips zones or chunks its
  conditions cannot match (`TableStorage::scan_share`, D31) is estimated
  to keep no more rows than the scan reads.
- **Index join** (`Plan::IndexJoin`, `physical/indexed.rs`). An inner or
  LEFT outer join on equalities is an index nested-loop join when:
  - its right operand reads a table, filtered or not by a condition that
    cannot raise an error, or through a projection of its columns (as a
    derived table of them, or of such a one; an updatable view is read
    as its table);
  - an index of the table has first columns the equalities give values
    for, of the indexed column's kind (so the index places them), and
    later columns NOT NULL (an index holds no key with a null);
  - the left's estimated rows times a probe's cost (its pages, a level
    of descent each, and its rows, a fetch each) is below the cost of
    reading the hash join's right operand.
  So is a RIGHT outer join whose left operand reads a table so: the
  right's rows probe it, under a projection putting the left's columns
  first. A projection of another expression is not probed: its value for
  a row given nulls would not be null, and it would be evaluated for
  fewer rows.
- **Estimates.** A left operand that is a join is estimated as join
  ordering does: by the distinct values of the columns each equality
  compares, one pair in as many as the more distinct has values (a
  table's rows, as if a key, when its statistics lack them).
- **Execution.** Each left row probes as pulled; a null finds nothing;
  nothing is held. The engine is told each probe's rows are read
  (`TableStorage::read_by_condition`). The rest of the condition,
  unprobed equalities and the table's filter are judged on
  each pair found, so none may raise an error (D15). A LEFT join keeps a
  left row with no pair kept with nulls. The statement checks for a
  cancel before each probe and at each entry (D44). Rows come in the
  left's order, each row's matches in the index's.
- **Join order** weighs a relation's probes beside a hash join, so a few
  rows are joined first to the table they probe.
- **Build side.** A hash join builds the operand of fewer estimated rows
  (`physical/join.rs`):
  - an inner join whose right has more is planned as the join of its
    operands swapped, its left built, unless the left gives the order
    asked for;
  - a RIGHT outer join on keys is planned as the LEFT one of its
    operands swapped, probed by batches (D31), unless its left has more;
  - either under a projection putting the columns back in order;
  - an operand that can raise an error may be built: each operand is
    read whole either way, so only which error comes first changes
    (D15). A RIGHT join keeping an ON conjunct that can raise is not
    swapped: a LEFT one with such a conjunct is not probed by batches.

*Why:* the General Rules define results, not plans; any plan giving the
same results and errors is the optimizer's to choose.

**D36. Settings.**
- **Registry.** Each setting is declared once: kind, default, whether a
  session may set it, and when a change takes effect (each statement;
  each transaction; each connection a server takes, e.g. its longest
  message; or when its reader starts, e.g. a server's addresses).
  Plug-ins, the server too (D42), declare theirs the same way: every
  component is configured by settings, never a configuration of its own.
- **Plug-ins.** Every kind declares and reads its own by one hook,
  `Configurable` (`settings`, `configure`), given by its `configurable`
  (default: none):
  - the kinds: engines, index methods, functions, system tables, data
    types, hash functions, encodings, catalog and log storages,
    credentials, a server's connections, spills;
  - declared under its name as it is registered; one outside it, or a
    built-in's, refuses the plug-in;
  - told the values when registered, after each change, and once the
    database opens and replays them: from a snapshot, no lock held, so
    it never calls back (R-ERR-4);
  - a setting of effect Start it keeps as it started, recording it
    (`Settings::mark_started`).
  - So the column store bounds the frozen-group headers it holds
    (COLUMNSTORE.HEADER_CACHE_BYTES).
- **Logs.** The database's log is told LOG.DURABILITY
  (`Log::set_durability`) when given and after each change, the same
  way, from a snapshot (D25).
- **Sessions** set for themselves, over the database's value: whether
  commits wait for the log (LOG.COMMIT_SYNC, D25); whether the function
  call syntax is accepted (SQL.FUNCTION_CALLS); each optimizer rule
  (D35).
- **Scopes:** built-in default, database value, session value.
- **Tunables.** A number an operator tunes a subsystem by (a memory or
  pool size, the pages read ahead, a threshold, a background pace such
  as LOG.SYNC_AFTER_MS, a timeout, a durability) is a setting; the
  constant it was named by is its default.
- **Constants.** These stay constants:
  - formats on disk; the standard's implementation-defined values
    (CONFORMANCE.md);
  - shapes fixed when made: stripes, shards, vectors, a sketch, the
    server's runtime and the upper bounds it puts on SERVER.WORKERS and
    SERVER.MAX_LOGINS (which are settings);
  - the planner's weights an engine states for its own costs, measured
    by its benchmark, and what it spends on estimates;
  - limits guarding the reading of a format (a block's longest payload);
  - how often diagnostics report one trouble (once a minute);
  - the response to a system failure (the pause after an accept fails).
- **Names.** `GROUP.NAME`, a plug-in's under its own name. A quantity
  names its unit (`_BYTES`, `_MS`). A word means one thing throughout.
  Choices use `ON`, `OFF`, other words only where two will not do. A
  threshold names what it counts after AFTER (`_AFTER_CHANGES`,
  `_AFTER_IMAGES`). The standard's SET statements keep their syntax: SET
  TIME ZONE sets SESSION.TIME_ZONE.
- **Renames.** During alpha a name is changed in place, no alias kept
  (VERSIONING.md); so were these (old → new):

  | Old | New |
  |---|---|
  | DEFAULT_ENGINE, UNIQUE_INDEX_METHOD | TABLES.ENGINE, TABLES.UNIQUE_INDEX_METHOD |
  | TRANSACTION_POLICY STRICT/PERMISSIVE | TRANSACTION.STRICT ON/OFF |
  | LOCK_WAIT WAIT/NO_WAIT, LOCK_TIMEOUT | LOCKS.WAIT ON/OFF, LOCKS.TIMEOUT_MS |
  | SESSION_END COMMIT/ROLLBACK, TIME_ZONE | SESSION.COMMIT_AT_END ON/OFF, SESSION.TIME_ZONE |
  | CATALOG_NAME, CATALOG.COMPACT_AFTER | CATALOG.NAME, CATALOG.COMPACT_AFTER_IMAGES |
  | MEMORY_BUDGET, WORK_MEMORY, SPILL_BATCH | MEMORY.BUDGET_BYTES, MEMORY.WORK_BYTES, SPILL.BATCH_BYTES |
  | EXECUTOR.DECORRELATE_AFTER | EXECUTOR.DECORRELATE_AFTER_RUNS |
  | STORAGE.PAGE_SIZE, LOG.SEGMENT_SIZE | STORAGE.PAGE_BYTES, LOG.SEGMENT_BYTES |
  | STATEMENT_TIMEOUT, DIAGNOSTICS.SLOW_STATEMENT | STATEMENT.TIMEOUT_MS, DIAGNOSTICS.SLOW_STATEMENT_MS |
  | SERVER.LOGIN_TIMEOUT, SERVER.IDLE_TIMEOUT, SERVER.SHUTDOWN_TIMEOUT | the same, `_MS` |
  | SERVER.TRUST LOCAL/OFF | SERVER.TRUST_LOCAL ON/OFF |

- **Effect** as declared: a per-transaction one is taken as the
  transaction begins. One that waits for a restart says so where
  settings are shown: its reader records the value it started with
  (`Database::settings_started`; the server as it starts listening, the
  database's pool as it opens, or an in-memory one's as it is made),
  shown in INFORMATION_SCHEMA.SETTINGS as STARTED_VALUE, with IS_PENDING
  YES while the set value differs.
- **In memory.** A database kept in memory refuses settings fixed at
  creation once it holds a schema, table or log (as no directory will
  create it then), and `turboreld --memory` as it starts.
- **Unreadable.** A setting the server cannot read (e.g. a closed
  database's) reads as its default, never another value.
- **Persistence.** The database's values are definitions, logged and
  replayed like schema changes; a session's never persist.
- **Node settings.** A setting declared the node's (`Definition::node`,
  D57) is the node's alone:
  - never logged nor in an image, which replicas share;
  - given as the node starts (`--set`, the config file, or before the
    host opens the database), lasting while it runs;
  - SET DATABASE changes it for the running database only;
  - shown with VALUE_SOURCE NODE; never a session's nor fixed at
    creation (-910 as declared);
  - no built-in setting is one: making one so is a decision of its own.
- **At creation.** Fixed when a database is created in its directory:
  page size, checksum (STORAGE.CHECKSUM, a registered hash function),
  encodings (STORAGE.ENCODINGS), log segment size. Given before
  creation, recorded in its first image, refused after; given again at
  open, they must match. So a database opens with no options. Those read
  at open (STORAGE.POOL_BYTES), given before it opens, hold from then
  on.
- **`OpenOptions`** keeps only what the files do not fix, for the host
  program to give: the log of older row versions. The log's durability
  is a setting, LOG.DURABILITY (D25), kept with the others.
- **Privileges.** Only the database's owner (D43) changes its values; a
  session sets its own. The Rust API is unrestricted: the host program
  holds the database.
- **SQL.** SET TRANSACTION and SET SCHEMA (standard);
  `SET name = value` for a session's own values; `SET DATABASE name =
  value` for the database's, by its owner, outside a transaction, kept as
  `Database::set_setting` keeps them, so a running server is configured
  from a client. ISO/IEC 9075:1992 has neither: its session SET
  statements [§16.1–§16.5] set no setting (`parser/session.rs` gives
  their BNF). INFORMATION_SCHEMA.SETTINGS lists every setting, its value
  and its source.
- **Config file.** A source of the database's values as a server starts,
  like `turboreld --set` (D42), not a configuration of its own: each key
  names a setting, its value set as `--set` sets it. A subset of TOML
  1.0, read by turboreld's own parser, no dependency (D3);
  `turboreld/src/config_file.rs` gives its grammar.

*Why:* a server's behaviour must be visible and changeable without
rebuilding it, by the people entitled to change it.

**D37. System tables and the Information Schema.**
- **System tables.** A plug-in; rows are made from the definitions as a
  statement runs, as its user sees them.
- **Information Schema.** The standard's own view definitions [ISO/IEC
  9075:1992 §21.2], over Definition Schema tables only those views may
  read [§21.3]. SELECT on them is PUBLIC's, nothing else granted, so
  they cannot be changed.

*Why:* the standard's own views, over tables simulating its Definition
Schema, give the standard's answers with no second description of the
catalog to keep in step.

**D38. The memory budget, and spilling.**
- **Budget.** One per database, in bytes (MEMORY.BUDGET_BYTES). Every
  buffer pool reserves its pages from it when made, returns them when
  dropped; a reservation past it is refused (SQLCODE -918).
- **Full pool.** A page wanted while every frame of its pool is held is
  refused too (-918), after a moment's wait for frames latched by a read
  (D30): the pool, below every engine (D30), knows no statement to bound
  a wait by; threads holding pages while waiting for one more would wait
  for each other; the pool is too small for the work.
- **Undo.** Never refused by the budget: a failed statement's undo and a
  ROLLBACK's, with the collection after (`Undoing`), reserve what they
  need even past the limit until the undo returns it. A page they want
  from a full pool is waited for, up to ten seconds, as every other
  holder is refused and lets go, and an undo holds a few pages at most.
  A refusal would damage a table (D19).
- **Spilled lists** read back need no budget: a run's last batch, not yet
  written, is read where held, so a commit's check reads its changes
  without another page.
- **Plug-ins** get the budget when registered (`StorageEngine::attach`,
  `IndexMethod::attach`), so their caches share the limit; each owner
  keeps its own pool and eviction policy. A database in memory gives its
  engine and index methods one shared pool (as one in a directory shares
  its data file's), of STORAGE.POOL_BYTES, made with its first table.
- **Visibility.** Each reservation names its holder.
  INFORMATION_SCHEMA.MEMORY_RESERVATIONS shows the database's owner each
  one and its bytes, others none, as a spill area reveals what others'
  statements do.
- **Work area.** An operator holding rows (sort, grouping, DISTINCT, a
  hash join's right operand, EXCEPT, INTERSECT) holds up to
  MEMORY.WORK_BYTES in memory, a session may set for itself (its
  statements read its settings over the database's, D36). A row counts
  its values and what each holds apart from itself (a string's
  characters, a bit string's or a plug-in value's bytes).
- **Spill area.** Past that it spills to an area as large, reserved from
  the budget: a temporary pager file whose buffer pool is the area, so
  eviction writes it to disk. Its creation is not made durable
  (`Pager::create_scratch`), as nothing reads it after a crash, so a
  spilling sort waits for no sync. It holds tables (byte keys to values,
  in insertion order) and runs (records read back in order). Areas come
  from a provider, an extension point (`Spill`), also given to plug-ins
  through the `Host`, so one may give partitioned or parallel areas.
- **Sort.** Spilling keeps the orders the planner relies on. A sort is a
  stable external merge. Rows held, or each run, are sorted by a prefix
  of the first key beside each row's place: a number in key order (of a
  number, string, datetime by UTC instant, interval or bit string).
  Equal prefixes compare whole, or by the other keys only if every
  prefix holds its value whole (`exec/sort/prefix.rs`); faster, as the
  rows, each in memory of its own, are not read per comparison. A run
  writes each row beside its prefix, which the merge compares, reading
  runs a batch at a time (`SpillRun::into_batches`), records where they
  lie, decoding a row only when prefixes tie or a value has none, and
  when given. A pass rewriting a run copies records as they are
  (`exec/sort/merge.rs`).
- **Order.** Grouping and DISTINCT give rows as their first rows came.
- **Hash join.** Partitioned (Grace), partitions repartitioned while
  they outgrow the work area; left rows numbered so joined rows merge
  back into the left's order.
- **Partitioning.** Grouping, DISTINCT, EXCEPT and INTERSECT partition
  likewise, by their keys, once what they hold outgrows the work area.
  DISTINCT gives its held rows as they came, then the rest's first of
  each key, merged by number. EXCEPT and INTERSECT partition both
  operands alike, the second's rows as counts. Grouping numbers each
  group by its first row.
- **Depth.** Each repartitions while a partition outgrows the work area,
  up to SPILL.PARTITION_DEPTH times. Past that, DISTINCT, EXCEPT and
  INTERSECT keep its keys in a spill-area table; a hash join keeps its
  right rows there, as for a partition that does not shrink, each probe
  reading its key's rows as it joins them (`SpillTable::values`), never
  all at once.
- **Halves.** DISTINCT once partitioning, and EXCEPT and INTERSECT
  without ALL, split their work area in two, as a grouping of DISTINCT
  set functions does: half for the partitions and their keys, half for
  the bag beside them (a partition's keys past SPILL.PARTITION_DEPTH, or
  the rows given already).
- **Runs.** Written in batches of SPILL.BATCH_BYTES; an operator reads or
  writes as many runs at once as MEMORY.WORK_BYTES holds two batches of.
  A run read once returns its pages as read (`SpillRun::into_records`),
  so a merge holds only what it has still to read; one read again (a
  product's right operand, a subquery's rows) keeps them.
- **Accumulators.** A plug-in's is opaque, never written out: past
  SPILL.PARTITION_DEPTH, grouping makes a partition in passes, each
  leaving the groups it has no room for to the next.
- **Outer join on keys.** Partitioned like the hash join, both operands
  numbered: each left row with its matches or nulls, in the left's
  order, then unmatched right rows, in the right's; partitions
  repartitioned likewise. One that does not shrink, or is past
  SPILL.PARTITION_DEPTH, is joined a work-area-sized part at a time,
  the left partition reread per part, whether each left row matched kept
  in a run beside it. A right operand in memory has a matched flag per
  row, counted in the work area.
- **No keys.** A product, and an outer join on no keys, reread their
  right operand per left row, from a spill-area run once it outgrows the
  work area. A right operand in a run has no flags: for a preserved
  right, the left rows are kept in a run, and once the left is read each
  right row is tried against them again.
- **Subqueries.** Rows spill as a product's right operand; a
  decorrelated subquery's, kept by key, as a hash join's. Rows per set
  of outer values are kept while all kept fit the work area, each with
  its key and entry: rows in memory count twice (for what a comparison
  makes of them); spilled rows count as a whole work area, which with
  their key and entry always exceeds it, so they are never kept. A
  subquery with no outer references always keeps its rows, spilled or
  not, its only run.
- **DISTINCT set functions.** The values one took, in every group of a
  grouping pass, are keys of one bag, which spills; the grouping's work
  area is split, half its groups', half the bag's.
- **Row IN.** IN, NOT IN, ALL and SOME of a row, nulls in it or the
  subquery's rows or not, are answered from what is built once of those
  rows, never by comparing the row with every row: per pattern of nulls
  they have, a hash of them on the columns neither they nor the row has
  null; each column's least and greatest value; and, by the place of a
  row's first null, the least and greatest of the values before it.
- **Held rows.** Rows a statement holds beyond its operators are held up
  to MEMORY.WORK_BYTES too, then written to spill-area runs in a list
  that spills (`spill/list.rs`, `exec/held.rs`):
  - a read-only cursor's rows read at OPEN (its plan able to raise an
    error, or read before its transaction changes a table it reads),
    given back in order, each run read once, the first error after the
    rows before it;
  - a SCROLL cursor's rows as read, read back by place a run at a time;
  - an updatable cursor's row ids, in its order, sorted as ORDER BY;
  - a searched UPDATE's or DELETE's object rows (with an UPDATE's new
    values) and an INSERT's candidate rows, all chosen before any row
    changes;
  - what a data-changing statement did (each row's id, an UPDATE's or
    DELETE's old values) and the constraint checks it gathered, kept for
    its end, a UNIQUE constraint's rows checked one at a time as read
    back;
  - deferred constraints' checks, kept until made, freed at the
    transaction's end;
  - what its referential actions keep: the rows matching each row
    changed, all found before any is acted on; for MATCH PARTIAL, the
    referenced table as it was before the statement, made once per
    table, and the rows each row matched then.
- **Kept maps.** What referential actions keep by key, the values a
  statement gave [ISO/IEC 9075:1992 §11.8 GR7] and the MATCH PARTIAL
  associations dropped [GR4], is a map that spills (`spill/map.rs`): in
  memory up to MEMORY.WORK_BYTES, then all in a spill-area table.
- **Referential checks** read a row at a time and stop at the first row
  they need: whether a row has a match, and NO ACTION's rows left
  without one.
- **Reordered scans.** A scan planned in an order its table no longer
  gives is sorted as ORDER BY.
- **Spill files.** In `tmp/` of a database's directory, cleared when it
  opens; for one in memory, the system's temporary directory.
- **Undo records.** The coordinator's stay apart from the write-ahead
  log, which is only written while the database runs.
- **Transactions.** What one holds is bounded like an operator's:
  - the coordinator's undo entries (for indexes, and engines that do not
    undo their own changes): up to MEMORY.WORK_BYTES, then spill-area
    runs, read back a run at a time to undo a statement or transaction;
  - the row store tracks up to TRANSACTIONS.TRACKED_ROWS rows of a
    transaction's reads, or writes, of a table for the commit's check;
    past that it counts every row, which may refuse a commit needlessly;
  - its changes, and the garbage its commit makes: up to
    TRANSACTIONS.HELD_BYTES, then a spill area of the database's work
    areas. A change keeps the version it replaced only if that was the
    transaction's own; a committed one is already in the undo store;
  - a committed transaction's changed rows, which others' reads are
    checked against: up to as many bytes, then each column's least and
    greatest value;
  - each list spills alike (`spill/list.rs`), its segments written
    together in one run, so many small statements share a run, not a
    page each.
- **Collection** hands each version it removes to the coordinator as it
  goes, for its index entries to go (`TableStorage::gc`); an index build
  reads the garbage it needs entries for a segment at a time.
- **Recovery** keeps the row changes it reads in such a list too, undoing
  newest first, redoing oldest first, and mends the indexes in two
  passes over it.
- **Checkpoint.** Logs the index entries of versions awaiting collection
  as it reads them, after its gate opens, for the commits it captured.
  Meanwhile, or while an index is built, the table's collection is held
  back; what awaits it is read a segment at a time, its lock taken for
  each, so commits adding to it wait for one segment at most.
- **Encryption.** Spill files pass through their temporary files'
  encodings (`TempFiles::pipeline`): encrypted while the database
  encrypts anything (D48).

*Why:* a server must bound its memory, and one budget over every cache
bounds it without binding plug-ins to one page cache.

**D39. Directions for extensibility.** Beyond today's plug-ins:
- **Protocols.** The server's wire protocol is a plug-in
  (`server::Protocol`, D42), so several protocols can serve one database.
- **Data types** share one contract, `SqlType` (D53): SQL's standard
  types are built in behind it; types beyond the standard are plug-ins
  through it.

*Why:* as for engines and index methods, the core fixes the protocols
between components, and implementations are replaceable.

**D40. Diagnostics: typed events to a pluggable sink.**
- **Sink.** The database reports its life, and failures an operator must
  know, as typed events (`diagnostics::Event`) to a `Diagnostics` plug-in
  set with `Database::set_diagnostics`; by default they are dropped.
- **Database events.**
  - opened (created, reopened, or recovered, with recovery's report) and
    closed; a checkpoint; a setting changed; a plug-in registered;
  - users and the owner changed (D43);
  - a failure of its own (a damaged table, corrupt data, an I/O error, a
    failed commit, memory refused), with its error;
  - a statement whose work took DIAGNOSTICS.SLOW_STATEMENT_MS or longer,
    with who ran it and its text as written, values included. Its work
    is its call; a cursor's is its opening and fetches, reported once
    done with, not the time between, which is its client's.
- **Server events**, via its database (`Database::tell`):
  - where it listens; stopping and hurrying; failed accepts;
  - failed TLS handshakes and logins (user, peer and mechanism, never the
    proof), including Hellos refused before any authenticator runs
    (protocol version or mechanism not served, user name no identifier,
    reported as written), each slowed and counted by address as a refused
    proof is (D43), so they flood no faster;
  - connections refused past SERVER.MAX_CONNECTIONS.
  - What may flood is reported at most once a minute, counted.
- **DIAGNOSTICS.LEVEL**, the database's (ERROR, WARNING or INFO, the
  default): events below it reach no sink.
- **Not events.** A failure of a statement's own SQL (syntax, privileges,
  constraints, conflicts) goes to its client; what is counted (I/O, pool
  hits) stays in counters.
- **Delivery.** No lock is held. An event carries no row data except a
  slow statement's text, and no wall-clock time: the sink stamps it.
  `Event` is non-exhaustive, so a sink takes events added later. A sink
  never calls back into the database, and returns soon.
- **No printing** in production code (R-STYLE-7): it reports through
  diagnostics.
*Why:* a server's operators need to know what the database did and what
went wrong, in a form a program can route and filter; a library's host
needs the same without parsing text.

**D41. Table settings: options an engine declares for its tables.**
- **Engine options.** Each storage engine, plug-ins included, declares its
  tables' options (`StorageEngine::table_options`): name, kind and
  default, as a setting has, and whether a change is in place or a
  rebuild.
- **Plug-in options.** Any other plug-in declares options of every table,
  of any engine (`Database::declare_table_option`), under its own name
  ("NOTED.FILL"). An index method's indexes get their table's descriptor
  (`IndexMethod::create_on`, `open_on`); other plug-ins read the catalog
  as they run.
- **Once.** An option is declared once, by an engine or a plug-in. A
  table moving engine keeps plug-in options.
- **Undeclared.** A table reopened with an option no plug-in declares yet
  keeps it as written, listed as NOT DECLARED, and reads it as its kind
  once declared: a plug-in registered late, or no longer, never stops a
  database opening.
- **Storage.** A table's descriptor keeps values it sets
  (`TableDesc::options`), others default; `create_table` and `open_table`
  read them there.
- **SQL**, definitions logged as written and replayed:
  - `CREATE TABLE t (...) USING engine WITH (name = value, ...)`;
  - `ALTER TABLE t SET (name = value | DEFAULT, ...)`;
  - `ALTER TABLE t USING engine [WITH (...)]`: moves the table, its
    options reset to those given.
- **Applying.** An in-place change reaches the table's storage and indexes
  once published, never before commit (`TableStorage::reconfigure`,
  `Index::reconfigure`). A rebuild or engine change copies the rows into
  a new storage generation, as ALTER TABLE does for columns (D33).
- **Index options.** An index method declares its indexes' options the
  same way (`IndexMethod::index_options`), set by
  `CREATE INDEX ... WITH (...)` and kept in `IndexDesc::options`.
- **ALTER INDEX.** In place: `Index::reconfigure`. A rebuild builds anew
  under a working name while transactions write the table, as CREATE
  INDEX does, then takes the old one's place and name in one change,
  earlier snapshots reading the old. Replay rebuilds from the table's
  rows.
- **Plug-in index options.** Any other plug-in declares options of every
  index, of any method (`Database::declare_index_option`); undeclared ones
  are kept, as for tables.
- **Names.** Option names are the plug-ins' own, so a part may be an SQL
  key word ("AUDIT.LEVEL").
- **Listing.** INFORMATION_SCHEMA.TABLE_OPTIONS: every option, value and
  source of each table the user holds a privilege on; INDEX_OPTIONS, of
  their indexes.
*Why:* what suits one table (its sampling, its layout, its engine) does not
suit every table, and an engine knows best which of its choices a table may
make.

**D42. The server: three modes over one core.**
- **Modes.** Embedded (the `turborel` crate), served (`turboreld`), or
  both: a program serves its embedded database with the server's
  library, its sessions beside its clients'.
- **Crates**, each depending only on those below it:
  - `turborel`: the core, std only, synchronous;
  - `turborel-protocol`: the native wire protocol's messages and codec on
    the core's types, no I/O;
  - `turborel-client`: a blocking connection for programs that connect
    rather than embed; std only but for features `tls` and `scram`;
  - `turborel-server`: the server as a library, for any program;
  - `turboreld`: the program: command line, signals, opening and closing
    the database.
  - The server does not depend on the client, nor the library on the
    program. On the client: `turborel-cli`, the command-line client
    `turborel` (D46). On the core: plug-ins `turborel-lz4` and
    `turborel-aes` (D48). `turborel-test-support`: shared test fixtures
    (R-TEST-10), used by tests only.
- **tokio** 1.54.1 (MIT), for network I/O only, with rt-multi-thread, net,
  io-util, sync, macros and time, plus TLS's crates (D26). The program
  adds signal, LZ4, AES-256-GCM, ChaCha20-Poly1305, SHA-256, SHA-512 and
  HMAC-SHA-256 (D26, D48), and rpassword for a password typed at a
  terminal (D46). Chosen as Rust's most used and reviewed async runtime;
  its `unsafe` is its own, none written here (R-SAFE-1). The core never
  sees it.
- **socket2** 0.6.5 (MIT or Apache-2.0), feature `all`, sets keepalive
  times; tokio already takes that version and feature, so no new code.
  - tokio's socket layer, of rust-lang/socket2, co-owned by tokio's
    maintainers; its `unsafe` (socket-option system calls) is its own,
    none written here.
  - One advisory (RUSTSEC-2020-0079, unsound address layout), fixed in
    0.3.16.
  - Its README's only readiness claim is platform support: Linux, macOS
    and Windows tested per change, all working except what is behind
    `all` (probe count, reading times back). So the server's tests read
    each time back from a socket on each platform offering it.
- **Threads.** Tokio's tasks do I/O only.
  - Statements and session ends run on at most SERVER.WORKERS blocking
    workers; authentication checks on at most SERVER.MAX_LOGINS other
    blocking threads, so logins never delay statements.
  - Rows reach the connection through a bounded channel in batches of
    about SERVER.BATCH_BYTES: a slow reader holds a worker but only a few
    batches of memory.
  - One request at a time per connection, so waits for a worker never
    outnumber connections.
- **Client time limits.**
  - SERVER.LOGIN_TIMEOUT_MS (a minute by default), from accept, bounds
    TLS and authentication together.
  - Closed, session rolled back (SQLSTATE 08006), after waiting
    SERVER.IDLE_TIMEOUT_MS for a request or for the client to take what
    it is sent, or SERVER.IDLE_IN_TRANSACTION_TIMEOUT_MS for a request
    in an active transaction, locks held (both no limit by default). So
    a client that stops reading frees its worker, one that stops sending
    its connection.
- **TCP keepalive** is on for each accepted connection, so the system
  detects a client gone without closing.
  - SERVER.KEEPALIVE_IDLE_MS, SERVER.KEEPALIVE_INTERVAL_MS and
    SERVER.KEEPALIVE_PROBES (default 0: the system's), read at each accept
    (`server/src/keepalive.rs`), rounded up to whole seconds as most
    systems count, bounded by Linux's limits (32767 seconds, 127 probes).
  - Windows sets idle time and interval together: one given, the other is
    Windows' default (2 hours, 1 second). Probes are set from Windows 10
    version 1703.
  - A time the platform cannot set fails each accept, reported
    (`accept_failed`).
- **Before login**, messages are at most 16 KiB
  (`native::LOGIN_MESSAGE_BYTES`), memory taken as bytes arrive, not as a
  header declares.
- **Ending a connection.** An operator ends one by its
  INFORMATION_SCHEMA.CONNECTIONS ID (`Server::operator`, `Operator::end`):
  its work is cancelled (D44) until it stops, then it is closed, told so
  (SQLSTATE 08006), session rolled back.
- **In SQL** (owner only, SQLCODE -104; in a transaction or not, neither
  being a definition or logged; BNF in `parser/served.rs`):
  `END CONNECTION id` ends one so; `CANCEL CONNECTION id` cancels its
  work, the session going on. An unserved ID is SQLSTATE 08003; the
  issuing connection goes on. The core reaches the server via the
  extension point `ServedConnections`, which the server registers
  (R-SAFE-3).
- **Client waits** (`Timeouts`): connect, TLS and each login step, 30
  seconds by default; later replies, no limit by default; Terminate's
  close, the read limit else 60 seconds, reading at most one reply.
  Replies are at most `MAX_BODY` unless the program says fewer, memory
  taken as bytes arrive. Over a named pipe only connect is bounded.
- **Extension points** (R-SAFE-3):
  - `auth::Authenticator`, a mechanism of as many steps as it needs;
    built in `password`, `scram-sha-256`, `scram-sha-256-plus`,
    `certificate` (D43), and
    `trust` (local clients, only while SERVER.TRUST_LOCAL is ON);
  - `transport::Listener`: built in TCP, Unix sockets and named pipes;
  - `server::Protocol`: built in the native protocol.
- **TLS** (D26) on TCP, per SERVER.TLS: OFF, ON (offered) or REQUIRED;
  chain and key in PEM files SERVER.TLS_CERTIFICATE and SERVER.TLS_KEY.
  - One port serves both: the first byte tells (a TLS handshake begins
    with 22, RFC 8446 §5.1). REQUIRED refuses a clear client (SQLSTATE
    08004). The handshake runs in the connection's task. Unix sockets and
    named pipes need none.
  - Authenticators see `Peer::secure`, so one taking a secret refuses it
    in the clear.
  - SERVER.TLS_CLIENT_CA, a PEM file of authorities, makes each TLS
    client be asked for a certificate, verified by rustls: none is
    served, another's fails the handshake, a verified subject's common
    name goes to the authenticators (`Peer::certificate`).
- **TLS reload** (`Tls::reload`), no restart: at the next TLS connection
  after SERVER.TLS_CERTIFICATE, SERVER.TLS_KEY or SERVER.TLS_CLIENT_CA
  changes, and on SIGHUP (Unix) for a certificate renewed in place. New
  connections use the new files; secured ones go on. Unloadable files
  keep the old, reported once per change (`Event::TlsReloadFailed`);
  success is reported (`Event::TlsReloaded`). SERVER.TLS takes effect at
  start.
- **Native protocol**, version 1 (`turborel_protocol`, D45): framed
  messages (a kind and a little-endian length); Hello with version, user
  and mechanism; every error with SQLCODE, SQLSTATE, message, rule and
  position; values exactly as stored. Port 7422 by default.
- **Sessions.** One per connection, of the authorization identifier its
  authenticator accepts. Each statement is a `<direct SQL statement>`
  [ISO/IEC 9075:1992 §20.1] (`Session::direct`); a client creates only
  its own schema [§11.1 AR1]. Terminated by the client (`Session::end`),
  it ends as SESSION.COMMIT_AT_END says, a failed commit sent as its
  error before closing; lost, failed or cut by shutdown, it rolls back
  [§20.1 GR2a].
- **Configuration** is the database's settings (D36), declared by the
  server on its database, read:
  - at start: SERVER.HOST, SERVER.PORT, SERVER.SOCKET, SERVER.TLS,
    SERVER.TRUST_LOCAL, and SERVER.PASSWORD_ITERATIONS (PBKDF2 iterations
    of verifiers made from then on);
  - as connections are secured or authenticated: TLS's files and
    SERVER.SCRAM_WITHOUT_TLS;
  - as statements and connections begin: SERVER.WORKERS,
    SERVER.MAX_CONNECTIONS, SERVER.MAX_REFUSALS (connections past it
    refused at once, each told why, SQLSTATE 08004; any more closed
    unanswered), SERVER.MAX_MESSAGE_BYTES, SERVER.BATCH_BYTES,
    SERVER.MAX_STATEMENTS (prepared statements per connection),
    SERVER.MAX_LOGINS, SERVER.LOGIN_DELAY_MS and the timeouts, so a
    change needs no restart.
- **Closing.** A connection's place (SERVER.MAX_CONNECTIONS, or a
  refusal's of SERVER.MAX_REFUSALS) and its row of
  INFORMATION_SCHEMA.CONNECTIONS are given back as its stream shuts
  down or drops, before the socket closes (`holding.rs`).
  - A client that has seen its connection closed is never counted:
    not by the limits, `stopping`, nor CONNECTIONS.
  - Over TLS they go before close_notify is sent: a client that does
    not take it keeps its socket open outside the limit, for
    SERVER.IDLE_TIMEOUT_MS at most (until the server hurries, if 0).
  - *Why:* a client that ends a connection at the limit and connects
    again at once must not be refused.
- **Start options.** `turboreld --set NAME=VALUE` sets one;
  `turboreld --config FILE` sets a configuration file's (D36), all
  checked as setting names before any is set, errors with line and
  column; `--set` wins. Other command-line options say where the
  database, log and keys are, not how it behaves.
- **Diagnostics** (D40): `log::Lines` writes each event as a UTC-stamped
  JSON line to standard error or appends to `turboreld --log FILE`, set
  before the database opens so its opening is reported.
  - A bounded queue, written by thread `turborel-server-log` from the
    server's runtime module (D55): a slow disk or blocked pipe delays no
    statement; overflow lines are dropped, counted, the count reported.
  - SIGHUP (Unix) reopens the file, for log rotation, and reloads TLS's
    files; off Unix a log is rotated with the server stopped.
- **Errors.** The server's own are SQLCODE -919, connection exception:
  SQLSTATE 08004 connection refused, 08006 failed, 28000 credentials
  refused [ISO/IEC 9075:1992 §22.1, Table 23].
- **Portability.** TCP everywhere; SERVER.SOCKET a Unix socket on Unix, a
  named pipe on Windows, refused elsewhere; Ctrl-C everywhere, SIGTERM
  and SIGHUP on Unix.
- **Named pipes** (Windows; tokio's `named_pipe`, feature net):
  - The server makes the name's first instance, so it fails to start if
    another process holds the name, and none can take it while it
    listens: an instance always waits, the next made before a connected
    one is handed over.
  - Remote clients are refused: a client is local (`Peer::local`),
    secure as a Unix socket's.
  - Default pipe security: full for creator, administrators and system;
    read-only for others, who can send nothing and close at
    SERVER.LOGIN_TIMEOUT_MS. Narrower needs `unsafe` system API, not
    written here (R-SAFE-1).
  - The client opens the pipe as a std file, so the server can identify
    but never impersonate it, retrying while busy within its connect
    timeout. std bounds no file reads or writes, so an open pipe's are
    unbounded.
- **Stopping.** No new connections; each finishes its statement, is
  rolled back and closed; then the database closes. Past
  SERVER.SHUTDOWN_TIMEOUT_MS (30 s by default, 0 for no limit), or on a
  second stop, it hurries: statements are cancelled (D44) and waits on
  clients end, so each connection closes, rolled back; then the database
  closes.
*Why:* one core serves every mode, so the embedded and the served database
are the same database; I/O that waits must not hold a thread, and a
statement that waits must not hold an I/O thread; and a server's behaviour
is configured, seen and changed like the database's.

**D43. Users, passwords and the database's owner.**
- **Authorization identifiers.** Which identifiers are is
  implementation-defined [ISO/IEC 9075:1992 §5.4; Annex B, item 41]; any
  is, for a session the host program starts.
- **Users** are who server clients may prove they are: in the catalog
  with a verifier, logged and replayed as definitions, in the checkpoint
  snapshot, made outside transactions. A client's name is an
  `<identifier>`, so `ann` is ANN, as in SQL.
- **Verifiers** are opaque to the core. A `Credentials` plug-in makes one
  from a password (`Database::set_credentials`); the authenticator of the
  same scheme checks passwords against it.
- **Server verifier**: PBKDF2-HMAC-SHA256 (D26) with SCRAM-SHA-256's
  keys, `$scram-sha-256$i=N$salt$StoredKey$ServerKey`. Naming scheme and
  iterations keeps old verifiers valid after either changes. It keeps
  StoredKey and ServerKey, never the salted password, which would let its
  reader log in by SCRAM.
  One verifier serves both mechanisms.
- **`password`** needs a secure connection (`Peer::secure`: TLS, Unix
  socket or named pipe): the password travels whole. The client refuses
  first, sending no Hello over plain TCP for a secret mechanism
  (`Mechanism::secret`), as `password` is. An unknown user is checked
  against a decoy verifier, refused like a wrong password, as slowly.
- **`scram-sha-256`** (RFC 5802, RFC 7677) proves the password without
  sending it, and proves the server to the client.
  - Client-first in Hello; server-first, client-final and server-final
    each a Challenge or an Answer; an empty Answer once the server's
    signature is checked; then Ready, which the client refuses if it
    comes before that check (`Mechanism::ready`).
  - A header asking for another identity (`a=`) is refused. Hello names
    the user; client-first's `n=` is read but not used to name it. A
    password is its UTF-8 bytes, without SASLprep.
  - An eavesdropper can guess the password from an exchange at leisure.
    So plain TCP needs SERVER.SCRAM_WITHOUT_TLS ON (OFF by default); the
    client treats it as secret unless told (`Scram::in_the_clear`).
- **`scram-sha-256-plus`** (RFC 5802 §6, RFC 7677) binds the exchange to
  its TLS connection, so a relaying server in the middle is refused.
  - **Binding types**: two.
    - `tls-exporter` (RFC 9266 §2), over TLS 1.3 alone: 32 bytes of
      keying material, label `EXPORTER-Channel-Binding`, empty context
      (rustls's `export_keying_material`, both sides). It binds the
      connection itself. Not over TLS 1.2: RFC 9266 §2 needs the
      extended master secret there, which rustls does not report.
    - `tls-server-end-point` (RFC 5929 §4), over TLS 1.2 and 1.3: the
      hash of the server's certificate, SHA-256 for an MD5 or SHA-1
      signature, else the signature's own hash. No hash, or two, as
      Ed25519 or RSASSA-PSS of a mask hash not its message hash: none.
    - `tls-unique` is not taken: rustls exposes no Finished message.
  - **Offer**: the protocol has no mechanism list; the connection is the
    offer. Over TLS with a binding, both mechanisms are served.
  - **Client's choice**: `p=tls-exporter` over TLS 1.3, the binding RFC
    9266 §3 makes TLS 1.3's; else `p=tls-server-end-point`; with neither
    (TLS 1.2, a certificate of no binding), `scram-sha-256` with `y`.
    Off TLS, `n`.
  - **Header rules**: PLUS only with `p=tls-exporter` or
    `p=tls-server-end-point`, each only where the connection has it (the
    type the client names, RFC 5802 §6.1); `scram-sha-256` only with `n`
    or `y`; `y` refused where the server has either binding (a
    downgrade); `n` taken, from a client that binds nothing. `c=` must be
    the header then the binding's data, else refused as a wrong proof.
    The proof covers the header, so the type cannot be changed between.
  - **Once a connection**: a connection is authenticated once and a
    refusal closes it, as RFC 9266 §4.1 asks of `tls-exporter`.
  - **Which certificate**: the hash is computed as TLS's files are taken
    and kept with the configuration they make; a connection takes both at
    once, so it is bound to the certificate its handshake presented,
    through any reload (`Peer::end_point`). The exporter is the
    connection's own (`Peer::exporter`). The client binds to what its
    handshake gave (`tls::channel`, `Mechanism::bind`), for that
    connection alone.
  - **Format**: no new message or field: the mechanism's name in Hello
    and the GS2 header carry it, within protocol version 1 (D45).
  - **Servers**: `Scram::plus` shares its secret with `Scram`, so an
    unknown user's decoy salt is the same under both; turboreld serves
    both.
  - An unknown user gets a stable salt from its name and a server
    secret, like a real user's, and is refused as a wrong proof is. The
    command-line client uses SCRAM (D46).
- **`certificate`** takes a client whose certificate, verified against
  SERVER.TLS_CLIENT_CA (D42), has a subject of exactly one common name
  that, read as an `<identifier>`, names an existing user: `CN=ann` is
  ANN. One name compared whole is the simplest mapping that cannot be
  stretched; the authorities answer for whom they certify.
- **Failed logins** slow their address, never their user (lockout would
  let anyone lock out anyone).
  - A Hello refused before any authenticator counts too, after waiting
    its address's turn.
  - The next login waits 100 ms, doubling per failure, up to
    SERVER.LOGIN_DELAY_MS (ten seconds by default); waiting logins go a
    delay apart. A success, or ten minutes without failure, resets it.
  - IPv6 counts by /64; a Unix socket's or named pipe's clients as one.
    At most 10,000 addresses are kept.
- **Sessions** last while their user does: a dropped user's sessions end
  at their next request, refused (SQLSTATE 28000), rolled back, closed.
  A new password ends none: they proved themselves at login, and the
  operator may end one (D42).
- **`trust`** accepts a local client as the user it names, only if
  SERVER.TRUST_LOCAL is ON. It is OFF by default, so every client proves
  itself unless the operator says otherwise: on a shared machine, any
  account could otherwise connect as the owner.
- **The owner** is a user: the first created (so a database never has
  users but no owner), or another made owner by the host program, by
  `turboreld user owner`, or by the owner handing it on.
  - In SQL, `ALTER DATABASE OWNER TO name` (`Session::set_owner`): checked
    and made in one change of the definitions, outside a transaction
    (SQLCODE -917), by the owner alone (-104), to a user (-103), logged
    and replayed as a definition and reported (`OwnerChanged`). No
    session ends; the old owner's goes on, no longer owner.
  - The owner cannot be dropped until another owns the database.
- **Managing users.** In SQL, only the owner: `CREATE USER`, `ALTER USER`
  and `DROP USER` (`parser/users.rs` gives their BNF and ALTER
  DATABASE's, in the standard's notation). INFORMATION_SCHEMA.USERS shows
  the owner every user, others only themselves, never a verifier.
- **Setup.** `turboreld --dir PATH user ...`, while no server holds the
  directory lock. For an in-memory database,
  `turboreld --memory --owner NAME` makes NAME its first user and owner
  at start, password read as `user add` reads one; else no one could
  manage it in SQL. Passwords are typed unechoed at a terminal
  (rpassword, as in D46), else read from standard input; never from the
  command line.
- **SET SESSION AUTHORIZATION** [ISO/IEC 9075:1992 §16.4]: a session begun
  by the owner sets any authorization identifier, user or not; any other
  only the one it began with (SQLSTATE 28000) [GR4, GR5]. The default
  schema is unchanged [§4.30]. A Failure's 28000 ends the connection only
  with a connection exception's SQLCODE, as a server refusing credentials
  sends; a statement's has another.
- **Audit.** User and owner changes go to the diagnostics
  (`Event::UserChanged`, `OwnerChanged`), never a verifier, to audit who
  may connect.
*Why:* a server reached from other machines needs clients to prove who
they are; the core keeps what is proven against without knowing how it
was made, so mechanisms are plug-ins, as everything else is. SCRAM binds
by `tls-server-end-point` because both sides already hold the
certificate, whatever the TLS version, and a client bound to a relay's
certificate fails at the real server.

**D44. A running statement is cancelled from another thread.**
- **Stop.** Statements are cancelled through the session's `Stop`
  (`Session::canceller`), which any thread may hold.
- **Windows.** Each call into the session, and each cursor fetch, is a
  window; the first opened while none is open takes a new generation, and
  a cancel marks the running one. So a cancel arriving as a statement
  ends never stops the next, and one made while nothing runs does
  nothing.
- **Checks.** A statement looks:
  - before and after reading each row of a table or index;
  - as a spilled operator reads back each record, or batch, of its work
    area's runs, as a sort's merge does, so passes over partitions and
    runs that read no table (grouping, DISTINCT, EXCEPT, INTERSECT, joins,
    products, sorts) stop too;
  - while waiting for a lock, waking every 20 ms.
- **Effect.** A cancelled statement fails (SQLCODE -920, SQLSTATE TB920)
  and is undone as any failed statement is [X3.135-1986 §7.3 GR3b i]; its
  transaction goes on. A cursor cut short stays so: every later fetch
  fails, none seeming the last.
- **STATEMENT.TIMEOUT_MS**, a session setting in milliseconds (no limit by
  default), cancels a statement that has run that long, each call and
  fetch timed alone from its window's opening. A row read checks the
  clock every 64th time, so a row costs a count; a lock wait checks at
  every wake.
*Why:* a server must stop a statement its client gives up on without
ending its session, and never the wrong one; and an operator must be able
to bound what any statement takes.
**D45. Protocol version 1, and prepared statements.**
- **Dynamic SQL.** Prepared statements are SQL-92's dynamic SQL
  [ISO/IEC 9075:1992 §17]. `Session::prepare` checks a
  `<preparable statement>` once [§17.6], types each
  `<dynamic parameter specification>` by context [§17.6 GR5], and
  describes the result [§17.8, §17.9 GR3, GR4]. `execute_prepared`
  [§17.10] and `open_prepared` [§17.13, §17.14] run it, casting each
  value to its parameter's type [§17.9 GR5].
- **Refused** (SQLSTATE 37000): a `?` GR5 gives no type [GR4]; any
  Format, Syntax or Access Rule broken while preparing [GR4a].
- **Accepted.** A `<comment>`, as drivers send them, though SQL's
  PREPARE refuses one [GR4q] (D58). SQL text also prepares by name
  (D58).
- **Positioned DELETE or UPDATE** finds its cursor when prepared [GR2],
  else when it runs.
- **Plug-ins.** A scalar function states each argument's `?` type
  (`ScalarFunction::parameter_type`); by default, refused.
- **Kept plans.** A query, an INSERT, and a searched UPDATE or DELETE
  keep parsed text and bound plan, each `?` a slot typed by context
  (`Expr::Dynamic`); other statements keep parsed text only. Each
  execution fills the slots, cast [§17.9 GR5], then rewrites and plans,
  so access paths follow its values and current statistics, never a
  generic plan (`db/prepared/plans.rs`, `logical/dynamic.rs`).
- **Validity.** A kept plan holds while these are unchanged:
  - the snapshot's generation (every published change of definitions,
    settings, functions, optimizer rules);
  - the catalog read, by identity: every change makes a new one, a
    transaction's uncommitted ones and GRANT and REVOKE included;
  - the session's authorization identifier, default schema, time zone
    and own settings, SQL.FUNCTION_CALLS included.
- **Rebinding.** Otherwise the statement is rebound, its Syntax and
  Access Rules checked against current definitions [§17.6 GR10], so a
  revoked privilege is missing at the next execution. Declared local
  temporary tables make a new catalog per call, so their statements are
  rebound every time.
- **Never kept.** A binding that reads a value: a `?` other than as a
  slot, or the statement's instant (CURRENT_DATE, CURRENT_TIME,
  CURRENT_TIMESTAMP, a datetime CAST, a DEFAULT written with them, in
  the statement or a view it reads; `value::datetime::Clocked`).
  Unwritten INSERT columns take their defaults anew.
- **Counting.** `Prepared::plans_made` counts bound plans made, not
  executions served; clones share both kept plan and count.
- **Messages.**
  - Prepare, answered by Prepared: parameter types and columns.
  - Execute, with values and a row limit: a query's cursor opens and
    sends up to the limit, then Suspended if more remain (Fetch sends
    them) or Complete, closing the cursor.
  - Close [§17.16] and Deallocate [§17.7].
- **Statement numbers.** Client-chosen, at most SERVER.MAX_STATEMENTS.
- **Errors.** 24000: cursor open or closed wrongly [§13.2-§13.4 GR1,
  §17.7 GR2]. 26000: statement not prepared [§17.10 GR1]. 07001: values
  not one per parameter [§17.9 GR5a].
- **Complete** carries SQLCODE and SQLSTATE, so warnings reach the client
  (D47), including those a query's rows raised, as a set function or CAST
  may [ISO/IEC 9075:1992 §6.5 GR1b, §6.10]. Complete and Error end with a
  byte: is the transaction still active? An SQL-agent refuses to leave or
  end a connection with one active (D46).
- **Pipelining.** Requests may be sent before the last is answered;
  answers come in order.
- **Ready** gives the product, version and codename ("turborel 0.0.0
  (alpha)", `turborel::product`, VERSIONING.md) only once authenticated,
  and a random 128-bit key.
- **Cancel.** A connection whose only message is Cancel with that key
  cancels what the session runs (D44), over TCP, TLS, a Unix socket or a
  named pipe; the server closes it silently.
- **Capabilities.** Exchanged by Hello and Ready; version 1 defines none,
  so later ones need no new version.
*Why:* drivers expect dynamic SQL; clients read large results in parts
and must stop what they start. Repeats redo parsing and binding for
nothing; planning per execution keeps the plan the values choose;
invalidating by what binding read matches binding anew. A separate cancel
connection needs no second writer, which a TLS stream cannot have.

**D46. The command-line client.**
- **Program.** `turborel`, crate `turborel-cli`, on `turborel-client`;
  like `turboreld`, a library called only by its `main.rs`.
- **Input.** `-c`, files (`-f`), or standard input (prompted at a
  terminal). Split at each `;` outside a `<character string literal>`,
  `<delimited identifier>` or `<comment>` [ISO/IEC 9075:1992 §5.2,
  §5.3]; the last needs none. Each runs alone as a
  `<direct SQL statement>` [§20.1].
- **Output.** Rows stream as a table, never held: columns as wide as the
  type's values (at most 60 characters) or the name; numbers right;
  null as `--null` says, blank by default. Other statements print OK, OK
  with a warning and its SQLSTATE, or no data; a query's warning and
  SQLSTATE follow its row count.
- **Banner.** Typed sessions open with one on standard output: client
  version, copyright and licence; server product and version from Ready;
  where and as whom; how to end. Untyped runs print only statement
  output.
- **Failures** go to standard error; the run continues. Untyped runs stop
  at a cancelled statement, or at the first failure with
  `--stop-on-error`; any run stops when its connection is lost. Exit
  status 1 if any statement failed.
- **SQL-agent.** The client library is the SQL-agent [ISO/IEC 9075:1992
  §15] (`turborel_client::agent`): it runs CONNECT, SET CONNECTION and
  DISCONNECT itself, all else on the current connection; a server sent
  one of those three refuses it (SQLCODE -900).
- **Addresses.** A `Connector` makes connections; the CLI's
  (`cli/src/dial.rs`) reads an `<SQL-server name>` as 'host' (port
  7422), 'host:port', a Unix socket path beginning '/', or a Windows
  named pipe beginning '\\'. DEFAULT is the command line's server,
  connected at start. Each connection uses the command line's
  credentials: a typed password is asked for again; a TURBOREL_PASSWORD
  one is reused.
- **One server per transaction.** In an active transaction, CONNECT and
  SET CONNECTION are refused (0A001) [§15.1 GR1, §15.2 GR1], as is
  DISCONNECT of that connection (25000) [§15.3 GR6].
- **Where.** `--host`/`--port` (127.0.0.1, 7422); `--tls FILE` (trust
  that PEM file) and/or `--tls-system-roots` (the platform's certificate
  authorities) (D26), with `--server-name`, presenting `--certificate`
  FILE and `--key` FILE if named; or `--socket`: a Unix socket or
  Windows named pipe, refused clearly elsewhere. Each address of a host
  is tried until one answers; a refusal is not retried.
- **Who.** `--user`, else USER or USERNAME.
- **Password.** Typed unechoed (`-W`) or from TURBOREL_PASSWORD, never
  the command line, where local users may see it; proved by
  SCRAM-SHA-256 (D43), never sent, and only over TLS, a Unix socket or a
  named pipe: over plain TCP the client refuses to begin. Else a
  presented certificate proves the user its common name names
  (`certificate`); else the server is asked to trust the user.
- **Ctrl-C** cancels the running statement (D44) on the current
  connection, as SET CONNECTION and DISCONNECT change it, via tokio's
  signals on their own one-thread runtime (SIGINT on Unix, console
  Ctrl-C on Windows). With no current connection it ends the program.
  The password is typed before the handler is installed.
- **Dependencies** (R-SAFE-4): tokio 1.54.1 (MIT), already the server's,
  features rt and signal only; rpassword 7.5.4 (Apache-2.0), unechoed
  terminal input on Unix and Windows, small enough to review, maintained
  since 2015.
*Why:* operators need to run SQL without writing a program. Streaming
bounds memory; a password off the command line and unechoed stays
private.

**D59. Import: rows from a pluggable source, over a connection.**
- **Library.** `import` (`turborel_client::import`) inserts a `Source`'s
  records into a table over a connection; `--import` is one caller.
- **`Source`.** A trait any program may implement: headings (names, and
  types if known), asked once; then records one at a time, each a value
  per field plus its record number and, for line sources, line. It holds
  one record and refuses one past its own bound, so memory stays bounded.
  `import` is generic over it, no `dyn` (R-SAFE-3).
- **Built in.** `Delimited`, written in the tree (D3):
  - CSV per RFC 4180 §2: a record per line, ended by CR LF or LF
    (optional on the last); optional header; quotes enclose a field
    holding the delimiter, a doubled quote or a line break; a named
    delimiter replaces the comma;
  - tab-separated: split at tabs, never quoted.
- **Fields.** Unquoted null text (empty by default) is null; a quoted
  empty field is the empty string. UTF-8, a leading byte order mark
  skipped. A quote in an unquoted field, or anything but the delimiter
  after a closing quote, is refused, naming record, line and field.
  Records are at most 16 MiB by default, the protocol's longest body.
- **Protocol.** Rows go through Prepare and Execute (D45), unchanged,
  one Execute per row, answered before the next.
- **Columns.** Those the headings name, read as `<identifier>`s; else the
  table's, in order, as a prepared `SELECT *` describes them [ISO/IEC
  9075:1992 §17.9 GR3]. The table is read as a `<table name>` and sent
  as delimited identifiers, so nothing else enters the SQL.
- **INSERT.** One is prepared, a `?` per column typed as the column
  [§17.6 GR5]. The server casts each value [§17.9 GR5f, g], except a
  string for a character column: the client assigns it as INSERT does
  [§13.8 GR4b iii, §9.2 GR3b, GR3e], so one too long is refused (22001),
  not truncated with a warning as by CAST [§6.10 GR5c ii, GR6c ii].
- **Transaction.** An active one is refused, since import commits: every
  batch, 1000 rows by default, all at once for 0.
- **Stopping.** A refused row (by the server or as too long) or a record
  with too many or too few fields stops it: the batch rolls back, and the
  error gives record, line, reason and rows committed. With skip bad
  rows, such a row is reported and import continues if only that row was
  undone (the transaction is still active, or had no rows). A lost
  connection, cancel (D44) or unreadable record always stops it.
- **Command line** (D46): `--import FILE`, an input in turn with `-c`
  and `-f`; `--table`, `--format csv` or `tsv`, `--header`,
  `--delimiter`, `--batch`, `--skip-bad-rows`; `--null` sets the text
  shown and read as null. A skipped row fails the run.
*Why:* operators load data from other systems, mostly CSV. A source
trait lets any program import any format unknown to the client; the
built-in reader is small enough to review. A prepared INSERT keeps the
protocol and applies server casts and constraints to every row; batches
bound rollback, one record at a time bounds memory.

---

## 2. Project layout

```
Cargo.toml              workspace and package; lint levels (D10)
clippy.toml             lint thresholds
rustfmt.toml            formatting (D9)
rust-toolchain.toml     pinned toolchain (D8)
.cargo/config.toml      the `cargo xtask` alias
README.md               what turborel is; how to build and test it
RULES.md                this file
CONFORMANCE.md          what is implemented of the standards, and the
                        choices they leave to an implementation
TODO.md                 open work
CONTRIBUTING.md         changes, commit messages, pull requests, issues
SECURITY.md             how to report a vulnerability
AGENTS.md               the guide for coding agents
CLAUDE.md               points Claude Code to AGENTS.md
.github/                CI (workflows/), issue forms, pull request
                        template
src/                    the library (lib.rs); one concept per module
protocol/src/           the native wire protocol: messages, codec (D42)
client/src/             the client: a connection over the protocol (D42)
server/src/             the server, as a library (D42)
server/test/            the server's integration tests, with a client
turboreld/src/          the server program: command line, signals (D42)
turboreld/test/         the program's integration tests
cli/src/                the command-line client `turborel` (D46)
cli/test/               its integration tests
lz4/src/                LZ4 compression, an encoding plug-in (D48)
aes/src/                encryption (AES-256-GCM, ChaCha20-Poly1305) and
                        cryptographic hash functions (D26, D48)
support/src/            fixtures shared by the crates' tests (R-TEST-10)
replication/src/        the replication crate (D57)
doc/                    the manual (R-DOC-8)
test/integration/       the integration test binary: public API only
  main.rs               crate root and module list
  common.rs             shared fixtures and helpers
  <area>.rs             one module per area
bench/                  benchmarks, run by `cargo xtask bench`
  harness.rs            the harness every benchmark uses
xtask/                  project tools: verify, coverage, trace, bench,
                        mutate
```

- **Targets.** `Cargo.toml` declares each test and benchmark target with
  its path, with discovery off, so the directories are `test/` and
  `bench/`.
- **Modules.** Every other file under a target's directory is a module of
  that target.

---

## 3. Code rules

### R-STYLE: formatting and idiom

| ID | Rule | Checked by |
|---|---|---|
| R-STYLE-1 | Code **shall** be formatted by `rustfmt`. | `cargo fmt --check` |
| R-STYLE-2 | Code **shall** pass clippy `pedantic` and restriction lints, `-D warnings`. | clippy |
| R-STYLE-3 | Names **shall** follow Rust API Guidelines casing; code implementing a production is named after it: `QuerySpecification`, `query_specification`. | review |
| R-STYLE-4 | Modules are `foo.rs`, submodules in `foo/`; no `mod.rs`. | `mod_module_files` |
| R-STYLE-5 | No wildcard imports, except `use super::*` in unit tests. | `wildcard_imports` |
| R-STYLE-6 | A name **shall not** shadow an unrelated binding. | `shadow_unrelated` |
| R-STYLE-7 | No `dbg!`; production code **shall not** print. | `dbg_macro`, `print_stdout`, `print_stderr` |

### R-SIZE: short functions and files

| ID | Rule | Checked by |
|---|---|---|
| R-SIZE-1 | A function **shall** have at most 50 lines of code. | `too_many_lines` |
| R-SIZE-2 | A source file **shall** have at most 600 lines; split by the standard's clauses or pipeline stages. | `cargo xtask file-length` |
| R-SIZE-3 | A function **shall** take at most 5 parameters, at most 1 a `bool`; else use a struct, tuple or enum. | `too_many_arguments`, `fn_params_excessive_bools` |
| R-SIZE-4 | A module **should** hold one concept, stated in its `//!` doc. | review |

### R-DOC: comments and documentation

| ID | Rule | Checked by |
|---|---|---|
| R-DOC-1 | Every public item **shall** have a doc comment, with `# Errors` if it returns `Result` and `# Panics` if it can panic. | `missing_docs`, `missing_errors_doc`, `missing_panics_doc` |
| R-DOC-2 | Comments **shall** say *why*: the rule implemented, the choice made, the invariant relied on. | review |
| R-DOC-3 | Documentation **shall** build without warnings. | `cargo doc` with `-D warnings` |
| R-DOC-4 | Behaviour the standard governs **shall** cite it as printed: `[X3.135-1986 §8.6 GR2]`, `[ISO/IEC 9075:1992 §11.19 SR13]`. Implementation-defined choices say so and state the choice. Beyond the standards: `[EXTENSION]`; ungoverned internals: `[DESIGN]`. BNF in doc comments goes in backticks. | review |
| R-DOC-5 | No commented-out code, no `TODO` or `FIXME` comments: open work goes in TODO.md, and code never cites TODO.md. | review; `cargo xtask verify` |
| R-DOC-6 | Every file **shall** start with a `//!` doc giving its responsibility and clauses implemented. | review |
| R-DOC-7 | Each implementation-defined choice **shall** be listed in CONFORMANCE.md with its implementing file. | review |
| R-DOC-8 | A change visible to a user, operator or program **shall** update the manual, `doc/`, in the same commit: every page stating what changed, in its format (statement pages: NAME, SYNOPSIS, DESCRIPTION, RULES, PRIVILEGES, ERRORS, EXAMPLES, STANDARD, SEE ALSO; chapters: sections). New statements, settings, program options and events get a page or entry. Claims are checked against the code; examples are run, with the changed output pasted. | review |

### R-CONST: no magic numbers or strings

| ID | Rule | Checked by |
|---|---|---|
| R-CONST-1 | Production numeric literals other than `0` or `1` **shall** be named `const`s, documenting the value's source. | review; `default_numeric_fallback` |
| R-CONST-2 | Strings matched or looked up (key words, plug-in names, settings, paths) **shall** be named `const`s, defined once. Messages for people are not, but cite their rule (R-ERR-2). | review |
| R-CONST-3 | Related constants **shall** live together in the concept's module, under a header naming their source. | review |
| R-CONST-4 | Tests may use literal fixtures; expected values restating a constant **should** use it. | review |

### R-ERR: errors, not panics

| ID | Rule | Checked by |
|---|---|---|
| R-ERR-1 | Production code **shall not** panic: no `unwrap`, `expect`, `panic!`, `unreachable!`, `todo!`, `unimplemented!`, indexing, slicing or `process::exit`. | clippy restriction lints |
| R-ERR-2 | Every failure **shall** be a `SqlError` with a SQLCODE and a plain-words message. In source, a message begins with the broken rule, `"[X3.135-1986 §8.3 SR6] ..."`, kept apart as the error's `rule`; or, if no standard governs, `[DESIGN]` or `[EXTENSION]`, which the error drops. Format errors name tokens instead (D24). | review; `error.rs` tests |
| R-ERR-3 | No `Result` **shall** be silently discarded: `let _ = fallible()` needs a comment on why the error cannot matter. | `unused_must_use`; review |
| R-ERR-4 | A `RefCell` borrow or lock guard **shall** be scoped to the block needing it, never held across a plug-in call or any call that could take it again. A poisoned lock is reported (SQLCODE -910), never unwrapped. | review |
| R-ERR-5 | A failed statement **shall** leave no effect [X3.135-1986 §3.3]; writes go through the transaction coordinator's undo. | tests |

### R-NUM: arithmetic and conversions

| ID | Rule | Checked by |
|---|---|---|
| R-NUM-1 | Integer arithmetic **shall** be explicit about overflow: `checked_*`, `wrapping_*`, `saturating_*` or `overflowing_*`. | `arithmetic_side_effects` |
| R-NUM-2 | `as` **shall not** be used on numbers; use `From`, `TryFrom` or a checked helper. | `as_conversions`, `cast_*` |
| R-NUM-3 | A divisor **shall** be shown non-zero, and `MIN / -1` handled. | `integer_division`; review |
| R-NUM-4 | Floats **shall not** be compared for equality except by the standard's comparison rules in `value.rs`. | `float_cmp`; review |
| R-NUM-5 | A numeric literal whose type would default **shall** carry a suffix or typed binding. | `default_numeric_fallback` |

### R-CF: control flow testable to MC/DC

| ID | Rule | Checked by |
|---|---|---|
| R-CF-1 | A boolean decision **shall** have at most 4 conditions; else extract a named predicate. | review |
| R-CF-2 | Cognitive complexity **shall** be at most 15, block nesting at most 5. | `cognitive_complexity`, `excessive_nesting` |
| R-CF-3 | A `match` on an enum **shall** list every variant, with no `_`. | `wildcard_enum_match_arm` |
| R-CF-4 | Conditions **shall** have no side effects, except R-DEV-5's consuming idioms. | review |
| R-CF-5 | Or-patterns **should** appear only at a pattern's root, and match guards **should** be avoided: both hide conditions from MC/DC test design. | review |
| R-CF-6 | Recursion **shall** be bounded by a named depth constant, failing with a `SqlError` past it. | review; tests |

### R-SAFE: language subset and resources

| ID | Rule | Checked by |
|---|---|---|
| R-SAFE-1 | No `unsafe` code. | `unsafe_code = "forbid"` |
| R-SAFE-2 | No `mem::forget`, `process::exit` or global mutable state. | `mem_forget`, `exit`; review |
| R-SAFE-3 | `dyn Trait` **shall** appear only at the extension points listed atop `src/db.rs`, the traits they hand out, the storage foundation's traits (D30), and row streams. Each extension trait **shall** document its contract; every built-in implementation **shall** pass the same contract tests. | review; tests |
| R-SAFE-4 | Each dependency **shall** have a §1 decision covering rationale, version, license and review; only cryptography and compression plug-ins, the server crate and the command-line client may have any (D3, D26, D42, D46, D48). | review |
| R-SAFE-5 | No thread **shall** be started except through the thread manager (`src/threads.rs`, D55), and none unnamed. | xtask; review |

---

## 4. Test rules

### R-TEST: levels and layout

| ID | Rule | Checked by |
|---|---|---|
| R-TEST-1 | Each test **shall** sit at the lowest level showing its requirement: unit (`#[cfg(test)] mod tests`, private logic), integration (`test/integration/`, public API only) or doc (examples in `///` docs). | review |
| R-TEST-2 | Tests **shall** derive from requirements, not code: normal-range (boundaries included) and robustness (invalid inputs, exact SQLCODE expected). | review |
| R-TEST-3 | A test's name **shall** say the behaviour checked; an integration test may end in its clause: `fetch_targets_8_6`. | review |
| R-TEST-4 | Test code may `unwrap`, index and panic: a panic is a failure. Every other rule applies. | lints |
| R-TEST-5 | Tests **shall** be deterministic and independent: no clock, randomness, network beyond loopback (the server's tests, D42) or order dependence; each builds its own `Database`. Case generators use a fixed seed, so every run makes the same cases (`test/integration/random_queries.rs`). | review |
| R-TEST-7 | Every fixed defect **shall** get a regression test, failing without the fix, its doc comment saying so. | review |
| R-TEST-8 | New decisions of two or more conditions **shall** have tests showing MC/DC, naming the condition pairs in doc comments. | review |
| R-TEST-9 | Tests **should** be mutation-checked periodically. `cargo xtask mutate [NAME]` applies each mutation in `xtask/src/mutate/list.rs` (file, snippet occurring once, replacement, tests that must fail) in turn, runs its tests, restores the file, and reports uncaught mutations and named tests that passed. Run it when changing a mechanism the list breaks; a new mechanism adds a mutation. | `cargo xtask mutate`, periodic; `verify` checks each mutation still applies |
| R-TEST-10 | A fixture or helper shared by modules **shall** be written once: in the test binary's `common.rs`, or, if shared by crates, in `turborel-test-support` (`support/`). Tests use `#[test]`, no other framework. | review |

R-TEST-6 was withdrawn with the golden-file tests (D6).

### R-TRACE: traceability

| ID | Rule | Checked by |
|---|---|---|
| R-TRACE-1 | Every test **shall** have a `/// Verifies:` paragraph starting with a tag: `[X3.135-1986 §…]`, `[EXTENSION]`, `[DESIGN]` or `[TOOL] R-…`. | `cargo xtask verify` |
| R-TRACE-2 | Code implementing a requirement **shall** cite it (R-DOC-4). `cargo xtask trace '§8.6'` lists a clause's tests. | review; `xtask trace` |
| R-TRACE-3 | CONFORMANCE.md **shall** change with the change implementing or changing a clause. | review |

### R-BENCH: benchmarks

| ID | Rule | Checked by |
|---|---|---|
| R-BENCH-1 | Every benchmark **shall** run on `bench/harness.rs`: benchmarks hold workloads; the harness times, reports and keeps them, and holds what they share. | review |
| R-BENCH-2 | Benchmarks measure, not test: no timing assertions; `verify` skips them. A performance claim **shall** cite `cargo xtask bench --compare` against the prior code. | review |

---

## 5. Structural coverage and MC/DC

| ID | Rule | Checked by |
|---|---|---|
| R-COV-1 | Coverage **shall** be measured by `cargo xtask coverage` over unit and integration tests, for `src/`. On the pinned toolchain, doc tests are excluded and unit-test modules count as covered code. | `xtask coverage` |
| R-COV-2 | Coverage **shall not** fall below the floors in `xtask/src/baseline.rs`; a change raising it **should** raise them. | `xtask coverage` |
| R-COV-3 | Target: 100 % statement, decision and MC/DC coverage (D2). Close gaps by requirements-based tests, removing dead code, or documenting deactivated code; never by requirement-less tests. | review |
| R-COV-4 | MC/DC **shall** be shown by test design (R-TEST-8) and checked in review (D22). | review |
| R-COV-5 | A coverage tool used for credit **shall** first be qualified (DO-330, TQL-5). | process |

- **Measurable.** Stable rustc gives region, line and function coverage,
  approximating statement coverage. Branch coverage needs nightly; MC/DC
  cannot be measured (D22).
- **Masking.** Masking MC/DC is acceptable (CAST-6), which matters for
  Rust's short-circuit `&&` and `||`.

---

## 6. Deviations

| ID | Rule | Checked by |
|---|---|---|
| R-DEV-1 | Every `#[allow]` and `#[expect]` **shall** carry a `reason` citing the rule and justification. | `allow_attributes_without_reason` |
| R-DEV-2 | Lint baseline counts **shall never** rise, nor lints be added; record lower counts with `cargo xtask lint-audit --write` (D11, D20). | `cargo xtask lint-audit` |
| R-DEV-3 | Permanent deviations, each with its reason in code: printing in `xtask`; test-code allowances (R-TEST-4); `as` in the two float roundings std lacks (`value::round_to_single`, `value::decimal::nearest_f64`); `Code::sqlcode`, an enum whose discriminant is its SQLCODE. | lints |
| R-DEV-4 | Production code **should** use `#[expect]`, not `#[allow]`, so an unneeded deviation fails the build. | review |
| R-DEV-5 | A condition may consume input if it is one call, alone or under `!`, whose only effect is consuming the input decided on: the parser's `accept_*`, `pop_front`, `pop`, `Option::take` (D16). Never inside `&&`, `\|\|` or a match guard. | review |

---

## 7. Process and tools

| ID | Rule | Checked by |
|---|---|---|
| R-PROC-1 | `cargo xtask verify` **shall** pass before every commit: format, lints, lint baselines, docs, file length, traceability and citations, every test. `--coverage` adds coverage. | pre-commit hook; CI |
| R-PROC-2 | Every change **shall** be reviewed against the checklist below. | review |
| R-PROC-3 | Commits **should** be small and single-purpose, in CONTRIBUTING.md's form (`area: summary`, then why), citing rules applied or changed. | review |
| R-PROC-4 | A threshold change **shall** change RULES.md, `clippy.toml` and `xtask` together. | review |
| R-PROC-5 | A release's crates **shall** be published together at its number, from a pushed commit passing `cargo xtask verify --coverage`, dry run first, per VERSIONING.md. | review |
| R-TOOL-1 | Project automation **shall** be Rust in `xtask/`, never shell scripts (D7). | review |
| R-TOOL-2 | `xtask` **shall** follow these rules, use only std, and test its own decisions (`[TOOL]` requirements). | `xtask verify` |
| R-CM-1 | The toolchain is pinned (D8); a change **shall** be its own commit, passing `cargo xtask verify --coverage`. | review |
| R-CM-2 | `Cargo.lock` and the baselines **shall** be under version control. | review |

The review checklist:

- requirement cited in code (R-DOC-4) and tests (R-TRACE-1);
- normal-range and robustness tests (R-TEST-2); MC/DC pairs for new
  decisions (R-TEST-8);
- no magic numbers or strings (R-CONST);
- comments explain why (R-DOC-2);
- conditions without side effects (R-CF-4); borrows and locks scoped
  (R-ERR-4);
- CONFORMANCE.md updated (R-TRACE-3), and TODO.md where open work
  changed.

---

## 8. DO-178C

- **Scope.** These rules make the code and its verification acceptable
  DO-178C inputs, not certifiable alone: certification belongs to a
  process run under plans agreed with the authority.
- **DO-178B.** Meeting DO-178C as written here also meets DO-178B.

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

1. **Plans** (PSAC, SDP, SVP, SCMP, SQAP) are not written; this file
   feeds the SDP and SVP.
2. **Low-level requirements:** doc comments and unit tests stand in.
3. **Data and control coupling analysis:** not done.
4. **Source to object code traceability** (Level A): not done.
5. **Tool qualification:** the compiler and coverage tools are not
   qualified (R-COV-5).
6. **Independence of verification** needs an organisation, not a rule.
7. **Dynamic memory:** the engine heap-allocates throughout, a DO-332
   vulnerability area; airborne use needs bounded allocation or analysis.

- **Adoption.** Adopted 2026-10-23 with a code audit; that week every
  finding was fixed and lint baselines and over-long files paid down.

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
