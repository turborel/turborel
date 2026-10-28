# Contributing to turborel

turborel is a relational database: people trust it with data they cannot
get back. A change is judged first on whether it keeps that data correct,
durable and isolated, then on what it adds. The process follows the Linux
kernel's (*Submitting patches* and *Reporting issues* in its
documentation), adapted to a pull-request workflow and to a database.

Contents: [Before you start](#before-you-start) ·
[Setting up](#setting-up) · [Making a change](#making-a-change) ·
[Changes that touch data](#changes-that-touch-data) ·
[Commit messages](#commit-messages) ·
[Pull requests](#pull-requests) · [Review](#review) ·
[Reporting issues](#reporting-issues) · [Security](#security) ·
[License](#license)

---

## Before you start

- Read [RULES.md](RULES.md): §3 (code rules), §4 (test rules), §7
  (process), and the decisions (§1) of the area you will change. Every rule
  applies to every change; `cargo xtask verify` checks those a tool can.
- [CONFORMANCE.md](CONFORMANCE.md) says what is implemented of the
  standards; [TODO.md](TODO.md) is the open work.
- **SQL starts from the standard's grammar.** Read the BNF of the clause in
  the standard before writing code, and cite its rules by number
  (`[ISO/IEC 9075:1992 §11.36 GR3]`). Never cite from memory. An extension
  the standards do not cover is proposed as BNF in an issue and agreed
  before it is built.
- **Open an issue first** for anything larger than a fix: a new feature, a
  change of a decision in RULES.md, a new dependency (RULES.md R-SAFE-4), or
  a change of a file or wire format. Agreeing on the design first saves
  everyone a rewrite.

## Setting up

```
rustup toolchain install          # the toolchain rust-toolchain.toml pins
cargo xtask install-hooks         # run verify before each commit (once per clone)
cargo xtask verify                # every check a change must pass
cargo xtask verify --coverage     # and structural coverage, as CI runs it
```

CI runs `verify` on Linux, macOS and Windows, and coverage on Linux
(RULES.md D21). A change that passes on one platform and fails on another
is not done.

## Making a change

- **One logical change per commit**, and each commit passes `cargo xtask
  verify` on its own: anyone bisecting may land on any commit of a series.
  Move code in one commit and change it in another. Do not mix a
  reformatting or a rename with a change of behaviour.
- **Tests come with the change**, at the lowest level that shows it
  (R-TEST-1): normal cases, boundaries, and robustness cases with the exact
  SQLCODE expected (R-TEST-2). Each test's doc comment starts with `///
  Verifies:` and names what it verifies (R-TRACE-1). A fixed defect gets a
  test that fails without the fix (R-TEST-7).
- **Documents change in the same commit:** CONFORMANCE.md for a clause
  implemented or changed (R-TRACE-3), RULES.md for a decision made or
  changed, TODO.md for open work done or found. Code never cites TODO.md.
- **The manual changes in the same commit** (R-DOC-8): a change of what
  users, operators or programs see, SQL, a message, a setting, an
  option, updates each page of `doc/` that states it, in that page's
  format, its examples run again and their output pasted as it is. A new
  statement gets a reference page of the sections the others have.
- **No `unsafe`** (R-SAFE-1), no panics in production code (R-ERR), and no
  dependency without a decision in RULES.md (R-SAFE-4).
- **Performance claims cite measurements:** `cargo xtask bench NAME --save
  before` on the code before, then `--compare before` on yours (R-BENCH-2).
  Give the numbers, and the costs: memory, latency of other paths, code
  size.
- **Coverage floors only rise** (R-COV-2).

## Changes that touch data

These need more than tests that pass. Say in the commit message what was
done for each that applies.

- **File formats** (the data file, the log, the catalog file, blocks,
  pages, records, sealed data): say what happens to files written before
  the change. A file is never silently misread. Either it is read as
  before, or it is refused with a clear error.
- **The log, checkpoints and recovery** (RULES.md D25): add a test that
  crashes at the point the change concerns and recovers. Fault injection
  and damaged files have their own tests (`test/integration/faults.rs`,
  `damage.rs`, `durability.rs`, `recovery.rs`).
- **Transactions and concurrency** (D28, D29): say which anomalies the
  change could let through, and show each is refused. Interleavings are
  set up deterministically on one thread where possible (R-TEST-5).
- **Query results:** a planner or executor change must not change any
  query's result. Compare results with and without it on the queries it
  affects, nulls and empty inputs included.
- **The wire protocol** (D45) and **settings** (D36): say what an older
  client or an existing database sees.

## Commit messages

The form is Linux's:

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

- **Subject:** `area: summary`, at most 75 characters, in the imperative
  ("refuse a cursor...", not "refused" or "refuses"), with no full stop.
  The summary says what changes. It is not a file name, and it is not
  reused within a series.
- **Body:** wrapped at 75 columns, written for someone reading `git log` in
  years, with no context. Say what the change does as plain prose; the
  diff shows how. Do not write "as discussed" or "v2": the message stands
  alone.
- **Areas**, by where most of the change is:

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
  | `protocol`, `client`, `server`, `turboreld`, `cli`, `lz4` | their crates |
  | `xtask`, `bench`, `test`, `support` | the tools, benchmarks and shared test fixtures |
  | `doc` | `doc/`, README.md, RULES.md, CONFORMANCE.md, TODO.md, AGENTS.md, CLAUDE.md, this file |
  | `treewide` | a mechanical change across many areas |

- **Trailers**, at the end, one per line, never wrapped:

  | Trailer | Meaning |
  |---|---|
  | `Fixes: <12+ hex digits> ("<subject>")` | the commit that introduced the defect fixed |
  | `Closes: <link>` | the issue this commit resolves |
  | `Link: <link>` | background: a discussion, or an issue fixed only in part |
  | `Reported-by:` | who reported the defect fixed; with `Closes:` |
  | `Suggested-by:` | who suggested the idea |
  | `Co-authored-by:` | a co-author, as GitHub credits one |
  | `Reviewed-by:` | a reviewer who checked the change and is satisfied with it |
  | `Acked-by:` | a maintainer of the area who approves it, without a full review |
  | `Tested-by:` | someone who tested it, saying where if that matters |

  `Reported-by:` and `Suggested-by:` may be added without asking. The
  others name someone who said so, on the record. `git config
  core.abbrev 12` gives 12-digit hashes, and `git log -1
  --pretty='Fixes: %h ("%s")' <commit>` writes the `Fixes:` line.

## Pull requests

A pull request is a patch series; its description is the cover letter.
The template asks for:

- **What and why:** the problem, its impact, and the approach. Each commit
  message still stands alone.
- **The series:** what each commit does, in order.
- **Testing:** `cargo xtask verify --coverage` passed, the tests added and
  what they verify, and the crash, concurrency or result checks of
  [Changes that touch data](#changes-that-touch-data).
- **Compatibility:** files, the log, the protocol, settings, SQL behaviour.
- **Performance:** `bench --compare` numbers, if it claims any.

Then:

- **Base it on `main` and keep it rebased.** Do not merge `main` into it.
- **Revising:** rewrite the commits so the series is again what should be
  merged. Do not add "fix review comments" commits. Force-push, and say in
  a comment what changed since the last version ("v2: split the log
  change out; renamed..."). That comment is the text Linux puts below
  `---`: it helps review, and never reaches the history.
- **Trailers given in review** (`Reviewed-by:`, `Tested-by:`, `Acked-by:`)
  go into the commits they cover. Drop them from a commit that later
  changes substantially, and say so.
- **Keep series reviewable:** about 15 commits at most. Split a larger one
  into series that each stand alone.

## Review

Reviewers check each commit against RULES.md §7's checklist:

- the requirement is cited in the code and the tests;
- normal and robustness tests are there, with MC/DC pairs for new decisions;
- there are no magic numbers or strings;
- comments say why;
- conditions have no side effects;
- borrows and locks are scoped;
- the documents are updated, the manual among them (R-DOC-8).

For [changes that touch data](#changes-that-touch-data), reviewers also
check that the crash, isolation, result and compatibility checks were done.
Review comments are about the change, never the person. A reviewer who is
satisfied gives `Reviewed-by:`.

## Reporting issues

Use the issue forms: **Bug**, **Conformance**, **Performance** or
**Feature**. As Linux asks:

- **Try the latest `main`** first. Search existing issues, and add to one
  that matches rather than opening another.
- **Reproduce from an empty database:** the smallest script of statements
  that shows it, with the schema, the data, and the sessions if more than
  one is involved.
- **Say the impact first.** Put it in the title's first word when it is
  severe:
  - `[DATA LOSS]`: committed data lost or damaged, or a database that will
    not open;
  - `[WRONG RESULT]`: a query returns wrong rows;
  - `[CRASH]`: a panic, an abort or a hang;
  - `[REGRESSION]`: worked in an earlier commit. Give the last good commit
    and, if you can, the first bad one (`git bisect`).
- **Give the version and environment:** the commit (`git rev-parse
  --short=12 HEAD`), the platform and architecture, and how turborel ran
  (embedded, `turboreld`, or the command-line client).
- **Give the settings that differ from their defaults:**
  `SELECT SETTING_NAME, SETTING_VALUE, VALUE_SOURCE FROM
  INFORMATION_SCHEMA.SETTINGS WHERE VALUE_SOURCE <> 'DEFAULT'`.

## Security

Do not open a public issue for a vulnerability. Report it privately, as
[SECURITY.md](SECURITY.md) says.

## License

turborel is under the BSD 2-Clause license in [LICENSE](LICENSE). A
contribution is made under that license. Its contributors keep their
copyright, and own their changes: whoever submits one answers for all of
it, however it was written.
