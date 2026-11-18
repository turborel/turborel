# turborel rules

How turborel is designed, written and verified. Every change follows these
rules; `cargo xtask verify` checks those a tool can check, and review
checks the rest.

Rules (R-…) say how code and tests are written. **shall** is mandatory,
and a departure needs a recorded deviation (§6). **should** is expected, and
a reviewer may accept a reasoned exception.

Decisions (D1…) record how the system is built and why. Change one only by
changing this file in a reviewed commit. A decision says what was chosen and
why, and the module it names holds the mechanism.

Cite both by ID in code, tests, reviews and commit messages, as in
`RULES.md D29` or `R-ERR-2`.

Before changing code, read §3 (code rules), §4 (test rules) and the
decisions covering your area. What turborel implements of the standards is
in [CONFORMANCE.md](CONFORMANCE.md), and open work is in [TODO.md](TODO.md).

The rules serve as the *Software Code Standards* and the core of the
verification approach DO-178C expects (§8).

[1 Decisions](#1-decisions) · [2 Layout](#2-project-layout) ·
[3 Code](#3-code-rules) · [4 Tests](#4-test-rules) ·
[5 Coverage](#5-structural-coverage-and-mcdc) · [6 Deviations](#6-deviations) ·
[7 Process](#7-process-and-tools) · [8 DO-178C](#8-do-178c) ·
[9 References](#9-references)

---

## 1. Decisions

| # | Decision |
|---|---|
| **The project** | |
| D1 | The project is turborel, and SQL-86 is its first dialect |
| D2 | DO-178C Level A objectives are the target |
| D3 | No third-party dependencies |
| D6 | Production code only |
| D23 | A manual of what is settled, and no design documents yet |
| **Tooling and process** | |
| D4 | Tests in Rust's standard levels |
| D5 | One integration test binary for each crate tested from outside |
| D7 | Automation in Rust |
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
| D32 | SQL-86, SQL-92 and SQL:2023 are one language |
| D33 | Schema changes are transactional |
| D47 | Schema statements on their own |
| D49 | Temporary tables, an instance per session |
| D50 | Domains copy their constraints onto their columns |
| D51 | An assertion is a query whose rows break it |
| D52 | A view's REFERENCES come from what its columns are drawn from |
| D53 | One contract for data types, with the standard types built in |
| D54 | Datetimes kept in UTC, shown in the session's zone, read from a clock |
| D58 | Dynamic SQL in SQL text, with a session as the module |
| **Architecture** | |
| D25 | One log, owned by the database |
| D26 | Cryptography only from vetted crates, in plug-ins |
| D48 | Compression and encryption as encodings, per file or per table |
| D55 | One thread manager, with each thread labelled and shown |
| D56 | One file manager a database, with open files limited and shown |
| D57 | Replicated state is the log, the catalog and the durable files, as opaque bytes |
| D27 | A thread-safe core over snapshots of the definitions |
| D28 | Transactions may wait for locks, and deadlocks are refused |
| D29 | Engines own concurrency, at logical timestamps |
| D30 | A storage foundation of independent components |
| D31 | The built-in engines are a row store and a column store |
| D34 | Index methods support comparisons and function predicates |
| D35 | The optimizer and planner |
| D36 | Settings |
| D37 | System tables and the Information Schema |
| D38 | The memory budget, and spilling |
| D39 | Directions for extensibility |
| D40 | Diagnostics are typed events to a pluggable sink |
| D41 | Table settings are options an engine declares for its tables |
| D42 | The server has three modes over one core |
| D43 | Users, passwords and the database's owner |
| D44 | A running statement is cancelled from another thread |
| D45 | Protocol version 1, and prepared statements |
| D46 | The command-line client |
| D59 | Import is rows from a pluggable source, over a connection |
| D60 | Collations are sort keys, and a collation never changes |
| D61 | Regular expressions are XQuery's, by a matcher of our own |
| D62 | A sequence generator's place is logged apart from transactions |
| D63 | Identity and generated columns are given their values as rows are written |
| D64 | TRUNCATE TABLE replaces a table's storage |
| D65 | LIKE and AS are found again from what they read |
| D66 | A key of NULLS NOT DISTINCT holds its nulls in its index |
| D67 | A recursive query is iterated over held work tables, bounded by a setting |
| D68 | Arrays and rows are values of shared parts, nested to a bound |
| D69 | A distinct type's value is its source's, untagged |
| D70 | SEARCH and CYCLE are their expansion, its growing columns widened |
| D71 | Triggered actions run inside their statement, nested to a bound |
| D72 | Spatial pages use R-link split stamps and R* partitioning |
| D73 | SQL-invoked routines are bound as their owner, a function's body a subquery of its parameters |
| D74 | Overloads are chosen by one place per kind of number |
| D75 | Roles: one authorization cell, applicable roles by fixed point |
| D76 | Routine bodies: one program, two hosts |
| D77 | SQL-server modules: catalog routines of a module, its tables in a schema of its own |
| D78 | Polymorphic table functions: compiled as bound, run whole as the cursor opens |
| D79 | Large objects stream: chunked, logged by chunk, read by range, reached by locators |
| D80 | Multisets are unordered values, paired by classes and a matching |
| D81 | Structured types: values name their type, typed tables keep their own rows, references are octets |
| D82 | System-versioned tables keep their historical rows in a hidden table of their own |
| D83 | A column is renamed in place, refused while a text names it |
| D84 | SQL/JSON items hold SQL values, and a path is parsed once, apart from SQL |
| D85 | Geometry values start with XY representations through SqlType |
| D86 | An inverted index keeps terms, places and counts in one B-link tree |
| D87 | SQL/MM Full-Text is built in, its methods calls of registered functions |
| D88 | Savepoints reuse the undo marks, and cancel staged definitions |
| D89 | The JSON type is an SQL/JSON item stored parsed, as a typed binary tree read in place |
| D90 | JSON_TABLE is a collection derived table, its plan a cursor of one tree |
| D91 | An inverted index of JSON documents' paths and values, every row found rechecked |
| D92 | An index's key part may be an expression that cannot raise, matched by its canonical form |
| D93 | VECTOR is built in, without order or equality, its metrics plug-ins |
| D94 | An approximate search may miss rows, never return a wrong one, and raises what an exact one would |
| D95 | The vector index methods are one Index over engines, each serving its own metric approximately |
| D96 | A DiskANN index keeps its graph in pages and its codes in budgeted memory |
| D97 | An HNSW index is held in budgeted memory, kept as an image and a log in pages |
| D98 | An IVF index writes its lists through, its centroids and directory held in budgeted memory |
| D99 | The inverted index answers a sparse DOT approximately |
| D100 | A hybrid index is a full-text part and a vector part, and HYBRID_SEARCH the query of their two lists fused |
| D101 | Embedding models are schema objects bound to registered plug-ins |
| D102 | A vector pipeline is four row triggers on its source |
| D103 | What waits for an embedding is found in the rows, filled after commit |
| D104 | A scalar function may take a batch's rows at once |
| D105 | Bracketed comments are separators, and a leading `/*+` one carries hints |
| D106 | XML values are trees of the data model, read by a parser of our own |

### The project

#### D1. The project is turborel, and SQL-86 is its first dialect

The crate is `turborel`. Its first dialect is ANSI X3.135-1986, and later
ISO SQL standards extend it (D32). The engine is the product, and a
standard is one stage of it.

#### D2. DO-178C Level A objectives are the target

We aim for them where tooling allows, and no certification is claimed.
Only Level A requires MC/DC, and writing to it from the start beats
retrofitting.

#### D3. No third-party dependencies

This covers the library, client and tools, so that every line is
reviewable and no supply chain needs qualifying. The exceptions are:

- cryptography in plug-ins (D26), and in the client and protocol only with
  their features `tls` and `scram`
- the platform's certificate authorities, in the client only with its
  feature `system-roots` (D26)
- compression in plug-ins (D48)
- tokio and socket2 in the server crate (D42)
- tokio's signals in the command-line client
- rpassword in the command-line client and `turboreld` (D46)

Generated data counts too. `src/value/collation/unicode13.bin` is no
dependency, but it can't be read line by line. So `cargo xtask unicode` makes
it from Unicode's own files, the build holds it to a digest, and
`cargo xtask unicode-check` runs Unicode's conformance file against it (D60).
`src/value/collation/cldr38.rules` and `cldr38.groups` can be read, but
`cargo xtask cldr` makes them from CLDR's files all the same, and the build
holds the rules to a digest too. `src/regex/unicode13.bin`, the General
Categories, blocks and case variants regular expressions read (D61), comes
from the same `cargo xtask unicode`, and the build holds it to a digest too.

#### D6. Production code only

There is no demo program, `examples/` directory or golden-file test,
because a toy costs upkeep and hides what is real. Built-in plug-ins are
what a production database ships, and a test needing another uses a
test-local one. `bench/` measures and is a tool.

#### D23. A manual of what is settled, and no design documents yet

The README introduces the project. RULES.md, CONFORMANCE.md and TODO.md
are the working documents. `doc/` is the manual, covering settled
behaviour that users and operators can see. A statement's page has the
sections NAME, SYNOPSIS, DESCRIPTION, RULES, PRIVILEGES, ERRORS, EXAMPLES,
STANDARD and SEE ALSO.

Internals are documented in doc comments, which live beside their code,
because documents written before a design settles go stale.

### Tooling and process

#### D4. Tests in Rust's standard levels

Tests are unit, integration or doc tests (R-TEST-1), laid out as Cargo and
the Rust Book describe.

#### D5. One integration test binary for each crate tested from outside

Each binary has a module per area, which gives one link step and shares
fixtures without `mod.rs` tricks. They are `test/integration/main.rs`,
`server/test/main.rs`, `turboreld/test/main.rs`, `cli/test/main.rs` and
`replication/test/main.rs`.

#### D7. Automation in Rust

Automation is `cargo xtask`, never shell scripts. It stays portable, and
it is reviewed, linted and tested like the product.

#### D8. The toolchain is pinned

It is pinned to 1.99.1 in `rust-toolchain.toml`, because the compiler is a
configuration item (R-CM-1).

#### D9. Formatting

`rustfmt.toml` sets `max_width = 140` and `use_small_heuristics = "Max"`,
the width that changed existing code least.

#### D10. Lints

Clippy `pedantic` plus the restriction lints in `Cargo.toml`, with
thresholds in `clippy.toml`, enforce R-ERR, R-NUM, R-CF and R-SIZE
mechanically.

#### D11. Lint baselines only shrink

Code predating a rule may carry a file-level `#![expect(...)]` listing
what it still breaks. None does today.

#### D12. Overflow checks in release builds

`overflow-checks = true` makes release builds behave like tested builds
(R-NUM-1).

#### D13. `doc_markdown` is allowed

This lets citations such as `GR3a` appear as the standard prints them
(R-DOC-4).

#### D14. Coverage floors only rise

See R-COV-2.

#### D16. Three consuming idioms are allowed in conditions

These are a recorded deviation from R-CF-4 (R-DEV-5). Each is one call
that only consumes the input decided on, and rewriting about 50 sites
would hide the parser's structure.

- `if self.accept_*(..)` in the parser
- `while let Some(x) = q.pop_front()`
- `if x.take().is_none()`

#### D17. `unnested_or_patterns` is allowed

The lint contradicts R-CF-5, which keeps a decision's conditions visible
for MC/DC test design.

#### D20. Lint baselines are counted

`cargo xtask lint-audit` builds with baselines off and compares each
file's count of each lint with `xtask/lint-baseline.txt`. A list of lint
names alone would let another violation of a listed lint pass.

#### D21. `verify` runs in a pre-commit hook and in CI

The hook (`cargo xtask install-hooks`) runs before each commit. CI runs on
Linux, macOS and Windows for every push and pull request, with coverage on
Linux. We need both because a hook can be skipped and only checks one
platform.

#### D22. No instrumented MC/DC

Test design shows MC/DC (R-TEST-8) and review checks it. Stable Rust
can't measure it, and a rule awaiting a nonexistent tool can't be met.

#### D24. Format errors name tokens, not clauses

A BNF violation (SQLCODE -102) says "expected *x*, found *y*" at its
position, and every other error cites its rule (R-ERR-2). Generic parser
helpers can't know their caller's clause, and the position identifies it
anyway.

### SQL semantics

#### D15. Whether a run-time error is raised does not depend on the plan

A WHERE or HAVING conjunct that can raise an error is evaluated on every
row or group the clause covers, whatever the access path or rewrite. When
several rows or conjuncts would raise, the statement raises the first the
plan meets, which may differ by plan. One of them is raised under every
plan.

The standard leaves no room to skip an operand. §5.18 GR2 defines AND only
by its truth table, and §5.21 GR2 and §5.23 GR2 apply the condition to
each row and group. The order isn't fixed, because §3.3.4.4 makes it
implementation-dependent (Annex C item 5).

The same holds for ON, which applies to every row of the operands' product
[ISO/IEC 9075:1992 §7.5 GR1c]. Raising conjuncts over products and outer
joins move onto their operand over a guard (`optimizer/raising.rs`,
`optimizer/outer.rs`). The planner reads an operand's table below its
guard (`physical/access/guarded.rs`): its conjuncts choose an index of
any method as the table's own filter's would, which leaves out only rows
of which no conjunct is true or raises, and stay over the product of the
index's rows and the guard, so each is evaluated on the rows the index
gives while the guard has a row, and on none while it has none. SUM and AVG judge only their final result, never a
running sum, so row order can't decide an overflow [ISO/IEC 9075:1992 §6.5
GR2b iv] (`functions/sum.rs`).

There are two exceptions. EXISTS stops at the first row its subquery's
condition keeps, which §8.8 GR1 allows because that row decides the
cardinality, and Annex C item 6 leaves an inessential part's exception
implementation-dependent. Scalar and row subqueries stop at their second
row, which makes them fail [§7.11 GR1]. IN, ANY, SOME and ALL read every
subquery row, so they raise any of their errors.

#### D18. A limit the standard does not set is not a syntax error

Valid SQL beyond an implementation limit (CONFORMANCE.md), or never (D32)
or not yet implemented, gets SQLCODE -900 ("valid, but not processed"),
not -103. -103 would falsely claim a broken Syntax Rule, and §3.4 requires
conforming SQL to be processed.

The parser reads the whole Format before refusing
(`Parser::refuse_later`), so SQL breaking a Format still gets -102.

#### D19. A failed undo damages its table

The statement or ROLLBACK gets SQLCODE -914 naming each table and failure,
and later statements on the table get -914, except DROP TABLE. With a
statement that could not be undone, the transaction can only roll back
(-913). `Database::damaged_tables` lists damaged tables.

No checkpoint or clean close runs while a table is damaged, so the last
checkpoint and the log keep what it should be. `Database::repair_table`
rebuilds damaged indexes from the rows. Reopening recovers damaged rows,
and a table kept nowhere can only be dropped. The memory budget never
refuses an undo (D38).

A silently half-undone table would break §7.3 GR3b i and §8.9 GR1.
Refusing the table instead hides a state no transaction produced, and the
log can put it right.

#### D32. SQL-86, SQL-92 and SQL:2023 are one language

ISO/IEC 9075:1992 extends X3.135-1986. The newer wins where they differ,
and restrictions SQL-92 keeps only for Entry or Intermediate SQL are
lifted. One consistent language serves programs better than a choice of
dialects.

SQL-92 and SQL:2023 are implemented side by side, each from its own text
and never from memory or other products. Where SQL:2023 differs from
SQL-92, it wins, as SQL-92 does over SQL-86. The texts are
local and untracked, and cited as `[X3.135-1986 §…]`,
`[ISO/IEC 9075:1992 §…]` and `[ISO/IEC 9075-2:2023 §…]`.

The module language and embedded SQL are not claimed. Character sets other
than the one there is, translations and leap seconds will never be
implemented (CONFORMANCE.md). One character set keeps comparison, keys and
storage simple. Collations don't need a second one (D60).

#### D60. Collations are sort keys, and a collation never changes

People sort and search text in their language, and expect `Bob` and `bob` to
be one name, so scalar order alone isn't enough. A collation
[ISO/IEC 9075-2:2023 §4.3.6] turns a string into a sort key, bytes that
compare as the strings do. UCS_BASIC and SQL_TEXT are the order comparison
always had. UNICODE is UCA 13.0, which UTS #10 maps to ISO/IEC 14651:2020, at
the Shifted option and four levels. CASE_INSENSITIVE [EXTENSION] compares each
character's simple case folding. CREATE COLLATION gives one of them another
name or pad characteristic.

The default stays as it was. A comparison by it is bare, so the optimizer,
zones, Bloom filters and batches see nothing new. A comparison by another
compares `Expr::SortKey`s, which are `Value::Binary`, so sorting, hashing and
spilling need no change either. An index on a collated column holds sort keys
as BINARY VARYING, its key from `IndexDesc::key_of` wherever a row or values
become a key. Such an index covers no query, gives no order, and is told to an
engine as reading every row, so MVCC may refuse more commits and never fewer.
Storage stays in scalar order, and only bare comparisons reach it. GROUP BY,
DISTINCT and UNION group by keys and show each group's first row. A hash join
makes its keys columns of its operands (`physical/collated.rs`).

§9.15 decides the collation, from the statement's text (`binder/collation.rs`).
It would refuse `C = 'x'` of any collated column, so a literal, parameter,
CAST or USER yields to a column's, as SQL-92's Coercible did. A session
collation applies to direct and prepared statements, as they are prepared, and
never to views or constraints, which keep what they were defined with.

A collation also gives a string's weights one level at a time
(`level_weights`) and where a string may be cut so that no contraction
or reordering spans the cut (`safe_cut`). Equality of character large
objects under a collation compares them so, a chunk at a time, never
making a sort key of a whole value (`value/collation/streaming.rs`, D79).
A collation splits a string into collation units too (`units`), each
weighed at each level as the whole string weighs it, for XQuery's
substring matching (D106).

UNICODE_<LOCALE> [EXTENSION] is UNICODE tailored by CLDR 38's rules, one for
each collation type of each locale but those for search. UNICODE_SV puts `å`,
`ä` and `ö` after `z`, UNICODE_EL puts Greek before Latin, and UNICODE_ZH
orders ideographs by pinyin. A tailored letter has to fit between UNICODE's
weights, so a tailoring's weights are 64 bits, UNICODE's in the upper half
(`value/collation/tailored`). The Han orders tail tens of thousands of
ideographs after one weight, which 16 bits could not count.

The rules tailor UNICODE itself rather than CLDR's root collation, which orders
105 of its 24,412 characters otherwise, currency symbols, length marks and a
few Cyrillic letters among them. `[reorder]` moves CLDR's script groups, laid
over UNICODE's primaries as UTS #35 describes for allkeys.txt
(`value/collation/reorder.rs`).

A collation's name fixes its behaviour. An index goes wrong when a library
changes its order underneath it, so the table is in the tree, a new
Unicode version is a new collation, and a database whose collation has another
version does not open. Case folds come from that table, not the toolchain's.

DROP COLLATION reverts its columns to the default [§11.44 GR2], which reorders
their indexes, so each table's rows are copied as ALTER TABLE copies them.
REVOKE USAGE on it with CASCADE reverts the same way the columns and domains of
the schemas it leaves without USAGE [ISO/IEC 9075-2:2023 §12.7 GR12n, GR12ad],
and DROP SCHEMA with CASCADE those of other schemas of the collations it takes
[§11.2 GR5]. A table whose foreign key references one reverted after it, or not
reverted, is checked again, so a key holds by the collation its referenced
columns end with. The reverts need no Access Rule, as the statements the
standard effectively executes run without one, so a table of another owner is
reverted too. A definition of a column or domain of a collation made holds the
collation's name shared to its end, which a revert holds exclusive.

#### D61. Regular expressions are XQuery's, by a matcher of our own

LIKE_REGEX, OCCURRENCES_REGEX, POSITION_REGEX, SUBSTRING_REGEX and
TRANSLATE_REGEX read a pattern as XQuery reads it
[ISO/IEC 9075-2:2023 §9.28], so F&O 3.1 §5.6 and the grammar of XSD 1.1 Part
2 Appendix G define what it matches, with the standard's changes for UTS #18's
newlines [GR9a]. No library reads that dialect without changes, and D3 keeps
one out anyway, so `src/regex.rs` reads, compiles and matches it.

A pattern compiles to a program for a Pike VM, which runs every way at once
and takes time linear in the string. A back-reference matches no automaton,
so a program with one runs on a backtracker, which counts its steps and gives
SQLCODE -900 past `MAX_STEPS` (D18) rather than run for ever. A program past
`MAX_INSTRUCTIONS`, or groups nested past `MAX_NESTING`, is refused the same
way. Categories, blocks and case variants come from Unicode 13.0, the version
the collations have, in a table made by `cargo xtask unicode` (D3). Matching
takes no account of a collation [F&O 3.1 §5.6.3], so a CASE_INSENSITIVE column
needs the flag `i`.

A constant pattern is compiled once for a statement. A pattern from a column
or a parameter is compiled again each time it changes, which is Feature F847.
So the binder makes each call its own function, keeping its last pattern, and
none is registered. TRANSLATE_REGEX replaces as fn:replace does, matching the
pattern again within each match [§9.29 GR9a].

XQuery 1.0's fn:matches, fn:replace and fn:tokenize read a pattern in the
dialect of XQuery 1.0 Functions and Operators §7.6.1 (`Regex::xquery`):
a newline is a line feed alone, `\s` is XML Schema's, and there is no
`(?:` and no flag `q`.

The pattern facet of XML Schema reads its patterns by the same parser
(`Regex::schema`), in the dialect of XML Schema Part 2 Appendix F: a
pattern matches a whole string, `^` and `$` are normal characters, `.` and
`\s` are as XML Schema defines them, and there are no flags, reluctant
quantifiers, groups that capture nothing or back-references.

#### D62. A sequence generator's place is logged apart from transactions

A sequence generator's descriptor is a definition, made, changed and dropped
in a transaction (D33). Its current base value is not, as no transaction
controls it [ISO/IEC 9075-2:2023 §4.29.1], and a value given out is never
given again. So the place is a counter in `db/sequences/counters.rs`, shared
by every snapshot, keyed by the generator's name and an epoch its descriptor
holds. CREATE SEQUENCE gives a new epoch, and so does ALTER with RESTART. The
schema statement's record logs the epochs it gave, so a replay makes the same
ones, and a descriptor rolled back never meets a counter of another's.

Before it gives out a value, a counter appends a record, of no transaction, of
where to go on after a crash, and syncs it. The record reserves a block of
values ahead, 32 at first, doubling up to 1024, so a long run of draws syncs
rarely and a crash skips at most 1024 values. The log is a plug-in, so no lock
is held across it (R-ERR-4). One draw reserves while the others wait.

A checkpoint settles every counter. It records each base in the image, with
the last epoch given, and drops what each reserved, so the next draw logs a
record after the checkpoint's start. A block reserved across the settling is
given up, as its record may be before the start. The image keeps a counter of
a name no descriptor has, of a descriptor's own epoch, or of an epoch later
than every descriptor's of its name, as an open transaction may yet commit it.
A database opened forgets every counter no descriptor names. An identity
column's generator is a descriptor here too (D63).

#### D63. Identity and generated columns are given their values as rows are written

An identity column's sequence generator is part of its column's descriptor
[ISO/IEC 9075-2:2023 §11.4 GR4], named as its table is. Its epoch and counter
are a sequence generator's (D62). CREATE TABLE and ADD COLUMN give the epoch,
and so does ALTER COLUMN with RESTART. The ALTER TABLE record logs the epochs
it gave, as a schema statement's does. A user's sequence generator may share
the table's name, so a checkpoint keeps each counter of a published
generator's own epoch, whatever another of its name holds.

A generated column's values are stored with the row. Each statement that
writes a table binds its generation expressions once, from the text the
descriptor keeps, and computes them of each row after its other values, the
identity column's among them [§15.11 GR6a, §15.14 GR9]. Reads then need no
expression, and indexes and constraints take the values as any column's. A
column computed as it is read would save the space, but every read path
would bind expressions, and the standard defines no such column.

DROP COLUMN CASCADE of a column a generation expression references first
drops those generated columns, each as an ALTER TABLE of its own, logged
apart, so a replay takes the same steps (`db/generated.rs`). DROP DOMAIN,
DROP COLLATION, DROP SCHEMA and REVOKE USAGE with CASCADE do the same for
the generated columns whose expressions name a domain or collation they
take away (`db/generated/named.rs`). The statement's own change then finds
none, and refuses if it does. Dropping a column copies rows, which a
change of definitions alone cannot, and an expression left naming what is
gone would no longer bind.

A generated column of ASYNC \[EXTENSION\] is not computed as its row is
written. It is the null value until the queue after commit sets it to
DEFAULT (D103).

#### D64. TRUNCATE TABLE replaces a table's storage

TRUNCATE TABLE [ISO/IEC 9075-2:2023 §14.10] gives a persistent table a new
generation of its storage, empty, as ALTER TABLE gives it one, with empty
indexes (`db/alter/truncate.rs`). The change is staged with the
transaction, logged as an ALTER TABLE record of a word no statement writes,
and published at commit, so recovery makes it as it makes ALTER TABLE.
RESTART IDENTITY restarts the identity column's generator in the same
change, its epoch logged with it (D62). No row is deleted one by one,
logged, or taken from an index, so the statement costs the same whatever
the table holds.

The cost is concurrency. The table's exclusive lock is held to the end of
the transaction, and a transaction that began before the truncation
committed is refused at its next use of the table, as after ALTER TABLE
[GR4, IA234]. The read that tells whether the table had a row is checked at
commit at every isolation level, so a row committed after the snapshot
fails the commit instead of going unseen. A temporary table's rows are each
session's (D49), so the session's own are deleted as a DELETE deletes them.

No row the statement did tells an assertion its change, and rows ALTER TABLE
copies or checks are none added. So each CHECK with a subquery and each
assertion that reads such a table is checked whole, and the totals an
assertion keeps are put right as that check is made (D51,
`db/integrity/rewritten.rs`).

#### D65. LIKE and AS are found again from what they read

A `<like clause>` and an `<as subquery clause>` stand for the column
definitions of what they read [ISO/IEC 9075-2:2023 §11.3 SR9b, SR10]. The
definition is logged as its text, as every schema statement's is (D33), and
made again from it at commit and by recovery, which find the columns again
(`db/contents.rs`). Logging the columns instead would need every type,
collation and generation written back as text, plug-in types among them.

So what was read must read alike then. The table LIKE names, and the
tables, views and sequence generators the query names, are held shared to
the end of the transaction, so no other alters or drops one before the
commit, and the log replays changes in the order they committed. Neither
names a session's declared local temporary table, which no log keeps, but
in a declared table's own definition, which none replays.

WITH DATA inserts the query's rows into the new table's storage before
the definition is staged, as ALTER TABLE copies rows (D33), so a failure
leaves no table. Its rows are logged before the definition, and recovery
redoes them after it, by the table's name and generation. A `<schema
definition>` run apart from a session has no transaction to insert in, so
AS is refused in a CREATE SCHEMA.

#### D66. A key of NULLS NOT DISTINCT holds its nulls in its index

An index holds no key with a null, as no comparison with a null is true
[X3.135-1986 §5.11 GR2], so a key of NULLS DISTINCT, the default, needs
none to find its duplicates. A key of NULLS NOT DISTINCT [ISO/IEC
9075-2:2023 §11.7] counts a null as a value, so its index holds every key,
and a second null is found as a second value is: by the same probe of the
same index at the statement's end, the same -911 for a row another
transaction is changing, and the same entries logged and found again by
recovery. Scanning for it instead would cost a read of the table each
time.

Whether an index holds nulls is decided in one place, from the table's
constraints, and every path that adds or removes an entry asks it
(`catalog/keys.rs`). Only the constraint's own index holds them, or one
built again to take its place, as each is made and dropped with the
constraint. A key with a null sorts before every value, so a probe whose
range is open below starts after them, and the `Index` contract says no
comparison probe selects one. A function lookup may return a null key as
an extra candidate; the executor checks the row against the whole WHERE
condition before returning it (D34). The planner reads an index whole
only of columns that are NOT NULL, and nulls are
ordered after every value where the index orders them first.

#### D67. A recursive query is iterated over held work tables, bounded by a setting

A WITH RECURSIVE list is bound stratum by stratum [ISO/IEC 9075-2:2023
§7.17 SR3j v], each recursive stratum one plan whose members read work
tables, the rows each member gave the iteration before
(`logical/fixpoint.rs`, `exec/recursive.rs`). A work table is a list of
held rows, spilled past MEMORY.WORK_BYTES, shared by the plans that read
it, so a deep recursion needs no more memory than its rows.

The General Rules evaluate every member again over every row, until no
member gives more [GR2c ii]. That costs the square of the iterations, so a
stratum of one member whose recursive operand keeps each row of its work
table as it would alone is evaluated over only the rows each iteration
adds, which gives the same rows. Any other stratum is evaluated as the
rules say. A recursive operand's column takes its anchor's type [SR19 b ii
2], so the types are found by binding again until they settle.

A recursion may have no finite fixpoint [NOTE 322], so the session setting
EXECUTOR.MAX_ITERATIONS refuses one past its count of iterations (SQLCODE
-918), and a work table is read only while its statement is not cancelled
(D44). SEARCH and CYCLE [§7.18] are their expansion (D70).

#### D68. Arrays and rows are values of shared parts, nested to a bound

An array type and a row type [ISO/IEC 9075-2:2023 §4.10, §4.12.2] are
data types like any other, two kinds of the `SqlType` contract (D53,
`value/types/array.rs`, `value/types/row.rs`). A value of either holds its
elements or fields behind one shared pointer, and a type holds its element
type or fields the same way, so a copy of either costs a count, and a value
of a scalar type is no larger than before. A data type is cloned, not
copied, so a scalar one still costs a few bytes to copy and no allocation.
A multiset is held the same way, its elements unordered (D80).

A type may hold another, so every walk of one recurses. Types nest at most
`MAX_TYPE_NESTING` deep, 16, checked as a type is read from SQL text, from
the catalog and from the protocol, so every walk is bounded (R-CF-6). An
array's maximum cardinality is at most `MAX_ARRAY_CARDINALITY`, which is
also the maximum of one written without it [IL008].

One element is assigned the same way wherever it is written, by UPDATE's
`SET A[i] = v` [§15.5 GR3b] or by a target `A[i]` of SELECT INTO, FETCH
or CALL [§6.4]. Both build the array anew with `with_element`, so the
extension with nulls and the SQLSTATEs 2200E and 2202E are written once.

Arrays take part in equality alone [§9.11]. The binder refuses an array
wherever values are ordered or grouped [§9.12 SR3, §9.14 SR3], keys and
indexes among them, so no array is ever keyed and its kind gives no key
bytes. A row is ordered field by field, and its key is each field's null
flag and key in turn. Equality is three valued at every depth [§8.2 GR1b],
so arrays of different cardinalities are unequal, and a pair of parts known
unequal decides though another is unknown.

UNNEST [§7.6] reads the FROM items before it. It is planned as one
operator over the plan of those items, giving each of their rows the rows
of its arrays, so it costs one pass over them and no join. When it stands
in a RIGHT or FULL join and reads an item outside that join, no such plan
keeps the outer join's rows, so the query is refused as unsupported
(SQLCODE -900) rather than evaluated otherwise.

A dynamic descriptor area holds the items of a composite type's parts after
it, in preorder, each with its LEVEL [§20.1]. COUNT counts every item and
TOP_LEVEL_COUNT those of LEVEL 0, so the area's limit of occurrences bounds
both. In the Information Schema a column's or domain's data type descriptor
is identified by its ordinal position, or 1 for a domain, and each part's
by its parent's, a point and its place, so ELEMENT_TYPES and FIELDS join
them without a counter of their own.

#### D69. A distinct type's value is its source's, untagged

A distinct type [ISO/IEC 9075-2:2023 §4.9, §11.51] is a data type of its
own kind (D53, `value/types/distinct.rs`). It holds its schema, its name
and its source type behind one shared pointer. It is FINAL and never
altered, so a column's descriptor holds the type itself, and DROP TYPE
drops every column of it first or refuses. A value of the type is its
source's value, with no tag [UV103]. The binder knows the type, so each
comparison, assignment and CAST is checked as it is bound, and no value
grows nor any path of execution changes.

The source is a predefined type or an array type [§11.51 SR5a]. An exact
source is kept as NUMERIC of its precision and scale, an approximate one
as FLOAT of its precision [IV223]. Values compare within one type alone,
as their source values do [GR2c], and those of an array source with none,
as no ordering is made of them.

The two functions of its definition [GR2b], FNUDT from the source and
FNSDT to it, are kept in the type's descriptor, each with its name, its
specific name and its EXECUTE grants, not as routines apart. Their
specific names are the type's name and _TO_DISTINCT or _TO_SOURCE, unique
while the types' names are. Each binds as the change of representation it
is, which is none. A name not qualified is found in the schema of the
statement's names, the SQL-path [ID047], before a function a plug-in
registered.

Every session's transform group for a distinct type is the one its
definition makes [ID048], so a value crosses to a host or a client as its
source's. The protocol describes a column of one as its source type, and
no client needs to know the type.

What references a type is found by binding each view's query, each search
condition and each generation expression again, in the definitions
without the type, or with the privileges a REVOKE leaves. One that no
longer binds referenced it. DROP TYPE's RESTRICT counts a transform only
if a CREATE TRANSFORM made it [§11.59 SR4e], since the one its definition
makes goes with it [GR2], and a user-defined cast that names the type
[SR4f iv] (D81). CASCADE
drops each column of a type based on it first, as ALTER TABLE DROP COLUMN
does, and refuses before dropping any where that would drop every column
of a table [§11.23 SR4].

#### D70. SEARCH and CYCLE are their expansion, its growing columns widened

A `<with list element>` with a `<search or cycle clause>` is replaced in
the binder by the expanded element ISO/IEC 9075-2:2023 §7.18 SR8 says it
is equivalent to, which is then bound and iterated as any recursive
element is (D67). So the clause costs no operator of its own, and its
rows are those of the rules (`binder/search_cycle.rs`). The expansion's
own columns are qualified by its own correlation names, so no name of the
statement is taken for one.

Read to the letter, the expansion cannot recurse. The anchor's `ARRAY
[ROW(...)]` has a maximum cardinality of 1 [§6.49 SR2], and a recursive
query's column takes the anchor's type [§7.17 SR17c]. So the path column
and DEPTH FIRST's sequence column would fail (2202F) at the second level.
BREADTH FIRST's level starts as the literal 0, NUMERIC(1,0), and would
fail (-301) at level 10. Those columns are therefore arrays of the
greatest maximum cardinality, `MAX_ARRAY_CARDINALITY` [IL008], which
bounds a path's length, and the level is INTEGER, which is NUMERIC(10,0)
here.

SR5c ii renames the fields of BREADTH FIRST's row to read its level as
LEVEL, which no SQL turborel takes can write. The level is read as the
row's first field, F1, as turborel names the fields of `ROW(...)` by place
(D68). A `*` of the recursive operand, which would take the columns the
expansion adds to the element, is each table's columns, the element's
those of its `<with column list>`. One of a NATURAL join, or a join USING
columns, is not supported (SQLCODE -900), nor is a UNION CORRESPONDING.

What follows from the rules is kept. The DEPTH FIRST sequence is an
array, so it is ordered by no operation [§9.14 SR3]. A UNION, not UNION
ALL, groups the arrays a cycle clause or DEPTH FIRST adds, so it is
refused [§9.12 SR3]. A cycle mark value of UNKNOWN is refused, since it is
neither equal to the other nor unequal [§7.18 SR6c].

#### D71. Triggered actions run inside their statement, nested to a bound

A triggered action runs inside the statement that activated it, under
that statement's single undo mark (`db/triggers/run.rs`). Each statement
of the action is a statement of its own, with its own referential
actions, checks and AFTER triggers [ISO/IEC 9075-2:2023 §15.21 GR6]. It
neither opens a transaction statement nor takes a mark of its own. So a
failure anywhere fails the outermost statement whole, as §15.21 GR6m
asks. The action runs at a savepoint level of its own [§15.21 GR6c,
GR6j], which holds no savepoint, since no savepoint statement runs in it
[GR6e iii] (D88). The action shares its statement's instant [§9.17
GR5].

Nesting is bounded by `MAX_NESTING`, 16, so the deepest chain runs on
half the default stack (`db/nested.rs`). Triggered actions and the
procedures CALL runs (D73) count against the one bound, each a context
pushed with its owner, schema and diagnostics area. A deeper one fails
with SQLCODE -900, raised as SQLSTATE 09000 of a trigger's action. A trigger that writes its own table recurses until its
WHEN condition stops it or the bound does.

A state change's transitions are held rows (D38), spilled past the work
area, so a transition table of any size is read whole. A database with
no trigger records nothing and takes no new path, so a write costs what
it did before triggers. An action and its WHEN condition are bound when
the trigger is defined, under its schema's owner, and kept with the
columns its tables had then [§11.21 Note 561]. A column added later is
seen by no action.

A BEFORE trigger changes its new row only through a `SELECT ... INTO` or
GET DIAGNOSTICS whose target is a new transition variable [§6.4 SR7, §6.6
SR13], since Part 2 gives it no assignment statement. A change of a view
with an INSTEAD OF trigger of its event, by INSERT, a searched or
positioned UPDATE or DELETE, a MERGE's clause, or through an updatable
view over it, executes the trigger and writes nothing through the view
[§14.12 GR6a, §15.6 GR11b, §15.7 GR17b, §15.12 GR2b].

#### D73. SQL-invoked routines are bound as their owner, a function's body a subquery of its parameters

A routine of an SQL routine body keeps its definition's text, as a
trigger does, and is bound again from it as it is invoked, under the
owner of its schema, of that schema [ISO/IEC 9075-2:2023 §4.35, §11.60
SR23b]. Its body is bound once more as it is defined, which checks its
rules, its privileges and its SQL-data access then, and records the
tables it reads (`db/routines/body.rs`). So the drops and revokes that
abandon a routine find it by that list, or by binding its body again
without what they take away, as views and constraints are found.

Each definition that invokes a routine, a view, a CHECK, a domain
constraint, an assertion, a trigger, a generated column or a routine
body, records the specific names of those it invoked as it was defined
in its horizon (`catalog::Horizon`), and bound again it chooses among
those alone [§4.35.2]. A function created later, or EXECUTE granted
later, never changes what it invokes, and as each choice was the first
of a set that still holds it, the choice is the same. A routine it
recorded that is gone makes it bind no more, never invoke another of
the name. DROP ROUTINE finds what invokes a routine from those lists
[§11.62 GR1]. EXECUTE does not narrow the choice there, so a revoke of
EXECUTE on a routine a definition invokes abandons it [§12.7 GR12o].

A function's body is a RETURN, bound as `(VALUES (v))`, a subquery whose
outer references are the parameters, a row named for the function
(`binder/invocation/body.rs`). An invocation is `Expr::Invoked`: its
arguments, stored in their parameters, then the body run with their
values as its outer values (`exec/invoked.rs`). A body of one value
reading no table is evaluated in place, its outer references replaced
by the values, so a function may be invoked in a CHECK or a generated
column, where no subquery runs. A column of the body's query hides a
parameter of its name, as an inner scope hides an outer.

A procedure's body is any statement a triggered action may be, or a
CALL, run by `run_statement` as a nested context (D71), its parameters
the dynamic values of a routine scope (`params/routine.rs`). A body of
an SQL control statement, of either kind of routine, is a program run as
D76 says. SELECT
INTO, GET DIAGNOSTICS and a nested CALL assign an OUT or INOUT one, and
CALL then assigns each to its target. A routine runs with its owner's
privileges, SQL SECURITY DEFINER the only behaviour, since T324 is not
claimed. A schema holds procedures of a name each of another number of
parameters, and functions of a name each told apart by the types of its
parameters [§11.60 SR20] (Feature T341, D74).

#### D74. Overloads are chosen by one place per kind of number

Of the functions of a name, an invocation invokes the one its
arguments' types choose [ISO/IEC 9075-2:2023 §9.18 SR10, §9.6]. The
candidates are the schema's SQL-invoked functions and its distinct
types' functions together, each taking as many arguments as it has
parameters, and each typed argument's precedence list holding its
parameter's designator [§9.7]. Left to right, those whose parameter's
designator comes first in the argument's list are kept, and one must be
left. A `?` decides nothing and takes its parameter's type once one is
chosen [§20.7] (Feature B033). Two functions that differ only where a
call has `?` are refused as alike.

Each exact numeric type is of the place NUMERIC and each approximate one
of FLOAT (`value/precedence.rs`). Every exact type is one type of a
precision and a scale here, whatever its name, so SMALLINT, INTEGER,
BIGINT, NUMERIC and DECIMAL could not choose among themselves without
their spellings deciding a call. So F(INTEGER) beside F(BIGINT) is
refused as not told apart [§11.60 SR20b], while F(INTEGER) beside
F(DOUBLE PRECISION) is taken, an exact argument invoking the first. A
literal keeps its own type. SR20b's values "arbitrarily
selected" of each parameter's type are taken to be of that type itself,
as of another type a function might not take its own arguments.

A registered scalar function overloads too, if it declares the types of
its parameters (`ScalarFunction::parameter_types`), chosen the same way.
The functions of the schema are searched first, and the registered ones
only if none of the schema the caller holds EXECUTE on takes the
arguments [§9.18 SR10h iv 1], so a function of the schema of another
type never hides a registered scalar function. Whether a call is of a
registered aggregate is told before its arguments are bound, since
whether its query is grouped depends on it. So it is the aggregate's
unless a function of the schema of the name has as many parameters as
the call has arguments, whatever their types.

SQL.FUNCTION_CALLS is checked as a definition is made, and never again.
A definition's text is read again with calls of registered functions
taken (`Parser::for_stored`), whatever the setting of the session using
it, since one refused when made made nothing. Reading a definition made
with the setting OFF so changes nothing. Its calls were each bound to a
function of the schema, recorded in its horizon (D73), and bound again
it chooses among those alone, so a registered function of the name is
never reached. Remembering the setting of each definition would put a
flag in every descriptor and choose the same calls. A definition is made
by binding the text it keeps. So while a statement of a session with the
setting OFF makes its definitions (`Parser::refusing_calls`), a text
read again whose tokens are written in the statement takes no such
call, and any other text is of a definition made before.

#### D75. Roles: one authorization cell, applicable roles by fixed point

Every privilege check reads one cell, `catalog::Who`: the top cell of
the authorization stack [ISO/IEC 9075-2:2023 §4.42.1.2], a user
identifier, a role name or both, and the roles it is read against. Its
enabled identifiers are its user, its role, and each role applicable for
the role [§4.42.4]: those granted to it, to PUBLIC, or to a role found
so, to a fixed point (`Roles::applicable`). With no role authorization
at all, the check is the one comparison it was before roles, building
no set, so binding costs no more.

A statement is checked with the session's cell, its current privileges.
A definition, an abandonment and a grantor are checked with a cell of
their owner or grantor alone (`Who::sole`), its applicable privileges,
which take in roles only if it is a role itself. So the role a session
sets helps what it runs, never what it defines in a user's schema, and
a view stands or falls with its owner's privileges whoever made it. A
trigger or routine runs as its schema's owner alone, CURRENT_USER null
if that is a role, while SESSION_USER stays the session's
(`db/nested.rs`).

Role statements are SQL-schema statements, in the transaction, each
taking one lock on no object exclusive, and GRANT of privileges taking
it shared (`db/roles.rs`). The log keeps each definition's cell, the
role beside the identifier, so a definition GRANTED BY CURRENT_ROLE or
made in a role's schema replays as it ran. Who may create a role
(IW141) is the database's owner and the users it allows, a right logged
and kept with the users (`ALTER USER ... CREATE ROLE`).

A REVOKE or DROP ROLE builds the role dependency graph [§12.7 GR5 to
GR7], then settles every privilege of every kind against the role
authorizations it leaves, to a fixed point, as each kind's own revoke
settles it (`db/roles/settle.rs`). What then binds no more as its
owner's, or needs a USAGE its owner holds no more, is abandoned as those
revokes find it. DROP ROLE takes every authorization and privilege
granted to the role or by it, by any grantor, so no descriptor names a
role that is gone and its name may be taken again; with RESTRICT it is
refused while the role granted any, and it never drops a schema's owner,
since no statement gives a schema another owner.

#### D76. Routine bodies: one program, two hosts

A routine's body, a trigger's action or a statement run directly that is
an SQL control statement, SIGNAL or RESIGNAL [ISO/IEC 9075-4:2006 draft
§11.2] is bound once into a program (`logical/program.rs`,
`binder/psm.rs`). Labels are resolved to blocks, variables to slots of
one frame row laid out at bind time, the parameters first, conditions
to SQLSTATE values, and handlers to their condition values. Its values
and queries are leaves, each a subquery whose outer references are frame
slots, bound as D73 binds a RETURN. Its other statements keep their text
and the scope they see, and are bound and run as a procedure's are.

One interpreter runs it (`exec/program/run.rs`), generic over a `Host`
that gives a leaf's rows, runs a statement, keeps local cursors, marks
and undoes changes, and keeps the diagnostics area stack. The session
host (`db/routines/program.rs`) runs procedures, triggers, statements
run directly, and functions that possibly modify SQL-data. Each of its
statements runs as a nested statement under a mark of its own, so one
that fails is undone alone, and UNDO and ATOMIC undo a block's
[ISO/IEC 9075-2:2023 §4.41.5]. The executor host (`db/routines/program/executor.rs`)
runs a function that does not possibly modify SQL-data where the query
invoking it is evaluated, with only that query's runner. It reads, keeps
its cursors as rows it fetched, and changes nothing, so it needs no mark.
A procedure that function CALLs is bound into the function's program and
run there too (`binder/psm/inline.rs`).

A program of a routine, a trigger or a function runs in the state and
transaction of the statement that runs it. A control statement NOT
ATOMIC run directly holds its own (`db/routines/program/host.rs`), as
its COMMIT or ROLLBACK ends its transaction [ISO/IEC 9075-4:2006 draft
§4.10.4]. The statements after it run in a new transaction, begun as the
first of them needs one, and the session, not the call, records the
transaction that runs on the thread (`db/running.rs`). COMMIT and
ROLLBACK take no mark, which would be of the transaction ended. Its
state is taken anew after a transaction ends or a definition changes,
so a statement sees what those before it defined.

A function that possibly modifies SQL-data is placed only where Part 2
lets a statement invoke one, not in a query expression, a search
condition or a CASE abbreviation [§7.17 SR25, §6.12 SR1a]. The binder
marks the top level of those places (`Binder::hosting`), and a subquery
clears it. Where the mark is, the values are evaluated by `Hosted`
(`db/routines/program/hosted.rs`), which runs such a function through
the session host in the context its invocation pushes: the SET values of
UPDATE and MERGE, the rows of an INSERT's or MERGE's VALUES, the
arguments of CALL, and a program's values. An UPDATE's rows are found
before its SET values are evaluated, so rows the function inserts are
not updated by the statement invoking it.

A program bound once may run nested in itself, through triggers, so a
local cursor's name is its declaration's with the number of the
execution that declared it (`params::local_key`). A loop checks for
cancellation on every iteration (D44). Nesting is D71's bound, which
programs count against as procedures do.

#### D77. SQL-server modules: catalog routines of a module, its tables in a schema of its own

A module's routines are the catalog's routines, each marked with its
module (`SqlRoutineDesc::module`), so CALL, invocation, binding as the
owner, dependencies and the Definition Schema treat them as D73 treats
any routine [ISO/IEC 9075-4:2006 draft §9.24 GR1]. Each keeps its text
as the `<schema routine>` it would be, CREATE then the routine, read
again within its module (`binder::definition_of`). The module descriptor
keeps its definition, default schema, path and the grants of EXECUTE on
it (`catalog/modules.rs`). EXECUTE on the module stands for EXECUTE on
each routine (`binder::held`), and a module's routine has no grants of
its own.

A module's declared temporary tables are the catalog's declared tables
in a schema no name reaches, `MODULE`, its schema and name, a line feed
between each (`catalog::module_schema`). The parser of a module's text
maps MODULE.T there (`Parser::in_module`), and outside it MODULE.T is the
session's own (§13.11). Each session has its own instance of each, as of
any temporary table (D49), and the Information Schema shows none
(`State::without_declared`).

The module's path is a list of schemas the body's routine names are
looked in (`params::RoutineScope::path`). A function's candidates are
gathered over the path and its subject routines kept of the earliest
schema that has one [§10.4 SR2]. A procedure is the one of its arity
in the earliest schema.

A drop never takes part of a module [§9.2 GR2a, §10.3 GR7]. The routines
a drop or revoke takes are widened to their whole modules and what
invokes those, to a fixed point (`State::module_closure`), then dropped
as any routines are, with the module's tables and descriptor last.

A subquery has an identity of its own (`Subquery::id`), kept by a copy
whose outer expressions alone differ. A runner keeps a subquery's rows
by it with its outer values (`exec/subquery.rs`). An address would not
do, as one made for a row and freed is another's after, and two
functions' bodies then read each other's rows.

#### D78. Polymorphic table functions: compiled as bound, run whole as the cursor opens

A `<PTF derived table>` is a plan node, `LogicalPlan::Polymorphic`, whose
inputs are its table arguments' plans, as a derived table's query is
its input (`logical/ptf.rs`). A data argument is an expression of no
row. The invocation is compiled as it is bound [§9.24]: its PTF data
area made, its describe component procedure run on it, and the
requested row types and proper result columns it leaves checked
(`db/ptf/compile.rs`). What compilation leaves rides in the node
(`db::ptf::Call`), so the plan of a statement, a view or a routine body
carries the result row type it was bound with [§4.35.4].

A describe component procedure that reads SQL-data reads it in the
transaction of the statement being bound (`binder::Compile`, set by
`Session::compiling` while a statement runs). Where none runs, as a
definition is bound, a read is refused, SQLCODE -900.

As the cursor opens, each table argument's rows are read whole and the
partitions found [§9.25 GR13 to GR16]. A virtual processor is run for
each in turn, on a copy of the data area compilation left (UA056,
`db/ptf/execute.rs`). Its component procedures run on the program of
D76, the executor host, with the data area beside the frame
(`run_component`). Their statements of a PTF data area are run there
alone (`db/ptf/statements.rs`).

A pass-through surrogate is a row's place among its table's rows,
BIGINT (UV133). The rows piped are kept, then mapped to the node's row:
the proper columns, then of each table argument of PASS THROUGH the
columns of the row its surrogate names, else its partitioning columns,
in the table's order [§7.6 SR12a].

A `<PTF derived table>`'s range variables, its correlation name's and
each table argument's, are items of the FROM row together, a group
whose first item holds the plan and the others `Source::Argument`
(`binder/ptf.rs`). Each names its own columns of the one node.

#### D79. Large objects stream: chunked, logged by chunk, read by range, reached by locators

CHARACTER LARGE OBJECT, NATIONAL CHARACTER LARGE OBJECT and BINARY LARGE
OBJECT hold values of up to 4G characters or octets (IL042), the length
when none is written (ID068). No layer holds such a value whole. A
`Value::Lob` is a list of parts, each inline octets, a range of a source
of chunks, a table's object or a client's locator (`value/lob.rs`). `||`
joins parts and SUBSTRING slices them, so neither copies a chunk. A chunk
is 256 KiB, cut at a character's end of a character one (`char_cut`).

Each table keeps its large objects in a space of its own, a B-link tree
from an object's id to its page (`engine/rows/lobs.rs`). A segment holds
one chunk in one page, sealed with its id, and a directory lists the
segments by id with their lengths. Objects never change. Their pages
pass through the buffer pool, read into no frame unless one holds them
already, so a value larger than the pool passes through it.

Every object belongs to one row, and the versions that name it are one
run of that row's history. A new version names an object only if the
version it replaces does, and the transaction layer copies any other
(`txn/lobs.rs`). Each write so knows the objects it made and dropped
without counting references. A commit puts what a version dropped with
its garbage, freed once no snapshot reads it. An undo frees what the
write made, and a commit frees what it made and no row it wrote names,
a retry's leavings. DROP TABLE frees the space whole.

Each object is logged as it is made, one `LobPut` record of at most a
chunk, before the row change that names it. Recovery keeps every object
it reads and at its end frees those that no row's newest version names,
which covers transactions that never committed. The objects of a
transaction whose definitions wait for its commit are made after them,
as their table may be one it made (D33). A checkpoint logs the
objects awaiting collection as `LobStale`, and recovery from it frees
them. The column store refuses a large object column, and a column of a
type holding one. A column of a distinct type keeps its large objects as
its source type's would (D69).

A table keeps objects for its own columns alone, so an object inside an
array, a multiset, a row or a structured value would name another
table's, or none once its row was gone. Each one is held whole in its
value before the row is written and logged (`value/nested.rs`), up to a
chunk, as what must be held whole is, and refused past one. A record and
its log record then hold it, so recovery and a standby need nothing more.

A value a function makes, as UPPER, TRIM, OVERLAY and TRANSLATE_REGEX
make one, is written a chunk at a time to the database's temporary
store, a scratch file whose pages a value frees as it is dropped
(`spill/lobs.rs`). A value read from a table is valid while its
statement's snapshot is held. That is the transaction at REPEATABLE READ
and SERIALIZABLE, and the statement at READ COMMITTED, unless a locator
of it holds the snapshot to the transaction's end.

A row an operator or a statement holds past its work area (D38) names
its large objects, and XML values kept apart, by what keeps them. The
log's form names a table's object by its id alone, which only the table
of the row holding it resolves, so a spilled record names a slice of a
table's object by the table and the id, and a temporary object by itself
(`spill/named.rs`). The spill holds one source of each table its records
name, which makes a source of any other object of that table, and each
temporary object they name. So a value read back is valid as the value
held in memory was: a table's while the statement's snapshot is held, as
no object a snapshot reads is freed, and a temporary one while the spill
is. Copying each value to the spill area would write again every object
a spilled row names, and holding it whole, as a value inside another is
held, would refuse one past a chunk. A spill holds a source a table and
one a temporary object of a chunk or more, never one a row. A set
function's state is no row, so ANY_VALUE and a group-invariant column of
a type that may hold such a value write no state, and their grouping
writes its rows.

A locator is a session's four-octet handle of a value [§4.37.5], valid
until its transaction ends (`db/lobs.rs`). FREE LOCATOR ends one early
and HOLD LOCATOR keeps one past commit, its value copied to the
temporary store if a table holds it. A rollback ends every one. A
client makes a value a piece at a time through a locator and sends that
as a parameter. An invalid one is SQLSTATE 0F001.

The server sends a large object of at most SERVER.LOB_INLINE_BYTES
whole, a longer one by a locator the client reads a chunk at a time
(D45). A request that would end its transaction keeps it once its reply
sent a locator. The client library reads one as a stream and writes one
as a stream, and an import sends a long field so.

A large object is compared for equality alone. ORDER BY, GROUP BY,
DISTINCT, keys, MAX and `<` refuse one [§9.12 SR3, §9.14 SR3]. Equality,
POSITION, LIKE and the regular expressions read it a chunk at a time.
What must be held whole is held up to a chunk and refused past one
(SQLCODE -900, SQLSTATE 0A000). That is a LIKE pattern, escape or regular
expression of a large object, and a match value under a collation of
contractions, whose substrings LIKE compares.

The costs are these. A long transaction writing large objects holds back
log truncation. A standby holds the objects of each open transaction. A
locator under READ COMMITTED holds back collection of every table until
its transaction ends. Each object written is one log record.

#### D80. Multisets are unordered values, paired by classes and a matching

A multiset type [ISO/IEC 9075-2:2023 §4.12.3] is a kind of the `SqlType`
contract beside arrays (D53, D68, `value/types/multiset.rs`). A value
holds its elements behind one shared pointer, as an array does, in the
order they were made, which nothing reads. A multiset has at most
`MAX_MULTISET_CARDINALITY` elements, 65535, as an array may. An
operation that would pass it is multiset value overflow, SQLSTATE 2200Q
[§10.9 GR3b], raised as the limit is passed, so nothing holds more.

Equality [§8.2 GR1b iii] and SUBMULTISET [§8.17 GR3c] ask whether the
elements of one pair with those of another, each pair equal, or each
equal or unknown. Searching pairings is factorial, so neither does. The
elements holding no null fall into classes of equal elements, found by
their buckets, and two equal elements are interchangeable, so the
classes pair whole by their counts. What they leave, with the elements
holding a null, pairs by a maximum bipartite matching whose edges join
elements whose equality is not false, a null element matching any
(`value/multisets.rs`). Kuhn's augmenting paths run on a stack of their
own, so the matching recurses not at all (R-CF-6). The grouping
operations, SET, IS A SET, MULTISET UNION DISTINCT, EXCEPT, INTERSECT and
INTERSECTION, count classes of elements not distinct from one another
[§8.15], nulls in one class, and keep each element's first operand
first.

Each grouping operation refuses a multiset whose elements are ordered
or grouped by nothing [§9.13 SR3], arrays, multisets and large objects
among them (SQLCODE -103), so the classes never compare such elements.
A multiset is itself grouped and ordered by nothing. ORDER BY, GROUP BY,
DISTINCT, keys, MAX and `<` refuse one, and a distinct type whose source
is a multiset has ordering form NONE. Its bucket and its normalized key
are its elements' summed, so two equal multisets in any order share
them, though no key of a multiset is ever stored.

MULTISET, MEMBER, SUBMULTISET, ELEMENT, COLLECT, FUSION and INTERSECTION
are no key words of SQL-92, so each is read as a word where it is one
and a column may be named so (`parser/multisets.rs`). MULTISET and
UNION, EXCEPT or INTERSECT are an operator only after a whole operand,
where no column stands. MULTISET INTERSECT binds tighter than UNION and EXCEPT [§6.50]. COLLECT
( X ) aggregates the row ( X ), as ARRAY_AGG does, so a null X stays an
element; a multiset value constructor by query is COLLECT of its rows,
the empty multiset of none [§6.52 GR3]. UNNEST takes one multiset alone
and no ordinality [§7.6 SR8e].

A CHECK, assertion or generation refuses what §9.16 calls a potential
source of non-determinism. Of multisets that is a grouping operation or
SET over elements of a character string, distinct or zoned datetime
type, whose equal values may differ [§9.16 SR1n, SR1o, SR1aq]. Elements
of a character set use its default collation, as arrays' do. The
protocol carries a multiset as tag 22 (D45), and a descriptor area as
TYPE 55 with its element's item after it [§20.1 Table 30].

#### D81. Structured types: values name their type, typed tables keep their own rows, references are octets

A structured type [ISO/IEC 9075-2:2023 §4.9.3] is a kind of the
`SqlType` contract (D53, `value/types/structured.rs`). The type is known
by its name alone. Its attributes, methods and ordering are the
catalog's, so the binder alone reads them, and a column declared of the
type is never stale as ALTER TYPE changes them. A value names its most
specific type and holds its attributes in their ordinal positions, a
supertype's first, so it is read without the catalog, as a log record
is. Method dispatch [§9.18 GR3b], TREAT and IS OF [§6.17, §8.19] read
that name. A hierarchy has at most `MAX_HIERARCHY_DEPTH` types, 256,
which bounds every walk up or down it. ALTER TYPE ADD and DROP
ATTRIBUTE are refused while a base table's column is of a type of the
family, whose stored values keep their attributes.

An ordering [§11.65] is applied as the binder binds. By MAP, each
operand of a comparison, grouping or ordering operation carries its map
function's value as its key (`functions::Keyed`), and compares, hashes
and sorts by it. That covers ORDER BY, GROUP BY and grouping sets,
DISTINCT, the set operations, windows, MAX and MIN, GREATEST, an
aggregate's ORDER BY, UNIQUE, row patterns, polymorphic table
functions' partitions, and the elements of a multiset. By STATE, a
value compares by its most specific type and each attribute, as the
value itself does. By RELATIVE, `x op y` is `RF(x, y) op 0`. A sort or
a grouping of values ordered by RELATIVE has no key to sort or group by,
so it is valid SQL refused (SQLCODE -900). A unique or referential
constraint and an index refuse a column of a structured type too
(SQLCODE -900). An index key decodes back to the value it keys, and
neither a map function's value nor a STATE identity, whose attributes'
types only the catalog holds, does.

A typed table [§4.17.9] keeps its own rows. A row of a proper subtable
has its superrow in each supertable, as the subtable's first columns.
A table named without ONLY is read as its own rows and then each proper
subtable's, cut to its columns (`binder/hierarchy.rs`). A searched
UPDATE, DELETE, MERGE, TRUNCATE or cursor changes each row where it is.
Every constraint of a supertable is checked of its subtables' rows at
the same positions, and a key or a referenced row is looked for across
the family (`db/integrity/inherited.rs`). A subtable is in its
supertable's schema, whose owner holds every privilege on it, so UNDER
and the inherited columns' privileges are not recorded. The owner of a
typed table holds SELECT WITH HIERARCHY OPTION on it. A grant of it on
a supertable gives its grantee SELECT on each proper subtable as the
grant was, read from the grant as needed rather than recorded [§12.2
GR8]. ONLY of a table that is not typed needs no privilege beyond its
SELECT, as no subtable could be left out.

A reference [§4.11] is `Value::Reference`, its octets, at most
`REF_OCTETS`, 1024. A system-generated one is the number the family's
sequence generator gives the row, so no REF is ever given twice, across
a crash too. A user-defined one is its representation type's value as
`value::codec` writes it, and a derived one its derivational
attributes' values written as a row's (UV103, UA063). References compare
by their octets alone. DEREF, `->` and a method reference are each an
invocation whose body is a subquery of the scope table over the
reference, as a function's body is (D73). The family is read most
specific first, so the value's most specific type is its table's. A
reference carries no type, so its most specific type is the reference
type of the type of the table whose row it identifies. TREAT of a
reference to a subtype [Feature S162] looks the row up in the scope's
family, or with no scope in each table of the referenced type or a
subtype, each read as its owner, as §6.17 has no Access Rules. One that
identifies no row keeps its declared type.

DROP SCOPE finds an impacted dereference by binding each definition
again without the scope (`db/alter/depending.rs`). One whose reference
comes from the column other than as a column reference, a subquery's
result say, is of the same scope and binds no more either. It is taken
as impacted too, so DROP SCOPE RESTRICT refuses a little more than
§11.18 SR4 names. A referenceable view is read-only, its rows changed
through its basis table (`db/views/referenceable.rs`). An UPDATE through
an INSTEAD OF trigger whose later mutated set clause of a column reads
that column is refused (SQLCODE -900). Its value would be the earlier
clause's, substituted wherever the column is named [§14.15 SR7c ii],
inside a subquery too, which the clauses rewritten as text do not reach.

Methods [§11.51 SR10, §11.56] are of a structured type or a distinct
type alike (`catalog::MethodHost`). A distinct type has no supertype, so
its methods override none, and a generalized invocation of one names
the type itself. A method, a cast, an ordering or a transform may be a
schema element of CREATE SCHEMA [§11.1], made after the routines
written before it.

A table/method privilege [§4.42.2] is SELECT on a typed table and one
method of its type, kept by the method's specific name. One granted by
a `<privilege method list>` is recorded [§12.3 SR8b i]. One SELECT on
the table grants [§12.2 GR6] is read from that grant as needed, for each
method its grantor holds EXECUTE on WITH GRANT OPTION. So a method
added later is covered at once, as the grant stays [§11.56 GR3]. The
owner of a typed table holds the privilege on every method of its type,
and the owner of a referenceable view does too, as it holds SELECT on
the view by its definition. A method reference needs the privilege on
the table that is the reference's scope [§6.23 AR1].

A user-defined cast [§6.13 SR12, GR3] is applied as the binder binds.
CAST to a supertype is the value itself. Any other CAST of or to a
structured type invokes the cast whose source comes first in the
operand's type precedence list, and casts its result again where the
target is no user-defined type. A value is stored by a cast AS
ASSIGNMENT [§9.2 GR2] where its type is not assignable to the site's.
That covers INSERT, UPDATE, MERGE, a function's result, PSM's SET,
SELECT INTO and FETCH, and a variable's default. An argument never
needs one, as a routine is a candidate only if each parameter's type is
in its argument's type precedence list [§9.18 SR10d].

A transform group [§4.48.1] is read from the SQL-session's own state,
set by SET TRANSFORM GROUP [§19.12, §19.13] and compared as a cached
plan is chosen. Its functions are what a host language or an external
routine [§4.9.6] crosses a value with, and turborel has neither, so a
structured value crosses to a client whole, as the protocol's tag of
its type writes it. A function that is an ordering's, a transform's or
a cast's is dropped only with CASCADE, which drops what it is the
function of [§11.62 SR4a, SR4d].

#### D82. System-versioned tables keep their historical rows in a hidden table of their own

A system-versioned table [ISO/IEC 9075-2:2023 §4.17.2.3] keeps the rows
its updates and deletes end as historical system rows. turborel keeps
them in the table's history, a table of DEFINITION_SCHEMA named `SYSTEM
HISTORY`, then the length of the table's schema name, its schema and
its name, so no two tables' histories meet. The history has the table's
columns, none generated and none of a default, its engine and options,
and no constraint and no index. It is made as the table is defined WITH
SYSTEM VERSIONING or altered ADD SYSTEM VERSIONING, as kept totals are
(D51), and dropped with the table or DROP SYSTEM VERSIONING. A table is
system-versioned exactly while it has a history, so its descriptor
records nothing more.

The table itself holds only current rows. Its indexes, its constraints,
its readers and garbage collection of its versions are what they were,
and none of them sees a historical row. A row changed by a transaction
later than the one that began it is copied to the history as it is
written, its end the transaction timestamp, in the same gate section's
transaction, so rollback and recovery undo or redo it as any insert.
The cost is one more insert for each update or delete, and a second
storage per versioned table. No statement adds, alters or drops a
column of a versioned table [§11.11 SR2, §11.12 SR2, §11.23 SR3], so a
history is never reshaped.

The transaction timestamp [§4.44] is the clock's instant as the
transaction first changes data, of TIMESTAMP(6) WITH TIME ZONE at
+00:00 (IV072), kept for the rest of it. Every table with a system-time
period, versioned or not, stamps the rows written to it with it: an
insert as BEFORE triggers would see the row, an update where the row is
written, both as the period's start, the end the highest value of the
column's type.

#### D83. A column is renamed in place, refused while a text names it

SQL:2023 renames no column [ISO/IEC 9075-2:2023 §11.10], so ALTER TABLE
… RENAME \[COLUMN\] … TO … is an extension. It changes the column's name
in the table's descriptor and nothing of its rows, so it copies none and
gives the table no new storage generation. What refers to a column by
its place, a key, a NOT NULL, an index, a privilege or a statistic, is
unchanged. What refers to it by its name as data changes with it: the
start or end of a period, its period constraint's text, written again
of the period's names, and each table option of columns, as
COLUMNSTORE.ORDER (D31).

What names a column in SQL text read again, a view's query, a CHECK, an
assertion, a trigger, a routine's body or a generation expression, is
not rewritten, for a name in a text may be captured by another scope
once changed. While a text that reads the table names the column or its
new name as an identifier, or a view that reads it selects `*` after
SELECT or a qualifier, the rename is refused, as DROP COLUMN RESTRICT is
[ISO/IEC 9075:1992 §11.15 SR4]. The test is of the text's tokens, so it
is conservative: a name of another table's column of the same spelling
refuses it too. A typed table's columns are its type's attributes, and a
system-versioned table's are not altered [§11.12 SR2], so neither is
renamed. The rename is logged as the action written, as every ALTER
TABLE is (D33), so recovery and a standby apply it again.

#### D84. SQL/JSON items hold SQL values, and a path is parsed once, apart from SQL

An SQL/JSON item [ISO/IEC 9075-2:2023 §4.48.3] is an `Item`
(`json/item.rs`): the SQL/JSON null, a scalar, an array or an object. A
scalar holds a `Value` that is a character string, a number, a truth
value or a datetime, and nothing else can be made one, so items are
compared and computed with by the code SQL's values are (`value.rs`). An
object keeps its members in the order written, those of one key all
(UA057), so that WITH UNIQUE KEYS can refuse them and an accessor take
the first (UA075). Arrays and objects nest at most `json::MAX_NESTING`
deep, 128, and one deeper is SQLCODE -900 (D18), so every walk of an item
is bounded (R-CF-6).

JSON text is parsed by RFC 8259's grammar [§9.42] (`json/text.rs`). A
number without an exponent is exact and one with an exponent approximate,
as the `<signed numeric literal>` of its characters is [GR3a iii 3 A IV].
An exact one of more digits than `MAX_EXACT_PRECISION` is DECFLOAT,
rounded to 34 digits, where an SQL literal is refused: RFC 8259 §6 lets
precision be limited, and identifiers of 19 and 20 digits are common in
JSON. An escaped surrogate that is not of a pair is no Unicode character,
so it is refused as text that is not JSON, SQLSTATE 22032.

The SQL/JSON path language [§9.45, §9.46] is not SQL. Its lexical rules
are ECMAScript 5.1's, it is case-sensitive and no word of it is reserved.
So it has a lexer and a parser of its own (`json/path/lex.rs`,
`json/path/parse.rs`) and never meets `lexer.rs` or `parser.rs`. A path
is parsed once to a tree (`json/path/ast.rs`), its like_regex patterns
compiled then (D61), so a constant path costs a statement one parse. The
tree is at most `MAX_PATH_NESTING` high, 64, counting what recursion reads and
what loops build alike, so the evaluator recurses over it with no counter
of its own (R-CF-6).

Evaluation (`json/path/eval.rs`) follows §9.46's General Rules, an
SQL/JSON sequence being a vector of items. An exception is an `SqlError`
of the SQLSTATE its rule names, which a filter's predicate catches as
Unknown [§4.48.5]. CEILING, FLOOR, ABS and MOD, for the item methods and
`%`, are computed by §6.31's rules in `json/path/eval/numbers.rs`, over
the path's numbers, which keep exact results exact. SQL's FLOOR and
CEILING (`functions/numeric.rs`) compute the same over SQL values.

The General Rules of JSON_EXISTS, JSON_VALUE and JSON_QUERY [§8.23,
§6.28, §6.35] are functions of the library too (`json/query.rs`), over
values already evaluated: the context item, the path, the PASSING
arguments and each clause's choice. They parse the context's text
[§9.47], evaluate the path, and cast or serialize the sequence to the
type returned [§9.43, §9.48, §9.49]. ON ERROR replaces a data exception,
SQLSTATE class 22, and nothing else: a Syntax Rule broken or a limit
passed is the statement's.

The rules of IS JSON [§8.22] and of the constructors JSON_OBJECT,
JSON_ARRAY, JSON_OBJECTAGG and JSON_ARRAYAGG [§6.34, §10.11] are there
alike (`json/predicate.rs`, `json/construct.rs`), the aggregates as
states the executor is to drive, given their rows in the order to keep.

The three query operators' syntax is bound to these functions, as SQL
reaches the regular expressions (D61): JSON_EXISTS, JSON_VALUE and
JSON_QUERY are each a call of a function of its own
(`functions/json.rs`), made by the binder (`binder/json.rs`) and
registered nowhere. The binder parses the path specification, a literal,
once, so a bad path is refused as the statement is prepared, and checks
§9.46 SR4 and SR8 of the parsed path then. The call holds the path and
the clauses, the operands are its arguments, and the name it shows
spells every clause, so two calls of other clauses are never taken for
one expression. A PASSING argument's identifier names the variable of
its value as SQL defines it, upper case unless delimited [§9.46 SR4]. A
cast's warning reaches the statement through
`ScalarFunction::call_warned`, as a CAST's does. The query operators
are potential sources of non-determinism [§9.16 SR1g, SR1h, SR1al], so a
constraint or a generation expression holds none.

IS JSON, JSON_OBJECT and JSON_ARRAY by enumeration are bound alike, each
a call of a function of its own holding its clauses. The aggregates are
set functions of a row, which is never the null value, as ARRAY_AGG is:
JSON_OBJECTAGG's of its name and value, JSON_ARRAYAGG's of its value and
sort keys, so the null values their rules keep reach the accumulator and
the executor's grouping, FILTER, windows and spilling serve them as they
serve every set function. JSON_ARRAYAGG orders its rows when it is
finished, stably, as ARRAY_AGG does, and JSON_ARRAY by query is that
aggregate of its query's rows, as ARRAY by query is. What an aggregate
holds is bounded by the members and elements one may make, `MAX_MEMBERS`
and `MAX_ELEMENTS`. An input that is a JSON-returning function, a
constructor or JSON_QUERY, is FORMAT JSON implicitly [§10.12 SR4a], so
constructors nest. JSON_TABLE is to call the same engine as the query
operators.

#### D33. Schema changes are transactional

CREATE SCHEMA run by a session [ISO/IEC 9075:1992 §20.1 GR6b ii, iii],
CREATE TABLE, CREATE VIEW, GRANT, REVOKE (D47), DROP SCHEMA, TABLE and
VIEW, and ALTER TABLE all run in transactions. SQL-92 lets schema and data
statements share a transaction [§4.28], and an irreversible schema change
is a trap.

Definition changes are staged on a copy, published at commit and forgotten
at rollback. Each holds its table's exclusive lock until the transaction
ends. It is logged under its transaction, and recovery applies it at the
commit record. ALTER TABLE copies rows to a new storage generation, so
rollback leaves the old one intact. A transaction begun before a change it
can't see is refused on next using the table (SQLCODE -911).

#### D47. Schema statements on their own

CREATE TABLE, CREATE VIEW, GRANT and REVOKE run outside a `<schema
definition>` [ISO/IEC 9075:1992 §20.1], in the session's transaction
(D33). Operators define and secure tables over a connection, one statement
at a time, and a definition commits together with the rows that fill it.

Only an existing schema's owner defines objects in it. Unqualified names
are in the session's default schema [§5.4 SR8b]. Kept texts store their
names' schema and are reread there, whatever the default.

Beyond SQL-92's text, a constraint's schema must belong to its definer,
else SQLCODE -104, because a name in another's schema would block that
owner's own constraints of the name [EXTENSION]. DROP SCHEMA RESTRICT is
refused (SQLCODE -103) while a constraint named in it belongs to another
schema's table or domain.

GRANT and REVOKE follow §11.36 and §11.37. A privilege stands while its
grantor holds it with grant option, as a fixed point built from owners'
privileges, so mutually supporting cycles fall together [§11.37 SR11].
Warnings are completions, SQLCODE 0 plus their SQLSTATE [§22.2, Table 24].
SQL-92's PUBLIC rules replace SQL-86's narrower ones. REVOKE USAGE on a
collation follows ISO/IEC 9075-2:2023 §12.7 GR12, as SQL-92 has no rule for a
column of it.

#### D49. Temporary tables, an instance per session

A global or created local temporary table [ISO/IEC 9075:1992 §4.9, §11.3]
is defined like a persistent one, in the catalog and logged, with empty
storage. Each session gets its own instance of rows, indexes and lock
(`db/temporary.rs`) when its transaction first touches the table. Giving
the session's view of the definitions its own storage meets §4.9 without
changing how a table is found, bound or checked.

Instances are never logged or recovered. Their pages live in a spill file
through a pool of MEMORY.WORK_BYTES (`db/temporary/pages.rs`), so an
instance may outgrow the budget. A session is one module, so its declared
local temporary tables [§13.11] live in a schema MODULE that only it can
see (`db/declared.rs`).

One definition serves every session, so ALTER TABLE changes each session's
rows. It is refused (SQLSTATE TB911) while another transaction has an
instance's rows open, which keeps a transaction's table as it began
(`db/temporary/reshape.rs`).

#### D50. Domains copy their constraints onto their columns

A domain's column takes its data type and keeps a copy of its default and
of each constraint as a table CHECK with VALUE the column [ISO/IEC
9075:1992 §11.21]. A table CHECK is already bound, checked, deferred and
logged, so this needs no new machinery. The Information Schema shows the
domain's constraint and not the copies. DROP DOMAIN CASCADE leaves the
copies as table constraints [§11.27 GR1c]. CAST to a domain needs USAGE
and checks its constraints [§6.10 GR13].

#### D51. An assertion is a query whose rows break it

An assertion [ISO/IEC 9075:1992 §11.34] is a schema constraint on no
table, bound as `SELECT 1 FROM (VALUES (1)) WHERE NOT (condition)`. Any
row means it is broken [GR2]. It is checked when made and at the end of
each statement changing a table it reads, or at commit if deferred. A CHECK
with a subquery is already checked, deferred and logged, so an assertion
needs only its query.

A whole check reads its tables on every statement, however small the
change. Where a form can be proved equal to the whole check, given the
assertion held when the statement began, it is checked from the changed
rows instead (`db/assertions/changes.rs`, `forms.rs`, `queried.rs`,
`delta.rs`). Counts and exact sums use kept totals in a hidden
DEFINITION_SCHEMA table (`assertions/kept.rs`, `totals.rs`), folded by the
job `turborel-assertion-totals` (D55) and shown in
INFORMATION_SCHEMA.ASSERTION_TOTALS.

Every other form is checked whole. That includes SUM and AVG of
approximate numbers, DISTINCT sums, outer joins, set operators and
groupings of joins. Keeping a query's result current under every change is
a design of its own, for forms that are rare in assertions.

#### D52. A view's REFERENCES come from what its columns are drawn from

An owner holds REFERENCES on view column CV if they hold it on every
underlying column of CV in the view's leaf tables, and on some column of
each [ISO/IEC 9075:1992 §11.19 GR6b]. Underlying columns are recorded at
definition in the view's descriptor (`Underlying`), because deriving them
at every privilege check would bind the view each time.

#### D53. One contract for data types, with the standard types built in

One trait, `value::SqlType`, defines a type, so every type, built in or
not, is stored, logged, indexed and sent the same way. Standard types live
in `value/types/`, and the parser and binder know their syntax, because a
standard type is grammar as much as storage. Built-in values are `Value`
variants, so comparison and hashing avoid a virtual call.

Plug-in types are registered (`Database::register_type`) before the
database opens from its files. Their values are `Value::Extension`, stored
by type name, and a database naming an unregistered type does not open. A
client without the plug-in holds a value as `Unregistered` and returns it
unchanged. No privilege is needed to use a plug-in type, because it has no
schema or owner [§4.26, §11.36 SR1].

A scalar function of a name is registered again only as an overload,
each of its name declaring the types of its parameters and none alike
(D74). Any other is refused (SQLCODE -910), and none is replaced, so no
session's calls change under it. The five set functions alone can be replaced by an
aggregate of their name. Set functions may write and merge their state
(`Accumulator::write_state`, `merge`), and a grouping uses that only if
all of its functions can (D31, D38).

A scalar function's call gives a value or an error, never a warning, so
no call pays for asking for one. A completion condition a call raises
each time it runs is one its binding knows of, and the binder binds the
call as `Expr::Warned`, which raises the warning after each evaluation
[ISO/IEC 9075:1992 §3.3.4.1]. The optimizer folds no such expression
away, as it folds only what raises no warning. XMLMAP of a column it
cannot map is one (D106).

#### D54. Datetimes kept in UTC, shown in the session's zone, read from a clock

A time or timestamp is kept as its UTC instant to the microsecond [ISO/IEC
9075:1992 §4.5.1], plus its displacement if WITH TIME ZONE. A date is its
days. SESSION.TIME_ZONE defaults to the database's, `+00:00`, and SET TIME
ZONE changes it [§16.5]. Results must not depend on the host's zone.

Intervals are months or microseconds in 128 bits, so a leading field of
maximum precision and the sum of two never overflow. A statement reads the
clock once at its start, so every CURRENT_DATE, CURRENT_TIME and
CURRENT_TIMESTAMP in it is the same instant [§6.8 GR3]. The clock is a
plug-in (`Database::set_clock`), and tests set one that stands still
(R-TEST-5).

#### D58. Dynamic SQL in SQL text, with a session as the module

ISO/IEC 9075:1992 §17's statements run through `Session::execute`, with the
session as `<module>`, since D32 claims no module language. SQL-92 defines
dynamic SQL for module procedures, and a session already plays that part.
Programs and drivers need only SQL.

Values are the program's parameters (`Params`). Prepared statements,
allocated cursors and descriptor areas belong to the session by name and
survive transactions [§17.6 GR10] until deallocated or the session ends
[§20.1 GR2b]. Direct SQL has no parameters [§20.1 SR1].

PREPARE and EXECUTE IMMEDIATE refuse a `<comment>` [§17.6 GR4q] or a
missing cursor (SQLSTATE 34000). The API's prepare (D45) accepts both,
because drivers' SQL often has comments.

The implementation-defined limits are 100 occurrences for a descriptor
area without WITH MAX, at most 1,000 [§17.2 SR2, GR3], and 64 areas a
session. A plug-in's type has TYPE code -1 [§17.1 Table 18].

### Architecture

#### D25. One log, owned by the database

Each database has one logical log, in a versioned, engine-neutral format.
It holds row changes (table, row id, before-image and after-image) and
changes of definitions. A large object's chunks are records of their own,
each logged before the row change that names them (D79). A transaction commits when its commit record is
durable, so commit order is log order.

The `Log` is a plug-in. turborel ships a memory log and a segmented file
log with group commit. A database in a directory opens its log through a
`LogStorage` and its catalog through a `CatalogStorage` (`db/stores.rs`).

With LOG.COMMIT_SYNC OFF (D36) the session is answered once its commit
record is written, before it is synced. A crash before the next sync loses
that commit and every commit after it, so recovery still finds a prefix of
the commits. The job `turborel-log-sync` (D55) limits how long this lasts
to LOG.SYNC_AFTER_MS.

LOG.DURABILITY (D36) sets what a sync does, and a change applies from the
next sync. Only a sync that reaches the disk claims records durable. A
replicated database's journal ignores this setting, because VR needs a
record durable before it is acknowledged (D57).

Each engine declares how it persists, as `Logged`, `Forced`, `External` or
`None`. Catalog images hold the definitions, the settings and where each
table and index is. Opening loads the image and replays definitions logged
since.

A checkpoint writes every engine's state as of a log position, so the log
can be truncated behind it. The row store's file is shadowed, so a crash
leaves it as the last checkpoint did. Recovery reopens the tables, then
redoes and undoes from the log by the database's one replay (D57).
Checkpoints fall due after CHECKPOINT.AFTER_CHANGES row changes. The log
is kept from the last checkpoint or the oldest open transaction's first
record, and `Event::CheckpointOverdue` (D40) warns when it grows too far.

The segment appended to keeps zeros written past its records, so a
commit's sync carries no growth of the file, which costs a journaling file
system a second flush. Zeros after the last record are this fill, kept on
open and cut off as the segment is sealed.

A segment cut short by a crash ends the log. Valid records after invalid
bytes are damage, unless the invalid bytes show an interrupted write. That
means an unwritten sector of zeros, a zero length ending them, or a record
past the file's end. Damage refuses the log (SQLCODE -916) and cuts
nothing, so later commits aren't lost.

Opening takes an exclusive lock on the directory's `lock` file before
clearing or reading anything, so a database is open in one place at a
time.

A failed sync stops changes, because it may have lost logged data and a
later sync could succeed without it. The file log never retries. Row
changes, writing commits, definitions and checkpoints are refused
(SQLCODE -915), reads and rollbacks go on, and reopening recovers from the
log.

A commit whose record is written but whose sync fails is in doubt. It gets
SQLCODE -922 (SQLSTATE 08007), no session sees it, and it keeps its locks
and versions until the database is dropped. On reopening it commits if its
record reached disk and rolls back otherwise.

One commit point makes transactions atomic across engines without
two-phase commit. Logical records keep engines independent, and a pluggable
log allows replicated or remote ones.

#### D26. Cryptography only from vetted crates, in plug-ins

Password hashing, TLS, encryption at rest and cryptographic hashes may use
vetted crates, in their plug-ins only. The core stays dependency-free (D3).
Cryptography written here would be unreviewed and likely wrong, and a
server needs it.

All cryptography comes from aws-lc-rs. It is maintained by AWS, it is
rustls's default provider, and it has a FIPS 140-3 build. We don't use
`ring`, whose README calls it an experiment that third parties shouldn't
depend on.

R-SAFE-4 requires this record of each dependency.

| Crate | Version | License | Used for |
|---|---|---|---|
| aws-lc-rs | 1.18.2 | ISC AND (Apache-2.0 OR ISC) | all cryptography |
| rustls | 0.23.46 | Apache-2.0, ISC or MIT | TLS, in the client only under feature `tls` |
| tokio-rustls | 0.26.6 | MIT or Apache-2.0 | TLS in the server |
| rcgen | 0.14.10 | MIT or Apache-2.0 | test certificates, test support feature `tls` |
| rustls-native-certs | 0.8.5 | Apache-2.0, ISC or MIT | platform roots, client feature `system-roots` |
| security-framework | 3.7.0 | MIT or Apache-2.0 | macOS keychain |
| security-framework-sys | 2.17.0 | MIT or Apache-2.0 | macOS keychain |
| core-foundation | 0.10.1 | MIT or Apache-2.0 | macOS keychain |
| core-foundation-sys | 0.8.7 | MIT or Apache-2.0 | macOS keychain |
| bitflags | 2.13.2 | MIT or Apache-2.0 | macOS keychain |
| schannel | 0.1.30 | MIT | Windows system store |
| openssl-probe | 0.2.1 | MIT or Apache-2.0 | finding the system bundle elsewhere |

rustls-native-certs has no `unsafe` of its own. The macOS crates' `unsafe`
is their calls into system frameworks. openssl-probe's one `unsafe` is in a
function we never call, and it stays at 0.2.1 in Cargo.lock because 0.2.2
was under two weeks old. None has an open advisory, and
security-framework's RUSTSEC-2017-0003 was fixed in 0.1.12. We rejected
rustls-platform-verifier as a larger change of trust, and webpki-roots
because a list fixed at build time goes stale.

Passwords use PBKDF2-HMAC-SHA256 at SERVER.PASSWORD_ITERATIONS, default
600,000, which is OWASP's FIPS 140 choice (D43). Argon2id is OWASP's first
choice, but its one Rust implementation claims no audit or production
readiness and has no FIPS path. A verifier names its scheme, so Argon2id
can be added later.

SCRAM-SHA-256 (D43) is one implementation in `turborel-protocol`, under
feature `scram`, checked against RFC 7677's example. The client trusts
authorities in the files it is given, and the platform's only on request,
because a platform trusts many authorities and your own server usually has
your own. No vetted crate in the tree reads a certificate's subject, so
the protocol walks the DER itself (`turborel_protocol::der`).

`turborel-aes` provides AES-256-GCM and ChaCha20-Poly1305 as encodings
(D48), and SHA-256 (id 11), HMAC-SHA-256 (id 12) and SHA-512 (id 13) as
hash functions. A file records the function's id and the name of the key
behind each digest, never the key, and is refused (SQLCODE -910) if the
keyring lacks it. Keyed checksums don't stop a block or page the database
once wrote being put back in place. Digests are at most 512 bits
(`hash::MAX_DIGEST_BYTES`).

#### D48. Compression and encryption as encodings, per file or per table

Encodings (D30) are registered with the database. STORAGE.ENCODINGS lists
those every file passes through (D36), and the table option ENCODINGS
lists those its rows pass through (D41). What compresses well or must be
hidden differs by table. Ids are never changed or reused. They are 1 LZ4,
2 AES-256-GCM and 3 ChaCha20-Poly1305, and tests use 1000 and up.

`turborel-lz4` uses lz4_flex 0.14.0 (MIT) with only its safe encoder and
decoder (features `safe-encode` and `safe-decode`). It claims no production
readiness, and we took it on its use (about 160 million downloads, with
arrow, parquet and tantivy depending on it) and its fuzzing. `turborel-aes`
uses D26's aws-lc-rs.

Keys come from a `Keyring` the host supplies, never from database files,
or encrypting would hide nothing. Storage place and key name are
authenticated, so data changed, moved or of another key is refused
(SQLCODE -916).

Tables share one data file whose pages keep their slots whatever their
encoded length, so the engine seals table data, not the pager. Sealed data
names its encodings, so the option changes in place. An index keeps keys
in the clear, so a table with confidential encodings gets no index
(SQLCODE -103) unless the database's files are encrypted too.
`ALTER TABLE ... RESEAL` [EXTENSION] rewrites a table under its current
encodings so an old key can be retired.

Changing STORAGE.ENCODINGS or STORAGE.CHECKSUM_KEY applies at once to new
writes (`Database::tell_formats`). The job `turborel-rekey` (D55) rewrites
old pages (`db/rekey.rs`), paced by STORAGE.REKEY_BYTES_PER_SECOND. A file
is settled once its header names the formats the settings do, so the sweep
resumes across crashes with nothing else recorded. `Event::FormatsSettled`
and INFORMATION_SCHEMA.FILE_FORMATS report progress. STORAGE.CHECKSUM
stays as the database was made.

Spill files (D38) are encrypted while the database encrypts anything, under
an ephemeral key held only in memory, because no later process reads them.
They follow a change page by page (`disk/pager/live.rs`), because moving
holders to new areas would copy their tables and refusing would fail other
sessions' statements.

A name not registered is refused when set (SQLCODE -103). A database
missing an encoding a table names still opens, and that table's rows are
refused (SQLCODE -910).

#### D55. One thread manager, with each thread labelled and shown

All threads of the database, server and command-line client, and all
background work, start through `src/threads.rs`, never `thread::spawn`
elsewhere (R-SAFE-5). Ad hoc threads were invisible to operators, outlived
their purpose, swallowed errors and made tests timing-dependent.

Threads are labelled `turborel-<kind>[-<n>]`. Background work runs as jobs
under a per-database policy of automatic, manual (`Database::run_due`) or
paused, so tests and operators choose when it runs. Errors become events
(D40), and a panic is caught, reported and restarted per policy.
Plug-ins add jobs through `Database::add_job`, and a layer that knows no
database asks through a weak `JobAsker`.

A database's threads are all joined before its files and lock are
released. No job drops its database last, so the lock is free when the
program's last handle drops. Job turns refuse a thread already holding
them (R-ERR-4). INFORMATION_SCHEMA.JOBS shows threads and jobs to the
database's owner.

#### D56. One file manager a database, with open files limited and shown

Every file a database opens, creates, removes or locks goes through its
file manager (`src/files.rs`), which wraps its `FileSystem`. Files had been
opened ad hoc, with descriptors unbounded and failures unseen. Engines and
index methods get the manager through `Host`.

Each file has a class that decides what create, remove, sync, close and
open do. The classes are durable (catalog, data file, log), durable at
commit (shadow files), unsynced (undo log) and scratch (spill files).

FILES.MAX_OPEN caps open descriptors, 1024 by default and 0 for none. Past
it the least recently used file is closed and reopened on next use. Files
with unsynced writes are synced first, because a writeback error no sync
saw is lost with the descriptor. If that sync fails, the failure is kept
for the file's next sync.

A plug-in's directory is written or shadowed (`PluginFiles::declare`). A
shadowed one reaches the disk only at a checkpoint (`disk/shadow.rs`). An
in-memory database's files live in `MemoryFileSystem`, reserved from
MEMORY.BUDGET_BYTES. On Unix, files are 0600 and directories 0700.

While anything is encrypted (D48), events carry no statement text, so no
value meant for an encrypted table reaches a log in the clear.
INFORMATION_SCHEMA.FILES shows files to the database's owner.

#### D57. Replicated state is the log, the catalog and the durable files, as opaque bytes

All durable state is in the `Log`, the `CatalogStore` or files registered
as durable (D56). A feature keeping it elsewhere declares an export hook or
is refused when replicated (SQLCODE -910). Replication orders, stores and
moves these undecoded, and only the core's one replay, `Database::replay`,
interprets them. Most of SQL-92 and all later SQL is still to build, and
none of it should need a replication change.

`turborel-replication` (std only) holds the extension points `Consensus`,
`Transport`, `StateTransfer` and `Journal`, with Viewstamped Replication as
the first consensus (`replication/src/vr.rs`). The core's seam is
`Standby` (`db/standby.rs`), and the core never depends on the replication
crate. `Driver` is content-blind and joins these together, and the job
`turborel-replica` runs its turns. The network is in the server
(`server/src/replication.rs`, `replication/net.rs`), with mutual TLS.

We depart from [VRR] for durability. A record is durable before PREPAREOK,
the view-number is written before a replica acts on a view, and a new
primary serves only once the log it took over is delivered. `vr.rs` lists
every departure.

A database following a group refuses what it would log (SQLCODE -917) and
keeps serving reads. It recovers from its last checkpoint alone
(`Database::open_following`), because its log's later records may not be
committed.

These were approved with this decision.

- a `Log` for commit beside a content-blind driver, and the core's
  `Standby` seam
- a record durable before a backup acknowledges it
- a sync in doubt never stamped (D25), and when replicated, resolved by
  consensus or the node deposed
- LOG.DURABILITY never reaching a replica's journal (D25)
- a following database's checkpoint refused while a reader holds versions
  awaiting collection (SQLCODE -911)
- a `node` property for settings never logged (D36)
- reads on the primary confirming its view
- members changed by SQL on the primary (ALTER DATABASE REPLICATION
  MEMBERS), the log carrying each epoch's members, a removed node serving
  nothing until stopped, and member numbers never reused
- an export carrying the records its checkpoint is recovered with
- a frame to a replica the transport doesn't link dropped as if lost
- no at-most-once for SQL clients in alpha
- stale snapshot reads on backups, with a least position
- state transfer by the durable files the file manager knows

These checks hold the design in place, and each fails the build or
`verify`.

- Replay matches on `Change` with no wildcard arm (R-CF-3), so a new kind
  doesn't compile until replayed.
- `record::KINDS` lists every kind. One test fails until a new kind is
  listed, and another until a statement writing it is shown recovered.
- Every registered durable file is in the export manifest
  (`Database::export_manifest`) or carried by the log or catalog.
- Contract tests of `Consensus` and `Transport` run against each
  implementation (R-SAFE-3).
- The log suites (`support/src/logs.rs`) run against every `Log`.
- VR runs hundreds of seeds of a deterministic simulator, checking that
  committed records survive view changes in order, at most one primary
  serves a view, and the group converges. `cargo xtask simulate` runs
  longer sweeps.
- The simulator runs every replica through the real `Driver`, including
  real databases that lose unsynced writes on a crash
  (`support/src/volatile.rs`).
- The mechanisms of VR, windows, state transfer and recovery are
  mutation-checked (`xtask/src/mutate/list/vr.rs`, R-TEST-9).

#### D27. A thread-safe core over snapshots of the definitions

A `Database` is shared across threads, and extension traits are
`Send + Sync`. The definitions form an immutable snapshot. A statement
keeps the one it started with and holds no definitions lock, and a change
copies, edits and publishes the snapshot. Schema changes and queries never
wait for each other, and no lock cycle can form through the definitions.

A writer whose snapshot predates a change to its table is refused (SQLCODE
-911). A plug-in calling into its own session is refused (SQLCODE -910).
`create_index` on a versioned table waits for no transaction.

#### D28. Transactions may wait for locks, and deadlocks are refused

LOCKS.WAIT says whether a statement meeting a lock, or a row another
transaction is changing, waits for that transaction to end or is refused at
once (SQLCODE -911). LOCKS.TIMEOUT_MS bounds the wait. A wait closing a
cycle, through tables or rows, is refused at once, and only the statement is
undone. Sessions on their own threads should wait, and a program driving
several sessions from one thread must not, so LOCKS.WAIT is OFF by default
and ON in a database a server serves, whose sessions each run on a worker
of their own as their statements come.

#### D29. Engines own concurrency, at logical timestamps

Only the engine knows its versions and rows. Transaction ids and commit
timestamps come from increasing counters, never the wall clock. An engine
that isolates transactions itself keeps row versions, so readers and
writers never block each other and writers meet only on the same row. A
row's second writer waits for the first to end (D28), and goes on if it
rolled back. If it committed, a transaction reading at one snapshot is
refused (SQLCODE -911), and commit checks that nothing read has changed.

Commit is atomic across engines. Each validates at the commit timestamp and
then commits at it. A commit returns only once every commit before it has
ended, so its session's next snapshot counts it. A transaction that wrote
nothing commits at its snapshot, unchecked. Its end collects only the tables
whose versions await collection, since it leaves no garbage of its own.

Below SERIALIZABLE [ISO/IEC 9075:1992 §4.28, §14.1 GR5], a transaction
reads at one snapshot (REPEATABLE READ) or a new one per statement (READ
COMMITTED, and READ UNCOMMITTED, which reads nothing uncommitted). Write
skew commits, and constraint checks stay serializable. An update one
statement bases on what it read is never lost. At the per-statement levels a
searched UPDATE or DELETE that finds a row changed by a transaction it does
not see, under LOCKS.WAIT ON, reads the version committed, checks its search
condition of it, and changes that version or leaves the row
(`TableStorage::newest`, `write_newest`). A MERGE does the same of a
target row, checking its ON condition and its clause's condition of that
version joined to the same source row, and never inserts a row it leaves.
A positioned write, or a
referential action, is refused instead. A value read by one statement and
written back by a later one can overwrite a change committed between them. An engine that ignores `TableStorage::read_mode` reads as
SERIALIZABLE.

A transaction that names no level takes TRANSACTION.ISOLATION, READ COMMITTED
by default as in PostgreSQL. The standard says SERIALIZABLE
[ISO/IEC 9075:1992 §14.1 SR2], and that setting at SERIALIZABLE conforms.

The coordinator's table locks (D28) remain for engines that leave
concurrency to it. A database transaction whose writes change nothing any
snapshot reads says so (`TableStorage::preserving`), and is checked against
no other.

#### D30. A storage foundation of independent components

Below every engine, log and index sit portable file I/O, a block manager,
a pager, a buffer pool with pluggable eviction, an encoding pipeline and
pluggable hash functions (CRCs for checksums, MurmurHash3 for hash tables).
None knows what's above it, so each layer can be tested, fault-injected and
benchmarked alone. We never use mmap, because it leaves write order to the
operating system and breaks write-ahead logging. File formats are the same
on every platform.

A file's header names the codecs it was written with. Pool hits are
lock-free apart from the frame's latch. Eviction waits a bounded while for
busy frames, then is refused as if all were pinned (D38), because the latch
may be its own thread's. A reader-biased latch would need a reviewed
exception to R-SAFE-1.

Read-ahead is an optimization and never fails a read. The job
`turborel-read-ahead` (D55) reads STORAGE.READ_AHEAD_PAGES at a time, at
most an eighth of the pool's frames, and leaves whatever it can't read to
the scan.


#### D31. The built-in engines are a row store and a column store

Whole engines are the extension point. Two are built in, on one code base
over one data file, sharing one value codec, record format, statistics
module, buffer pool, MVCC and row ids. A table chooses by USING, else
TABLES.ENGINE, and moves by ALTER TABLE ... USING.

The row store suits SQL's point reads, updates and cursors, and is the base
other layouts build on. Each table is a B-link tree (Lehman and Yao, 1981)
keyed by a logical 64-bit row id. Row ids are stable, so indexes never need
rewriting. The newest version of a row is in the tree and older ones are in
an undo store, which is on disk for a database in a directory, so a long
reader holds disk and not memory. A table's large objects are a space of
immutable objects beside it, each owned by one row (D79).

The column store suits scans of a few columns of many rows. New rows stay
in the tree as a delta, as in SQL Server's deltastore, HyPer's Data Blocks
and TiFlash's delta tree, which keeps writes and key lookups as fast as in
the row store. The maintenance thread freezes them into immutable groups of
compressed column chunks (`engine/columns`) once COLUMNSTORE.FREEZE_ROWS
rows are committed before every snapshot. The tradeoff is that a row
fetched by key reads a page per column (`bench/analytics.rs` compares them).
COLUMNSTORE.LAYOUT chooses COLUMNS or PAX (Ailamaki et al., VLDB 2001),
which keeps a whole row on one page.

Frozen rows change like any row. A row's newest state is its tree entry if
it has one, else its group's values. COLUMNSTORE.REWRITE_FRACTION triggers a
group's rewrite. COLUMNSTORE.ORDER and COLUMNSTORE.BLOOM trigger rewrites to
match when they change. Chunks use plain, constant, frame-of-reference, run,
dictionary, ALP (Afroozeh et al., SIGMOD 2023) and FSST (Boncz, Neumann and
Leis, VLDB 2020) encodings, whichever is smallest.

A scan is given the columns needed and the conjuncts of a filter that can't
raise an error (`TableStorage::scan_where`). Each conjunct holds for every
row the filter keeps, so an engine may skip rows they don't match, and the
filter still judges every row given. The row store skips by zone maps per
1024 row ids (`rows/zones.rs`), and the column store by frozen chunk.
A zone is never narrower than what a snapshot may read, and it narrows with
a bounded lag. A write stales its zone only if the replaced value may have
been all that reached a bound. After a pass over every zone due, the zones
rest for a second, so steady writes cost maintenance one pass a second.
Row counts and zones are saved with the pages at each checkpoint, so
opening needs no pass over the rows.

Maintenance (`db/maintenance.rs`) is work an engine leaves for later, such
as statistics, freezes and rewrites. No transaction waits for it, and no
plan waits for or makes a pass. The row store reads every row of a table of
up to 60,000 rows for its statistics, and a fixed sample of about 30,000 of
a larger one's, scaling its distinct values by Haas and Stokes's estimator
(`rows/stats.rs`). So a pass costs the same however large the table grows.

Execution runs by batch where it can (`exec/batch.rs`), with vectors of up
to 2048 values per column. EXECUTOR.BATCHES is ON by default, and OFF reads a
row at a time so the two can be compared. The batch tests read each answer
both ways. A batch that raises gives its earlier rows first, then the
error, so results and errors match the row path (D15).

A filter comparing a scalar call with a constant can read its column
arguments directly from the batch (`exec/batch/judge/calls.rs`). Constant
arguments and batch values are passed by reference through
`ScalarFunction::call_borrowed`, with the temporary large-object store
(D79). Its default copies the arguments and calls `call_with` exactly
once; an override must preserve all results, errors and side effects.
Direct unary and binary row expressions can use the
same hook. Prepared geometry relationships borrow both operands, avoiding
reference-count changes to their shared type and payload. Calls still run once per selected row,
in row order, and stop at the first error with the earlier matches retained.
Comparisons retain operand order and propagate large-object read errors.
Arguments that need expression evaluation use the general row evaluator.

Column-only row projections consume each input value at its last output
position and copy it only for earlier duplicates (`exec/project.rs`). An
identity projection returns its input allocation. An index join likewise
moves the fetched inner row into its pair; it copies only the outer row,
which later matches still need. These changes avoid retaining extra
references to large values while preserving output order and multiplicity.
An index join can also retain a column-only output projection. When its
only conjunct is a spatial intersection, it refines borrowed source rows
and builds only the selected values, rather than copying both geometries
into a pair that the projection immediately discards. Adaptive scans use
the same binding (`exec/join/indexed/refined.rs`). Additional ON conjuncts
retain the full pair for evaluation before projection; a fallback to
ordinary probes retains the requested output columns.
Adjacent column-only projections are composed during physical planning
(`physical/project.rs`). This includes the projection that restores a
reversed join's column order, so the join receives the final selection.
Repeated columns and output order are preserved. A computed expression
on either side prevents composition; even an unused expression must still
be evaluated for its errors (D15).

#### D34. Index methods support comparisons and function predicates

An index method creates indexes and says which probes they answer, and the
planner picks by estimate. Three are built in, paged through the buffer
pool. `btree` is a B-link tree, ordered, for equality and ranges. `hash` is
a linear hash table (Litwin, 1980), for equality only, and its readers lock
nothing. `rtree` indexes two point coordinates or four rectangle coordinates
(D72), or one fixed-SRID geometry through envelope projections (D85).
`xml_path`, `xml_value`, `xml_property` and `xml_primary` index one XML
column's paths and values for XQuery's lookups (D106); no comparison
probes them.
A method can support independent comparisons on several key columns
without an equality prefix (`IndexMethod::supports_columns`).

Search, nearest-neighbour and key-row streams retain their index until
they close. Dropping an index removes it from new catalog snapshots;
its pages are reclaimed only after existing readers release it. A stream
must keep this ownership itself, without relying on an enclosing plan or
catalog snapshot to retain the index.

A comparison probe finds exactly what SQL compares. A probe value of
another type is placed by its anchor and each entry checked, and a number
of the other kind is placed among the column's values (`physical/probe.rs`)
[X3.135-1986 §5.11 GR4]. The planner probes only with conjuncts that can't
raise an error (D15).

A method may also support a WHERE filter written as `f(column, value)`
or `f(value, column)`, where `f` is a registered function returning BOOLEAN
or null and `value` is a constant or parameter. This is a function
predicate. `IndexMethod::supports_predicate` declares support for that
function, the column's position in the index key, its position in the
call and the key types. Support covers every accepted value of the other
argument, including null, and defaults to false. `Probe::Predicate`
carries the function, both positions and the argument value to the index.

The function opts in through `ScalarFunction::index_predicate`. It must
give the same answer for the same inputs, have no side effects and raise
no error for any arguments its type checks accept. It returns null when
either argument is null. Its `deterministic` method must also return true.
These promises and the method's support hold throughout their registered
life. Without the opt-in, or if an argument expression or any other part
of the filter can raise, the planner reads the table whole. A lookup must
not hide an error on a row it skips (D15).

The index returns every matching inserted `(Key, RowId)` pair, each once.
It may also return pairs that do not match, called extra candidates. The
keys contain inserted values unless `Index::key_projection` explicitly
declares a deterministic projection (D85). A projected index counts
`(projected key, row id)` pairs and returns projected keys. Its equality
lookups return projection-equality candidates; constraint and referential
checks compare original values. It provides neither SQL key order nor
covering reads. The executor fetches each row version visible to the
transaction and checks the whole WHERE condition, even for an exact
implementation. Only TRUE keeps the row; FALSE and unknown discard
it. A function lookup always fetches rows and never uses an index-only
scan.

An index of collation sort keys cannot supply the function's original
column values, so it is not used for a function lookup. If another key
column may be null, an index that omits null keys could miss a matching
row. The lookup is then allowed only if the index holds null keys. The
planner keeps any requested sort in that case, since the surviving nulls
may require a different order from the index's (D66).

`Index::estimate` counts all candidates before the WHERE check. Without
an estimate, the planner prices fetching every table row; without
`probe_pages`, it uses the usual estimate of a tree descent. The lookup
cost includes fetching every candidate. The estimated result count is
calculated separately and cannot exceed the candidate count.

Every prepared execution fills both the lookup argument and the WHERE
filter from its parameter values. When comparing plans for reuse, the
planner compares the registered function and both column positions but
ignores the argument value. These lookups use the usual plan-learning
rules, not the shortcut for a lookup of one unique key (D45).

Storage cannot infer which rows an arbitrary function selects. It treats
every key and range as a possible match without calling the function.
The executor records a read of the whole table, including nulls, so
conflict checks detect rows inserted or changed by another transaction.
Methods protect their own state when sessions call them concurrently and
never call back into a database or session. Scalar functions run without
a storage lock held (D27, D42, R-ERR-4).

A comparison scan is index-only when every column it reads is in the key,
of a type the key codec gives back exactly (`engine::key::restores`). A versioned
engine keeps an entry per version, so a row is made from its entry only
where every reader sees that row's one version
(`TableStorage::visible_to_all`). An entry that collection fails to take
back is owed (`txn/owed.rs`), and a close owing any is not clean.

`CREATE INDEX`, `DROP INDEX` and `ALTER INDEX` are extensions, run by the
owner of the table's schema. The index is built while transactions write
the table, so they run outside a transaction, each effective at once. An
index takes no UNIQUE, because a UNIQUE constraint makes its own.
INFORMATION_SCHEMA.INDEXES lists them. Settings are D36 and options are D41.

#### D35. The optimizer and planner

Rules rewrite the logical plan, and the physical planner picks access paths
and join order by estimated cost. The General Rules define results, not
plans, so any plan giving the same results and errors is the optimizer's
to choose. A rewrite is made only where it can't change a result or an
error (D15).

Rules are plug-ins (`Database::with_optimizer`). Each has its own ON/OFF
setting, OPTIMIZER and its name upper case, which a session may set.

Costs come from the index method's own answers where given, else the
table's statistics, else the defaults PLANNER.EQUAL_SELECTIVITY,
RANGE_SELECTIVITY and NULL_FRACTION. No plan waits for a statistics pass.
Distinct keys come from a sketch kept with the index.

A table of at least 1,024 rows, with no order wanted, is read by a probe that
fixes every column of a unique index without pricing another path. The probe
finds one row at most, so nothing could be cheaper by enough to be worth the
zones and counts a comparison reads. The values still come from the execution
(D45): a prepared statement's `?` fixes a column as a constant does, a null
standing in for its value until each execution fills it
(`physical/kept.rs`). A searched UPDATE or DELETE keeps the probe of its
object rows the same way. A query learning its plan (D45) is planned with its
values too, each `?` compared with a column standing in its probe for the
value, its slot noted, so another execution's value can take its place.

Fetches are priced by where the page is found, with a row scanned costing
1. A page in the pool costs 8.6, in the system's cache 72, and from disk
5900. These weights come from `bench/kept.rs` and `bench/pages.rs`.
STORAGE.SYSTEM_CACHE_BYTES is a setting because reading the cache's size
from the system takes `unsafe` (R-SAFE-1).

An inner or LEFT join on equalities becomes an index nested-loop join when
its cost is below the hash join's (`physical/indexed.rs`). A hash join
builds the operand of fewer estimated rows (`physical/join.rs`), swapping
operands under a projection that restores column order. Inner joins on
inequalities can probe independent index columns too. Each outer row binds
the bounds, and the complete join condition checks each visible candidate.
Both table orders are costed, including derived-table column projections.
An untested nullable key column or a raising inner filter forbids the path.
These joins use the existing indexed-row fetch and serializable read tracking.

#### D36. Settings

A server's behaviour must be visible and changeable without rebuilding it,
by the people entitled to change it. Each setting is declared once in a
registry, with its kind, default, whether a session may set it, and when a
change takes effect. Plug-ins and the server declare theirs the same way
through one hook, `Configurable`, and never keep a configuration of their
own.

Scopes are built-in default, database value and session value. Database
values are definitions, logged and replayed, and a session's never persist.
Only the database's owner (D43) changes its values. `SET name = value` sets
a session's own and `SET DATABASE name = value` sets the database's. Neither
is in ISO/IEC 9075:1992 (`parser/session.rs` gives their BNF).
INFORMATION_SCHEMA.SETTINGS lists every setting, its value and its source.

A number an operator tunes a subsystem by is a setting, and the constant it
was named by is its default. These stay constants.

- formats on disk, and the standard's implementation-defined values
- shapes fixed when made, such as stripes, shards, vectors and sketches
- the planner's weights an engine measures for its own costs
- limits guarding the reading of a format
- how often diagnostics report one trouble
- the response to a system failure

Names are `GROUP.NAME`, and a plug-in's sit under its own name. A quantity
names its unit (`_BYTES`, `_MS`). A word means one thing throughout. Choices
use `ON` and `OFF`, with other words only where two won't do. A threshold
names what it counts after AFTER (`_AFTER_CHANGES`). During alpha a name is
changed in place with no alias (VERSIONING.md).

Page size, checksum and log segment size are fixed when a database is
created and recorded in its first image, so a database opens with no
options. Settings that wait for a restart show STARTED_VALUE and IS_PENDING
in INFORMATION_SCHEMA.SETTINGS.

A node setting (`Definition::node`, D57) belongs to one node, and is never
logged or in an image, because replicas share those. No built-in setting is
one apart from replication's own, and adding one is a decision of its own.

The config file is a source of database values as a server starts, like
`turboreld --set` (D42). It is a subset of TOML 1.0 read by turboreld's own
parser with no dependency (D3). `turboreld/src/config_file.rs` gives its
grammar.

#### D37. System tables and the Information Schema

System tables are plug-ins whose rows are made from the definitions as a
statement runs. The Information Schema is the standard's own view
definitions [ISO/IEC 9075:1992 §21.2] over Definition Schema tables only
those views may read [§21.3]. That gives the standard's answers with no
second description of the catalog to keep in step. Each view has the
columns ISO/IEC 9075-11:2006 draft Clause 5 gives it, and the views of
usage are there too, their Definition Schema tables made from what each
definition keeps for RESTRICT, or binding it again where it keeps
nothing, as VIEW_COLUMN_USAGE binds a view; a column needing what the
catalog does not keep, as a routine's timestamps, is not defined until
the catalog keeps it. The short name views [§5.77] are the standard's
definitions too, each over its view, so they show what it shows. SELECT
on them is PUBLIC's and nothing else is granted. SQL_FEATURES and
SQL_PARTS read rows `cargo xtask features` writes from the standards'
feature taxonomies and CONFORMANCE.md's table of the features claimed,
which `verify` checks the rows against, so the views cannot drift from
the claims; SQL_SIZING's values are read from the constants that keep
each limit.

#### D38. The memory budget, and spilling

A server must bound its memory, and one budget over every cache bounds it
without binding plug-ins to one page cache. Each database has one budget in
bytes, MEMORY.BUDGET_BYTES. Pools reserve from it, a reservation past it is
refused (SQLCODE -918), and so is a page wanted from a full pool. Plug-ins
get the budget when registered. INFORMATION_SCHEMA.MEMORY_RESERVATIONS shows
each reservation to the database's owner.

Undo is never refused by the budget, because a refusal would damage a table
(D19). A failed statement's undo and a ROLLBACK reserve what they need even
past the limit.

An operator holding rows keeps up to MEMORY.WORK_BYTES in memory, then
spills to a temporary pager file whose buffer pool is its area. Areas come
from the `Spill` extension point. A spill file's creation isn't made
durable, because nothing reads it after a crash. Spill files live in `tmp/`
of a database's directory, cleared at open, and pass through encodings
while the database encrypts anything (D48). A spilled row's large objects
and XML values are named by what keeps them, not copied (D79).

Spilling keeps the orders the planner relies on. A sort is a stable external
merge (`exec/sort/`). A hash join is partitioned (Grace), and grouping,
DISTINCT, EXCEPT and INTERSECT partition likewise, up to
SPILL.PARTITION_DEPTH times. Past that they keep keys in a spill-area table,
or make a partition in passes.

Everything else a statement or transaction holds is bounded the same way.
Held rows and kept maps spill (`spill/list.rs`, `spill/map.rs`,
`exec/held.rs`). A transaction tracks up to TRANSACTIONS.TRACKED_ROWS rows
for its commit's check and then counts every row, which may refuse a commit
needlessly. Its changes and garbage are held to TRANSACTIONS.HELD_BYTES.
Recovery uses the same lists.

#### D39. Directions for extensibility

As with engines and index methods, the core fixes the protocols between
components, and implementations are replaceable. The server's wire protocol
is a plug-in (`server::Protocol`, D42), and data types beyond the standard
are plug-ins through `SqlType` (D53).

#### D40. Diagnostics are typed events to a pluggable sink

A server's operators need to know what the database did and what went
wrong, in a form a program can route and filter. A library's host needs the
same without parsing text. The database reports typed events
(`diagnostics::Event`) to a `Diagnostics` plug-in set with
`Database::set_diagnostics`, and by default they are dropped.
DIAGNOSTICS.LEVEL filters them.

Events cover the database's life, its own failures, statements slower than
DIAGNOSTICS.SLOW_STATEMENT_MS, and through `Database::tell`, the server's
listening, failed logins and refused connections. Anything that may flood
is reported at most once a minute, counted. A failure of a statement's own
SQL goes to its client and isn't an event.

No lock is held while delivering. An event carries no row data except a slow
statement's text, and no wall-clock time. A sink never calls back into the
database. `Event` is non-exhaustive, so a sink takes events added later.

#### D41. Table settings are options an engine declares for its tables

What suits one table doesn't suit every table, and an engine knows best
which of its choices a table may make. Each engine declares its tables'
options (`StorageEngine::table_options`), and any other plug-in may declare
options of every table (`Database::declare_table_option`). Index methods
and plug-ins do the same for indexes. Each option is declared once.

A table reopened with an option no plug-in declares keeps it as written,
listed as NOT DECLARED, so a plug-in registered late or gone never stops a
database opening.

Options are set by `CREATE TABLE ... USING engine WITH (...)`, `ALTER TABLE
... SET (...)` and `ALTER TABLE ... USING engine`. An in-place change reaches
storage once published, never before commit. A rebuild copies the rows into
a new storage generation (D33). INFORMATION_SCHEMA.TABLE_OPTIONS and
INDEX_OPTIONS list them.

#### D42. The server has three modes over one core

turborel runs embedded (the `turborel` crate), served (`turboreld`), or
both, with a program serving its embedded database through the server's
library. One core serves every mode, so the embedded and served database
are the same database. Each crate depends only on those below it. The
server doesn't depend on the client, nor the library on the program.

tokio 1.54.1 (MIT) handles network I/O only, and the core never sees it. It
is Rust's most used and reviewed async runtime, and none of its `unsafe` is
written here (R-SAFE-1). socket2 0.6.5 (MIT or Apache-2.0), feature `all`,
sets keepalive times. tokio already takes that version and feature. Its one
advisory, RUSTSEC-2020-0079, was fixed in 0.3.16. Times behind `all` are
read back from a socket in the server's tests on each platform.

I/O that waits must not hold a thread, and a statement that waits must not
hold an I/O thread. Tokio's tasks do I/O only. Statements run on at most
SERVER.WORKERS blocking workers, and authentication on at most
SERVER.MAX_LOGINS other threads. A worker writes the replies of the request
it runs, a batch of SERVER.BATCH_BYTES at a time, each within
SERVER.IDLE_TIMEOUT_MS, so a slow client holds its worker and a batch of
memory, no more. A fast client's next request the worker reads itself. The
socket is handed to it, the runtime's interest in it given up, and it waits
a millisecond or so apart from the runtime (`transport::Direct`), its
permit to run statements given up meanwhile. So a request answered wakes
one thread, the one that runs it, and a request that comes while the worker
waits wakes no other. A waiting worker holds no permit, so as many keep
waiting as the blocking pool has threads for, while statements wait for
permits too. A request that comes
later the connection's task reads through the runtime, which takes the
socket back. A stream TLS secures, or a named pipe, is not handed so, and
takes the task's path. Each connection runs one request at a time.

Time limits bound every client, and memory is taken as bytes arrive, never
as a header declares (`SERVER.*_TIMEOUT_MS`, `native::LOGIN_MESSAGE_BYTES`).
A connection's place is given back before its socket closes, so a client
reconnecting at the limit isn't refused (`holding.rs`).

An operator ends a connection with `END CONNECTION id`, or cancels its work
with `CANCEL CONNECTION id` (owner only, SQLCODE -104). The core reaches the
server through the extension point `ServedConnections`. The other extension
points are `auth::Authenticator`, `transport::Listener` and
`server::Protocol`.

TLS (D26) runs on one port with cleartext, and the first byte tells them
apart (RFC 8446 §5.1). SERVER.TLS is OFF, ON or REQUIRED. Certificates reload
without a restart (`Tls::reload`). SERVER.TLS_CLIENT_CA asks clients for a
certificate. The native protocol is version 1 (D45) on port 7422.

Each connection has one session, and each statement is a `<direct SQL
statement>` [ISO/IEC 9075:1992 §20.1]. A terminated session ends as
SESSION.COMMIT_AT_END says, and a lost one rolls back [§20.1 GR2a].

Configuration is the database's settings (D36). Command-line options say
where the database, log and keys are, not how it behaves. The server's own
errors are SQLCODE -919 [§22.1, Table 23]. Diagnostics go as JSON lines
through a bounded queue (`log::Lines`), so a slow disk delays no statement.

SERVER.SOCKET is a Unix socket on Unix and a named pipe on Windows, refused
elsewhere. Pipes keep default security because narrower needs `unsafe`
system API (R-SAFE-1), and remote clients are refused. On stopping, each
connection finishes its statement and closes. Past
SERVER.SHUTDOWN_TIMEOUT_MS, or on a second stop, statements are cancelled.

#### D43. Users, passwords and the database's owner

A server reached from other machines needs clients to prove who they are.
Users are in the catalog with a verifier, logged and replayed as
definitions. Verifiers are opaque to the core, which keeps what is proven
against without knowing how it was made, so mechanisms are plug-ins
(`Credentials`, `Authenticator`). Any identifier may be an authorization
identifier for a session the host program starts [ISO/IEC 9075:1992 §5.4,
Annex B item 41].

The server's verifier is PBKDF2-HMAC-SHA256 with SCRAM-SHA-256's keys,
`$scram-sha-256$i=N$salt$StoredKey$ServerKey`. It keeps StoredKey and
ServerKey, never the salted password, which would let its reader log in by
SCRAM. Naming the scheme and iterations keeps old verifiers valid.

There are five mechanisms.

- `password` sends the password whole, so it needs TLS, a Unix socket or a
  named pipe.
- `scram-sha-256` (RFC 5802, RFC 7677) proves the password without sending
  it and proves the server to the client. An eavesdropper can still guess
  from an exchange, so plain TCP needs SERVER.SCRAM_WITHOUT_TLS ON. A
  password is its UTF-8 bytes, without SASLprep.
- `scram-sha-256-plus` binds the exchange to its TLS connection, so a relay
  in the middle is refused. It uses `tls-exporter` (RFC 9266) over TLS 1.3
  and `tls-server-end-point` (RFC 5929) otherwise. `tls-unique` is out
  because rustls exposes no Finished message. A client claiming `y` where
  the server has a binding is refused as a downgrade.
- `certificate` takes a client whose verified certificate has exactly one
  common name naming an existing user. One name compared whole is the
  simplest mapping that can't be stretched.
- `trust` accepts a local client as the user it names, only while
  SERVER.TRUST_LOCAL is ON. It is OFF by default, because on a shared machine
  any account could otherwise connect as the owner.

An unknown user is refused exactly like a wrong password, against a decoy
verifier or a stable decoy salt. Failed logins slow their address, never
their user, because lockout would let anyone lock out anyone. The delay
starts at 100 ms and doubles up to SERVER.LOGIN_DELAY_MS.

A dropped user's sessions end at their next request (SQLSTATE 28000). A new
password ends none, because they proved themselves at login.

The owner is a user, the first created or another made owner, so a database
never has users but no owner. Only the owner runs `CREATE USER`, `ALTER
USER`, `DROP USER` and `ALTER DATABASE OWNER TO` (`parser/users.rs` gives
the BNF). The owner can't be dropped until another owns the database.
`turboreld user` manages users while no server holds the directory lock.
Passwords are never read from the command line. SET SESSION AUTHORIZATION
follows [§16.4 GR4, GR5]. User and owner changes go to diagnostics, never
with a verifier.

#### D44. A running statement is cancelled from another thread

A server must stop a statement its client gives up on without ending its
session, and never the wrong one. Statements are cancelled through the
session's `Stop` (`Session::canceller`), which any thread may hold. Each call
and cursor fetch is a window with its own generation, so a cancel arriving
as a statement ends never stops the next.

A statement checks before and after reading each row, as a spilled operator
reads back each record or batch, and while waiting for a lock. It fails with
SQLCODE -920 (SQLSTATE TB920) and is undone as any failed statement
[X3.135-1986 §7.3 GR3b i], and its transaction goes on. A cursor cut short
stays failed. STATEMENT.TIMEOUT_MS cancels a statement that has run that
long.

#### D45. Protocol version 1, and prepared statements

Prepared statements are SQL-92's dynamic SQL [ISO/IEC 9075:1992 §17], which
drivers expect. `Session::prepare` checks a `<preparable statement>` once
[§17.6], types each `?` by context [§17.6 GR5] and describes the result.
Failures to prepare are SQLSTATE 37000. Unlike SQL's PREPARE it accepts a
`<comment>`, as drivers send them (D58).

A statement keeps its parsed text and bound plan. Each execution fills the
slots, then rewrites and plans, so access paths follow its values
(`db/prepared/plans.rs`). A query whose plan is one probe fixing a whole
unique key, which no value changes (D35), keeps that physical plan too, made
once with its slots and filled at each execution (`physical/kept.rs`).

Another query learns its physical plan. Planning a short range query costs
about a seventh of its execution's CPU, measured under sysbench's
read_only. Each of its first executions plans it twice, with its values,
which it runs, and with its slots, its paths chosen by its values. Once five
in a row give both plans one shape (`physical/kept/shape.rs`), the plan with
slots is kept and filled at each execution. A query whose values keep
choosing other paths stops learning after twenty and is planned at each
execution. A table the kept plan reads that gains or loses more rows than
its statistics are gathered again for has it learn again
(`db/prepared/plans/learning.rs`). A later value that would choose another
path, as a range of the whole table would, gets the learned plan, which
gives the same rows at another cost.

A kept plan holds while the
snapshot's generation, the catalog read, and the session's identity, schema,
zone and settings are unchanged. Otherwise the statement is rebound against
current definitions [§17.6 GR10], so a revoked privilege is missing at the
next execution. A binding that reads a value, such as CURRENT_DATE, is never
kept.

A statement keeps the session collation it was prepared under, as §9.15
applies at PREPARE, so a later SET COLLATION changes no statement prepared
(D60).

Prepare, Execute (with a row limit), Fetch, Close and Deallocate are the
messages, because clients read large results in parts and must stop what
they start. Complete carries SQLCODE and SQLSTATE so warnings reach the
client, and Complete and Error end with whether a transaction is active.
Requests may be pipelined. A prepared query's columns come with Prepared
alone; Execute sends its rows.

Query and Execute carry whether the transaction ends with the statement
[EXTENSION]: committed as it succeeds, rolled back as it fails, so a
statement that is its own transaction, as drivers' autocommit has it, is
one request and one reply, and the server runs each request alone, never
reading past it for a commit sent with it. The client library sends every
statement so under `set_autocommit`; a cursor it opens keeps its
transaction.

Ready gives the product and version only once authenticated, along with a
random 128-bit key. A connection whose only message is Cancel with that key
cancels what the session runs (D44). This needs no second writer, which a
TLS stream can't have. Capabilities are exchanged by Hello and Ready, so
later ones need no new version.

The protocol stays at version 1 while turborel is alpha, so a change to
it is made in place. It carries large objects (D79) and multisets (D80).
A large object is sent whole, or as its locator with its lengths. A Lob
request reads a range of one, makes one, appends to it, or frees or
holds its locator, and a Lob reply gives back the octets read or the new
locator. A multiset type is tag 22 and its element type; a multiset
value is tag 22, its cardinality and its elements, never more than its
maximum cardinality.

#### D46. The command-line client

`turborel` (crate `turborel-cli`, on `turborel-client`) lets operators run
SQL without writing a program. Input comes from `-c`, `-f` or standard
input. Each statement runs alone as a `<direct SQL statement>` [ISO/IEC
9075:1992 §20.1]. Rows stream as a table, which bounds memory. Failures go
to standard error and the run continues, unless `--stop-on-error` is given
or the connection is lost. Exit status is 1 if any statement failed.

`--timing` reports each attempted SQL statement's elapsed time in whole
nanoseconds to standard error, after the statement. It uses the client's
monotonic clock, from starting the statement through receiving its outcome
and writing any result rows: server work, network transfer and streamed
output are included; writing a failure's diagnostic is not. Reading input,
the initial connection and login, and imports are outside it. Nanoseconds
are the unit, not a promise of clock precision. A failure to write timing
is ignored, as one to write a diagnostic is. Timing is off by default and
changes no protocol message (D45).

The client library is the SQL-agent [ISO/IEC 9075:1992 §15]
(`turborel_client::agent`). It runs CONNECT, SET CONNECTION and DISCONNECT
itself, and refuses them in an active transaction [§15.1 GR1, §15.2 GR1,
§15.3 GR6]. The CLI's `Connector` is `cli/src/dial.rs`.

A password is typed unechoed (`-W`) or comes from TURBOREL_PASSWORD, never
the command line, where local users may see it. It is proved by
SCRAM-SHA-256 (D43), and the client refuses to begin over plain TCP. Ctrl-C
cancels the running statement (D44).

R-SAFE-4 records two dependencies. tokio 1.54.1 (MIT) is already the
server's, with features rt and signal only. rpassword 7.5.4 (Apache-2.0)
reads unechoed terminal input on Unix and Windows, is small enough to
review, and has been maintained since 2015.

#### D59. Import is rows from a pluggable source, over a connection

Operators load data from other systems, mostly CSV.
`turborel_client::import` inserts a `Source`'s records into a table, and
`--import` is one caller. `Source` lets any program import a format the
client doesn't know. It holds one record at a time, which bounds memory.
`import` is generic over it, with no `dyn` (R-SAFE-3). The built-in
`Delimited` reads CSV per RFC 4180 §2 and tab-separated text, written in the
tree (D3).

Rows go through Prepare and Execute (D45) unchanged, one prepared INSERT with
a `?` per column, so server casts and constraints apply to every row. The
client assigns strings for character columns as INSERT does [ISO/IEC
9075:1992 §13.8 GR4b iii, §9.2 GR3b, GR3e], so one too long is refused
(22001) and isn't truncated with a warning as by CAST.

Import commits, so an active transaction is refused. It commits every batch,
1000 rows by default, which bounds rollback. A refused row stops it and
rolls back the batch, unless `--skip-bad-rows` is given. A lost connection,
cancel or unreadable record always stops it.

---

#### D72. Spatial pages use R-link split stamps and R* partitioning

`engine::rtree::RTree` stores finite two-dimensional bounds and counted
row identifiers in buffer-pool pages. Closed bounds retain their f64
coordinates; touching edges and point bounds intersect. Geometry predicates
must check the geometry after this candidate search. Rectangle containment
does not implement the topological ST_Contains relationship.

Subtree choice minimizes overlap enlargement above leaves and area
enlargement higher up. Splits choose the axis by summed margins, then the
distribution by overlap and area, after
[Beckmann et al., sections 4.1–4.2](https://sigmodrecord.org/publications/sigmodRecord/9006/pdfs/93605.98741.pdf).
Costs share a coordinate scale so finite coordinates cannot produce NaN
from overflowing area calculations. Stored bounds are never scaled.

Readers use the expected-child-stamp protocol of
[Kornacker and Banks, sections 3.1–3.2](https://www.vldb.org/conf/1995/P134.PDF).
A split gives the new right sibling the old stamp and gives the old node
a fresh stamp. A search follows siblings through the stamp it remembered
from the parent. This finds entries moved by a split without also visiting
a sibling already queued from the parent. Allocated page identifiers
supply unique stamps; pages are not recycled within a live tree. These
stamps are internal structure metadata, independent of SQL sequences and
transaction commit numbers.

Writers enlarge ancestor bounds before inserting, and recheck split
stamps under node locks. A full node splits before the new entry is added:
the partition applies to its existing entries, rather than to an overflow
including the proposed entry. Allocation or parent-posting failure then
leaves the logical insertion absent (D19). Readers can traverse an
unfinished split, and later writers finish posting it. The tree uses the
database's shadowed checkpoints and logical row recovery (D25), with no
additional physical write-ahead log.

Forced reinsertion and deletion condensation are excluded. They move
entries between unrelated branches, which split stamps cannot track for
a scan already partway through the tree. Empty nodes stay linked. This
trades space and possible overlap after updates for a smaller concurrency
protocol; benchmarks must include aged trees, not only fresh loads. The
implementation is an R-link tree with R* heuristics, not the complete
R*-tree insertion algorithm. Node and metadata formats are versioned
independently and reject unknown versions.

Nearest-neighbour traversal expands the branch with the lowest distance
bound next. Each arithmetic step of a branch bound rounds downward, so
rounding cannot hide a nearer entry. Leaf rectangle distances and row
identifiers order the results; every branch tied at that distance is
expanded first. The frontier reserves memory before growing (D38), and
dropping the iterator releases it. A new entry behind the distance already
returned is omitted, as concurrent inserts are optional to an index scan.
This ordering is exact for indexed rectangles and points. A geometry
consumer must rerank candidates by exact geometry distance and continue
until the remaining lower bound exceeds its kth exact distance.

The built-in `rtree` method accepts two approximate-numeric columns in
(X, Y) order, or four in (min X, min Y, max X, max Y) order. Coordinates
must be finite and rectangle bounds ordered. The method keeps the complete
coordinate key, checks SQL comparisons after candidate traversal and returns
no SQL key order. As other ordinary indexes do, it omits keys containing
NULL; NULLS NOT DISTINCT constraints are unsupported. The planner uses
independent coordinate bounds only when omitted null keys cannot satisfy
the query. Prepared probes replace each comparison value independently.
Ordinary MVCC key checks and serializable predicate tracking apply. These
are numeric index keys, not geometry types or SQL/MM routines.

Intersection joins traverse pairs of branches and prune disjoint bounds.
They descend one side at a time and stream matching leaf pairs. Split
chains partition each input independently, so the product of two moved
groups is not visited twice. The pending pairs and two decoded leaf pages
reserve memory (D38); result pairs are never collected in the index.

#### D85. Geometry values start with XY representations through SqlType

`geometry::GeometryType` registers GEOMETRY through D53. It is a single
type whose values distinguish Point, LineString, Polygon,
MultiPoint, MultiLineString, MultiPolygon and GeometryCollection. These
are value kinds, not a SQL structured-type hierarchy. Hosts register the
type and the functions in `geometry::FUNCTIONS` before opening files.
The server program registers them automatically before opening files;
calls still require the existing SQL.FUNCTION_CALLS setting.

The interchange subset is XY WKT and WKB from OGC 06-103r4 §§7–8. Readers
check complete input, finite coordinates, line cardinality, ring length
and closure, and homogeneous collection member types. They do not check
polygon self-intersection, hole placement or other topological validity.
Z/M, curves, surfaces beyond polygons and EWKB flags are refused. WKB
accepts either byte order per component and is written little-endian.
For empty points it uses the NaN-pair interchange convention documented
by [GEOS](https://libgeos.org/specifications/wkb/); no non-finite coordinate
enters the geometry's envelope. An empty geometry is not SQL NULL.

The existing D53 type-parameter syntax also permits `GEOMETRY(srid)`.
Its one unsigned parameter must fit a nonnegative signed 32-bit integer.
Assignment, existing-value CAST, record/key encoding and decoding enforce
that label, including on empty values. Text CAST supplies the destination
SRID while constructing the geometry. An SRID constraint never relabels
an existing value. Plain GEOMETRY retains arbitrary signed identifiers.
The descriptor's own version 1 encoding is a version byte, a parameter
count byte (0 or 1), and, if present, the SRID as a little-endian u32.
Unknown versions, invalid counts and out-of-range parameters are refused.

Every value has an opaque signed 32-bit SRID, zero if not supplied. No
identifier is validated against a CRS catalog, no axis order is changed,
and no coordinate transformation or geodesic operation is implied.
WKT and WKB omit SRID; the versioned stored payload retains it. Version 1
is a byte of version, an i32 little-endian SRID and canonical XY WKB.
Unknown versions and noncanonical stored encodings are refused. Existing
file, catalog, log and wire formats keep their versions: this payload is
carried by the existing extension-value representation.

The stored WKB reader checks little-endian order at every component,
positive zero and the writer's exact empty-point NaN bits while parsing.
It checks signed-zero encodings in a contiguous pass over each coordinate
array; the ordinary parser still checks finite values and ring structure.
Together with the ordinary structural and complete-input checks, these
accept exactly the bytes a read-and-rewrite comparison accepts, without
allocating a second encoding. Interchange remains permissive about byte
order, signed zero and the NaN pair's representation.

SQL equality, ordering, hashing and ordinary index keys compare the
canonical payload bytes, including SRID and coordinate order. This is
representation equality, not ST_Equals. Signed zero is normalized, but
ring order and line direction are preserved. Large values remain subject
to the ordinary index key limits. An `rtree` on one `GEOMETRY(srid)`
column stores envelopes instead; the row retains the original geometry.

Readers accept at most 16 MiB of WKT or WKB, 100,000 components, rings and
coordinates in total, and 16 nested geometry levels including the root.
Each reader checks the same budget; binary counts are checked against
remaining input before allocation. SQL constructor parameters and text
or binary observer results use the existing SQL string limits. An
observer exceeding its SQL result limit fails with SQLCODE -900. The
Rust interchange methods retain the larger representation limits.

`ST_POINT` constructs a point from two numeric coordinates; `ST_X` and
`ST_Y` read an ordinate, returning NULL for POINT EMPTY and refusing
other kinds. `ST_SETSRID` relabels coordinates without transforming them.
`ST_MAKEENVELOPE` constructs a rectangular polygon from minimum X/Y and
maximum X/Y coordinates. Both axes must have finite, strictly positive
extents: reversed and zero-area rectangles fail with SQLCODE -301. It
uses comparisons rather than subtracting coordinates, so extreme and
subnormal bounds do not require an area calculation. This free-function
extension is distinct from SQL/MM's ST_Envelope method. `ST_MINX`,
`ST_MAXX`, `ST_MINY` and `ST_MAXY` observe extrema of any geometry kind,
returning NULL for an empty point set.
`ST_GEOMFROMTEXT`, `ST_GEOMFROMWKB`, `ST_POINT` and `ST_MAKEENVELOPE` accept an optional SRID;
`ST_ASTEXT`, `ST_ASBINARY`, `ST_SRID`, `ST_ISEMPTY` and `ST_GEOMETRYTYPE`
observe the representation. ST_GEOMETRYTYPE uses SQL/MM-style names,
including ST_GeomCollection, rather than uppercase WKT names. They are
extension functions through the
existing opt-in call syntax.
This subset claims neither SQL/MM Spatial nor Simple Features conformance.

`ST_INTERSECTS` and `ST_DISJOINT` test closed XY point sets and return
integer 1 or 0 as OGC 06-103r4 §6.1.2.3 describes. Polygon topology must
be valid; these operations do not validate it. Empty sets intersect
nothing, NULL propagates, and mixed SRIDs fail with SQLCODE -301 before
empty or envelope shortcuts. They do not opt into D34's total Boolean
predicate contract. Thus planning must preserve their runtime errors.
Orientation first tries Shewchuk's binary64 rounding-error bound
([reference implementation](https://www.cs.cmu.edu/afs/cs/project/quake/public/code/predicates.c),
`ccwerrboundA`). It accepts a sign only when both products, the determinant
and the error bound are normal, and the determinant's magnitude exceeds
that bound. Other cases use outward-rounded intervals, then exact bounded
integers on binary64's dyadic coordinates. No tolerance changes a
topological decision, including at extreme exponents.

`ScalarFunction::spatial_operation` declares equivalent spatial semantics,
including result types, NULL propagation and SRID errors. It is separate
from D34's error-free Boolean opt-in. For `ST_INTERSECTS(column, value) = 1`
(or either reversed operand/comparison), a single fixed-SRID geometry
column permits envelope candidates. The other argument must be a valid
constant or parameter, and every other conjunct must be non-raising.
The full condition remains a residual. If a later parameter has a different
SRID, the index returns all entries, including empties, so pruning cannot
hide the runtime error. NULL and matching-SRID EMPTY queries return none.
An empty stored geometry projects to an origin rectangle, adding candidates
but no false negatives. NULL keys are omitted. Readers compare the visible
version's projection with the entry before evaluating the full condition.
Spatial reads currently record a whole-table serializable read (D34).
Geometry window probes group at most 4096 envelope/row-id pairs before
fetching rows. Blocks of at least 1024 pairs are sorted by row id to reuse
nearby row-store pages; smaller results keep their traversal order. The
buffer starts at 64 pairs and grows once, with both allocations charged
during the handover. Each cursor retains at most 160 KiB of candidate
elements; actual vector capacity is charged to D38's budget. A refused
allocation keeps the smaller buffer or streams directly. Every candidate,
including different versions of one row, retains its own envelope. A
prefetched index error follows all preceding candidates. Nearest searches
and reference-mismatch scans do not use these unordered blocks.
Each new projected join probe renews the row-store leaf-locality trial.
This preserves the row-byte buffer and transaction bookkeeping while
allowing a dense probe to benefit after a sparse probe exhausted its hints.
Geometry cardinality estimation counts up to 1000 candidates exactly.
Beyond that it samples 64 paths across the root's branches, weighting leaf
matches by each visited branch's fanout. It reads at most 256 sample nodes
after locating the root and retains only the root and current path node.
Changed split stamps, pending splits, malformed paths, arithmetic overflow
or an exhausted page budget decline the estimate. The estimate is at least
the candidate count already observed; it is approximate planning input,
never a bound used to skip or accept rows. Numeric coordinate probes retain
their existing exact-count cutoff.
Geometry index locators use version 2: version and width bytes, a u32 SRID
and a u64 metadata page, all integers little-endian. Numeric locators keep
version 1; tree page formats are unchanged.

An inner join on `ST_INTERSECTS(left, right) = 1` can probe one geometry
index for each outer row. Both columns must enforce the same SRID, all
other conjuncts and the indexed table's filter must be non-raising, and
the complete ON condition rechecks each visible pair. Planning considers
both table orders and column projections. A NULL outer geometry makes no
probe; an empty geometry finds no pair. Spatial probes retain the same
whole-table serializable read as filters. This is not a simultaneous
traversal of two indexes. Geometry outer joins do not yet use this path.

For an unordered spatial inner join, the executor can choose a single
table scan instead of repeated probes (`exec/join/indexed/adaptive.rs`).
It first runs ordinary probes. Completed probes accumulate their actual
candidate reads. Only after that count reaches the table's row count does
the executor buffer and price the remaining input. This conservative gate
avoids repeating much of a selective join's index work just to estimate
its cost. It can delay adaptation on storage with expensive random reads;
the gate is not a substitute for a calibrated storage cost model.
An initially selective prefix does not prevent a later broad probe from
triggering adaptation. Already-emitted outer rows are never replayed.
It buffers the remaining input within the work-area and global memory
budgets, reserving row slots, payloads and hierarchy storage before growth.
Refused growth resumes probing with the buffered rows and the first
row that did not fit, without rereading input. Missing estimates retain
probes. Otherwise the estimated probe
fetch work is compared with one scan plus four binary searches per
possible leaf, priced with the existing comparison cost. This conservative
price does not assume that hierarchy pruning will succeed. This choice
affects work only.

The buffered envelopes are partitioned by alternating coordinate centres
into a balanced hierarchy (`spatial/packed.rs`). Each leaf holds at most
64 rows in four sorted boundary arrays with inclusive prefix masks, one
bit per row (`spatial/masks.rs`). A scan skips disjoint groups through
subtree links, without allocating a search stack per table row. Within
each leaf, the masks identify exactly the overlapping envelopes, including
touching boundaries and degenerate rectangles. Original row positions
preserve duplicate inputs across partitions. It is still only a geometry
candidate set. Each
candidate runs the exact relationship, optionally prepared for that outer
row, followed by every remaining ON conjunct. NULL and empty bounds add
no candidate. Reads use the ordinary table scan's visibility, cancellation
and serializable tracking. This path promises no input order; an ordered
join retains probes. EXPLAIN calls the choice `AdaptiveSpatialJoin`.

The same-SRID column proof and non-raising residuals also establish that
this join has no General Rule error of its own. An intersection can report
an SRID mismatch, but cannot encounter one between those columns. A cursor
can therefore stream its results without first retaining all pairs for
D15's eager error check (`physical/raising.rs`). A raising outer plan or
output expression still forces that check. Malformed stored payloads
remain storage errors; this proof neither suppresses them nor validates
polygon topology. Other spatial plans remain conservative.

Non-raising spatial joins with column-only output use lookahead of at
most 64 result rows (`db/stream/chunks.rs`). This amortizes transitions
between geometry evaluation and cursor fetching without retaining the
complete result. The first fetch returns its row directly, without waiting
to fill a group. Row slots and payloads draw on the work-area and global
memory budgets. A refusal keeps the already-produced current row after
the reserved prefix and resumes direct streaming; it adds no resource
error. A storage error follows all earlier rows and ends the stream.
The ordinary cancellation guard surrounds lookahead: a cancel while
filling a group fails the active fetch immediately and remains an error
on later fetches, never a successful end of data (D44).
Closing the cursor releases the reservation. A same-session write still
settles the cursor through this iterator before changing its input (D15).

`ST_DISTANCE` measures minimum Cartesian separation in coordinate units,
returning DOUBLE PRECISION, zero for intersecting sets and NULL for empty
sets. It applies the same SRID and validity requirements. Outward-rounded
intervals prove endpoint classification and bound rounding in ordinary
measures. Uncertain cases use exact dot products and a scaled exact
determinant preserves small perpendicular distances despite cancellation
or intermediate overflow. Final magnitudes remain approximate binary64
values; underflow may round a positive distance to zero. Only a final
minimum beyond the finite range raises SQLSTATE 22003, not an intermediate
component distance. It is not a substitute for a topological predicate.

Distance bounds round each positive operation down. The Euclidean term
uses unscaled squares and a square root, combined with the greatest axis
gap. Overflow saturates that term rather than changing the scale: each
operation is monotone, so enlarging either envelope cannot increase its
bound. A finite geometry distance is at least this bound of its envelopes.
This relates pruning to the computed measure without an error epsilon;
it does not certify bounds for individual segments inside that geometry.
Distance norms stay within the greatest component and outward-rounded L1
bounds; exact determinant scales use encoded binary powers of two.

The operation metadata also identifies distance and emptiness calls.
`Index::nearest` may supply distance candidates, each with a nonnegative,
finite lower bound on the computed scalar result, in bound order. It
certifies error-free distance evaluation for stable entries or declines
before output so the caller can scan. The geometry R-tree certifies only
when query and root coordinates are within MAX/8 on each axis, including
the captured root's split siblings. Ancestor enlargement precedes insertion
and commit visibility; the certificate covers every retained snapshot
entry. A candidate stream uses the same monotone bound at branches and
leaves, retains its index and reserves its frontier memory. Numeric R-tree
nearest traversal keeps its existing Euclidean entry order.

Ascending `ST_DISTANCE(column, value)` with NULLS LAST and a known OFFSET
plus FETCH FIRST ONLY count can select `NearestScan` on a fixed-SRID
geometry column. The other argument is a constant or parameter, in either
position. Other projections and filters must be non-raising; declared
emptiness calls on stored fixed-SRID geometries have a narrow type-based
proof. Planning prices candidate fetches using filter selectivity against
scanning and sorting. Without emptiness statistics it uses range selectivity
for emptiness and the complementary share for nonemptiness, excluding NULLs.

Execution certifies the index before emitting rows, fetches the visible
version and checks its projection, then evaluates the complete filter and
distance once. A fixed-capacity heap keeps the kth scalar distance within
both MEMORY.WORK_BYTES and the database budget. Search stops only at a
strictly greater bound; the ordinary SQL sort handles secondary keys and
ties. If traversal exhausts with fewer qualifying non-NULL distances than
requested, a scan supplies NULL distances, including omitted NULL keys.
Uncertified requests or an unaffordable threshold heap scan before output.
Both paths preserve lazy runtime errors. Every indexed read records a
whole-table serializable predicate and observes cancellation. WITH TIES,
PERCENT and other distance orders retain ordinary plans. Learned plans
fill and prepare both their distance measure and full filter each run.

Constant folding may replace a scalar call with its `specialize` result
(D53). The replacement preserves its contracts and errors, remains correct
if given different arguments, and must not perform work with visible side
effects. Retained state reserves memory from the supplied database budget
(D38); failed preparation leaves the original call for runtime. Filling a
kept physical plan also folds and prepares its calls using that execution's
values, without changing access paths or raising skipped errors. Both paths
honor the constant-folding rule's presence and setting. An indexed function
join also prepares calls for each outer row, whose columns stay fixed for
one probe. It changes only scalar implementations, leaving argument
expressions and the complete join condition in place. An empty probe
evaluates no call; advancing the outer row releases the previous prepared
state. This path uses the same optimizer switch and memory budget. Geometry
relationships use this to keep one decoded constant operand per expression,
immutable and without a shared cache lock. They check operand identity
before using that representation and release the reservation with its last
owner. A single rectangular ring can locate points through its closed
envelope. Recognition requires four distinct axis-aligned corners and
closure, in either ring direction; holes, diagonal edges and collapsed
rectangles retain general point location. It uses no area arithmetic or
tolerance, including for subnormal and extreme coordinates.
Polygon and MultiPolygon operands with at least one ring of 64
edges can also keep a Y-interval tree for each ring. The threshold follows
the measured crossover in `bench/geometry.md`; shorter boundaries retain
linear scans. A query point visits only subtrees whose closed ordinate
range contains its Y; the shared exact ray-crossing rule still decides
boundary membership and shell-minus-hole interiors. These immutable trees
reserve vector capacity before allocation, have depth bounded by the
geometry coordinate limit, and decline independently when memory is short.
The decoded operand remains usable without them. Distance reuses the
prepared intersection decision, then keeps its ordinary separation kernel
and result bounds; segment-distance pruning is not implied.
Spatial probes with equal declared semantics and deterministic
functions have one learned plan shape even when preparation changes the
function object. Ordinary function probes still require object identity;
matching SQL names alone never establish equivalence.

#### D86. An inverted index keeps terms, places and counts in one B-link tree

The index method `fulltext` (`index/fulltext.rs`) finds rows by the words
of one character string column, for TEXT_MATCHES \[EXTENSION\] and the
full-text search of ISO/IEC 13249-2:2002 (CD, SC32 N0640). Its column's
text is cut into words by the tokenizer (`fulltext/text.rs`), whose every
rule is implementation-defined [13249-2 §4.1, §4.4, §4.5] and listed in
CONFORMANCE.md. The tokenizer reads a document a character at a time and
holds only the word it is reading, so a large object is never held whole
(D79).

An index is one B-link tree (D31), its keys tagged by their first byte.
Each distinct document of a row, its row and its hash, takes a document
number from a counter in `C`, never used again. `D` keys map the row and
hash to its entry count, number and length, and `N` keys map the number
back. A term's postings are blocks of at most 128 documents under `P`
keys of the term and the block's first number. A block holds the numbers
as varint deltas, then each document's term frequency, then each
document's places, its own length first so a reader skips them: each
word's ordinal, sentence, paragraph and character offset as deltas. A
document whose places would take more than a quarter of a block keeps
them in `Q` chunk entries. The block keys are the skip list, so a search
of a rare and a common term seeks past the common one's blocks without
reading them. `H` keys find the rows of a document's hash, `S` keys keep
each term's document frequency and greatest frequency, `W` keys map a
stem to the terms of it and `C` keeps the documents and their total
length. A document's postings are written as its first entry is
inserted and taken out at its last, so versions of a row with the same
text (D29) cost a count and nothing more. Stop words are posted nowhere,
but count among the ordinals, so distances stay right [13249-2 §5.1.5].
Postings take about 7 bytes each and a document's own entries about 90.

The keys the index gives are a projection (D85's contract), a 128-bit
hash of the document's tokens, so documents equal under SQL's comparison
project alike and a reader keeps a row only if its visible version
hashes the same. A document's places are sorted in a spill area (D38)
before its postings are written, so no document is held whole in memory
either. Writes of one row's entries are serialized by row stripes, as
the R-tree's are (D72), so a version's insert and an older one's removal
by garbage collection never lose a posting. A block is rewritten under a
stripe of its term, taken after the row's and one at a time, so writers
of one term never lose each other's documents. Entries are not logged.
Recovery mends or rebuilds the index from its table (D25), and undo
inserts and removes the full key, which tokenizes alike every time. A
bulk load numbers documents in row order and writes full blocks.

Its options (D41) are FULLTEXT.LANGUAGE, ENGLISH or NONE, and
FULLTEXT.STOP_WORDS, DEFAULT for the language's list, NONE, or a list of
words written with commas, normalized as a document's words are. The
index is rebuilt for a change of either, as its postings follow them.
FULLTEXT.K1 and FULLTEXT.B, the weights of BM25 ranking, change in
place. A term is kept to its first 255 bytes, cut at a character, and a
word to its first 256 characters. A longer one is found by its prefix
and then checked against the row.

#### D87. SQL/MM Full-Text is built in, its methods calls of registered functions

The public types and routines of ISO/IEC 13249-2:2002 (CD, SC32 N0640)
§12.1 are built in, as the index method `fulltext` (D86) is. FULLTEXT
and FT_PATTERN are types beyond the standard (D53), registered as the
database starts, and their routines are scalar functions (D74). A
registered type is no user-defined type, so it declares no methods. A
method of a value of one, `doc.Contains(p)` or `t.doc.Rank(p)`, is
bound as the call of the registered function of its name whose first
argument is the value, `CONTAINS(doc, p)` (`binder/structured.rs`).
Such a call is bound whatever SQL.FUNCTION_CALLS is, as the standard
writes the method that way. AS names no supertype of a registered type.

Contains gives an INTEGER, 1 or 0, so a query writes `doc.Contains(p) =
1`. A function declares the value its call is compared equal to where it
stands for a predicate (`predicate_truth`), and a full-text index finds
the rows of that comparison as of a BOOLEAN call (D34). Contains raises
2FF01, 2FF02 or 2FF03 of a malformed pattern, so it is no
`index_predicate`. It promises `rechecked_errors` instead. A call raises
only of its pattern, alike for every row, and a call of a null document
is null before its pattern is read. The index, given a pattern that
raises, finds every row it holds of a value, so the WHERE check raises
at the first row of a document exactly where a scan would (D15).

Thesauri are the rows of the tables of clause 8, FT_THESAURUS's
TERM_DICTIONARY, TERM_HIERARCHY, TERM_SYNONYM and TERM_RELATED, which a
user makes with that clause's definitions. A scalar function reads no
table, so a call of Contains or Rank whose pattern may name a thesaurus
is given the relations as its last argument (`appended_query`). The
binder appends an ARRAY of a query of them, read once in the statement's
snapshot and with its user's privileges, if the tables are there. A
constant pattern that names no thesaurus is given nothing. The index
reads only the column and the pattern (`probe_arguments`), and finds
every row it holds for a thesaurus expansion, which only the WHERE check
expands. A statement's thesauri hold at most 65,535 relations, the
greatest cardinality of an array.

`NEW FullText(...)` is bound as the call of the registered function of
the type's name, FULLTEXT. LANGUAGE is a reserved word, so its method is
written `doc."LANGUAGE"`.

A rank is a plug-in's choice, not the standard's [§5.1.3]. A `Ranking`
weighs a term in a document from its index's statistics and bounds that
weight for every document. BM25 is built in, and `Database::
register_ranking` adds others, which an index names by its option
FULLTEXT.RANKING, checked at CREATE and ALTER INDEX. The binder appends
to a call of Rank or Contains the name of the full-text index of its
document's column (`ranked_by_index`). A function that reads the
statement's context (`uses_context`) is called with it (`call_in`), and
the statement reads each index's statistics once, so every call and the
index's own bounds see one snapshot. A rank is therefore NOT
DETERMINISTIC, yet fixed within a statement. Of no index, Rank is BM25
with every inverse document frequency 1. A character string document is
read in the index's language, as the index reads it.

A function that scores (`ranks`) is ordered greatest first through the
nearest-neighbour path (D85), its bounds negated so the stream still
ascends. `ORDER BY RANK(doc, p) DESC FETCH FIRST n ROWS ONLY` reads the
index when a WHERE lookup of the same column excludes null documents,
which DESC sorts first. The executor recomputes each candidate's rank,
and a weight above its ranking's bound is SQLCODE -910, so a lying
plug-in is refused, never trusted. A pattern given as a string is a
constant cast to FT_PATTERN; a cast of a constant that succeeds raises
for no row, so a lookup or a nearest read takes its value (`settled`).

FT_INFORMATION_SCHEMA's views FT_FEATURES and FT_SCHEMATA [§9] are system
tables (D37) of the views' columns, as clause 10's base tables are a
model only [§10.1].

#### D88. Savepoints reuse the undo marks, and cancel staged definitions

A savepoint [ISO/IEC 9075-2:2023 §17.5] is taken as a statement's undo
mark is, but held across statements (`txn/savepoints.rs`). It notes
where the undo entries after it begin, takes a savepoint of each engine
that undoes its own changes, an engine joining later taking one as it
joins, and notes the place in the log of the first change after it.
ROLLBACK TO SAVEPOINT applies the undo entries after it, returns each
engine to its savepoint and logs the changes since as undone, with the
record a failed statement logs (`StatementUndone`). So no new log
record, undo entry or wire message is needed, and recovery and a
standby leave out what was cancelled as they leave out a failed
statement's changes. Going back may cross the start of the statement
running, as a procedure's body goes back to its caller's savepoint, so
that statement's own bookkeeping is clamped to the savepoint.

Changes of definitions are staged on a copy until the transaction
commits (D33). A savepoint notes how many are staged, and going back
cancels those after, makes the copy again from the rest and discards
the storage the cancelled ones made, which no other transaction saw.
Recovery and a standby drop a pending definition logged after the place
a `StatementUndone` names, so a cancelled one is never made at commit.
In a routine's body going back is refused if it would cancel a change
staged before the statement that invoked the routine began, since that
statement runs on those definitions. The locks taken since stay held, as
the transaction goes on [§17.8 NOTE 743].

The session keeps the names and levels (`db/savepoints.rs`). A function,
a procedure of NEW SAVEPOINT LEVEL, a triggered action and an ATOMIC
compound statement each push a level, popped with the context that
pushed it (`db/nested.rs`). A savepoint statement in a triggered action
is refused as the trigger is defined (IA046), and one in a procedure an
action calls raises 0W000 as it runs [§15.21 GR6e iii]. Going back
closes every cursor (IA168), makes invalid the locators given since
[GR3f] and deallocates no prepared statement (IA167). A transaction has
at most 1,024 savepoints (IL208), each holding a savepoint of each
engine that undoes its own changes.

#### D89. The JSON type is an SQL/JSON item stored parsed, as a typed binary tree read in place

A value of the JSON type (`Value::Json`, `value/json.rs`) is an SQL/JSON
item in a binary form (`json/binary.rs`), shared by reference count, so
a copy costs nothing. A document is a version octet, 1, then its root
node. A node is a tag octet and its body, integers little-endian:

```text
0 null | 1 false | 2 true
3 integer: i64
4 decimal: units i64, scale u8
5 wide decimal: units i128, scale u32
6 decimal floating point: coefficient i128, exponent i32
7 approximate: f64 bits
8 string: octets u32, UTF-8
9 datetime: moment u8 (date, time, timestamp), utc i64, zone minutes i16, zoned u8, precision u8
10 array: count u32, size u32, an offset u32 of each element, the elements
11 object: count u32, size u32, an offset u32 of each member in key order,
   the members in written order, each key octets u32, key, value node
```

Offsets count from the first element or member. An object's directory
is sorted by key octets, members of one key in written order, so a
member is found by binary search and the first written of a key is the
one a member accessor takes (UA075). The members themselves stay in
written order (UA057, US039), which serialization keeps. An exact
number is stored in the narrowest of its three encodings.

Accessors read the octets in place without making an item (`Node`), and
a lax path of members, wildcards and integer subscripts is evaluated on
them (`binary::fast_path`), from the root to what it selects. Any other
path is evaluated on the item made whole.

A value read from a record, the log, the catalog or the wire is checked
whole before use (`binary::validate`); damaged octets are SQLCODE -916,
never a panic.

Comparison is by a comparison key (`binary/key.rs`), prefix-free and
ordered as octets: the JSON null, then numbers by exact value, strings
by code point, false, true, datetimes (times, then dates and timestamps
by UTC instant), arrays element by element, objects by their members
sorted by key and value key (§8.2 GR10, IA224, IA228, IA229). Equal keys
are exactly equivalent items (§4.48.3), so `Item::equivalent` uses them,
and the key is the index key, the spill and grouping key, and the
bucket. A key decodes to an equivalent item, not the same one, so no
index-only scan gives a JSON column (`engine::key::restores`).

A JSON value is stored whole in its record, at most
`MAX_DOCUMENT_OCTETS` (256 MiB), with no 32 767 limit. The row store
keeps a long value on overflow pages of its own, as it does a long
string. It is not a large object (D79): random access by binary search
needs the document in memory, and a locator would read it whole on each
access anyway.

On the wire a JSON value is its stored form, type tag 25, which a
client shows as its JSON text; in the self-describing codec (log) it is
tag 20; in the catalog's type tags, 29. The format version stays 1 while
turborel is alpha.

#### D90. JSON_TABLE is a collection derived table, its plan a cursor of one tree

JSON_TABLE is planned as a collection derived table, the logical
operator UNNEST's with a JSON table attached. Its context item, PASSING
values and DEFAULT expressions are the operator's collections, so a
lateral reference, a dynamic parameter, a subquery and a LEFT join's ON
condition go through the paths UNNEST's take. The JSON table is the
standard's transformation done once, at bind time, as one primitive per
COLUMNS clause in pre-order and a plan tree of parents, siblings and
primitives [§7.11 SR1k]. It runs as a cursor of that tree that makes a
row at a time, null extended by OUTER and the UNION of siblings, never
the derived tables the transformation writes.

#### D91. An inverted index of JSON documents' paths and values, every row found rechecked

The index method `json` (`index/json.rs`) finds rows by the JSON
documents of one column, a character string's or the JSON type's, for
JSON_EXISTS, JSON_VALUE compared equal and JSON_CONTAINS \[EXTENSION\].
A document is parsed as the query operators parse it, WITHOUT UNIQUE
KEYS. A value of the JSON type gives its item already parsed.

A document posts terms (`index/json/terms.rs`). Each is 16 bytes, the
hash of a path and the hash of a value, both MurmurHash3. A path is the
chain of member names from the root, array steps dropped, so `$.a[3].b`
and `$.a.b` are one path. Every node posts its path with no value, and
every scalar posts its path with its canonical value. Numbers are
canonical as the `f64` nearest their exact value, so 1, 1.0 and 1E0 post
alike. Strings are canonical by code point with trailing spaces taken
off, so both comparisons a call may make find them. true, false and null
post as themselves. A scalar that is not a string also posts the string
JSON_VALUE's implicit RETURNING makes of it, and a string reading true
or false posts that truth value too, so a JSON_VALUE result compared
equal finds it. Each term a document posts counts once.

An index is one B-link tree (D31), its keys tagged by their first byte.
`D` keys map a row and its document's hash to the versions holding it,
and `H` keys find the rows of a hash. A posting is a `P` key of the term,
the document's hash and its row, so a term's postings are in one order
and merge. A term's count of documents is a `P` key after its postings.
A document's postings are written as its first version is inserted and
taken out at its last, under a stripe of row locks, so versions of a row
with the same document (D29) cost a count. A null document posts
nothing. The keys are projected by the document's hash (D85), its
characters hashed with trailing spaces dropped, or a JSON value's
comparison key, so a reader rechecks each visible row by its hash.

A call is compiled to the terms a document it is true of must post
(`index/json/need.rs`), an AND and OR of terms. A member chain needs each
node on it. A filter `@.k == literal` or `== $var` needs that value at
its path, and `&&` and `||` combine. A wildcard, a method, `!`, `like_regex`,
a comparison other than equality, or a mode the walk cannot follow needs
nothing more at that point, so strict mode, wildcards and methods ask a
superset. JSON_VALUE compared equal also needs the value at its path,
of RETURNING the implicit VARCHAR or BOOLEAN only, where no cast changes
a value. JSON_CONTAINS needs each leaf of its candidate at its path. A
search drives by the rarest part of an AND, by the counts, and tests the
rest by one descent each, and merges an OR's parts in order.

Every row found is checked by the whole WHERE condition (D34). The
planner probes only for a call that reads the column whose other
arguments are constants or parameters, of NULL ON ERROR (JSON_EXISTS's
default FALSE ON ERROR included) and NULL ON EMPTY. Such a call raises
of a document only when the index cannot read it. A document that is no
JSON text, or past a limit of turborel's, posts the term UNREAD, which
every probe finds, so the WHERE check raises of it where a scan would
(D15). A call of ERROR ON ERROR is never a lookup.

The options are JSON.PATHS_ONLY, ON or OFF, which posts paths alone, and
JSON.MAX_DEPTH, 0 to 128, the most member names posted; a deeper path is
found by its first names. Both are rebuilds (D41) and are kept in the
locator, so an index opened reads as it was made. A bulk load sorts
every entry's keys in a spill area (D38) and loads the tree in key
order, writing each term's count as its postings pass. Recovery mends
the index from the rows redone, as every method's (D25).

#### D92. An index's key part may be an expression that cannot raise, matched by its canonical form

A key part of CREATE INDEX may be `( <value expression> )` \[EXTENSION\],
written among column names in any order. Only `btree` and `hash` take
one. The expression is bound over its table's row as a generation
expression is [ISO/IEC 9075-2:2023 §11.4 SR10] (`binder/key_expression.rs`,
`Purpose::Key`). So it reads only the table's columns and has no
subquery, set function, host or user value, or possibly
non-deterministic function. The JSON operators are the exception: they
are possibly non-deterministic [§9.16 SR1g, SR1h, SR1al] only through a
datetime of the session's time zone and through errors, and both are
excluded below.

An expression is admitted only if it raises no exception condition of
any row (`index/expression.rs`). D15 then holds for a lookup by it: a
probe that skips rows skips no error a scan would raise. The analysis
admits a column, a constant, a negation of a number, and a sum,
difference or product of exact numerics whose result precision, by
X3.135-1986 §5.9 SR4, fits 18 digits. It also admits a CASE whose
conditions cannot raise and whose results are admitted. A call is
admitted if it is deterministic, reads no session context and its
function says so (`ScalarFunction::index_safe`, false by default). UPPER
and LOWER of one non-LOB character string say so. JSON_VALUE, JSON_QUERY
and JSON_EXISTS say so (`JsonCall::key_safe`) when all of these hold.
The context item is JSON or a non-LOB character string. Every PASSING
argument is a scalar of no time zone. The path calls no datetime(),
time_tz() or timestamp_tz(). ON ERROR is not ERROR. A DEFAULT is a
constant that casts to RETURNING, or a value of that type. RETURNING has
no time zone, or for JSON_QUERY is JSON or the implicit type. Anything
else, a quotient, a CAST, a domain, CURRENT_DATE, a sequence, a routine,
is refused with SQLCODE -103, the message naming the part that can
raise. An implementation limit, as SQLCODE -900 of JSON nested over 128
deep, fails the write, so every stored row has its key.

The catalog keeps each expression as written, with its place in the
key. `IndexDesc::columns` holds `EXPRESSION_PART` (`usize::MAX`) there,
so a path that reads key columns alone passes the index by. The text is
bound again whenever the index enters a catalog (`State::with_derived`):
at creation, as the database opens, and as ALTER TABLE remakes the
table's indexes, so a moved column is read where it is now. The format
stays at version 1: the catalog codec and the CREATE INDEX log record
carry the expressions after their other fields.

A write computes the key of each row (`IndexDesc::computed_key`). An
UPDATE leaves an entry alone when no column the key reads changed, on an
engine that keeps one version of a row. An engine that keeps versions
gives each version its own entries, as for any index. A scan through such
an index records a read of the whole table for serializable conflict
checks, since its probe's condition is no condition on columns. A
temporary table's instances build and keep entries the same way.
Renaming a column an expression names is refused, as for a generation
expression (D31). Dropping one drops the index. If binding again fails
after another change, the ALTER fails.

The planner (`physical/expression_index.rs`) gives each distinct
expression of a table's indexes a planned column past the table's own,
of its type. That column is NOT NULL only if the expression is never
null: a NOT NULL column, a non-null constant, or a negation, sum,
difference or product of such. A query's subexpression of the same
canonical form is replaced by that column for choosing the path only.
The canonical form is the bound tree written out, each function by its
shown name, which for an SQL/JSON operator spells every clause. The
filter a scan keeps, and the conjuncts an index scan answers, stay the
query's own. So `=`, ranges, BETWEEN and IN of the expression probe it,
and ORDER BY of it, through a projection, reads a btree index in order
when the planned column is never null. EXPLAIN shows the probe as
`(T.A + T.B) = 3`. INFORMATION_SCHEMA.INDEXES shows the key part as
written, in parentheses. Expressions match by form only, so `B + A` is
not `A + B`.

#### D93. VECTOR is built in, without order or equality, its metrics plug-ins

VECTOR is a type beyond the standard (D53), built in as FULLTEXT is
(D87). `TypeRegistry::builtin` holds it, and its scalar functions and
aggregates are registered as the database starts. A type may read words
and `*` as its parameters (`SqlType::param_word`) and fold the
parameters written into those it stores (`SqlType::pack_params`), so
`VECTOR(768, INT8, SPARSE)` stores two numbers, `MAX_PARAMS`. `*` is
told apart from a written 0 until it is packed, and VECTOR(0) is
refused.

A type says whether its values have an order (`SqlType::ordered`).
VECTOR's have none, and no equality either (the vector design document
R3.11, R3.12). The binder refuses one, with SQLCODE -103, as an operand
of a comparison, `=` and `<>` among them, BETWEEN, IN, ORDER BY, GROUP
BY, DISTINCT, a set operation, MAX and MIN, and as a column of a PRIMARY
KEY, UNIQUE or FOREIGN KEY constraint. A UNIQUE constraint of a vector
column is refused before any method is asked, whatever
TABLES.UNIQUE_INDEX_METHOD names. An index method that keys by `=`
refuses a vector column at CREATE INDEX (`db/vectors.rs`, `keyed_by`).
A vector index method keys by its own search, `supports(CompOp::Eq)`
false, and takes one. Recovery compares keys by their encoded bytes, not
by `=`, so it is unaffected. VECTOR_EQUALS compares two vectors bit for
bit.

The distance operators `<-> <=> <#> <+> <~> <%>` are delimiters, read
between additive expressions and comparisons, and parsed as a call of
VECTOR_DISTANCE with the metric as a string. `+ - * /` with a vector
operand are bound as VECTOR_ADD, VECTOR_SUBTRACT, VECTOR_MULTIPLY,
VECTOR_SCALE and VECTOR_DIVIDE. SUM and AVG of a vector are VECTOR_SUM
and VECTOR_AVG. A registered function says which of its arguments it
takes as key words (`ScalarFunction::word_argument`), and the binder
writes an identifier alone there as a string, so `VECTOR_DISTANCE(a, b,
COSINE)` and `VECTOR_NORM(v, NORM1)` read as written. The binder
appends to VECTOR_DISTANCE written without a metric the metric of an
index on its first argument's column, if one describes one
(`IndexMethod::metric`, R6.3). Otherwise it is COSINE.

A metric is a plug-in (`VectorMetric`), as is a quantizer (`Quantizer`).
They are kept in registries the database shares, not copied with each
change of its state (`db/vectors.rs`), so VECTOR_DISTANCE and the vector
index methods, which read them at each call, see a metric registered
later. `Database::register_metric` and `register_quantizer` add to them,
each told to the diagnostics sink as `Extension::Metric` or
`Extension::Quantizer`. A metric is named by a regular identifier in
upper case, or by two joined by a period, a schema and a name, which an
index names by `DISTANCE schema.udf_name`. A second VECTOR type, a
second metric of a name, or a metric whose result breaks its declared
contract, a negative distance of one NON_NEGATIVE, is SQLCODE -910
(R-SAFE-3).

A distance function of SQL (CREATE VECTOR DISTANCE FUNCTION, §8) is an
SQL-invoked function whose descriptor holds what it declares more
(`DistanceDesc`), its properties, its formats and its body. It has
EXECUTE, DROP, persistence and the Information Schema of any routine.
VECTOR_DISTANCE naming it is bound to a `CallbackMetric` of its body,
bound as a generation expression over a row of its two parameters, and
checked for EXECUTE (R8.5). The call is noted as an invocation, so a
view, constraint, trigger or routine naming it depends on it and DROP
RESTRICT is refused. Its body binds at creation. Its name is no
metric's, neither its own name nor `schema.name`. Only LANGUAGE SQL is
built in, and another is SQLCODE -900. A vector index measures by a
built-in or registered metric, never by a distance function of SQL, as
an index's entries must not depend on a body a user may drop.

INFORMATION_SCHEMA.VECTOR_COLUMNS and VECTOR_DISTANCE_FUNCTIONS are its
catalog (§16), shown by privilege as the other views are.

#### D94. An approximate search may miss rows, never return a wrong one, and raises what an exact one would

FETCH APPROX, a VECTOR_SEARCH, a HYBRID_SEARCH, or VECTOR.APPROX_DEFAULT
ON with a row limit asks for the nearest rows by a distance, and lets an
index find them approximately (the vector design document §9.2, R13.1).
Without one of these a query is exact, whatever indexes there are, and
FETCH EXACT always is.

An index method answers a nearest read in one of three ways
(`index/nearest.rs`). `No` is no answer. `Exact` certifies lower bounds,
as the R-tree does, and is D15-safe as the nearest read always was.
`Approximate` is offered only to a query that asks for approximation,
and only when the index serves the distance's metric. An approximate
index's contract is relaxed. It may miss candidates, its distances need
not bound anything, and its stream ends when it has no more. It must
still certify that the call cannot raise, or decline with None.

The executor never trusts an approximate index's distances
(`exec/nearest/approx.rs`). Every candidate's row is read as the
transaction sees it, the full WHERE condition is checked, and the exact
distance is computed by the query's own expression. The rows kept are
the first k by exact distance, then row identifier, R9.5's tie order,
and the stable sort above keeps it. Too few rows widen the search
(R9.3). The index is asked again twice as wide, at most
`MOST_WIDENINGS` (4) times. Then the table is read exactly, giving the
rows an exact query gives (R9.2). VECTOR.APPROX_FALLBACK OFF refuses
instead, SQLSTATE 0A000, SQLCODE -900. An index that declines, or whose
engine or reservation gets -918, falls back at once.

The D15 reading is this. A query that asks for approximation raises
whatever an exact one would. The planner reads an approximate index only
under the gate the certified read has (`physical/nearest/safe.rs`). No
part of the WHERE clause or the projection, the distance aside, may
raise, unless it is a call that raises alike for every row of a value
(`ScalarFunction::rechecked_errors`). Every distance promises that, so a
distance in the WHERE clause, a range search (R9.1), does not bar the
index. The gate also requires every vector column of the call to be of
fixed shape and not FLOAT64, and the metric to be built in, so an error
is raised alike for every row. Skipping a row then never hides an
error. The read takes the whole table's serializable predicate first, as
the certified read does, so a missed row is the request's allowance and
never a phantom. Approximation may change which rows come back, never
whether an error is raised, and a row that fails the WHERE clause never
comes back.

A distance function answers the planner through three hooks of
`ScalarFunction`. `vector_metric` names the metric of a call,
VECTOR_DISTANCE's from a constant in either order, COSINE if none is
written, and each shorthand its own. INNER_PRODUCT has none, as it
orders greatest first. `rechecked_errors` is true of every distance.
`parameter_type` gives a `?` in a distance's vector place the type of
the other vector operand, so a query vector given as text is read in the
column's dimensions and format.

`IndexMethod::serves_metric` says whether an index serves a call's
metric. By default it is `same_metric` of `metric`, its own, with
EUCLIDEAN and EUCLIDEAN_SQUARED serving each other as they are monotone
in one another, the radius squared or rooted. An index whose column is
declared NORMALIZED serves any of COSINE, DOT, EUCLIDEAN and
EUCLIDEAN_SQUARED (R13.1), which order rows alike on vectors of norm 1
since `|a − q|² = 1 + |q|² − 2 a·q`. Only the column's vectors need norm
1, not the query's, and a radius is carried to EUCLIDEAN's terms
(`index/vector/options.rs`, `unit_radius`).

The filter strategy follows the condition's estimated selectivity
(R13.2, `physical/nearest/approx.rs`). Below 1% it is a pre-filter, the
ordinary plan with no approximate index. Up to 50% it is an in-filter,
where the index asks a `RowFilter` of each candidate as it searches.
Above 50% it is a post-filter, asking for more candidates and filtering
them after. The `RowFilter` is called only during `Index::approximate`
with no lock held (R-ERR-4), and reads the row as the statement sees it.

Hints follow the fetch clause as `HINT ( ... )` (R13.4,
`parser/query/ordered/approx.rs`). VECTOR_PREFILTER, VECTOR_INFILTER and
VECTOR_POSTFILTER override the estimate, VECTOR_INDEX(t i) chooses the
index, and NO_VECTOR_INDEX(t) bars approximate indexes of the table.
VECTOR_WAIT_FRESH there changes no read, since every built-in index is
changed as its table is, with no delta store merged later. The queue of
embeddings is waited for by the statement's leading comment hint
instead (D103, D105).

#### D95. The vector index methods are one Index over engines, each serving its own metric approximately

`index/vector.rs` adapts each engine to one `Engine` trait, and one
`VectorIndex<E>` implements `Index` over it, for the methods `diskann`,
`hnsw` and `ivf`. The methods differ only in their options and in how a
search's knobs reach the engine.

Each indexes one dense VECTOR column of fixed dimensions and format
(R3.6, R10.3). FLOAT64 is refused, since the engines keep binary32. A
SPARSE or open column, two columns, UNIQUE and NULLS NOT DISTINCT are
refused with -900. A vector method makes no UNIQUE index, as vectors
have no `=` (R3.11). There is no CREATE UNIQUE INDEX statement, so the
refusal is reached by a UNIQUE constraint whose
TABLES.UNIQUE_INDEX_METHOD names a vector method, and the method layer
refuses as the one layer every path reaches. A null is not indexed
(R10.6).

The metric comes from the database's registry by name (D93), an unknown
one -103. A graph index (diskann, hnsw) takes only a SYMMETRIC,
NON_NEGATIVE metric (R8.2), so DOT is refused with -900 unless the
column is declared NORMALIZED, when a graph of DOT or COSINE measures by
EUCLIDEAN. ivf is no graph and takes DOT. A built-in metric is measured
by the vector kernels on f32 slices. A registered metric gets the
vectors packed in the column's own format, and a failure or null is
taken as farthest, so a broken metric misplaces entries but never fails
a write.

A metric measured by norms (COSINE) is measured by the norms the engines
keep. Each engine stores the squared norm of every vector it holds, of
the vector as a search reads it (a FLOAT16 record's or a code's decoded
values), and a search works out its query's once. A distance then sums
`q·v` alone (`adapt::normed`, DiskANN's `Metric::normed`, HNSW's and
IVF's `Metric::between`). Each norm is summed as `kernels::squared_norm`
sums it, the order the one-pass COSINE sums in, so the distances are bit
for bit those of summing both norms at every call. Normalizing vectors
as they are written would save the norms but not keep the distances or
the vectors' identity, so the norms are kept instead. Any other metric
ignores them.

Keys are projected (D85). Every vector projects to one constant key, a
null to itself, as the engines keep no original values to return. The
executor rechecks each candidate's row, then measures the exact distance
itself (D94). Entries are counted (D29) by the engines. A search answers
each row once, at its nearest entry.

Each method declares TARGET_ACCURACY (1 to 100, default 90, changed in
place), PQ_BYTES (0 to 65535, 0 for the quantizer's default, rebuilt)
and NORMALIZED (ON or OFF, default OFF, rebuilt) in its own option group
(`index/vector/options.rs`). The query's SEARCH_LIST, BEAM_WIDTH and
PROBES win over the index's own options. TARGET ACCURACY a maps to a
list of wanted × 100 / (101 − a), and to probes of lists × 100 / ((101 −
a) × 80), capped at the list count (`index/vector/ask.rs`). A search
whose query names no accuracy and no list aims at the index's
TARGET_ACCURACY, unless the method's own SEARCH_LIST, EF_SEARCH or
PROBES is set, which it then searches by. NORMALIZED ON refuses a
vector whose norm is not 1 as it is written, SQLSTATE 22000, SQLCODE
-301.

A `RowFilter` is called with no lock held (R-ERR-4). HNSW and IVF call
it from their walk. DiskANN's engine filter must be `Fn + Sync`, so the
method asks the filter after each search instead, doubling the list
while fewer rows than asked are admitted, as the engine's own filter
would. Each row is asked once, and the filter's first error ends the
search.

CREATE INDEX on a table while writers go on fills the index one insert
at a time (`build_index`, `fill_index`), since `Index::load`'s contract
forbids concurrent writers. The locked path (`schema.rs`) and recovery's
rebuild call `load_index`, which runs each engine's bulk build.

The locator is version 1, the column's element code, its dimensions as
u32 and the meta page as u64. Another version, element or dimension
count is -916. Dropping an index marks it discarded, and the last
reference let go frees its pages. The method's checkpoint flushes every
live index it made, then syncs its file, so a database reopens with no
rebuild (R14.6). ALTER INDEX SET of an option that needs no rebuild
retunes in place.

#### D96. A DiskANN index keeps its graph in pages and its codes in budgeted memory

The graph's nodes are pages of the method's file (`index/diskann.rs`,
`engine/diskann.rs`). Its PQ codes and the nodes within CACHE_HOPS of
the medoid are held in memory reserved of the budget (D38). A bulk build
is Vamana's two passes (the DiskANN paper, Subramanya et al., NeurIPS
2019, §2.3), merged from overlapping clusters past BUILD_MEMORY [§3.1].
An index filled by inserts grows by FreshDiskANN. Maintenance trains its
codes at TRAIN_AT nodes and consolidates once CONSOLIDATE_PERCENT of its
nodes are dead. A search's list grows by the dead share first. The
codebook is borrowed per query (`Codes::table`), so a database
quantizer's own distance table serves. A checkpoint writes the meta
page, and recovery mends the index from the rows redone (D25).

A search gives each node it expands the full-precision distance its read
brought, in place of its code's, in the candidate list
(`Frontier::settle`), so the list is ordered and cut by exact distances
wherever they are known. With PQ_BYTES 0 the built-in PQ's codes of a
DiskANN index are half the dimensions (`DIMS_PER_CODE_BYTE`), still an
eighth of a FLOAT32 vector, as a byte for each four dimensions lost
most of its recall on clustered data. An index grown by inserts prunes a
list that a back edge takes past R to R × 10 / 13 (`fresh.rs`,
`SLACK`), so the next back edges are appended with no prune; load's
Vamana and consolidation still prune to R. Maintenance makes its
hot-node cache again each time its live nodes double, and right after it
trains its codes. `DiskAnnMethod::in_memory` pools its pages in a
database's default pool (`db::POOL_BYTES`), as its searches read a page
at every hop. RERANK ON gives the executor the whole search list to
order again, OFF the rows asked.

A node record keeps its vector's squared norm in its header, between the
version count and the vector, worked out from the stored vector as it
is encoded (`node.rs`). A search measures each node it expands by that
norm and its query's, worked out once in `walk_with`. Vamana's build
and FreshDiskANN's prunes measure vectors held in arrays, by `distance`.

#### D97. An HNSW index is held in budgeted memory, kept as an image and a log in pages

The graph is held in memory reserved of the budget (R10.7, D38,
`engine/hnsw.rs`). Each change is appended to a log in pages as it is
made, and a checkpoint folds the log into a new image, so reopening
needs no rebuild (R14.6). The metric, codec and filter run outside the
graph's locks (R-ERR-4).

Maintenance repairs a copy of the graph with no lock held and swaps it
in only if the graph had no change meanwhile. A swap lost to writers is
counted, and after `MOST_LOST` (2) in a row maintenance raises a fence
(`engine/hnsw/fence.rs`). Writers arriving wait at it, and it waits for
those already writing, then copies, repairs and swaps with no writer
between, so it makes progress under constant writes. A swap also waits
for no writer past the fence, so an insert links its back edges in the
epoch it published its node in. A back edge whose list another writer
keeps changing is chosen again until it is written, never dropped, as
it is the way searches reach the new node. Each try lost is another
writer's change made, so the writers as a whole make progress.

Beside each node's vector or code the graph keeps the squared norm of
the vector a search reads, worked out as a node is added and again when
maintenance codes the store (`Graph::recode`). The image keeps no norms,
as a load adds its nodes one by one. A hop gives its callback each
neighbour's vector with its norm, and a search or an insert's search
measures by those and a norm of its query worked out once.

#### D98. An IVF index writes its lists through, its centroids and directory held in budgeted memory

Lists are pages written through as each change is made
(`engine/ivf.rs`). Only centroids, the lists' directory and an entry map
are in memory reserved of the budget (D38). A checkpoint writes the
directory records, then syncs. IVF.LISTS 0 is the square root of the
rows, at least 16 (§10.4), and IVF.PROBES 0 scans one list in eight.
IVF takes DOT, since it is no graph, and no metric property is checked
for it.

An entry keeps, after its row, count and hash, the squared norm of the
vector a scan reads, its code's decoded if it is coded, so a scan
measures each entry by it and the query's, worked out once a search.
Retraining, which stores each entry again, works it out again.

#### D99. The inverted index answers a sparse DOT approximately

`index/sparse/nearest.rs` serves `NearestSupport::Approximate` for a
call of DOT, asked approximately and ascending, of one column of SPARSE
layout whose type fixes its dimension count and format (R10.3). The
distance of a candidate is its dot product negated, as the built-in DOT
measures a sparse vector. A radius r keeps the rows of a dot product at
least −r. The query's filter is the engine's filter, asked of a row's
projected key. A query vector of other dimensions or format, a null, or
another metric is declined, and the executor reads the table, so a call
that would raise still raises. A -918 from the engine declines too.

Keys are projected to one constant key, as the dense indexes' are (D85,
D95). The engine's hits carry no hash of their vector, so a hash
projection could not be checked against them. The method's `metric` is
DOT. It is among `vector_methods`, so it is registered on files too.

#### D100. A hybrid index is a full-text part and a vector part, and HYBRID_SEARCH the query of their two lists fused

The method `hybrid` (`index/vector/hybrid.rs`) indexes two columns, a
text and a vector. It holds a full-text method and the four vector
methods, and makes each index of two parts, a full-text index of the
first column and an index of the second of the method
HYBRID.VECTOR_METHOD names, HNSW by default (R10.2). Each part's
descriptor is the index's, of its one column and of the options of its
method's group alone (FULLTEXT., HNSW., IVF., DISKANN., INVERTED.). The
method declares the union of the options. The locator is a version
byte, the text part's locator's length as a u32, then the two locators.
Another version or a cut locator is -916.

The index holds keys with a null. `IndexSpec.nulls` is true for it, and
`with_derived` sets it on reopen. Each part holds its own column's value
and neither a null, so a document with no embedding is found by its
words and one with no text by its vector. A change goes to the text
part, then the vector part. If the vector part fails, the text part's
change is taken back, so a failure leaves neither (D19). `load` streams
the text part's bulk load and inserts each vector as it passes. Keys
project to one constant key (D85), since a search of one part cannot
know the other part's key of a row. `rows` of the projected key and
`search(Probe::All)` give each row once, the text part's first.

The planner (`physical/nearest.rs`) asks an index of several columns of
the column's position in its key, carried in `NearestRead::position`.
The hybrid method answers a query of position 0 as the full-text method
does, a certified top-k of RANK (D87), and of position 1 as its vector
method does, approximately (D94). RANK of the first column of a hybrid
index is given the index's name, as of a full-text index
(`binder/appended.rs`), so its statistics are the text part's. CREATE
HYBRID VECTOR INDEX (`parser/ddl/hybrid_index.rs`) desugars to CREATE
INDEX ... USING hybrid. Temporary tables refuse the method, as they
refuse `inverted`. The text part serves CONTAINS of the text column,
asked of the full-text method of that column alone, each row found under
the projected key.

An index of one text column and a MODEL embeds its text
(`db/index_statements/hybrid_embedded.rs`). The parser keeps MODEL,
CHUNKER and MODE as the options HYBRID.MODEL, HYBRID.CHUNKER and
HYBRID.MODE. §11.1 says the index stores "a full-text index on body,
the chunks, their embeddings and an ANN index over the embeddings", so
it is four objects. The hybrid index `i` is of the text column alone,
its full-text part and nothing else, so CONTAINS and RANK of the text
read it a document a row. A table i_CHUNKS (DOC_ID, CHUNK_ID,
CHUNK_TEXT, EMBEDDING) holds the chunks. An index i_VECTORS of the
vector part's method and options orders the embeddings. A vector
pipeline i_PIPELINE (D102) keeps the table in step with the text.

An index statement runs outside a transaction and its steps cannot
share one, so they are ordered. `i` is made first with HYBRID.STATE
BUILDING, then the table, the vector index, and the pipeline, which
fills the table, each committed, then `i` is set READY. DROP INDEX sets
DROPPING, then drops the pipeline, the vector index, the table and `i`.
A failed step drops what was made. A database that opens to serve drops
every such index not READY and what it keeps, before it serves
(`hybrid_settled.rs`). HYBRID.STATE is the index's own, and a statement
that gives it is -103. The helper statements run in a session of the
same user, as a busy session refuses another statement.

ALTER INDEX of MODEL, CHUNKER, MODE or VECTOR_METHOD is a rebuild, as
§10.6's REBUILD PARAMETERS is (`hybrid_altered.rs`). The model is
checked to be there and the chunking to parse before anything changes.
Then `i` is set BUILDING with the new options, what it keeps is dropped
and made again, the text embedded again, and `i` set READY. A vector
part's other option goes to i_VECTORS too, and REBUILD rebuilds it.
Two columns and a MODEL, a CHUNKER or MODE of no MODEL, a MODEL taken
away, a table whose primary key is not one column, and a name of a
helper already taken are -103. One column and no MODEL is -900.

HYBRID_SEARCH (`parser/query/hybrid_search.rs`) is a table reference of
its own. The binder (`binder/hybrid_search.rs`) writes the query it
stands for from the catalog and reads it as a derived table. The vector list is
FETCH APPROX FIRST CANDIDATES of VECTOR_DISTANCE of the index's metric,
hinted to the index. The text list is FETCH FIRST CANDIDATES of RANK,
DESC NULLS LAST, with a rank above 0. Each list numbers its rows by
ROW_NUMBER and has MIN and MAX OVER () of its scores. A FULL JOIN on the
primary key fuses them, ordered by SCORE DESC then DOC_ID. RRF is the
sum over the lists holding a row of weight / (RRF_K + rank). WEIGHTED
is the sum of weight × the score normalized min-max over the list's
candidates, 1 if they all score alike, a vector score being 1 −
distance. CANDIDATES defaults to TOP_N × 4 (R11.1), RRF_K to 60, each
weight to 1. The query is read again with the statement's dynamic
parameters before the first value argument counted, so each `?` keeps
its number.

Of an index that embeds its text, the text list ranks the documents
through `i`, DOC_ID being the table's key, and the vector list reads
i_CHUNKS through i_VECTORS. Each list keeps a document once, the vector
list at its nearest chunk, by ROW_NUMBER over the document. A document
of the text list alone is given its chunk ranked highest for the words
(R11.2). QUERY_VECTOR may then be left out, and the vector searched for
is EMBED of the pipeline's model, QUERY_TEXT cast to CHARACTER VARYING
and 'query' (§11.2). Of an index of no MODEL, a missing QUERY_VECTOR is
-103.

The binder notes which argument each copy of a value argument in the
query is, as QUERY_TEXT may be written three times. The parser numbers
the k-th dynamic parameter of the query by a table of numbers built
from those copies, each copy's taking its argument's numbers in the
statement, so a parameter read twice is one parameter.

#### D101. Embedding models are schema objects bound to registered plug-ins

An embedding model is a schema object (`catalog/embedding.rs`), made by
CREATE EMBEDDING MODEL and bound to the plug-in (`Embedder`) its TYPE
option names, which the host registers (`Database::register_embedder`).
The database reads VERSION, BATCH_SIZE, ON_ERROR, MAX_RETRIES and the
shape options itself (`vector/embed/bind.rs`). The others go to
`Embedder::configured`, with FROM FILE's path as the option FILE.
CREATE and ALTER refuse a plug-in not registered. HASH_DENSE and
HASH_SPARSE are built in (`vector/embed/hashed.rs`), feature hashing
that gives equal vectors of equal texts on every machine. The database
has no networked plug-in of its own, so a host registers one.

The session appends `SHAPE = 'dims ELEMENT LAYOUT'` to the CREATE text
it logs. Recovery re-parses that text before the host registers any
plug-in, so a model of an unregistered plug-in is still defined, of its
shape, bound to a stand-in that refuses each call with VEC-0030, SQLCODE
-103. A table whose generated column calls the model is defined again
with it. A binding is cached by the model's description and whether its
plug-in is registered, so ALTER or a late registration binds again.

EMBED, VECTOR_EMBEDDING and EMBED_VERSION whose first argument is a
literal or bare name naming a model of the catalog bind in the binder
(`binder/embedding.rs`), which checks EXECUTE, and give VECTOR of the
model's shape (R12.1). A call naming no model calls the plug-in its
argument names. A literal that names neither is refused, so what named
a model dropped is found by binding it again. EXECUTE on a model is the
schema owner's, grantably, granted and revoked as USAGE on a sequence
generator is (`db/embedding/grants.rs`). DROP EMBEDDING MODEL RESTRICT
is refused while a pipeline or a generated column embeds by the model,
and CASCADE drops them.

#### D102. A vector pipeline is four row triggers on its source

A vector pipeline (CREATE VECTOR PIPELINE, §12.5) is four AFTER row
triggers on its source, made with it and named after it (`_INSERT`,
`_DELETE`, `_UPDATE_OLD`, `_UPDATE_NEW`), whose actions insert
CHUNK_TEXT's chunks into the target and delete them
(`db/embedding/pipeline.rs`). The target exists already, and is filled
in the creating transaction. Triggers were chosen over a new write hook
so that pipelines inherit statement atomicity, MERGE, cascaded changes,
recovery and privileges from triggers unchanged. MODE SYNC embeds a
chunk as it is inserted, with the model's version tag if the target
names a version column (R12.5). MODE ASYNC leaves both null for the
queue (D103). DROP of the pipeline drops the triggers and keeps the
target's rows. DROP TABLE of source or target and DROP EMBEDDING MODEL
drop it only with CASCADE.

#### D103. What waits for an embedding is found in the rows, filled after commit

What waits for an embedding is found in the rows themselves, never in a
separate queue table (`db/embedding/queue.rs`). It is a null embedding
of an ASYNC pipeline, a version column whose tag is not its model's, or
a null ASYNC generated column whose inputs are not null. It therefore
survives a crash with no log of its own. A round is one UPDATE per
column, in a transaction of the table schema's owner, run by the job
`turborel-embedding` after a commit that wrote and as the database
opens. Before it, the texts waiting are embedded ahead, BATCH_SIZE at a
time (R12.3), up to `MOST_PRIMED` (4096), so the UPDATE asks the model
for none of them. A round failing leaves its rows waiting for the next,
and INFORMATION_SCHEMA.EMBEDDING_QUEUE shows its error. A statement
whose text begins with `/*+ VECTOR_WAIT_FRESH */` (D105), run outside a
transaction, and `Database::wait_for_embeddings` run a round first.

A generation clause may end `[STORED] [SYNC | ASYNC]` (§12.4). An ASYNC
column, kept in the catalog codec as generation tag 6, is the null value
on INSERT (`binder/generated.rs`, `written_generations`). On UPDATE and
MERGE's update it is the null value if a column it reads is set, kept
otherwise, and computed if it is set to DEFAULT. The queue sets it to
DEFAULT later. The writing statement therefore never waits on a model,
and D63's computing of generated columns is unchanged for SYNC.

#### D104. A scalar function may take a batch's rows at once

`ScalarFunction` has `batches()` and `call_many()`. The batch projection
(`exec/batch/project.rs`) gives a function that batches the argument
rows of a whole batch at once, and falls back to a row at a time if the
batched call fails, so an error is raised at the row that causes it
(D15). EMBED of a model of the catalog batches, asking the plug-in
BATCH_SIZE texts at a time (R12.3).

#### D105. Bracketed comments are separators, and a leading `/*+` one carries hints

Bracketed comments [ISO/IEC 9075-2:2023 §5.2 SR11, SR12] (Feature T351)
are separators and nest (`lexer/comments.rs`). A statement whose text
begins with one whose contents begin with `+` is hinted by its words,
read by `lexer::hinted`. VECTOR_WAIT_FRESH is the one hint read there
(D103). An unknown hint is a comment alone, as hints never fail a
statement. The approximate search's hints are written after the fetch
clause instead (D94), where the parser reads them as syntax.

#### D106. XML values are trees of the data model, read by a parser of our own

SQL/XML (ISO/IEC 9075-14) makes an XML value an XQuery sequence. Its
nodes come from parsing text (§10.16) or from constructors. `xml::parse`
reads XML 1.0 Third Edition and XML 1.1, as Namespaces in XML 1.0 and 1.1
modify them. It is written here, as D3 requires.

The parser does not validate. It reads the internal DTD subset, expands
its entities and applies its attribute defaults. It reads no external
subset and no external entity, so no parse opens a file or a URL. A
reference that would need such an entity, or a declaration not read, is a
data exception, since the data model has every entity expanded. Binary
strings are decoded from UTF-8, UTF-16, UTF-16BE, UTF-16LE, ISO-8859-1 or
US-ASCII; any other encoding is refused.

A tree (`xml::Tree`) holds its nodes in document order, an element's
attributes right after it. Each node links to its parent, first child and
next sibling, so a node's index is its place in document order. Element
and attribute names are kept once per tree and nodes refer to them by
number, the tokenization Pal et al. describe. All text is in one buffer.
Namespace declarations are kept on the element that makes them, as the
data model allows.

Nothing recurses. Entities are read on a stack of texts, elements on a
stack of open elements, content models on a stack of groups. These bounds
keep each finite, and a value past one is SQLCODE -900 (D18):

- `MAX_NESTING`, 512: how deep elements, and content-model groups, nest.
- `MAX_TEXT`, 16 MiB: a tree's text, and what entities' replacement texts
  add to a parse, so nested entities cannot expand without end.
- `MAX_NODES`, 1,048,576: a tree's nodes and namespace bindings, as a text
  of little but markup makes many nodes of little text.
- `MAX_INPUT`, 64 MiB: the text XMLPARSE reads, a large object's whole.
- `MAX_STORED`, 64 MiB: a value's stored form, read whole when its nodes
  are needed.

An XML value is a sequence (`xml::Sequence`) of items, each a node or an
atomic value. A node is its tree, shared, and its place in it, so the
nodes one tree gives many values keep their identity; storing a value
copies it. An atomic value is its type and the string casting it to
`xs:string` gives, one string for each value, and of a qualified name
its namespace URI, its prefix in the string, and of a value of a
user-defined atomic type that type's name (below). Its type is a built-in
atomic type of XML Schema Part 2, primitive or derived, or one of the
three XQuery adds, `xdt:untypedAtomic` and the two durations; a table of
the 44 holds each one's name, base type, the primitive type a cast
treats it as, and the bounds or name production it fixes. A value is read
from its string when compared: a decimal as its digits, of any number, a
float or double, a duration as months and microseconds, a date or time as
its local value and time zone. Equality and the orders are XML Schema's,
the partial orders of durations and of dates and times giving no order
where it is indeterminate. `xml::cast` casts among the types as XQuery
does, checking the facets a derived type fixes, years kept to 0001-9999
and seconds to the microsecond, as SQL's are, decimals of any digits. A
type added to the stored form takes a tag after those it had, so a value
stored before reads as it did. `xml::serialize` writes
a value by the XML output method of XSLT 2.0 and XQuery 1.0 Serialization,
after its sequence normalization. It writes no CDATA section, document
type declaration or indentation, and writes as character references what a
parse would otherwise change.

A simple type (`xml::SimpleType`) is a built-in type, a list, a union, or
a restriction of one, holding its facets merged with its base's as it is
made, so a validation never walks its derivation. Each pattern is
compiled once, as `regex` reads XML Schema's (D61). Validating a literal
normalizes its white space, matches each derivation step's patterns,
reads its value, then checks each other facet; a literal or value not
valid is err:FORG0001, as a cast reports it. Lists and unions nest at
most `MAX_TYPE_NESTING`, 32, deep (D18). XMLCAST of an XML value to an
SQL type casts as the restricted type §9.5 maps the SQL type to, made
once as the statement is bound.

A value's stored form (`xml::encode`, version 1) is binary and read in one
pass, in document order, as Pal et al. store XML. Each node item has a
table of its names, each once, and of its type annotations, each once,
then its nodes as events: document, element (with its annotation, its
nilled, is-id and is-idrefs properties, its bindings and attributes),
end, text, comment and processing instruction; each atomic item its
type's tag and its lexical form, of a QName or NOTATION its namespace
URI, and of a user-defined atomic type that type's name. A typing's
atomic types, of a typed value's items, name their user-defined types
too, from the item's table of names. Its fields are D30's. The
annotations, those of atomic values too, were added to version 1 in
place, as formats stay at version 1 in alpha and no database of an
earlier layout is read. A node below the root of
its tree is stored as a copy, an element declaring every binding in its
scope. The XML indexes read it, below.

The XML type's values are `Value::Xml` (`value::XmlValue`), held in one
of three forms: a sequence; a binary large object of the stored form, as a
table keeps it; or its serialization, as the protocol carries it. Each is
turned into another only when needed. A table keeps the stored form as it
keeps a BLOB (D79): in the record when short, else as its own large
objects, logged chunk by chunk, so recovery and replication need nothing
of their own. A value is held whole when read, at most `MAX_STORED`, the
one place XML departs from D79, as a value must be a tree to be read. A
value inside an array, a multiset, a row or a structured value is held
whole in its row's record, as D79 holds a large object there, at most
`MAX_STORED` octets of stored form.

XQuery 1.0, the text XMLQUERY evaluates, is parsed by `xquery::parse`,
written here as D3 requires, by the lexical rules of XML 1.1 or 1.0
(XMLQUERY reads by XML 1.1's, as turborel has Feature X211). Its lexer
reads tokens as the parser asks for them, so each part of a query is
read in the state the grammar puts it in: whitespace and nested comments
separate the tokens of expressions, where a keyword is a name the parser
recognizes by what follows it; a direct constructor's tags, attribute
values and content are read character by character, and each enclosed
expression as tokens again. The parser is recursive descent, one method
a production, and makes a tree of the grammar's productions that writes
itself back as text parsing to the same tree. A chain of operators of
one precedence is one node, so a long chain makes no deep tree. The
parser raises what the text alone shows, as SQLSTATE 10000, XQuery
error, naming the XQuery error code: syntax errors (err:XPST0003), and
the static errors of the prolog and of direct constructors' attributes
that need no static context. Imports are analysed with the static
context: a schema import (`xquery/analyze/import.rs`) and a module
import (`xquery/analyze/modules.rs`).

The parser recurses, so its depth is bounded (R-CF-6): each
`ExprSingle`, each direct element constructor and each operator's right
operand of a higher precedence counts one level, and a query deeper than
`xquery::MAX_NESTING`, 64, is SQLCODE -900 (D18). Every walk of the tree
it makes is bounded with it. The functions the recursion passes through
hold only what it needs, so the deepest query of each shape parses, is
written back and is dropped within half of a thread's default stack, as
a test checks.

A query is analysed once, as its statement is bound (`xquery::Query`),
into a plan (`xquery/plan.rs`) that its evaluations share and none
changes. The analysis makes the static context of XQuery's Appendix C.1
and §10.20 of Part 14, the namespaces the binder holds in scope among it,
and resolves every name of the tree: each QName to an expanded QName,
each variable to a slot of its frame or a global, each function call to a
function of the prolog, a constructor function's cast, or a function of
the library. A cast or constructor function to `xs:QName`, of a string
literal alone, is evaluated there, its prefix resolved by the statically
known namespaces. A static error is raised there; no static typing is
done, so a type error is raised as the query is evaluated. The library
(`xquery::Library`) is the built-in functions and those plug-ins
register (`Database::register_xquery_function`), each an
`XQueryFunction` of its expanded QName, parameter types and result type,
called with its arguments converted by the function conversion rules and
its dynamic context (`xquery::Context`): the focus, the current dateTime
and implicit timezone, the collations and the base URI. A prolog's
`external` function is the library's of its name and arity. Collations
are the codepoint collation and the database's (D60), each reached by its
URI.

The evaluation holds a sequence as a vector of `xml::Item`s, a node its
tree, shared, and its place, so a node keeps its identity; a variable's
value is held once and shared by every reference to it. The focus, the
context item, position and size, is passed down to each operation, and
the variables of a body are the slots of its frame; a prolog's variables
are evaluated once, as first read. Document order within a tree is the
order of its nodes; between trees it is by where each tree is held,
stable while any node of it is held (`xml::Node::document_order`). A
constructor builds its element in a tree of its own with `Builder`, a
direct element nested in it built in place, each node of an enclosed
expression copied by the construction and copy-namespaces modes. In
construction mode preserve, the default, the element a constructor
makes is annotated `xs:anyType` (`xml::Annotation::any_type`), as
XMLELEMENT's is, which evaluates `element { $QNAME } { $SEQ }` in that
default context; in strip it is `xdt:untyped`.

The evaluation recurses as the plan nests and as functions call functions,
so it counts each operation evaluated within another, each FLWOR clause
and each call, and is SQLCODE -900 past `xquery::MAX_DEPTH`, 256: the
deepest evaluation of each shape a test tries runs within half of a
thread's default stack. A sequence holds `xml::MAX_NODES` items at most,
a string `xml::MAX_TEXT` octets, and the trees one evaluation constructs
`xml::MAX_STORED` octets together, each past it SQLCODE -900. Each item
of a FLWOR expression, a path step, a predicate and a quantified
expression, and each call, looks whether the statement was cancelled
(D44), through `ScalarFunction::call_in_statement`, which a statement
calls of a function that is `in_statement`.

XMLQUERY and XMLEXISTS are each a call of a function holding the
compiled query, their context item and variables its arguments, an SQL
value cast to XML(SEQUENCE) by XMLCAST. XMLTABLE is a collection derived
table, as UNNEST makes, of one collection: an array of rows, a row of each
item of its row pattern's value, each column's value the XMLCAST of its
own compiled pattern's value, which the function evaluates with the cast
the binder made of it.

The functions and operators of XQuery 1.0 Functions and Operators, the
Candidate Recommendation of 3 November 2005, are a library
(`xquery::library`) over sequences and atomic values, one file for each
of its sections. Each `fn:` function is a row of its section's table, its
local name, the numbers of arguments it takes and its body, and the
library's built-in functions (`xquery::Library::builtin`) are those rows,
an `XQueryFunction` of each arity, so each function has one
implementation, which a query and a test call alike. A body is called
with the call's `xquery::Context`: the context item, position and size,
the current dateTime and implicit timezone, the default collation and
those known, the base URI, and the statement's cancellation. A function
applies the function conversion rules of its own signature to its
arguments, atomizing them, casting untyped values and promoting numbers
and URIs, so its parameters are declared `item()*` and the evaluation
converts nothing before it. A constructor function of an atomic type is
a cast, as the analysis makes it. Each `op:` operator is a plain
function of `library::op`, named after it: the evaluation of XQuery's
arithmetic, comparisons, ranges, node comparisons and union, intersect
and except calls them by the operator mapping, and the evaluation's
atomization and effective boolean value are the library's. Its errors
are XQuery errors (`xquery::ErrorCode`), SQLSTATE 10000, the code of the
text's Appendix C in the message.

An `xs:decimal` or `xs:integer` is reckoned by a decimal of digits of
the library's own, bounded at `MAX_DIGITS`, 1,024, past which an
operation overflows, err:FOAR0002; a quotient of decimals is exact if
it ends within 18 digits after the period, or the scale of either
operand, and is truncated there otherwise. A float's arithmetic is
rounded to single precision, an overflow an infinity. No function
recurses on a tree: fn:deep-equal compares trees from a list of the
pairs of nodes left. A string or sequence a function would make past
`MAX_TEXT` or `MAX_NODES` is SQLCODE -900. fn:distinct-values, whose
work grows as the square of its values, looks whether the statement was
cancelled at each value (D44).

A collation is one of the evaluation's (`xquery::Collator`), named by
the code point collation's URI or by `urn:turborel:collation:` and the
name of a collation of D60; strings compare by sort keys with no
padding. Substring matching is by collation units
(`Collation::units`): characters, of the code point collation and of
the collations of one character at a time; of UNICODE and its
tailorings each character their steps begin at whose normalized form
begins with a starter, and what follows it up to the next, so a
contraction, or a character and its marks, is one unit. A unit is
weighed at every level as the whole string weighs it, each collation
element's weights its unit's, so the units' weights, joined, are the
string's and a match never depends on where the string is cut. A
minimal match is a run of the text's units whose weights at each level,
joined, are the sought string's, from a unit of some weight to another,
found unit by unit, each level's weights so far a beginning of the
sought's; a unit of no weight at any level is ignorable.

Normalization and full case mapping read a table of Unicode 13.0, the
version of the collations and regular expressions, which `cargo xtask
unicode` makes (`xquery/library/unicode13.bin`); Hangul syllables are
composed by arithmetic. The fully normalized form [XML 1.1 Appendix B]
of a string, its one relevant construct, is its NFC, and a string whose
NFC begins with a composing character has none (err:FOCH0003); the
composing characters are read from the same table, the marks by their
class and the others as second in a primary composite's pair.
fn:matches, fn:replace and fn:tokenize use D61's matcher in XQuery
1.0's dialect.

A date or time of no time zone is compared and subtracted in the
implicit time zone; the current dateTime is the statement's instant
(D54) in it. No function reads an external resource, as the parser
reads none: the available documents and collections are empty and there
is no default collection. The value comparison of two atomic values
that fn:index-of, fn:distinct-values, fn:deep-equal, fn:max and fn:min
compare by is `library::value_equal` and `value_order`, and a value
comparison's `library::value_comparison`, which raises err:XPTY0004 of
types the operator mapping does not compare.

XML Schema Part 1, Structures, is `xml::schema`, written here as D3
requires. A schema is read from schema documents (`schema::read`) into an
arena of its components, each kind in a table of its own, components
referring to others by their place, so the cycles a schema has (an
element of a type whose content holds the element) are indexes. A named
component takes its place when first referred to and is filled in when its
properties are needed, so a reference may come before what it names; a
reference that names nothing is refused as the schema is read, so a
schema read is complete and no validation meets a missing component. A
document is checked against the structure the Schema for Schemas gives
each element, kept as a table of the attributes each may have and the
stages its children come in, not by validating it against the Schema for
Schemas. `<include>`, `<redefine>` and `<import>` find documents only in
the `Library` the schema is read with, by schema location, or for an
import by namespace, so reading a schema opens no file and no URL. Each
content model is compiled to a deterministic automaton (`Automaton`):
Glushkov's construction, numeric occurrence ranges unfolded into copies
nested so they add no more follow edges than a sequence, then the subset
construction with transitions labelled by particle, which is how Unique
Particle Attribution is checked, as [Schema1] Appendix H describes. An all
group is its particles, each taken at most once. A simple type definition
of a schema document is read whole when first named, as it names no type
but simple ones, so one named while it is read is circular. It holds the
`xml::SimpleType` it defines by restriction, list or union, which keeps
the rules of its facets as it is made, so a string is validated against a
defined type as against a built-in one; the reader checks each {final}. A
node of a union type is annotated with the union, its typed value read as
the kind of the atomic values validating it gave, as a union's member
varies from node to node. These bounds keep reading finite, each SQLCODE
-900 past it (D18):

- `schema::MAX_DEPTH`, 64: how deeply declarations, groups and types
  nest where reading recurses, the deepest schema of each shape read
  within half of a thread's default stack, as a test checks. A complex
  type's base is read before it, so a derivation's steps nest no more
  than reading a type does.
- `schema::MAX_CHAIN`, 64: the types of a derivation, or the declarations
  of a substitution group, a walk up one follows; and the simple type
  definitions read within one another, each naming the next, read within
  half of a thread's default stack, as a test checks.
- `schema::MAX_DOCUMENTS`, 256: the documents one schema is read from.
- `schema::MAX_POSITIONS`, 4,096: the particles of a content model once
  its occurrence ranges are unfolded.
- `schema::MAX_STATES`, 16,384: the states of a content model's automaton.

A node is validated (`schema::validate`) as XQuery's validate expression
validates it, the way SQL/XML's XMLVALIDATE and IS VALID do: a new tree is
built as the old one is walked in document order, elements on a stack of
those open, each with its content model's state, so nothing recurses and
the depth validated is the tree's. The first failure ends it, naming the
validation rule broken, as any failure makes the validation root invalid.
The new tree's elements and attributes carry their types' annotations
(`xml::Annotation`), each kept once in a table of the tree, and the
nilled, is-id and is-idrefs properties; a node's typed value is read from
its string value as its annotation says, as the data model lets an
implementation keeping string values do, so the tree holds no typed
values beside its text. An element of simple content has one text node,
its schema normalized value, before its comments and processing
instructions; a default attribute or element value is added as text.
Identity constraints and the ID/IDREF table are checked over the new tree
once it is built, of a document node alone, as XQuery asks. A copy keeps
the annotations, as construction mode preserve does. `schema::validate`
returns a `Failure` naming the rule and XQuery's error code apart from
turborel's own errors, so XMLVALIDATE raises SQLSTATE 2201R and XQuery's
validate expression err:XQDY0027 from one result. XQuery atomizes a node
to its typed value, and matches its annotation in a node test: a
built-in type's by the built-in hierarchy, a registered XML Schema's by
that schema's Type Derivation OK if a schema import brought it into the
in-scope schema types, its name, an anonymous type's too, resolved in the
schema imported (`xml::schema::annotated_type`), else as derived from
`xs:anyType` alone. A copy made by construction mode preserve keeps
the annotations; one made of an unvalidated document node (§10.19), as
XMLDOCUMENT of XML(CONTENT(UNTYPED)) and CAST BY VALUE to an UNTYPED type
make it, keeps none (`Copying::UNVALIDATED`).

A registered XML Schema is a catalog object (`catalog::XmlSchemaDesc`):
its name, target namespace, schema location, the text of the document it
was registered with, the registered XML Schemas whose documents it read,
and the USAGE granted on it. The catalog stores those alone; the schema
read from the document is not stored but read again as the catalog is,
each after those it uses, with their documents alone as its library, so
nothing derived can disagree with what was registered. As the catalog is
logged, checkpointed and replicated as opaque bytes (D57), registered XML
Schemas need nothing of their own there. USAGE on one is granted and
revoked by GRANT and REVOKE USAGE ON XML SCHEMA, as on a sequence
generator, Part 14 leaving how to the implementation; the permanently
registered XML Schemas, INFORMATION_SCHEMA.XS, XSI and SQLXML, are
constants every user holds USAGE on. `catalog::xml_schemas::may_use` is
the one check of USAGE every statement naming a registered XML Schema
makes [ISO/IEC 9075-14:2006 draft §11.6 AR1]. The Information Schema's
XML_SCHEMAS, XML_SCHEMA_NAMESPACES and XML_SCHEMA_ELEMENTS read
Definition Schema tables made from the descriptors and the schemas read
from their documents, as D37 makes every such table, and their short
name views, XML_SCHEMAS_S, XML_SCH_NSPACES_S and XML_SCH_ELEMENTS_S,
read them as D37's do; a type of XMLSCHEMA
shows its registered XML Schema by its kind's descriptor
(`Descriptor::xml_schema`).

An XML type of XMLSCHEMA (`value::xml::Typing::Schema`) holds what its
descriptor includes, `value::types::XmlSchemaNamed`: the registered XML
Schema's name, as a registered XML Schema is identified by its name
[§4.2.6], and the namespace and element its element clause indicates.
It is held behind an `Arc`, so the type, once `Copy`, is cloned as
cheaply; the name, not the schema, is held, so the type is written,
stored in the catalog and described over a connection as itself, and
reads back the same; a function that validates holds the schema read,
taken from the catalog as its statement is bound. The type is resolved
from what is written (`ast::TypeName::XmlSchema`) as a statement is
processed (`State::indicated`): a URI indicates, of the registered XML
Schemas of its target namespace, and of its location if written, that
the user may use, the one of the least name, the permanently registered
ones last; the namespace and element an element clause indicates must be
the schema's, else 2201M or 2201N as the type is bound. A value of a
type of XMLSCHEMA is one XMLVALIDATE, or CAST BY VALUE, made; one whose
elements are not all typed is refused at such a site with 2201R, so a
value sent as text is not taken for one, while a value a table holds is
taken as it is, as it was checked as it was stored.

XMLVALIDATE and IS VALID are each a function holding how an element finds
its registered XML Schema: the one indicated, or, data-driven, of each
namespace the one the user may use of the least name, chosen as the
statement is bound. Each validates by `schema::validate` in strict mode
the item §10.22 is given: of DOCUMENT the document node, so its identity
constraints and IDs are checked; of CONTENT and SEQUENCE each element
child of a document node alone, so they are not. The permanently
registered XML Schemas validate by their components (below).

XQuery's validate expression, strict or lax, validates by
`schema::validate` against the in-scope schema definitions, which Part 14
makes an implementation-defined collection of registered XML Schemas the
current user may use [ISO/IEC 9075-14:2006 draft §10.20 SR2e]: of each
target namespace the one XMLVALIDATE's data-driven case chooses, chosen
as the statement is bound and held by the compiled query
(`xquery::InScopeSchema`), the permanently registered one of a namespace
no other has. A definition
whose query has a validate expression notes them as it is defined, as it
notes a registered XML Schema it indicates, so its horizon keeps them and
it depends on them; bound again, its in-scope schema definitions are
those it kept, of each namespace the least name, never one registered
later [§4.2.6]. Each is read
alone, and the components of several make no one schema, so the effective
schema of a validation [XQuery §3.13] is the one of the namespace of the
element it validates, with what that one imports; of none, a schema of no
components, by which strict validation is err:XQDY0084 and lax validation
assesses the element by `xs:anyType`.

A prolog's schema import [XQuery §4.10] imports, of the registered XML
Schemas the current user may use (and of a definition bound again, those
it kept), the one of the target namespace whose schema location is a
location hint, else the one of the least name, the permanently
registered ones last (`xquery/analyze/import.rs`); it brings the
components of every namespace the schema has. Their type definitions
are in-scope schema types, a node test's type matched by the schema's
own derivations (`xquery::types::Defined`), and their global element and
attribute declarations those of schema element and attribute tests,
the substitution group a schema element test's element heads matching
too. err:XQST0035 is raised of a name two imported schemas define from
different documents, compared as the schema documents' trees; so
err:XQST0012, of imported schemas not valid together, is never raised, as
each registered XML Schema is valid and what two share is the same. A
simple type of the atomic variety of an imported schema is an atomic
item type [XQuery §2.5.4.2]: of `instance of`, `treat as`, typeswitch
and the sequence types of functions, whose conversion rules cast an
untyped value to it; a cast's target [XQuery §3.12.3], a value cast
to it checked against its facets by the `xml::SimpleType` it holds, a
string read as a literal, its patterns matched first, another value
cast as the built-in type and matched as its canonical form; and a
constructor function's [XQuery §3.12.5]. A list or union type, or a
complex one, is none (err:XPST0051). A definition notes what its
queries import, as what they validate by, and depends on it.

A value of a user-defined atomic type is a value of the built-in type
it restricts, its value space a subset of that type's [Schema2
§2.5.2.1], annotated with
its own type's name (`xml::Atomic::annotation`), so comparison,
arithmetic and the functions take it as a value of the built-in type,
each giving a value of a built-in type, and a cast to that type drops
the annotation [XQuery §3.12.3]. The `xml::SimpleType` of each simple
type definition of a schema holds the definition's name, an anonymous
one's made from its place as a node's annotation is, and annotates
each atomic value it validates or casts with it; so a node's typed
value is of its type [XQueryDM §3.3.1.2], a list's items of the item
type, a union's of the member that validated each, as a node's typing
(`xml::Typing`) records them. A value is matched against an atomic
item type of an imported schema by its annotation, resolved in that
schema as a node's is, and that schema's Type Derivation OK; a value of
a built-in type derives from none of them; and against a built-in type
by the built-in hierarchy of its built-in type.

turborel supports the Module Feature [XQuery §5.2.5]. A library module is
a registered XQuery module (`xquery::LibraryModule`), and a module import
imports one module of its target namespace, the set of modules §4.11
leaves to the implementation: of the main module, among the registered
XQuery modules the current user may use; of a library module, among
those it imported as it was registered (`LibraryModule::modules`), so a
library module imports what it was analysed with, whoever imports it. Of
these, the one whose location, or whose name, alone or qualified by its
schema's, is a location hint, else the one of the least name; a library
module importing its own target namespace imports another module of it.
Each module is analysed in its own static context [§4.11]: the
statement's namespaces and variables are the main module's alone, and a
library module's schema imports and in-scope schema definitions are
those it kept as it was registered (`LibraryModule::importable`,
`schemas`). The functions and variables of every module of a query are
one list each in its plan (`xquery/analyze/linked.rs`), so a call or a
reference is a place in it whichever module declares what it names, and
a module imported by several is analysed once; each declaration keeps its
module, whose construction and copy-namespaces modes, default collation,
base URI and in-scope schema definitions its evaluation reads
(`plan::ModuleContext`). A module imports a library module's own
functions and variables alone, as imports are not transitive. A
namespace imported twice is err:XQST0047; one no module may be imported
of err:XQST0059; a function or variable imported of the name, and of a
function the arity, of one declared or imported err:XQST0034 or
err:XQST0049; a library module's function or variable outside its target
namespace err:XQST0048; an imported declaration of a schema type, a
user-defined atomic type or an element's or attribute's type, of no
schema the importing module imports, by that schema or a type of that
name, err:XQST0036, checked once every module is analysed; and a
circularity through any module's declarations err:XQST0054. A library
module's functions and variables are given their places in the plan
before its prolog is analysed (`Analysis::reserve`), so a module it
imports that imports it back finds them: the cycles of imports of
modules of one namespace §4.11 allows are analysed, each module once,
their functions calling across the cycle as the evaluation's depth
bounds (`MAX_DEPTH`). A cycle of modules not all of one namespace is
err:XQST0073 (`Linked::cycle`). Imports nest at most `MAX_IMPORT_DEPTH`,
16, modules deep, each analysed within the one importing it (R-CF-6); a
chain deeper is SQLCODE -900 (D18). A library module given as a query, or
a main module registered as a library module, is err:XPST0003, as a
query is a main module and a library module is no query [§4].

A registered XQuery module is a catalog object (`catalog::XQueryModuleDesc`):
its name, target namespace and location, the text of the library module
registered, the registered XQuery modules and XML Schemas it imported as
it was registered, and the USAGE granted on it. The catalog stores those
alone; the module's parse is made again from its text as the catalog is
read (`db/snapshot/xquery_modules.rs`), a text that no longer parses, or
a module or schema it depends on that is not registered, a damaged
catalog, SQLCODE -916. As the catalog is logged, checkpointed and
replicated as opaque bytes (D57), registered XQuery modules need nothing
of their own there.

XQuery leaves where a module import finds its modules to the
implementation [§4.11, §5.2.5], and Part 14 gives no statement for it.
These statements were decided, an extension in the standard's notation:

```
<XQuery module definition> ::=
    CREATE XQUERY MODULE <XQuery module name>
    [ LOCATION <XQuery module location> ]
    AS <XQuery module text>

<XQuery module name> ::= <schema qualified name>

<XQuery module location> ::= <character string literal>

<XQuery module text> ::= <character string literal>

<alter XQuery module statement> ::=
    ALTER XQUERY MODULE <XQuery module name>
    [ LOCATION <XQuery module location> ]
    AS <XQuery module text>

<drop XQuery module statement> ::=
    DROP XQUERY MODULE <XQuery module name> <drop behavior>
```

XQUERY is no key word: XQUERY MODULE is read as two words where these
statements take them, so a table may still be named XQUERY. The text is
a library module, analysed as it is registered
(`xquery::library_module`) in a static context of its own, its schema
imports and module imports importing among the registered XML Schemas
and XQuery modules its schema's owner may use, as a registered XML
Schema reads those of its owner (`db/definitions/xquery_modules.rs`);
what it imported is kept in its descriptor, and it depends on that. Its
target namespace is of nonzero length, as a module import names one
[err:XQST0088], else SQLCODE -103; a duplicate name is SQLCODE -103.
Only the owner of its schema registers or drops one. A literal holds no
`<newline>` [X3.135-1986 §5.1 SR1], and XQuery reads any whitespace
between tokens, so the statement is logged as its text, as every
definition is, and replay registers it again from that text.
DROP XQUERY MODULE is a drop record of tag 18, after DROPPED_XML_SCHEMA's
17. RESTRICT is refused while a module imports it; CASCADE drops those,
at any remove (`State::xquery_module_dependents`, the one place what
depends on a module is found), as `Abandoned::xquery_modules`, which
`State::destroy` drops after what imports them. A registered XML Schema
dropped with CASCADE, or abandoned by a REVOKE, takes the modules that
imported it the same way; DROP SCHEMA CASCADE drops the schema's
modules.

A module imports only modules registered, so ALTER XQUERY MODULE
replaces a module's text, and its location if written, analysed again
and its imports found again and recorded, its USAGE kept
(`definitions/xquery_modules/alter.rs`): a module altered imports one
registered after it, so modules of one namespace import one another,
each recorded as depending on the next. A cycle of modules not all of
one namespace is err:XQST0073 as the ALTER closing it is analysed, the
module altered taking its new text wherever the cycle reaches it back.
What depends on the module must still analyse and bind with the new
text, as an ALTER of another object is refused that would break what
depends on it: each module importing it, at any remove, analysed as a
query importing it analyses it, and each view, constraint, assertion,
generated column, trigger and routine keeping it or one of those bound
again; else SQLCODE -103. So a function or variable they use is not
taken away, and a target namespace changes only if nothing imports the
module by it. The walk of what depends on a module ends on a cycle, each
module found once: DROP RESTRICT of a module of a cycle is refused, as
the next depends on it, CASCADE drops the cycle, and a REVOKE abandoning
one abandons the modules of the cycle, as each imports the next. ALTER
is logged as its text and replayed in order; a checkpoint image of
modules that depend on one another loads, as each is parsed alone and
only checked to name modules registered.

USAGE on a registered XQuery module is granted and revoked by GRANT and
REVOKE USAGE ON XQUERY MODULE, as on a registered XML Schema, the grants
kept and revoked by the same functions (`db/usage/xml_schemas.rs`); its
schema's owner holds it with grant option. `catalog::xquery_modules::
may_use` is the one check of it, of the modules a query's main module
may import and of those a module being registered may. A library module
imports what it imported as it was registered, so its importer needs
USAGE on it alone, as a routine's invoker needs EXECUTE on the routine
alone. REVOKE abandons, of each schema whose owner it leaves without
USAGE, the modules that imported it (`State::xquery_module_dependents`),
dropped with CASCADE, RESTRICT refused with 2B000. USAGE_PRIVILEGES shows
each grant, of OBJECT_TYPE XQUERY MODULE, an extension of §21.4's list.
No view of registered XQuery modules is added: Part 14 defines none, and
USAGE_PRIVILEGES already names each module a user may import.

A definition whose queries import a registered XQuery module keeps it,
as it keeps the registered XML Schemas it indicates: the binder notes
each module a main module imports (`Invoked::imported`), the
definition's horizon keeps them (`catalog::Horizon::xquery_modules`,
in the catalog as the rest of the horizon is), and, bound again, its
queries import among those alone (`Horizon::kept`), never one of a
lesser name registered later. A module it imports keeps what that
imports itself. `State::xquery_module_dependents` reads the horizons,
through the walk a registered XML Schema's dependents take
(`State::horizons_naming`): the views, constraints, assertions, triggers
and routines whose queries import a module dropped, or one that imports
it, are dropped by CASCADE and refuse RESTRICT, and a REVOKE abandons
them; a generated column's table refuses even CASCADE, as a column of a
type of XMLSCHEMA does.

Part 14 leaves how an XML Schema is registered to the implementation
[ISO/IEC 9075-14:2006 draft §4.2.6]. These statements were decided, an
extension in the standard's notation:

```
<XML Schema definition> ::=
    CREATE XML SCHEMA <registered XML Schema name>
    [ LOCATION <XML valid schema location URI> ]
    AS <XML Schema document>

<XML Schema document> ::=
    <value expression>

<drop XML Schema statement> ::=
    DROP XML SCHEMA <registered XML Schema name> <drop behavior>
```

`<registered XML Schema name>` is Part 14's `<schema qualified name>`
[§5.2], and `<XML valid schema location URI>` its `<character string
literal>` [§11.6]. A registered XML Schema is read from one document: a
character string, a binary string or an XML value, evaluated once; a
string is parsed as DOCUMENT, and its element is an `xs:schema`. Its
target namespace is the document's targetNamespace, the empty string of
none, as NO NAMESPACE indicates; LOCATION is the empty string if not
written. Its `<include>`, `<redefine>` and `<import>` find documents
among the registered XML Schemas alone, those the owner of its schema may
use (`catalog::xml_schemas::may_use`), as Part 14 asks of a registered
XML Schema an SQL schema statement names [§11.6 AR1a]: by LOCATION, or
for an import by target namespace; nothing is fetched. Those it read are
recorded and are what it depends on. A duplicate name is SQLCODE -103.
Only the owner of its schema registers or drops one, as of a collation,
and the owner may use it.

DROP XML SCHEMA RESTRICT is refused while anything depends on the one
named; CASCADE drops the registered XML Schemas that use it, directly or
through others, and whatever names them. One function,
`State::xml_schema_dependents`, finds what depends on a registered XML
Schema, of the schemas a caller names: the routines of a parameter or
result of a type of XMLSCHEMA of one, the views, CHECKs, domain
constraints, assertions, triggers and routines that indicated one as
they were defined, or had it in scope of a validate expression, and what
invokes such a routine, which CASCADE drops;
and the tables, domains and user-defined types of a column or type of
one, and a table of a generated column that names one or invokes such a
routine, which refuse even CASCADE, as no drop changes a column's
values. A definition binds the registered XML Schemas its `<XML valid
according to what>`s indicate as it is created [§4.2.6]: the binder
notes each where it notes the routines invoked (`binder::Invoked`), the
definition's horizon keeps them (`catalog::Horizon::xml_schemas`, in the
catalog as the rest of the horizon is), which is how its dependents are
found, with no binding again; and a URI of a definition bound again is
made the ID of the one kept of its namespace (`binder/xml/pinned.rs`), so
one of a lesser name, or one its owner may use later, never takes its
place. DROP SCHEMA CASCADE drops the schema's registered XML Schemas
as DROP XML SCHEMA CASCADE does.

The statement is logged as its text with the document written in as a
literal, so replay evaluates nothing again: every document a binary
string literal of its text's UTF-8 octets, one form that holds each, as
a `<newline>` is no character of a character string literal [X3.135-1986
§5.1 SR1]. Replay reads the octets back as UTF-8, the same text octet for
octet, and does not parse them as a binary string document is parsed, by
the encoding it declares: a character string document, a large object
too, is kept as written, its declaration naming any encoding.
The permanently registered XML Schemas of Table 1 [§4.2.6] are
INFORMATION_SCHEMA.XS, XSI and SQLXML, constants
(`catalog::xml_schemas::PERMANENT`), not catalog objects: none is
registered again or dropped, and anyone may use each. Their components
are read once, when first needed, from schema documents kept in the tree
(`xml/schema/permanent/`), as no text turborel is written from fetches
anything: XS's from the Schema for Schemas as [Schema1 Appendix A]
prints it, with the schema of the datatypes it includes as [Schema2
Appendix A] prints it, both copied unchanged under their W3C notice
(`xml/schema/permanent/LICENSE-W3C`), and a schema of the XML namespace it imports,
which no text on hand prints, so turborel's own, of `xml:lang` and
`xml:space` as XML 1.0 defines them; their definitions of the built-in
type names are the built-in definitions they represent [Schema1 §3.4.7,
§3.14.7]. Three of the Schema for Schemas' restrictions, `xs:all`,
`xs:complexRestrictionType` and `xs:simpleRestrictionType`, have content
models Particle Valid (Restriction) refuses by its letter [Schema1
§3.9.6] (RecurseAsIfGroup and RecurseLax compare occurrence ranges they
widen), so its restrictions' content models are trusted, as the text
makes it normative; every registered XML Schema's are checked. XSI's
are none of its own, as no attribute declaration of its namespace is
allowed [Schema1 §3.2.6], its four in every schema by definition
[§3.2.7]. SQLXML's are the XML Schema [ISO/IEC 9075-14:2006 draft
§22.1] prints, which a registered XML Schema's document may import.

XML values are not comparable, so they have no key, bucket or order, and
the binder refuses them where an equality, grouping or ordering operation
would take one, as one inside an array, a multiset, a row or a distinct
type. A distinct type of an XML type has no ordering, and its casts to
and from its source are its own, as every distinct type's are (D69),
though Part 14's table of CAST writes none between XML and a
user-defined type. Every XML type's type precedence list is XML alone.
A part of an array, a multiset or a row is cast to an XML type as CAST
of an XML value is, BY REF. Over a connection a value is its serialization as
CONTENT, and a client parses it as CONTENT when it needs its nodes. A
column is described with the type's modifiers. The column store keeps no
XML value, as it keeps no large object, nor one inside another value.

The constructors of XQuery that SQL/XML's operators evaluate are each a
function of `xml` over sequences, not XQuery text: `document` for
XMLDOCUMENT; `element` for XMLELEMENT and XMLFOREST, which reads its
content once into attributes, text and nodes to copy and builds the
element with `Builder`, its bindings those its names need, a prefix
bound to another URI renumbered; `comment`, `processing_instruction` and
`text`, each a tree of one node standing alone; and `concatenate`. A copy
follows the source tree's links, so none recurses. XMLAGG gathers its
rows as ARRAY_AGG does, sharing its key order, and makes the document
node of RETURNING CONTENT itself, so it is one aggregate over a window
too. An array or a multiset of content is mapped to text by its declared
type (`functions/xml/mapping.rs`, `xml_mapped`), an element of each
element, as §9.8 maps one, then parsed as other content is; its walk is
bounded as types nest.

The binder finds the `<XML lexically scoped options>` in scope, the
innermost first: the XMLNAMESPACES of the XMLELEMENTs and XMLFORESTs a
binder is binding; those of each `<with clause>`, kept with its query
names, which the context of a subquery, a derived table or a `<with
list element>` links to the clauses around it; then those of the
statement or definition, and of the compound statements it is in,
which its parameters carry. A subquery searches its own first, then
those of the query it is in. A definition keeps its options in the text
the catalog keeps, so it binds the same when read again; a view or a
routine sees none of what reads or invokes it. Outside every XMLBINARY
the encoding is HEX, for every operator that writes or reads a binary
string: XMLELEMENT, XMLFOREST, XMLCAST, and XMLMAP of no ENCODING.

XML indexes are built-in index methods (D34) of one XML column, after Pal
et al.'s XML indexes: `xml_path` (PATH_VALUE, §3.2.1), `xml_value`
(VALUE, §3.2.3), `xml_property` (PROPERTY, §3.2.2) and `xml_primary`
(the primary index, §3.1), named in CREATE INDEX's USING as the others
are, so a plug-in may add more. Each holds its own keys, a B-link tree in
the data file beside the others, checkpointed, reopened and recovered as
theirs are; none needs another. An index derives its keys from a
value's stored form in one pass over its events, building no tree. It
keys each element and attribute below each node item of the value, the
item being the context its paths start from: its path, its value and its
ORDPATH. A path is the node's kind and the tokens of its name and of each
element's above it up to its item, the reversed path of §3.1.1. Each index
keeps a dictionary in its tree, an expanded name to a token and a path to
a path id, given under a lock from counters there and never taken back,
so it is durable as the entries are; recovery makes again the entries of
the rows the log touched (D25), with the index's own ids. A value is the
node's typed value, its string value as `xdt:untypedAtomic` [XQueryDM
§6.2.3, §6.3.3], keyed twice, as a general comparison casts it [XQuery
§3.5.2]: as a string, by its first `VALUE_PREFIX_BYTES` (64) octets,
marked whole or cut, ordered by code points; and as an `xs:double`
[XQueryFO §17.1.1] if it is whole and casts, else as unordered. An ORDPATH
(§2.2) labels the node by the odd components a load gives, from its item
down, attributes before children; each component is its length in octets
then its octets, so labels compare as bytes in document order and a
node's begins with its ancestors'. Entries are counted, as a B-link
tree's are (D34): `xml_path` keyed (path id, value, row, ORDPATH),
`xml_value` (value, path id, row, ORDPATH), `xml_property` (row, path id,
value, ORDPATH).

`xml_primary` keeps the rows of the Infoset of §3.1.1: each element and
attribute keyed (row, ORDPATH), then its kind, its path id and its string
value, whole up to `PRIMARY_VALUE_BYTES` (512) octets, cut past them;
and each again keyed (row, path id, value, ORDPATH), as the PROPERTY
index keys the primary's rows (§3.2.2), so a row's nodes of a path and a
value are found by a seek. It answers a query of a row from its entries
alone (`Index::xml_answers`): it holds, it may and is checked, or it does
not hold, the row not given. Its nodes of the path are found by path and
value, then taken in ORDPATH order, which is document order. A query may
filter the node of any step of its path, its anchor, as
`/BOOK[@ISBN = "1"]/SECTION` filters a SECTION's BOOK (§3.1.2), by as
many filters as it has, a node holding if each holds of its anchor: an
anchor fixed of the node, as many steps above it as child steps follow
the filter's step, or below the item as child steps reach it, its
ORDPATH the node's less as many components, the parent join of §3.1.2;
the nodes below it are its rows of the Infoset whose ORDPATH begins with
its own, one range (§2.2). A query's relation holds of a node if an
element of its paths stands in it, as "a byte comparison of two ORDPATH
labels yields the relative order of the nodes" and a child's label
extends its parent's (§2.2), so the row's rows of the Infoset in a range
of keys are those of a range of places (`primary/related.rs`): an
ancestor is sought at each prefix of the node's ORDPATH; a following
element is read past the node's range, a following sibling past it
before its parent's range ends, of as many components; a preceding one
before the node, of no prefix of it, a preceding sibling after the
parent's own row; each range read until one is met. The check reads the relation as the
events come: an element above a node is open, one before it closed
before it opened, and a node waits for one after it as for an anchor
(`check/reading/relating.rs`). A row whose answer needs a value cut, a comparison that
raises, or whose versions held are of more than one value, as a row
being changed is, is checked. It was built because reading a large
value's nodes from the stored form, even in one pass, reads all of the
value, and a query of a few of its nodes then costs the whole value; the
primary reads the nodes asked and their anchors' subtrees. Small values
are cheaper checked, and the planner weighs `xml_cost` against
`XmlCost::of_check` of the value's octets.

An index is given a row's value whole, but may not be able to read it as
it takes its entries back: garbage collection frees a version's large
objects before its entries, and recovery and a spilled undo name an
object by its id (D79). So each (value, row) pair held is marked by the
value's identity: of a value its row holds, its octets' MurmurHash3, the
octets there to derive its entries again; of one held as a large object,
the object's id, never given twice, under which the index keeps the
entries it made, by row, and takes them back, or adds them again, with no
read of the value. A value that is not one document node, of several
items, an atomic value, a node of another kind or none, as XML(SEQUENCE)
holds, is marked too, by row and identity, and every lookup gives its
row: a query of it as its context item may raise an error, or start from
no document, where its entries alone would not show it [ISO/IEC
9075-14:2006 draft §6.18 GR1a ii 2].

A lookup (`Index::xml_rows`, `xml_cost`) is given a path of child and
descendant steps over expanded names or any name, ending at an element or
an attribute, and comparisons with string, `xdt:untypedAtomic` or number
literals, and gives the rows whose value may hold a node of that path of
which they hold, each once, with what it costs. The paths of the
dictionary are found by the tokens the last steps fix and matched step by
step; each matching path's entries are read in the comparisons' range of
keys; of the primary, its rows answered of the path. A query
(`XmlQuery`) is a lookup and filters of the nodes of its steps, at most
`MAX_FILTERS` (16): its rows are among those of its lookup, and of each
filter's, the lookup's steps to the filter's then the filter's
(`XmlQuery::lookups`), so a probe of several lookups (`XmlProbe`) gives
the rows each gives, those of all but the last held in a set that spills
(D38), each marked by how many gave it [Pal et al. §3.3]; or, of a query
that may raise an error, the rows any of the lookups it may raise of
gives. The rows are a superset, as a value is cut and a reader sees one
version of its row, so its caller checks each row it fetches, as for a
function lookup (D34): an index changes no result. The check
(`XmlLookup::matches`) reads the value's stored form in one pass, no tree
built: each element open keeps, as bits, which steps end at it and which
were matched at or above it, made of its parent's and its name's, and an
element's string value is its part of one buffer of the text read while
an element selected is open; it holds a stack of open elements, at most
`MAX_NESTING`. Its nodes are taken in document order, an element before
its attributes, the first that holds or raises answering, as a tree's
walk would answer, which a test of many values and lookups made checks.
A filter's anchor is an element open at which the lookup's steps to the
filter's end; each element below it keeps the filter's steps as bits
too, from the anchor; the filter holds once a node it selects holds,
else raises as its anchor ends if one raised. A node selected waits for
its anchors not yet known, answering false once one ends not holding of
no error, else, all known, the error one raised, as `and` may [XQuery
§3.6]; nodes waiting for the same anchors are kept as the first.
A number compared also
selects the values keyed unordered, so no row whose comparison would raise
err:FORG0001 is skipped (D15). D34's function lookup does not fit:
XMLEXISTS and XMLQUERY are no registered function of a column and a
value, and a lookup is a path, not a value. A value of more than
`MAX_KEYED_NODES` (262,144) elements and attributes, or of a key longer
than the tree's longest, is refused as it is indexed, SQLCODE -900 (D18).

The planner reads an XML index (`physical/access/xml.rs`) for a
conjunct of a table's filter that keeps a row only if a query's value of
it is not empty (`ScalarFunction::xml_filter`): XMLEXISTS, XMLQUERY NULL
ON EMPTY IS NOT NULL, and XMLTABLE's row pattern, which the planner adds
as an implied filter of the rows XMLTABLE joins, not LEFT, and checks no
row by (`XmlRowsOf`). The query is passed the column alone, or its copy,
as its context item. Of XML(DOCUMENT) or XML(CONTENT) each value is one
document node; one of XML(SEQUENCE) may not be, and the query raise an
error of it [ISO/IEC 9075-14:2006 draft §6.18 GR1a ii 2] or start from
no document. Every lookup gives such a value's row, so it hides none, but
the query of such a column is neither free of errors nor answered by an
index (`functions/xml/query.rs`). The query is reduced to an index's query
(`xquery/lookup.rs`) if it is one path of child, descendant and
attribute steps over names or `*` (`lookup/steps.rs`): `self` of a name
narrows the step before's; `parent` takes back a child or attribute step
as a condition of the step before, as `a/b/..` is `a[b]`, after a
descendant step, last, the path before; a last `ancestor` of a name a
step before tests, the path, exactly; of another, and `following`,
`preceding` and their siblings of a name or `*`, the query's relation of
an element of that name to the node before (`XmlRelated`), its lookup the
elements' path, exactly; a last kind test the path before, not exactly.
Each step
may have predicates whose
conjuncts [XQuery §3.6] are each a relative path of such steps, or its
comparison, or `.`'s, with a literal. A general comparison with a number,
or with a string by the codepoint collation, is kept; any other is left
to the check of each row. The last step's comparisons of `.` are the
lookup's, or else one of its other conditions, of child steps if one is,
continues the lookup, as `a[b op v]` is `a/b op v`; every other is a
filter of its step's node (`XmlFilter`). A comparison with a number may
fail to cast [err:FORG0001], and a value comparison may fail
[err:XPTY0004], of each node it is of whether a node below is selected or
not: the lookup of its own condition gives each row it may fail of, so a
probe of a query that may raise reads those lookups and gives the rows
any gives (`Reduction::raising`); one that raises none reads the
cheapest of its lookups and each its index counts, and gives the rows
each gives. Conjuncts of the column's queries joined by AND are one
probe too, of all their lookups. The rows come as
`Probe::Xml` (`XmlProbe`), opaque as a function lookup's
(`Probe::opaque`): storage records a read of the whole table, each
entry's key projects to null (`DerivedKeys`), and the filter keeps the
conjunct and checks each row fetched, so an index changes no result. Of
`xml_primary`, a query that is exactly XMLEXISTS's test, every
comparison kept and none raising, each filter's anchor fixed, is
answered (`Index::xml_answers`):
the conjunct is no filter's, a row the index holds of is kept as its
visible version is fetched, and one it may hold of is checked by
`XmlQuery::matches` (`exec/answered.rs`), of a SELECT's rows and of an
UPDATE's or DELETE's alike.
The rows a lookup leaves out are those its query gives the empty
sequence of with no error, a numeric comparison's lookup giving the
values that cast to no number, so it is used only if every other
conjunct of the table cannot raise one, or is such a query that raises
none (D15). A path, alone or compared with a string, raises none
(`ScalarFunction::raise_free`), so it is no raising conjunct to D15's
guards, and an ON condition of it is pushed onto its operand as a
comparison is; one that may raise is moved onto its operand over a
guard, below which the index is read (D15). A probe is priced by the
entries its index counts of each
lookup (`Index::xml_cost`), its rows those of the cheapest of every
lookup, each a share of the table's as if apart, or the entries of every
lookup of any; of the primary, which seeks each row it holds, no fewer
than its queries' own lookups count; and each row it gives but an answered one a check of
its value, against a full read that checks every row's, a check a pass
over a value of the column's average width (`XmlCost::of_check`); a
method that prices none is not read. EXPLAIN shows `column has` the
queries, joined by `and`, each filter as a predicate of its step, and
whether the index answers it.

The mappings of names \[EXTENSION\]. Part 14 maps SQL identifiers to XML
Names [§9.1, §9.2] and back [§9.3], `xml::identifiers`, and gives an
application no syntax to invoke them [§4.10.3 NOTE 10, §4.10.4 NOTE
11], though Feature X400 is that an application may invoke §9.3. The
library exports them (`xml::name_of`, `name_of_parts`, `identifier_of`,
`delimited`), and SQL calls them by these functions, decided in the
draft's notation:

```
<XML name to identifier> ::=
    XMLNAME_TO_IDENTIFIER <left paren> <character value expression> <right paren>

<identifier to XML name> ::=
    IDENTIFIER_TO_XMLNAME <left paren> <character value expression>
        [ <comma> { PARTIALLY | FULLY } ESCAPED ] <right paren>
```

XMLNAME_TO_IDENTIFIER gives the delimited identifier `"SQLI"` §9.3 GR5
makes, each double quote of SQLI doubled [ISO/IEC 9075:1992 §5.2].
IDENTIFIER_TO_XMLNAME applies §9.1 to the characters of one identifier,
partially escaped unless FULLY ESCAPED is written, the draft fixing no
default; §9.2 takes a sequence of identifiers, which a string is not, so
no function takes it. Each is a function of the registry
(`functions/xml/names.rs`) that the parser calls by its own syntax, as
the string functions are called (`parser/xml/names.rs`); the words are
no key words. Each gives CHARACTER VARYING as long as its argument's
declared length allows its mapping to be, at most `MAX_CHARACTER_LENGTH`,
and a longer result is SQLSTATE 22001. No error of the draft arises, as
SQL_TEXT is Unicode [§9.3 GR4].

XMLMAP \[EXTENSION\]. Part 14 maps a table, a schema or the catalog to
XML and to an XML Schema document [§9.11, §9.14, §9.17], but gives the
mappings no syntax [§4.10.9 NOTE 13, §4.10.10 NOTE 14, §4.10.11 NOTE
15]. turborel gives them one, a value expression of an XML type, written
here in the draft's notation:

```
<XML map> ::=
    XMLMAP <left paren> <XML map source>
        [ <XML map nulls> ] [ <XML map form> ]
        [ <XML map target namespace> ] [ <XML map results> ]
        [ <XML map encoding> ] <right paren>

<XML map source> ::=
      TABLE <table name>
    | SCHEMA <schema name>
    | CATALOG

<XML map nulls> ::= NULLS { ABSENT | NIL }

<XML map form> ::= AS { FOREST | ELEMENT }

<XML map target namespace> ::= TARGETNAMESPACE <character string literal>

<XML map results> ::= DATA | METADATA | DATA AND METADATA

<XML map encoding> ::= ENCODING { BASE64 | HEX }
```

Each clause gives a parameter of the mapping [§9.11 SR1, §9.14 SR1,
§9.17 SR1]. NULLS ABSENT and NIL are NULLS absent and nil. AS FOREST is
TABLEFOREST True and AS ELEMENT False. TARGETNAMESPACE is TARGETNS. DATA
is DATA True and METADATA False, METADATA the reverse, and DATA AND
METADATA both True, so one at least is \[SR2\]. ENCODING BASE64 and HEX
are ENCODING. A clause not written takes its default. TARGETNAMESPACE's
is the zero-length string, which the draft makes no namespace [§9.10
GR1]. The draft leaves the others to the implementation: NULLS ABSENT,
as XMLFOREST leaves a null value out; AS ELEMENT, so a table's mapping
is a document, as a schema's and the catalog's always are; DATA; and
the `<XML binary encoding>` in scope [§11.3], as of every other
operator, so HEX outside every XMLBINARY. XMLMAP, NULLS, ABSENT, NIL,
FOREST, ELEMENT, TARGETNAMESPACE, DATA, METADATA, ENCODING, BASE64 and
HEX are no key words, each read as a word where XMLMAP takes it. The
target namespace holds XML characters alone and is not the namespace of
`xml` or `xmlns`, which XMLNAMESPACES refuses too [§11.3 SR6].

The binder reads each table a mapping maps by a subquery, `SELECT * FROM`
the table, so it is read as a `<table reference>` reads it. TABLE needs
SELECT on its table, else SQLCODE -104, and a view whose query maps a
table reads it, so DROP TABLE RESTRICT is refused while it does. A
schema's or the catalog's mapping holds its XML visible tables alone
[§4.10.8], those whose SELECT the statement's authorization identifier
holds with a column it can map, so it is never refused for a table it
cannot read. Its tables are not among those a view reads: it maps the
tables there are as it runs. A view, routine or trigger whose text maps
a schema depends on the schema, found by reading the text as the lexer
does (`db/drop/mapped.rs`), as a constraint a schema names does on it:
DROP SCHEMA RESTRICT is refused while it is there, and CASCADE drops it.
A mapping of the catalog depends on nothing that can be dropped. A
mapping of a schema or the catalog could hold itself, through a view or
a routine that maps a schema or the catalog, so it leaves out a view
whose query, or that of a view it reads, holds one, and its tables are
bound for `Purpose::Mapped`, under which a mapping of a schema or the
catalog met through a routine is refused (SQLCODE -103) rather than
recursing without end. A mapping
reads rows in an order not determined, so a constraint holds none. A
function of each row's values writes its text, as §9.10 writes it, and
the rows of all the tables, each its table's index and its text, are
read UNION ALL, one table's after another's, by a set function that
writes the data as they come, each table's in its elements [GR12]. The
data is held whole as it grows, at most `MAX_INPUT` octets of all the
tables', so a mapping of any number of rows and tables is read as its
rows stream; past it, as the row that takes it there comes, and past the
limits of the value it makes, SQLCODE -900. A function of the data
writes the XML Schema document [§9.11 GR10] and parses each, as the
draft's XMLPARSE does, PRESERVE WHITESPACE.

A column is left out, as an XML unmappable column, if its type is
XML(SEQUENCE), a structured or reference type [§4.10.8], or, as §9.5
maps no type of them, DECFLOAT or a plug-in's type, or is made of one.
Rows, arrays and multisets are mapped as §9.8 maps them, each a complex
type of the types it is made of.

The value is what that parse gives, which XMLSERIALIZE writes as the
draft's XSR and XDR [§9.11 GR10f, GR12d]. Of METADATA it is the XML
Schema document, of XML(DOCUMENT(UNTYPED)). Of DATA it is the data, of
XML(DOCUMENT(UNTYPED)), or of a table AS FOREST of
XML(CONTENT(UNTYPED)), a document node of the rows' elements. Of DATA AND
METADATA it is the two results the draft names [§9.11 GR13], a sequence
of the schema document's document node then the data's, of
XML(SEQUENCE). The documents are of UTF-8 [GR8], and of XML 1.1 only if
a name in them is no name of XML 1.0 [GR7], so XMLDECL is written of XML
1.1 alone. They hold no white space between elements. A visible column
a mapping leaves out, as its type does not map, raises the warning
01010 [§9.11 GR3]. The binder, which knows it, binds such a mapping as
`Expr::Warned`, which raises its warning each time it is evaluated (D53),
so a mapping of no argument is not folded away as the statement is
planned.

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

`Cargo.toml` declares each test and benchmark target with its path, with
discovery off, so the directories are `test/` and `bench/`. Every other file
under a target's directory is a module of that target.

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
| R-DOC-4 | Behaviour the standard governs **shall** cite it as printed: `[X3.135-1986 §8.6 GR2]`, `[ISO/IEC 9075:1992 §11.19 SR13]`, `[ISO/IEC 9075-2:2023 §9.15 SR4e]`. Implementation-defined choices say so and state the choice. Beyond the standards: `[EXTENSION]`; ungoverned internals: `[DESIGN]`. BNF in doc comments goes in backticks. | review |
| R-DOC-5 | No commented-out code, no `TODO` or `FIXME` comments: open work goes in TODO.md, and code never cites TODO.md. | review; `cargo xtask verify` |
| R-DOC-6 | Every file **shall** start with a `//!` doc giving its responsibility and clauses implemented. | review |
| R-DOC-7 | Each implementation-defined choice **shall** be listed in CONFORMANCE.md with its implementing file. | review |
| R-DOC-8 | A change visible to a user, operator or program **shall** update the manual, `doc/`, in the same commit: every page stating what changed, in its format (statement pages: NAME, SYNOPSIS, DESCRIPTION, RULES, PRIVILEGES, ERRORS, EXAMPLES, STANDARD, SEE ALSO; chapters: sections). New statements, settings, program options and events get a page or entry. Claims are checked against the code; examples are run, with the changed output pasted. | review |
| R-DOC-9 | A major feature, a finished TODO.md item or a new capability users would look for, **shall** update README.md's "What it has" in the same commit: one short line added or an existing line extended, never a paragraph, so the summary does not go stale. | review |

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
| R-SAFE-3 | `dyn Trait` **shall** appear only at the extension points listed atop `src/db.rs`, the traits they hand out, the storage foundation's traits (D30), and row streams, and at a boundary that keeps a proof of `Send` or `Sync` within the compiler's default recursion limit, marked `[DESIGN]`. Each extension trait **shall** document its contract; every built-in implementation **shall** pass the same contract tests. | review; tests |
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
| R-TEST-9 | Tests **should** be mutation-checked periodically. `cargo xtask mutate [NAME]` applies each mutation in `xtask/src/mutate/list.rs` and its submodules (package, file, snippet occurring once, replacement, tests that must fail) in turn, runs its tests, restores the file, and reports uncaught mutations and named tests that passed. Run it when changing a mechanism the list breaks; a new mechanism adds a mutation. | `cargo xtask mutate`, periodic; `verify` checks each mutation still applies |
| R-TEST-10 | A fixture or helper shared by modules **shall** be written once: in the test binary's `common.rs`, or, if shared by crates, in `turborel-test-support` (`support/`). Tests use `#[test]`, no other framework. | review |

R-TEST-6 was withdrawn with the golden-file tests (D6).

### R-TRACE: traceability

| ID | Rule | Checked by |
|---|---|---|
| R-TRACE-1 | Every test **shall** have a `/// Verifies:` paragraph starting with a tag: `[X3.135-1986 §…]`, `[ISO/IEC 9075…]` of any part, `[ISO/IEC 13249…]` (SQL/MM) of any part, `[EXTENSION]`, `[DESIGN]` or `[TOOL] R-…`. | `cargo xtask verify` |
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

Stable rustc gives region, line and function coverage, approximating
statement coverage. Branch coverage needs nightly, and MC/DC can't be
measured (D22). Masking MC/DC is acceptable (CAST-6), which matters for
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
| R-PROC-1 | `cargo xtask verify` **shall** pass before every commit: format, lints, lint baselines, docs, file length, traceability and citations, no merge conflict marker in a source or document, threads, the claimed features SQL_FEATURES shows, every test. `--coverage` adds coverage. | pre-commit hook; CI |
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

These rules make the code and its verification acceptable DO-178C inputs,
but not certifiable alone. Certification belongs to a process run under
plans agreed with the authority. Meeting DO-178C as written here also meets
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

These items are outside the rules.

1. Plans (PSAC, SDP, SVP, SCMP, SQAP) are not written. This file feeds the
   SDP and SVP.
2. Doc comments and unit tests stand in for low-level requirements.
3. Data and control coupling analysis is not done.
4. Source to object code traceability (Level A) is not done.
5. The compiler and coverage tools are not qualified (R-COV-5).
6. Independence of verification needs an organisation, not a rule.
7. The engine heap-allocates throughout, a DO-332 vulnerability area, so
   airborne use needs bounded allocation or analysis.

These rules were adopted on 2026-10-23 with a code audit. Every finding
was fixed that week, and lint baselines and over-long files were paid down.

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
- ISO/IEC 9075-14, *SQL/XML*, written from its 2006 draft, and the W3C
  texts it names: XML 1.0 (third edition) and 1.1, Namespaces in XML 1.0
  and 1.1, the XML Information Set, and the XQuery 1.0 and XPath 2.0 data
  model.
- S. Pal, I. Cseri, O. Seeliger, G. Schaller, L. Giakoumakis and V.
  Zolotov, "Indexing XML Data Stored in a Relational Database", VLDB
  2004: tokenized names, and the XML indexes (D106).
- P. L. Lehman and S. B. Yao, "Efficient Locking for Concurrent Operations
  on B-Trees", ACM TODS 6(4), 1981: the B-link tree.
- W. Litwin, "Linear Hashing: A New Tool for File and Table Addressing",
  VLDB 1980: the hash table.
- R. N. Williams, *A Painless Guide to CRC Error Detection Algorithms*
  (1993); G. Cook, [CRC RevEng catalogue](https://reveng.sourceforge.io/crc-catalogue/all.htm);
  M. E. Kounavis and F. L. Berry, *A Systematic Approach to Building High
  Performance Software-based CRC Generators* (2005); D. V. Sarwate,
  *Computation of Cyclic Redundancy Checks via Table Look-Up* (1988).
