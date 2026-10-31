# Guide for coding agents

How to work on turborel as a coding agent, whatever the tool. It points
into the project's documents rather than repeating them: when this file
and [RULES.md](RULES.md) or [CONTRIBUTING.md](CONTRIBUTING.md) differ,
they win.

## What turborel is

A relational database system in Rust, queried in SQL: a library to embed,
a server (`turboreld`), a client, and a command-line client (`turborel`).
Its language is ANSI X3.135-1986 ("SQL-86") and ISO/IEC 9075:1992
("SQL-92") read as one language where the newer wins (RULES.md D32),
extended standard by standard through ISO/IEC 9075:2023 ("SQL:2023"),
beside turborel's own extensions. See [README.md](README.md).

## Before you change anything

- Read [RULES.md](RULES.md): §3 (code rules), §4 (test rules), §7
  (process), and the decisions (§1) of the area you will change. Every
  rule binds every change.
- Read [CONTRIBUTING.md](CONTRIBUTING.md): how a change is made, commit
  messages, and the extra checks for changes that touch data.

## Build and check

```
cargo xtask verify               # every check a change must pass
cargo xtask verify --coverage    # the same, then structural coverage
cargo xtask install-hooks        # run verify before every commit (once per clone)
cargo test --workspace           # the tests alone
cargo xtask coverage             # coverage report and floor check alone
cargo xtask trace '§8.6'         # which tests verify a clause
cargo xtask file-length          # the R-SIZE-2 check alone
cargo xtask lint-audit [--write] # lint baseline counts (R-DEV-2)
cargo xtask bench [NAME] [--save LABEL] [--compare LABEL]
cargo xtask mutate [NAME]        # break each listed mechanism, expect its tests to fail (R-TEST-9)
```

- **Every change must pass `cargo xtask verify --coverage`** before it is
  handed back. `verify` stops at the first failure; it runs, in order:
  `cargo fmt --all --check`; clippy on the workspace, all targets, with
  `-D warnings`; the lint baseline counts; `cargo doc` with warnings as
  errors; file length; traceability (`/// Verifies:`); citations (no
  source cites the work list); unit and integration tests; doc tests.
  `--coverage` then measures coverage (`xtask/src/coverage.rs`).
- **Coverage floors** are in `xtask/src/baseline.rs`: regions 94.0 %,
  lines 96.2 %. They only rise (R-COV-2); the report is at
  `target/coverage/html/index.html`.
- **The pre-commit hook** (`.githooks/pre-commit`) runs `cargo xtask
  verify`; `cargo xtask install-hooks` sets `core.hooksPath` to
  `.githooks`.
- **CI** (`.github/workflows/verify.yml`) runs `verify` on Linux, macOS
  and Windows, and coverage on Linux (D21).
- **Benchmarks** (R-BENCH) run on `bench/harness.rs`, never in `verify`.
  A performance claim cites `cargo xtask bench NAME --save before` on the
  code before, then `--compare before` on the change (R-BENCH-2).
- The toolchain is pinned in `rust-toolchain.toml` (D8, R-CM-1).
- Project automation is Rust in `xtask/`, never shell scripts (D7,
  R-TOOL-1).

## Where things are

Crates (D42, D46, D48, D57):

| Path | Crate | What |
|---|---|---|
| `src/` | `turborel` | the core: the database as a library, std only |
| `protocol/` | `turborel-protocol` | the native wire protocol's messages and codec, no I/O |
| `client/` | `turborel-client` | a blocking connection to a server |
| `server/` | `turborel-server` | the server as a library, on tokio |
| `turboreld/` | `turboreld` | the server's program: command line, signals |
| `cli/` | `turborel-cli` | the command-line client `turborel` |
| `lz4/` | `turborel-lz4` | LZ4 compression, an encoding plug-in |
| `aes/` | `turborel-aes` | AES-256-GCM and ChaCha20-Poly1305 encryption, SHA-256, SHA-512 and HMAC-SHA-256 checksums: plug-ins |
| `replication/` | `turborel-replication` | replication's extension points (`Consensus`, `Transport`, `StateTransfer`), std only |
| `support/` | `turborel-test-support` | fixtures the crates' tests share |
| `xtask/` | `xtask` | the project's checks and tools |

Tests and benchmarks: `test/integration/` (the core's one integration
binary, `common.rs` its fixtures), `server/test/`, `turboreld/test/`,
`cli/test/`, `bench/` (one file per benchmark, on `harness.rs`).

The core, `src/`, in the order a statement meets it (each `foo.rs` with
its submodules in `foo/`):

- `lexer.rs`: tokens, identifiers and key words.
- `parser.rs`: recursive descent, one method per BNF production.
- `ast.rs`: the parsed form of statements.
- `binder.rs`: the Syntax Rules that need the catalog; AST to logical plan.
- `params.rs`: parameters of statements.
- `logical.rs`: the logical plan, relational algebra.
- `optimizer.rs`: rewrite rules over the logical plan (`Rule`).
- `physical.rs`: the physical plan: access paths, joins, costs.
- `stats.rs`: statistics for the planner.
- `exec.rs`: execution of physical plans.
- `spill.rs`: where operators past their work area put rows (`Spill`).
- `db.rs`: the database, its extension points, sessions; `db/` holds
  statements, definitions, checkpoints and recovery.
- `txn.rs`: the transaction coordinator: locks, undo, commit.
- `log.rs`: the write-ahead log (`Log`).
- `catalog.rs`: schemas, tables, views, columns, indexes, privileges.
- `system.rs`: system tables and the Information Schema.
- `storage.rs`: the storage engine traits (`StorageEngine`, `TableStorage`).
- `engine.rs`: the built-in row store.
- `index.rs`: index methods (`IndexMethod`, `Index`): B-link tree, hash.
- `disk.rs`: the storage foundation: file I/O, blocks, pages, buffer pool,
  encodings.
- `hash.rs`: hash functions (`HashFunction`).
- `value.rs`: data types and values, arithmetic and comparison.
- `functions.rs`: scalar and aggregate functions as plug-ins.
- `settings.rs`: the settings registry (D36).
- `memory.rs`: the memory budget (D38).
- `diagnostics.rs`: typed events to a pluggable sink (D40).
- `error.rs`: errors as SQLCODE and SQLSTATE.
- `bytes.rs`: reading and writing the fields of stored formats.
- `sync.rs`: locks on shared state, poisoning reported.

## Code rules that bite most often

Lints are set in `Cargo.toml` and `clippy.toml`; `verify` makes every
warning an error. The ones most often broken:

- **No `unsafe`** (R-SAFE-1, `unsafe_code = "forbid"`).
- **No panics in production code** (R-ERR-1): no `unwrap`, `expect`,
  `panic!`, `unreachable!`, `todo!`, `unimplemented!`, indexing, slicing
  or `process::exit`. Every failure is a `SqlError` with a SQLCODE, its
  message beginning with the rule it breaks, or `[DESIGN]`/`[EXTENSION]`
  (R-ERR-2). Test code may panic (R-TEST-4).
- **No `as` on numbers** (R-NUM-2): `From`, `TryFrom` or a checked
  helper. Integer arithmetic is `checked_*`, `wrapping_*`, `saturating_*`
  or `overflowing_*` (R-NUM-1).
- **No `_` arm in a `match` on an enum** (R-CF-3,
  `wildcard_enum_match_arm`): list every variant.
- **Avoid match guards**, and keep or-patterns at the root of a pattern
  (R-CF-5): they hide conditions from MC/DC test design. A condition
  never consumes input inside a match guard (R-DEV-5).
- **Conditions have no side effects** (R-CF-4), at most 4 conditions to a
  decision (R-CF-1); cognitive complexity at most 15, nesting at most 5
  (R-CF-2).
- **No shadowing of an unrelated binding** (R-STYLE-6, `shadow_unrelated`).
- **Sizes** (R-SIZE): a function at most 50 lines of code, at most 5
  parameters and at most 1 `bool` among them; a source file at most 600
  lines (`cargo xtask file-length`). Split a file along the standard's
  clauses or the pipeline's stages.
- **Modules** are `foo.rs` with submodules in `foo/`, never `mod.rs`
  (R-STYLE-4).
- **No printing** in production code, no `dbg!` (R-STYLE-7).
- **No magic numbers or strings** (R-CONST): a literal other than `0` or
  `1`, or a string matched on, is a named `const` whose doc says where it
  comes from.
- **Docs:** every public item has a doc comment, with `# Errors` when it
  returns `Result` (R-DOC-1); every file starts with a `//!` doc stating
  its responsibility and clauses (R-DOC-6); comments say why (R-DOC-2);
  no commented-out code, no `TODO` or `FIXME` comments (R-DOC-5).
- **Deviations:** `#[expect(lint, reason = "...")]` citing the rule,
  rather than `#[allow]` (R-DEV-1, R-DEV-4). A lint baseline never rises
  (R-DEV-2).
- **Dependencies:** none without a decision in RULES.md (R-SAFE-4, D3).
- **Locks and borrows** are scoped, never held across a call into a
  plug-in (R-ERR-4).

## Tests

- Every test has a doc comment whose `/// Verifies:` paragraph starts
  with one of the tags `cargo xtask verify` accepts (R-TRACE-1,
  `xtask/src/trace.rs`): `[X3.135-1986 §…]`, `[EXTENSION]`, `[DESIGN]`
  or `[TOOL] R-…`. A test of SQL-92 behaviour starts with one of these and
  cites the clause after it, as the existing tests do:
  `/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.34 GR2] ...`.
- At the lowest level that shows the requirement (R-TEST-1), derived from
  the requirement, with robustness cases expecting the exact SQLCODE
  (R-TEST-2); MC/DC pairs named for decisions of two or more conditions
  (R-TEST-8); a regression test for every defect fixed (R-TEST-7).
- Deterministic: no clock, randomness or network beyond loopback
  (R-TEST-5). Shared fixtures are written once, in `common.rs` or
  `support/` (R-TEST-10); `#[test]` and no other framework.

## Citations of the standards

- Behaviour a standard governs cites it as printed (R-DOC-4):
  `[X3.135-1986 §8.6 GR2]`, `[ISO/IEC 9075:1992 §11.19 SR13]`.
- **Check every citation against the standard's text before writing it.
  Never cite from memory.** The texts are kept locally, untracked
  (`fipspub127.txt`, `sql1992.txt`, listed in `.gitignore`), and are
  never committed.
- Code never cites TODO.md (R-DOC-5); `verify` refuses it. Cite the
  standards, RULES.md's decisions, or CONFORMANCE.md.

## SQL work

- Read the BNF of the production in the standard before writing code; the
  parser has one method per production, named after it (R-STYLE-3).
- A SQL extension the standards do not cover needs its BNF written, in the
  standard's notation (as `src/parser/users.rs` gives for CREATE USER), and
  the maintainer's agreement before it is built.

## Documents

- [RULES.md](RULES.md): the decisions (D…) and the rules (R-…).
- [CONFORMANCE.md](CONFORMANCE.md): what is implemented of the standards,
  clause by clause, and the implementation-defined choices (R-DOC-7).
- [TODO.md](TODO.md): open work. Finished work leaves it.
- A finished feature is recorded in the same change: a decision in
  RULES.md if it is one (its row in §1's table too), CONFORMANCE.md for a
  clause implemented or changed (R-TRACE-3), and its item moved or removed
  in TODO.md.
- [doc/](doc/README.md): the manual, for users, operators and programs.
  A change of anything they see (SQL, a message, a setting, an option, an
  event, an API) updates each page that states it, in the same commit and
  in the page's own format (R-DOC-8). Search `doc/` for what changed, a
  statement's name, a message's text, a setting's name; check every claim
  against the code; run every example as written and paste its output as
  it now is. A new statement gets a page in `doc/sql/statements/`, of the
  sections the others have, and a line in its README; a new setting a row
  in Appendix C; a new SQLSTATE one in Appendix A.
- Committed documents state what is; they never say whether a standard's
  text is at hand, missing or to be acquired.

## Design stance

- **A multi-user server is the target.** Weigh every choice for many
  sessions on many threads (D27, D42), not only for an embedded program.
- **Pluggable.** Extension points are traits registered on `Database`
  (`register_engine`, `register_index_method`, `set_log`, `set_spill`,
  ...), listed at the top of `src/db.rs`; `dyn Trait` only there (R-SAFE-3).
  A number an operator tunes is a setting (D36), named `GROUP.NAME` with
  its unit.
- **No thread but through the thread manager** (D55, R-SAFE-5): background
  work is a job of `src/threads.rs`; the server's threads come from
  `server/src/runtime.rs`. `verify` refuses a thread started elsewhere.
- **Multiplatform.** A portable baseline everywhere; platform extras
  `cfg`-gated, and refused clearly elsewhere (as Unix sockets are off
  Unix, D42). Code compiled on one platform only is checked by CI there
  (Windows, macOS, Linux), so it cannot be verified locally on another:
  write it with care, and say so when handing it back.

## Commits and staging

- **Commit only when the maintainer asks.** Never on your own initiative,
  and never push without the maintainer's approval.
- A commit follows CONTRIBUTING.md's Linux form (`area: summary` in the
  imperative, at most 75 characters; a body wrapped at 75 columns saying
  the problem and why), one logical change, passing `verify` on its own.
- **No AI or agent attribution** in a commit or a pull request: no
  `Co-Authored-By` for an agent, no "Generated with", no `Signed-off-by`.
  Contributors own their code (CONTRIBUTING.md, License).
- **Stage with an explicit path list**, `git add -A -- <path> ...`, naming
  every file and directory of the change (with `Cargo.lock` and a new
  crate's directory when they change). Never `git add -A` or `git add .`
  of the whole tree.
- **Never stage `LICENSE`**: it is the maintainer's own. Never commit the
  standards' texts or scratch files.
