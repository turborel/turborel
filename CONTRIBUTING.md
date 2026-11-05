# Contributing to turborel

turborel stores data people depend on, and data that gets lost or corrupted
may be gone for good. So a change is judged first on whether it keeps data
correct, durable and isolated, and then on what it adds. The process follows
the Linux kernel's, adapted to pull requests and to a database.

[Before you start](#before-you-start) · [Setting up](#setting-up) ·
[Making a change](#making-a-change) ·
[Changes that touch data](#changes-that-touch-data) ·
[Commit messages](#commit-messages) · [Pull requests](#pull-requests) ·
[Review](#review) · [Reporting issues](#reporting-issues) ·
[Security](#security) · [License](#license)

---

## Before you start

Read [RULES.md](RULES.md) §3 (code), §4 (tests), §7 (process) and the
decisions for your area. Every rule applies to every change, and
`cargo xtask verify` checks those a tool can.
[CONFORMANCE.md](CONFORMANCE.md) says what is implemented of the standards,
and [TODO.md](TODO.md) lists open work.

Open an issue first for anything larger than a fix. That covers new
features, changes to a decision in RULES.md, new dependencies (R-SAFE-4) and
changes to a file or wire format. Agreeing on the design first saves
everyone a rewrite.

SQL starts from the standard's grammar. Read the clause's BNF before writing
code, and cite its rules by number, as in `[ISO/IEC 9075:1992 §11.36 GR3]`.
Never cite from memory. An extension the standards don't cover is proposed
as BNF in an issue and agreed before it is built.

## Setting up

```
rustup toolchain install          # the toolchain rust-toolchain.toml pins
cargo xtask install-hooks         # run verify before each commit (once per clone)
cargo xtask verify                # every check a change must pass
cargo xtask verify --coverage     # and structural coverage, as CI runs it
```

CI runs `verify` on Linux, macOS and Windows, and coverage on Linux (D21). A
change that passes on one platform and fails on another isn't done.

## Making a change

Keep one logical change per commit, and make each commit pass
`cargo xtask verify` on its own, because anyone bisecting may land on it.
Move code in one commit and change it in another. Don't mix a reformat or
rename with a change of behaviour.

Tests come with the change, at the lowest level that shows it (R-TEST-1).
Cover normal cases, boundaries, and robustness cases with the exact SQLCODE
expected (R-TEST-2). Each test's doc comment starts with `/// Verifies:`
(R-TRACE-1). A fixed defect gets a test that fails without the fix
(R-TEST-7).

Documents change in the same commit. Update CONFORMANCE.md for a clause
implemented or changed (R-TRACE-3), RULES.md for a decision made or changed,
and TODO.md for open work done or found. Code never cites TODO.md.

The manual changes in the same commit too (R-DOC-8). A change to what users,
operators or programs see updates each page of `doc/` that states it, with
its examples run again and their output pasted. A new statement gets a
reference page with the same sections as the others.

Some rules deserve a reminder.

- No `unsafe` (R-SAFE-1), no panics in production code (R-ERR), and no
  dependency without a decision in RULES.md (R-SAFE-4).
- Performance claims cite measurements. Run `cargo xtask bench NAME --save
  before` on the old code and `--compare before` on yours (R-BENCH-2). Give
  the numbers and the costs in memory, latency and code size.
- Coverage floors only rise (R-COV-2).
- A change to a mechanism that `xtask/src/mutate/list.rs` breaks, such as
  MVCC visibility, a checksum, the log's sync, a constraint or a lock, runs
  `cargo xtask mutate` (R-TEST-9). A new mechanism adds its mutation.

## Changes that touch data

These need more than passing tests. Say in the commit message what was done
for each that applies.

File formats include the data file, log, catalog file, blocks, pages,
records and sealed data. Say what happens to files written before the
change. A file is never silently misread. It is either read as before or
refused with a clear error.

For the log, checkpoints and recovery (D25), add a test that crashes at the
point the change concerns and recovers. Fault injection and damaged files
have their own tests (`test/integration/faults.rs`, `damage.rs`,
`durability.rs`, `recovery.rs`).

For transactions and concurrency (D28, D29), say which anomalies the change
could let through, and show each is refused. Set up interleavings
deterministically on one thread where possible (R-TEST-5).

A planner or executor change must not change any query's result. Compare
results with and without it on the queries it affects, including nulls and
empty inputs.

For the wire protocol (D45) and settings (D36), say what an older client or
an existing database sees.

## Commit messages

The form is Linux's.

```
area: summary in the imperative, at most 75 characters

The problem: what goes wrong, or is missing, and who it affects. A
wrong result, data lost, a crash or a refusal is said first, with
the statement that shows it.

Why this change, and how it solves the problem. Mention the choices
made and the alternatives rejected, the rules and clauses applied
(RULES.md D33, R-ERR-4, [ISO/IEC 9075:1992 §11.36 GR3]), and what
was measured, with numbers.

Fixes: 54a4f0239f2e ("txn: release locks of a transaction rolled back")
Closes: <link to the issue>
Reported-by: A Reporter <reporter@example.org>
```

The subject is `area: summary`, at most 75 characters, in the imperative
("refuse a cursor...", not "refused"), with no full stop. It says what
changes, isn't a file name, and isn't reused within a series.

The body is wrapped at 75 columns and written for someone reading `git log`
years later with no context. Say what the change does in plain prose, since
the diff shows how. Don't write "as discussed" or "v2".

Pick the area where most of the change is.

| Area | Covers |
|---|---|
| `parser` | `lexer.rs`, `parser/`, `ast/` |
| `binder` | `binder/`, `params/` |
| `optimizer` | `logical/`, `optimizer/` |
| `planner` | `physical/`, `stats/` |
| `exec` | `exec/`, `spill/` |
| `db` | `db/`: sessions, statements, definitions, checkpoints |
| `recovery` | `db/recovery/`, `db/files.rs`, `db/catalog_file/` |
| `txn` | `txn/` |
| `log` | `log/` |
| `catalog` | `catalog/`, `system/` |
| `rowstore` | `engine/rows/`, `engine/record/` |
| `index` | `index/`, `engine/tree/`, `engine/hash/` |
| `disk` | `disk/`: I/O, blocks, pages, the buffer pool, encodings |
| `value` | `value/`, `functions/`, `hash/` |
| `settings` | `settings/`, `memory.rs`, `diagnostics.rs` |
| `protocol`, `client`, `server`, `turboreld`, `cli`, `lz4`, `replication` | their crates |
| `xtask`, `bench`, `test`, `support` | the tools, benchmarks and shared test fixtures |
| `doc` | `doc/`, README.md, RULES.md, CONFORMANCE.md, TODO.md, AGENTS.md, CLAUDE.md, this file |
| `treewide` | a mechanical change across many areas |

Trailers go at the end, one per line, never wrapped.

| Trailer | Meaning |
|---|---|
| `Fixes: <12+ hex digits> ("<subject>")` | the commit that introduced the defect fixed |
| `Closes: <link>` | the issue this commit resolves |
| `Link: <link>` | background, such as a discussion or an issue fixed only in part |
| `Reported-by:` | who reported the defect fixed, with `Closes:` |
| `Suggested-by:` | who suggested the idea |
| `Co-authored-by:` | a co-author, as GitHub credits one |
| `Reviewed-by:` | a reviewer who checked the change and is satisfied with it |
| `Acked-by:` | a maintainer of the area who approves it without a full review |
| `Tested-by:` | someone who tested it, saying where if that matters |

`Reported-by:` and `Suggested-by:` may be added without asking. The others
name someone who said so on the record. `git config core.abbrev 12` gives
12-digit hashes, and `git log -1 --pretty='Fixes: %h ("%s")' <commit>` writes
the `Fixes:` line.

## Pull requests

A pull request is a patch series, and its description is the cover letter.
The template asks for what and why, the series commit by commit, testing,
compatibility and performance. Each commit message still stands alone.

Base it on `main` and keep it rebased, without merging `main` into it. When
revising, rewrite the commits so the series is again what should be merged,
with no "fix review comments" commits. Force-push, and say in a comment what
changed since the last version.

Trailers given in review go into the commits they cover. Drop them from a
commit that later changes substantially, and say so.

Keep a series to about 15 commits. Split a larger one into series that each
stand alone.

## Review

Reviewers check each commit against RULES.md §7's checklist, and that the
manual is updated (R-DOC-8). For
[changes that touch data](#changes-that-touch-data), they also check that the
crash, isolation, result and compatibility checks were done. Review comments
are about the change, never the person. A reviewer who is satisfied gives
`Reviewed-by:`.

## Reporting issues

Use the issue forms, **Bug**, **Conformance**, **Performance** or
**Feature**. Try the latest `main` first, and add to an existing issue that
matches rather than opening another.

Reproduce from an empty database, with the smallest script of statements
that shows it, including the schema, data and sessions involved.

State the impact first. Put it in the title's first word when it is severe.

| Tag | Meaning |
|---|---|
| `[DATA LOSS]` | committed data lost or damaged, or a database that won't open |
| `[WRONG RESULT]` | a query returns wrong rows |
| `[CRASH]` | a panic, an abort or a hang |
| `[REGRESSION]` | worked in an earlier commit (give the last good commit and, if you can, the first bad one) |

Give the commit (`git rev-parse --short=12 HEAD`), the platform and
architecture, and how turborel ran. Include settings that differ from their
defaults.

```
SELECT SETTING_NAME, SETTING_VALUE, VALUE_SOURCE
FROM INFORMATION_SCHEMA.SETTINGS WHERE VALUE_SOURCE <> 'DEFAULT'
```

## Security

Don't open a public issue for a vulnerability. Report it privately, as
[SECURITY.md](SECURITY.md) says.

## License

turborel is under the BSD 2-Clause license in [LICENSE](LICENSE), and a
contribution is made under that license. Contributors keep their copyright
and own their changes. Whoever submits a change answers for all of it,
however it was written.
