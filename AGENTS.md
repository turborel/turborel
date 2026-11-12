# Guide for coding agents

How to work on turborel as a coding agent, with any tool. If this file
disagrees with [RULES.md](RULES.md) or [CONTRIBUTING.md](CONTRIBUTING.md),
follow those.

turborel is a relational database in Rust, queried in SQL. It ships as a
library, a server (`turboreld`), a client and a command-line client
(`turborel`). See [README.md](README.md).

## Before you change anything

Read RULES.md §3 (code), §4 (tests), §7 (process) and the §1 decisions for
your area. Read CONTRIBUTING.md for how a change is made and the extra
checks for changes that touch data.

## Build and check

Every change must pass `cargo xtask verify --coverage` before it is handed
back.

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
cargo xtask simulate [--seeds N] [--from S] [--plan NAME] [--databases]
                                 # replication simulator sweeps longer than verify runs (D57)
```

`verify` stops at the first failure. CI runs it on Linux, macOS and
Windows, and coverage on Linux (D21). Coverage floors are in
`xtask/src/baseline.rs` and only rise (R-COV-2).

Benchmarks never run in `verify`. A performance claim cites
`cargo xtask bench NAME --save before` on the old code, then
`--compare before` on the change (R-BENCH-2).

Project automation is Rust in `xtask/`, never shell scripts (D7, R-TOOL-1).

## Where things are

| Path | Crate | What |
|---|---|---|
| `src/` | `turborel` | the database as a library, std only |
| `protocol/` | `turborel-protocol` | the wire protocol's messages and codec, no I/O |
| `client/` | `turborel-client` | a blocking connection to a server |
| `server/` | `turborel-server` | the server as a library, on tokio |
| `turboreld/` | `turboreld` | the server program |
| `cli/` | `turborel-cli` | the command-line client `turborel` |
| `lz4/` | `turborel-lz4` | LZ4 compression plug-in |
| `aes/` | `turborel-aes` | encryption and checksum plug-ins |
| `replication/` | `turborel-replication` | Viewstamped Replication and its extension points, std only |
| `support/` | `turborel-test-support` | fixtures the crates' tests share |
| `xtask/` | `xtask` | the project's checks and tools |

Tests live in `test/integration/` (the core's one integration binary),
`server/test/`, `turboreld/test/`, `cli/test/` and `replication/test/`.
Benchmarks live in `bench/`, one file each, on `harness.rs`.

`src/` modules, in the order a statement meets them. Each `foo.rs` keeps
its submodules in `foo/`.

| Module | What |
|---|---|
| `lexer.rs` | tokens, identifiers and key words |
| `parser.rs` | recursive descent, one method per BNF production |
| `ast.rs` | the parsed form of statements |
| `binder.rs` | syntax rules that need the catalog, AST to logical plan |
| `params.rs` | statement parameters |
| `logical.rs` | the logical plan |
| `optimizer.rs` | rewrite rules (`Rule`) |
| `physical.rs` | access paths, joins, costs |
| `stats.rs` | planner statistics |
| `exec.rs` | execution of physical plans |
| `spill.rs` | where operators past their work area put rows (`Spill`) |
| `db.rs` | the database, extension points, sessions |
| `txn.rs` | the transaction coordinator |
| `log.rs` | the write-ahead log (`Log`) |
| `catalog.rs` | schemas, tables, views, columns, indexes, privileges |
| `system.rs` | system tables and the Information Schema |
| `storage.rs` | storage engine traits (`StorageEngine`, `TableStorage`) |
| `engine.rs` | the built-in engines |
| `index.rs` | index methods (`IndexMethod`, `Index`) |
| `disk.rs` | file I/O, blocks, pages, buffer pool, encodings |
| `hash.rs` | hash functions (`HashFunction`) |
| `value.rs` | data types, values, arithmetic, comparison |
| `functions.rs` | scalar and aggregate functions |
| `regex.rs` | XQuery regular expressions, the matcher LIKE_REGEX and the regex functions use (D61) |
| `settings.rs` | the settings registry (D36) |
| `memory.rs` | the memory budget (D38) |
| `diagnostics.rs` | typed events to a pluggable sink (D40) |
| `threads.rs` | the thread manager (D55) |
| `files.rs` | the file manager (D56) |
| `clock.rs` | the clock (D54) |
| `error.rs` | errors as SQLCODE and SQLSTATE |
| `bytes.rs` | reading and writing stored fields |
| `sync.rs` | locks on shared state, poisoning reported |

## Rules that are broken most often

`verify` treats every lint warning as an error. RULES.md §3 and §4 have
the full list.

- No `unsafe` (R-SAFE-1).
- No panics in production code. That covers `unwrap`, `expect`, `panic!`,
  `unreachable!`, `todo!`, `unimplemented!`, indexing, slicing and
  `process::exit` (R-ERR-1).
- Every failure is a `SqlError`, and its message begins with the rule it
  breaks, `[DESIGN]` or `[EXTENSION]` (R-ERR-2).
- No `as` on numbers (R-NUM-2). Integer arithmetic is `checked_*`,
  `wrapping_*`, `saturating_*` or `overflowing_*` (R-NUM-1).
- No `_` arm in a `match` on an enum (R-CF-3). Avoid match guards (R-CF-5).
- Conditions have no side effects (R-CF-4) and at most 4 terms (R-CF-1).
- Keep functions to 50 lines and 5 parameters, and files to 600 lines
  (R-SIZE).
- No magic numbers or strings (R-CONST).
- Modules are `foo.rs` plus `foo/`, never `mod.rs` (R-STYLE-4).
- No printing or `dbg!` in production code (R-STYLE-7).
- Public items have doc comments, with `# Errors` when they return
  `Result` (R-DOC-1). Files start with a `//!` doc (R-DOC-6).
- Comments say why (R-DOC-2). No `TODO` comments or commented-out code
  (R-DOC-5).
- Use `#[expect(lint, reason = "...")]` citing the rule, not `#[allow]`
  (R-DEV-1, R-DEV-4).
- No new dependency without a decision in RULES.md (R-SAFE-4).
- Don't hold a lock or borrow across a call into a plug-in (R-ERR-4).
- Start threads only through `src/threads.rs` (D55, R-SAFE-5).
- Weigh choices for many sessions on many threads, not just an embedded
  program (D27, D42).

## Tests

Every test has a `/// Verifies:` paragraph starting with a tag `verify`
accepts (R-TRACE-1). The tags are `[X3.135-1986 §…]`, `[EXTENSION]`,
`[DESIGN]` and `[TOOL] R-…`. A SQL-92 test starts with one of these and
then cites its clause, for example
`/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.34 GR2] ...`.

Tests are deterministic, with no clock, randomness or network beyond
loopback (R-TEST-5). Fixtures shared by modules go in `common.rs` or
`support/` (R-TEST-10). A fixed defect gets a regression test (R-TEST-7).

## Citing the standards

Check every citation against the standard's text before writing it. Never
cite from memory. The texts are kept locally (`fipspub127.txt`,
`sql1992.txt`) and are never committed. Committed documents never say
whether a standard's text is at hand.

Code never cites TODO.md, and `verify` refuses it (R-DOC-5).

Read a production's BNF in the standard before writing code. A SQL
extension the standards don't cover needs its BNF written in the standard's
notation and the maintainer's agreement before it is built.

## Documents

A finished feature updates these in the same change:

- RULES.md, if it made a decision, including the row in §1's table
- CONFORMANCE.md, for a clause implemented or changed (R-TRACE-3)
- TODO.md, which only holds open work
- [doc/](doc/README.md), for anything users or operators see (R-DOC-8)

For the manual, search `doc/` for what changed, check each claim against
the code, and run every example and paste its real output. A new statement
gets a page in `doc/sql/statements/`, a new setting gets a row in Appendix
C, and a new SQLSTATE gets a row in Appendix A.

## Commits

- Commit only when the maintainer asks, and never push without approval.
- Follow CONTRIBUTING.md's form, with one logical change per commit.
- Add no AI or agent attribution to a commit or pull request. That means
  no `Co-Authored-By` for an agent, no "Generated with" and no
  `Signed-off-by`.
- Stage with an explicit path list, `git add -A -- <path> ...`. Never use
  `git add -A` or `git add .` on the whole tree.
- Never stage `LICENSE`, the standards' texts or scratch files.
- Code compiled for one platform only can't be verified locally on another.
  Say so when handing it back.
