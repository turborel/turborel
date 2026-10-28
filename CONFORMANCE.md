# Conformance

What turborel implements of the SQL standards, and the choices the
standards leave to an implementation. The standards are read as one
language (RULES.md D32): **ANSI X3.135-1986** ("SQL-86", FIPS PUB 127)
first, extended by **ISO/IEC 9075:1992** ("SQL-92"). Where they differ, the
newer one wins.

Citations follow the convention used in the code: `[X3.135-1986 §8.6 GR2]`
is clause 8.6, General Rule 2, of SQL-86; `[ISO/IEC 9075:1992 §11.19 SR13]`
is clause 11.19, Syntax Rule 13, of SQL-92. **F** is a Format (the BNF),
**SR** a Syntax Rule, **GR** a General Rule.

Contents: [The claim](#the-claim) ·
[SQL-86, clause by clause](#sql-86-clause-by-clause) ·
[SQL-92](#sql-92) · [Extensions](#extensions) ·
[Implementation-defined choices](#implementation-defined-choices) ·
[Limits](#limits) · [Readings of ambiguous text](#readings-of-ambiguous-text)

---

## The claim

The claim [X3.135-1986 §3.4]:

- **Type:** full SQL conformance to Level 2, for SQL-DDL (`<schema>`,
  through `Database::schema`) and SQL-DML.
- **Facilities:** direct processing of SQL statements through the Rust
  binding (`Session`, `Params`); §3.4 leaves the method and time of binding
  to the implementation. The module language and embedded SQL (COBOL,
  FORTRAN, Pascal, PL/I) are not claimed.
- **Environment** [§4.6, §4.7]: a database kept in a directory
  (`Database::open_files`) keeps its schemas and committed changes and is
  recovered after a failure [§4.16]. A database made with `Database::new`
  is held in memory and ends with the process.
- **Limits:** SQL-86 sets none beyond its implementor-defined choices.
  A statement past one of this implementation's [limits](#limits) gets
  SQLCODE -900 ("valid, but not processed").

One difference from SQL-86 comes from SQL-92: its reserved words are key
words here, so SQL-86 text that names something `VALUE`, `ADD` or `COLUMN`
(and others) writes the name as a delimited identifier, `"VALUE"`.

Every outcome has an SQLSTATE beside its SQLCODE [ISO/IEC 9075:1992
§22.1].

---

## SQL-86, clause by clause

**Done:** the Formats, Syntax Rules and General Rules are implemented and
tested. **Not claimed:** outside the claim above.

`cargo xtask trace '§8.6'` lists the tests that verify a clause.

### 3. Overview, and 9. Levels

| Clause | Status | Notes |
|---|---|---|
| §3.2 Notation, §3.3 Conventions | Done | The BNF is followed production by production. A Format error is SQLCODE -102, a Syntax Rule error -103. |
| §3.4 Conformance | Done | See [the claim](#the-claim). |
| §9 Levels | Done | Level 2 throughout. No Level 1 checking mode is provided. |

### 4. Concepts

| Clause | Status | Notes |
|---|---|---|
| §4.1 Sets, §4.2 Data types, §4.3 Columns, §4.4 Tables | Done | Base, viewed and grouped-view tables. |
| §4.5 Integrity constraints | Done | NOT NULL and UNIQUE, checked after each statement. |
| §4.6 Schemas, §4.7 The database | Done | Kept in a directory, or held in memory. |
| §4.8–§4.11 Modules, procedures, parameters, languages | Not claimed | A `Session` with `Params` is the binding §3.4 allows. Module text is not parsed, and a Rust library binds to no host language. |
| §4.12 Cursors | Done | Updatable cursors are working viewed tables; read-only cursors working base tables. See [implementation-defined choices](#implementation-defined-choices). |
| §4.13 Statements, §4.14 Embedded syntax | Done | Every `<SQL statement>` runs. Embedded SQL is not claimed. |
| §4.15 Privileges | Done | Owners and `<privilege definition>`s. |
| §4.16 Transactions | Done | Atomic, serializable, and atomic with respect to recovery for a database kept in a directory (RULES.md D25). Sessions run on many threads at once (D27); the built-in engine isolates transactions with versions (D29). |

### 5. Common elements

| Clause | Status | Notes |
|---|---|---|
| §5.1 `<character>` | Done | Unicode; end of line is `\n` or `\r\n`. |
| §5.2 `<literal>` | Done | |
| §5.3 `<token>` | Done | Key words in any case, after SQL-92. |
| §5.4 Names | Done | Module and procedure names belong to the module language, not claimed. |
| §5.5 `<data type>` | Done | |
| §5.6 `<value specification>` | Done | Parameters with indicators, literals and USER. `<variable specification>` belongs to embedded SQL and gets SQLCODE -900. |
| §5.7 `<column specification>` | Done | Outer references, looked up in the most local scope first. |
| §5.8 `<set function specification>` | Done | Over outer references too. |
| §5.9 `<value expression>` | Done | |
| §5.10 `<predicate>` | Done | |
| §5.11 `<comparison predicate>` | Done | A subquery gives at most one value; more is SQLCODE -811. |
| §5.12 `<between predicate>` | Done | |
| §5.13 `<in predicate>` | Done | Value lists and subqueries. |
| §5.14 `<like predicate>` | Done | With ESCAPE. |
| §5.15 `<null predicate>` | Done | |
| §5.16 `<quantified predicate>` | Done | |
| §5.17 `<exists predicate>` | Done | |
| §5.18 `<search condition>` | Done | Three-valued logic, by the GR2 truth tables. |
| §5.19–§5.25 Table expressions and query specifications | Done | A subquery runs once per distinct set of outer values [§5.21 GR3]. §5.25 GR3 is read literally: see [readings](#readings-of-ambiguous-text). |

### 6. Schema definition language

| Clause | Status | Notes |
|---|---|---|
| §6.1 `<schema>` | Done | |
| §6.2 `<table definition>` | Done | |
| §6.3 `<column definition>` | Done | |
| §6.4 `<unique constraint definition>` | Done | Enforced with an index. |
| §6.5 `<view definition>` | Done | Not materialized; WITH CHECK OPTION as SQL-92 defines it. |
| §6.6 `<privilege definition>` | Done | On tables and views. A privilege the grantor cannot grant is not granted, with a warning, as SQL-92 says, not refused [SR5] (RULES.md D47). |

### 7. Module language

| Clause | Status | Notes |
|---|---|---|
| §7.1 `<module>` | Not claimed | `Session` plays the module's part. GR2 (the implicit COMMIT or ROLLBACK at the end) is the setting SESSION_END. SR1 (one procedure opens each cursor) is not checked. |
| §7.2 `<module name clause>` | Not claimed | |
| §7.3 `<procedure>` | Not claimed | Parameters and the SQLCODE outcomes (GR2, GR3) are implemented; procedure text and the SQLCODE parameter declaration are not. |

### 8. Data manipulation language

| Clause | Status | Notes |
|---|---|---|
| §8.1 `<close statement>` | Done | |
| §8.2 `<commit statement>` | Done | Atomic across engines. |
| §8.3 `<declare cursor>` | Done | ORDER BY, UNION, updatability; any SQL-92 query expression. |
| §8.4 `<delete statement: positioned>` | Done | |
| §8.5 `<delete statement: searched>` | Done | |
| §8.6 `<fetch statement>` | Done | Indicators, truncation, SQLCODE 100. |
| §8.7 `<insert statement>` | Done | VALUES and query expressions; into updatable views. |
| §8.8 `<open statement>` | Done | |
| §8.9 `<rollback statement>` | Done | |
| §8.10 `<select statement>` | Done | |
| §8.11 `<update statement: positioned>` | Done | |
| §8.12 `<update statement: searched>` | Done | |

### Annexes A–F (embedded SQL)

Not claimed. Annex A says it "is not an integral part of the body of the
standard", and §3.4 lets a claim name its embedded languages; this one
names none.

---

## SQL-92

What of ISO/IEC 9075:1992 is implemented, beyond SQL-86. Where SQL-92 keeps
a restriction of SQL-86 for Entry or Intermediate SQL alone, the
restriction is lifted.

### Lexical elements and errors

- **Identifiers and key words** [§5.2]: SQL-92's reserved words are the key
  words, in any case; regular identifiers are folded to upper case, up to
  128 characters; delimited identifiers.
- **Literals** [§5.3]: empty and continued string literals; an
  `<introducer>` naming SQL_TEXT, of a literal or an identifier [§5.4].
- **Statements** [§20.1]: a direct statement ends in a semicolon, or none.
- **Valid SQL not implemented** (RULES.md D18): each construct of SQL-92
  not taken is read by its Format in full and refused with SQLCODE -900,
  naming what is not implemented; SQL that breaks a Format is a syntax
  error (SQLCODE -102), whatever it names.
- **SQLSTATE** [§22.1] beside every SQLCODE, with its subclass where the
  standard gives one.

### Direct invocation of SQL

- **Direct SQL statements** [§20.1, §20.2]: `Session::direct`, and every
  statement of a server's client (RULES.md D42), and of the command-line
  client, `turborel` (RULES.md D46). A query expression with an
  optional ORDER BY gives its rows as they are fetched, none being "no
  data" [§20.2 GR3]; a schema definition is processed as the session's
  own; any other statement gives its completion.

### Dynamic SQL

- **Prepared statements** [§17.6, §17.8–§17.10, §17.13, §17.14] through
  the Rust API (`Session::prepare`, `execute_prepared`, `open_prepared`)
  and a server's protocol (RULES.md D45), with `<dynamic parameter
  specification>`s [§6.2] whose types are deduced as §17.6 GR5 says and
  refused where it gives none [GR4]. Not implemented: the statements of
  dynamic SQL in SQL text, and the descriptor areas [§17.1–§17.5].

### Schemas and constraints

- **Schema definitions** [§11.1]: a schema named apart from its owner,
  `CREATE SCHEMA S AUTHORIZATION U`, `CREATE SCHEMA S` owned by the
  session's authorization identifier [SR2b], or SQL-86's `CREATE SCHEMA
  AUTHORIZATION U`; its name may be qualified by the catalog's. What is in
  it is its owner's, and its unqualified names are of it.
- **Constraints** [§11.4, §11.6–§11.9, §10.6]: NOT NULL, UNIQUE (whose
  columns may be null), PRIMARY KEY, FOREIGN KEY with MATCH FULL, PARTIAL or
  neither and every referential action, and CHECK, including CHECK with
  subqueries. Each is named, by the definition or by the implementation.
  Constraints are checked at the end of each statement; deferrable ones,
  with SET CONSTRAINTS [§14.2], at commit.
- **Defaults** [§11.5]: a literal, NULL, USER, CURRENT_USER, SESSION_USER or
  SYSTEM_USER; used by INSERT, DEFAULT VALUES, `SET ... = DEFAULT`, columns
  added, and the referential action SET DEFAULT.
- **DROP SCHEMA** [§11.2], **DROP TABLE** [§11.18], **DROP VIEW**
  [§11.20] and **ALTER TABLE** [§11.10–§11.17]: ADD and DROP COLUMN, ALTER
  COLUMN defaults, ADD and DROP CONSTRAINT; CASCADE and RESTRICT. They run
  inside transactions (RULES.md D33).
- **Views** [§11.19]: of any query expression; WITH LOCAL or CASCADED CHECK
  OPTION, checked against every view an update spans [GR11]. A view keeps
  the columns it saw when it was defined, whatever columns are added later
  [§11.11 GR2].
- **The REFERENCES privilege** [§10.3, §11.8 AR1, §11.9 AR1], and
  **INSERT on columns** [§10.3 GR4, §13.8 AR1]: INSERT, UPDATE and
  REFERENCES granted on a table extend to a column added later [§10.3
  GR3–GR5, §11.11 GR3]. A view's owner holds REFERENCES on each column of
  it drawn only from columns it holds REFERENCES on [§11.19 GR6b] (RULES.md
  D52), so a CHECK or an assertion may read a view.
- **The warning cursor operation conflict** [§11.8 GR8]: a referential
  action on a row changed through a cursor still open (SQLSTATE 01001).
- **Schema statements on their own** [§11.3, §11.19, §11.36, §11.37,
  §20.1]: CREATE TABLE, CREATE VIEW, GRANT and REVOKE, in a transaction
  (RULES.md D33, D47). GRANT gives what its grantor holds with grant
  option and warns of the rest (SQLSTATE 01007) [§11.36 GR14, GR15], in a
  schema definition too; REVOKE, with or without GRANT OPTION FOR,
  destroys what stood on what it revokes, the views, table and domain
  constraints and assertions its owners could no longer have, with
  CASCADE, or refuses with RESTRICT [§11.37 SR18, GR1-GR6], and warns of what it did not revoke (SQLSTATE
  01006) [GR8, GR9].

### Queries and expressions

- **CASE, NULLIF, COALESCE** [§6.9], **CAST** among the supported types
  [§6.10], the **string functions** and `||` [§6.6, §6.7, §6.13].
- **Scalar and row subqueries** [§7.11], **row value constructors** [§7.1]
  in comparisons, BETWEEN, IN, IS NULL, quantified comparisons and MATCH.
- **Predicates:** LIKE of any character strings [§8.5]; IS TRUE, FALSE and
  UNKNOWN [§8.12]; UNIQUE and MATCH [§8.9, §8.10].
- **Set functions** [§6.5] of any value expression, including over outer
  references [SR4, SR5].
- **Joined tables** [§7.5]: CROSS, INNER, LEFT, RIGHT, FULL and UNION
  joins, with ON, NATURAL or USING, nested and in parentheses; derived
  tables with AS and column lists [§6.3].
- **Query expressions** [§7.10]: UNION, EXCEPT and INTERSECT, ALL or not,
  CORRESPONDING; table value constructors of many rows [§7.2]; TABLE t.
- **Updatable queries with subqueries** [§7.9 SR12d], and searched UPDATE
  and DELETE that read the table they change.
- **Store and retrieval assignment** [§9.1, §9.2].
- **GET DIAGNOSTICS** [§18.1] of the diagnostics area [§4.19]: every
  statement item, and every condition item, the constraint, table and
  cursor a condition is about among them, and the column of a privilege
  refused or that a table lacks [GR3g]; the names of a table on which
  the session holds no privilege withheld [GR3j].
- **Global and created local temporary tables** [§4.9, §11.3]: each
  session's rows, ON COMMIT DELETE or PRESERVE ROWS [§14.3 GR3], written
  in READ ONLY transactions too [§13.6–§13.10 GR1], with the rules of
  which tables each refers to [§11.8 SR4, SR5; §11.9 SR3, SR4] and their
  privileges [§10.3 SR2] (RULES.md D49); and **declared local temporary
  tables** [§13.11], DECLARE LOCAL TEMPORARY TABLE MODULE.T, a session's
  own, in no view of the Information Schema, never altered, dropped,
  granted on or named by a view [§10.3 SR1, §11.10 SR5, §11.18 SR3,
  §11.19 SR5].
- **CHARACTER VARYING** [§6.1], CHAR VARYING and VARCHAR, of a length
  written: a value kept as assigned, unpadded [§9.2 GR3d, GR3e; §9.1 GR3d,
  GR3e], compared padded with spaces [§8.2 GR3a], indexed and kept unique
  so; CAST to it [§6.10 GR6]; SUBSTRING and TRIM of a varying type [§6.7
  SR2a, SR6e], UPPER and LOWER of their operand's [SR3a], || of a varying
  one if either operand is [§6.13 SR3a], and CASE and set operations of
  one if any is [§9.3 SR3a]; a dynamic parameter in a string's place of
  CHARACTER VARYING(32767) [§17.6 GR5].
- **NATIONAL CHARACTER** [§6.1], NATIONAL CHAR and NCHAR, fixed or
  VARYING, and the national character string literal, N'...' [§5.3]: the
  character string types and literals of the national character set
  [§6.1 SR2].
- **Datetimes** [§4.5.1, §6.1]: DATE, TIME(p) and TIMESTAMP(p), WITH TIME
  ZONE or not, and their literals, DATE '...', TIME '...' and TIMESTAMP
  '...' [§5.3]; kept in UTC, compared by their instants [§8.2 GR5],
  indexed and kept unique so; assigned to a datetime of their fields
  [§9.2 SR2, GR3l]; CURRENT_DATE, CURRENT_TIME and CURRENT_TIMESTAMP, one
  instant in a statement [§6.8]; CAST to and from character strings and
  between datetimes [§6.10 SR4, GR5e, GR6e, GR9-GR11]; EXTRACT of a
  datetime's fields and displacement [§6.6]; CASE and set operations of
  one datetime type [§9.3 SR3e]; a default of a datetime literal or a
  datetime value function [§11.5 SR3a v, SR3c, GR1d] (RULES.md D54).
- **Intervals** [§4.5.2, §6.1, §10.1]: INTERVAL of a qualifier, year-month
  or day-time, and its literals, INTERVAL '...' [§5.3]; compared, indexed
  and kept unique by their months or microseconds [§8.2 GR6]; assigned in
  a column's fields, 22015 if they leave it [§9.2 GR3m]; a datetime plus
  or minus an interval of its fields, 22008 of no valid datetime [§6.14];
  intervals added, multiplied and divided by numbers [§6.15 GR1-GR5];
  (A - B) and a qualifier, the difference of two datetimes [§6.15 GR6];
  AT LOCAL and AT TIME ZONE [§6.14 GR4]; CAST to and from strings, exact
  numbers of one field and intervals of their class [§6.10 SR4-SR6,
  GR3c, GR12]; EXTRACT of an interval's fields [§6.6]; SUM and AVG of
  intervals [§6.5 SR9e]; CASE and set operations of one class [§9.3
  SR3f]; a default of an interval literal [§11.5 SR3a vi]; OVERLAPS
  [§8.11]; SET TIME ZONE [§16.5] (RULES.md D54).
- **BIT and BIT VARYING** [§6.1], and the bit and hex string literals,
  B'...' and X'...' [§5.3]: a value of BIT(n) of n bits, else refused
  with 22001 or 22026, one of BIT VARYING(n) of at most n [§9.2 GR3f-GR3j];
  compared bit by bit [§8.2 GR4], indexed and kept unique so; || of BIT if
  both operands are, else BIT VARYING [§6.13 SR6], SUBSTRING of BIT
  VARYING [§6.7 SR7, GR6], CHAR_LENGTH, OCTET_LENGTH and BIT_LENGTH
  [§6.6 GR4-GR6]; CASE and set operations of BIT VARYING if any is
  [§9.3 SR3b]; CAST to and from character strings [§6.10 GR5d, GR6d, GR7,
  GR8], not numbers [SR4]; a default of a bit string literal [§11.5 SR3a
  ii, GR1b iv, v]. The warnings of implicit zero-bit padding and of
  truncation are not raised, as a cut character string's is not.
- **Domains** [§11.21–§11.27] (RULES.md D50): CREATE DOMAIN, in a schema
  definition too, with a default and CHECK constraints over VALUE; a
  column of one [§11.4], taking its type, default and constraints; ALTER
  DOMAIN SET and DROP DEFAULT, ADD and DROP CONSTRAINT, the rows there are
  checked; DROP DOMAIN, RESTRICT or CASCADE; CAST to one [§6.10]; and the
  USAGE privilege on one, granted and revoked [§10.3, §11.36, §11.37]. In
  the Information Schema: DOMAINS, DOMAIN_CONSTRAINTS, the domain of each
  column, and USAGE_PRIVILEGES.
- **Assertions** [§11.34, §11.35] (RULES.md D51): CREATE ASSERTION, in a
  schema definition too, checked as it is made and at the end of each
  statement that changes a table it reads, or at commit if deferred, and
  set by SET CONSTRAINTS; DROP ASSERTION; a table or column it reads
  dropped only with CASCADE, which takes it, as DROP SCHEMA does. In the
  Information Schema: ASSERTIONS, CHECK_CONSTRAINTS and the constraint
  usage views.
- **Cursors** [§13.1, §13.3]: SCROLL, with FETCH's every orientation
  (NEXT, PRIOR, FIRST, LAST, ABSOLUTE, RELATIVE) and FROM; INSENSITIVE;
  the `<updatability clause>`, FOR READ ONLY or FOR UPDATE [OF columns],
  FOR UPDATE with ORDER BY or SCROLL too, a positioned UPDATE setting only
  the columns it names and none its ORDER BY reads [§13.9 SR4, SR8].

### Transactions, sessions and the Information Schema

- **SET TRANSACTION** [§14.1]: READ ONLY or READ WRITE, and the isolation
  level, for the next transaction [§4.28]. SERIALIZABLE is serializable;
  REPEATABLE READ reads at one snapshot; READ COMMITTED and READ
  UNCOMMITTED at one for each statement. No level reads uncommitted
  changes or loses an update, and constraints are checked serializably at
  every level (RULES.md D29).
- **SET SCHEMA** [§16.2]: the default schema of unqualified names.
- **SET SESSION AUTHORIZATION** [§16.4]: the session's authorization
  identifier, the value trimmed and read as an `<authorization
  identifier>`; refused in a transaction. The default schema stays as it
  was [§4.30].
- **Connections** [§15]: CONNECT TO a server AS a name, USER a user, or TO
  DEFAULT; SET CONNECTION to a name or DEFAULT; DISCONNECT a name,
  DEFAULT, CURRENT or ALL. The client library is the SQL-agent
  (`turborel_client::agent`), and so the command-line client is; a server
  or a session refuses them (SQLCODE -900) (RULES.md D46). No transaction
  spans servers (SQLSTATE 0A001).
- **COMMIT and ROLLBACK** without WORK [§14.3, §14.4].
- **The Information Schema** [§21]: its 23 views, with the standard's own
  definitions, over the Definition Schema (RULES.md D37).

### Not implemented

- **Leap seconds**, by choice: a SECOND of 60 or 61, which Table 10 of
  §6.1 allows, is refused, a literal of one (SQLCODE -103) and a CAST of
  one (SQLSTATE 22018). A minute is always 60 seconds, so datetime
  arithmetic, whose results over a leap second are implementation-defined
  [§6.1 Table 10, note], is exact.
- **ALTER TABLE of a temporary table**, and a table or view defined on its
  own in a schema other than the session's default one (RULES.md D47).
- **Dynamic SQL in SQL text** [§17]: ALLOCATE and DEALLOCATE DESCRIPTOR,
  GET and SET DESCRIPTOR, PREPARE, DEALLOCATE PREPARE, DESCRIBE, EXECUTE,
  EXECUTE IMMEDIATE, ALLOCATE CURSOR, and the dynamic OPEN, FETCH, CLOSE
  and positioned DELETE and UPDATE, refused (SQLCODE -900): statements
  are prepared through the API and the protocol (RULES.md D45).
- **A name qualified by its catalog** [§5.4]: CAT.S.T, refused (SQLCODE
  -900); there is one catalog, and a name is qualified by its schema.
- **The module language and embedded SQL** (RULES.md D32): a parameter
  written with a colon, `:P`, refused (SQLCODE -900).
- **Never:** character sets other than the one there is, SQL_TEXT, and
  collations and translations (RULES.md D32): CHARACTER SET of another,
  its `<introducer>`, COLLATE, CONVERT, TRANSLATE, CREATE and DROP of a
  character set, collation or translation, a privilege on one, a schema's
  DEFAULT CHARACTER SET of another, and SET NAMES, each refused (SQLCODE
  -900). SET CATALOG (there is one catalog).

---

## Extensions

Facilities beyond the standards, marked `[EXTENSION]` in the code:

- **Data types beyond the standard** (RULES.md D53), plug-ins registered
  with `Database::register_type`: written in a column definition and in
  CAST by their name, `name` or `name(a [, b])`, a domain of the name in
  the default schema taken first; a value of one CAST from a character
  string, and its DEFAULT a character string literal; compared, ordered,
  grouped, indexed and kept unique as its type's kind says, comparable
  with values of its own type alone; logged, checkpointed and recovered;
  sent to clients as its type's name, its text and its bytes, shown as its
  text by one without the plug-in. A database whose files hold one is not
  opened without it.
- **Indexes** (RULES.md D34), of the methods `btree` and `hash` or a
  plug-in's: `CREATE INDEX name ON table (columns) [USING method] [WITH
  (...)]`, `DROP INDEX name`, `ALTER INDEX name SET (...)`, outside a
  transaction; through Rust, `Database::create_index` and `drop_index`;
  INFORMATION_SCHEMA.INDEXES and INDEX_OPTIONS. Index methods and other
  plug-ins declare the options (`Database::declare_index_option`).
- **Users** (RULES.md D43), which neither standard has: `CREATE USER name
  [WITH] PASSWORD 'literal'`, `ALTER USER name [WITH] PASSWORD 'literal'`
  and `DROP USER name`, by the database's owner, outside a transaction;
  through Rust, `Database::create_user`, `set_verifier`, `drop_user` and
  `set_owner`; INFORMATION_SCHEMA.USERS. A `Credentials` plug-in makes a
  verifier of a password; the server's is PBKDF2-HMAC-SHA256.
- **Settings** (RULES.md D36): `SET name = value` and `SET name TO DEFAULT`
  for a session's own values; `SET DATABASE name = value` and `SET
  DATABASE name TO DEFAULT` for the database's, by its owner (D43), outside
  a transaction; and INFORMATION_SCHEMA.SETTINGS, which lists every
  setting.
- **Table settings** (RULES.md D41): `CREATE TABLE ... USING engine WITH
  (name = value, ...)`; `ALTER TABLE ... SET (...)` and `ALTER TABLE ...
  USING engine WITH (...)`; INFORMATION_SCHEMA.TABLE_OPTIONS. Engines and
  other plug-ins declare the options (`Database::declare_table_option`). The row store
  takes OUT_OF_LINE, STATISTICS.SAMPLE, STATISTICS.BUCKETS,
  STATISTICS.REFRESH_FRACTION, STATISTICS.REFRESH_ROWS,
  TRANSACTIONS.TRACKED_ROWS and TRANSACTIONS.HELD_BYTES. The database
  declares ENCODINGS for every table: the registered encodings its rows
  pass through, such as LZ4 or AES-256-GCM with its key (RULES.md D48).
- **Plug-ins** for storage engines, index methods, functions, optimizer
  rules, system tables, the log, file systems, encodings, hash functions and
  diagnostics (the extension points are listed at the top of `src/db.rs`).
- **Function call syntax** for registered functions, in sessions that enable
  it (`Session::enable_extensions`).

---

## Implementation-defined choices

The choices the standards leave to an implementation. Each is cited in the
file that implements it.

| Rule | Choice | Where |
|---|---|---|
| §5.1 SR1 character set; §5.11 GR6 collating sequence | Unicode; Unicode scalar order | `lexer.rs`, `value.rs` |
| §5.3 end-of-line indicator | `\n` or `\r\n` | `lexer.rs` |
| §5.5 SR3 precision of NUMERIC and DECIMAL when omitted | 18 | `value.rs` |
| §5.5 SR7–SR9 precision of DECIMAL, INTEGER, SMALLINT | as specified; 10; 5 | `value.rs` |
| §5.5 SR10–SR12 FLOAT, REAL, DOUBLE PRECISION | 24 bits if p ≤ 24, else 53; 24; 53; FLOAT with no precision 53 | `parser.rs`, `value.rs` |
| §5.6 SR2 type of indicator parameters | exact, precision 9, scale 0 | `value.rs`, `params.rs` |
| §5.6 SR6 length of USER | the identifier's own length | `binder.rs` |
| §5.8 SR7, SR9b, c precision of COUNT and SUM; scale of AVG | 18; AVG of scale S has scale S + 2 | `functions.rs` |
| §5.9 SR4b, c precision of sums and products | 18 | `binder.rs` |
| §5.9 SR4d precision and scale of quotients | 18 digits, at the largest scale (at most 18) that fits, truncated toward zero, trailing zeros dropped down to max(s1, s2) | `value/decimal.rs`, `binder.rs` |
| §5.14 a null escape character (GR4 does not cover it) | the result is unknown | `exec.rs` |
| §8.3 GR3b vii where nulls sort | after every value | `value.rs` |
| §4.12 working base or viewed table | read-only cursors: base, created at OPEN (read as fetched where that gives the same rows; a SCROLL one's kept as read); updatable cursors: viewed, each row read when the cursor reaches it and kept only if it still satisfies the cursor's WHERE clause | `db/cursor.rs`, `db/stream.rs`, `db/scroll.rs` |
| ISO/IEC 9075:1992 §4.21 a transaction's changes seen through a cursor not INSENSITIVE | through a read-only one, none, as through an INSENSITIVE one; through an updatable one, a row deleted, or changed so it no longer satisfies the cursor's WHERE clause, is passed over by every FETCH, and a row inserted is not seen | `db/stream.rs`, `db/scroll.rs` |
| §4.12 a row inserted at a cursor's position | no effect: a cursor covers only the rows its table had at OPEN | `db/cursor.rs` |
| §4.12 an error in a statement naming an open cursor | the cursor's state and position are unchanged, except that a FETCH failing to assign a value [§8.6 GR6] leaves the cursor on the row it fetched, the targets assigned before keeping their values, and a failed OPEN leaves it closed | `db/cursor.rs`, `db/assignment.rs` |
| §8.6 GR5 the order targets are assigned in | the order of the `<fetch target list>`, stopping at the first that fails | `db/assignment.rs` |
| §7.1 GR2 the end of a session with a transaction open | COMMIT, or ROLLBACK, as the setting SESSION_END says | `txn.rs`, `db.rs` |
| §3.3, §7.3 GR3b ii negative SQLCODE values | -101 lexical, -102 Format, -103 Syntax Rule, -104 privilege, -201 constraint, -301 data, -501 cursor, -811 cardinality, -900 not implemented, -910 API misuse, -911 conflict with another transaction, -912 transaction policy, -913 commit failed, -914 table damaged by a failed undo, -915 I/O error, -916 corrupt stored data, -917 invalid transaction state, -918 memory budget spent, -919 connection exception (a server's), -920 statement cancelled, -921 dynamic SQL error | `error.rs` |
| §4.6, §4.7 how schemas are created and stored | `Database::schema` and the `Catalog` trait; in memory, or kept in a directory with its catalog file and log | `db.rs`, `catalog.rs`, `db/files.rs` |
| ISO/IEC 9075:1992 §11.1 AR1 the privileges a schema definition needs | through a session, as a server's clients run it: a schema of any name, its authorization identifier the session's, written or implicit [SR2b]; `Database::schema`, the host program's, creates any, naming its AUTHORIZATION | `db/direct.rs`, `db/schema.rs` |
| ISO/IEC 9075:1992 §17.6 GR5b-d, s, u CHARACTER VARYING of a dynamic parameter | of the longest length, 32767, its value CAST to it [§17.9 GR5g] | `params/dynamic.rs` |
| ISO/IEC 9075:1992 §6.13 SR3a VL and FL, the longest varying and fixed-length strings | 32767 characters each | `value.rs` |
| ISO/IEC 9075:1992 §17.6 GR5 a dynamic parameter as a CASE result, which GR5 gives no type | refused, as one GR4 refuses (SQLSTATE 37000) | `binder/case.rs` |
| ISO/IEC 9075:1992 §17.6 GR10 a prepared statement in a later transaction | valid; bound and planned again at each execution, against the definitions then | `db/prepared.rs` |
| ISO/IEC 9075:1992 §4.30, §15.1 GR8a the authorization identifier a session begins with, and the default SQL-server | the user the client connects as, at a server; for a session the host program starts, the identifier it names. DEFAULT is the server the command line names, connected to as it starts | `db.rs`, `cli/src/dial.rs` |
| ISO/IEC 9075:1992 §15.1 SR1, GR4 the implicit `<user name>`, and restrictions on one | the user the command line names; one named is proved as the command line says, a password typed asked for again, and the server accepts or refuses it (RULES.md D43) | `cli/src/dial.rs` |
| ISO/IEC 9075:1992 §15.1 GR8b how an `<SQL-server name>` determines the server | it is an address: 'host', 'host:port', '[IPv6 address]:port', or on Unix a socket's path beginning '/'; over TLS when the default server's connection is, trusting the same certificates. An address is no `<identifier>`, so CONNECT to one names its connection with AS [GR6] | `cli/src/dial.rs` |
| ISO/IEC 9075:1992 §16.4 GR4 whether a session sets another authorization identifier | a session begun by the database's owner sets any, a user or not; any other session only the one it began with (SQLSTATE 28000) (RULES.md D43) | `db/session.rs` |
| ISO/IEC 9075:1992 §20.1 GR2a whether direct invocation terminated unexpectedly | a server's connection that ends without its client terminating it: lost, failed or cut by the server stopping; its transaction is rolled back | `server/src/native.rs` |
| §6.5 GR1 whether a viewed table is materialized | no: its query is bound into the plan of each query that names it | `binder/views.rs` |
| §3.4 binding of programs to modules | `Session` and `Params` | `db.rs`, `params.rs` |
| ISO/IEC 9075:1992 §5.4; Annex B, item 41 which `<identifier>`s are valid `<authorization identifier>`s | any, for a session the host program starts; a server's client is one of the users the database keeps (RULES.md D43), or one its authenticator accepts, the name it gives read as an `<identifier>` | `db/users.rs`, `lexer.rs` |
| ISO/IEC 9075:1992 §6.2 SR4, GR10, GR11 CURRENT_USER, SESSION_USER, SYSTEM_USER | each is the session's authorization identifier: a session has no other, and a library runs under no operating system user of its own | `binder.rs`, `catalog.rs` |
| ISO/IEC 9075:1992 §9.1 GR3k, §9.2 GR3k a number stored or retrieved into an exact type | an exact number's extra fraction digits truncated; an approximate number rounded to the scale | `value.rs`, `value/cast.rs` |
| ISO/IEC 9075:1992 §22.1 SQLSTATE classes | statements are invoked directly, so a Format, Syntax Rule or Access Rule error is class 2A [§20.1 GR5]; this implementation's own conditions are class TB, the subclass the SQLCODE's digits; a server's are class 08, 08004 for a connection refused and 08006 for one failed, and 28000 for credentials refused; while preparing, a Format, Syntax Rule or Access Rule error is class 37 [§17.6 GR4a]; a conflict that undoes a statement is TB911, one that rolls back a commit 40001; a warning is a completion, its SQLCODE 0 [§22.2] | `error/state.rs`, `db/completion.rs` |
| ISO/IEC 9075:1992 §4.19 the size of the diagnostics area, and the order of its conditions | one condition, the statement's, successful completion too [§4.18]; MORE is always N | `db/area.rs` |
| ISO/IEC 9075:1992 §18.1 GR1e ROW_COUNT after other statements; Table 21's L; GR3b, GR3c an implementation's class origin; GR3k MESSAGE_TEXT | 0; the longest character string (L is never reached); TURBOREL for class TB; the error's message with its rule, empty for a completion | `db/area.rs` |
| ISO/IEC 9075:1992 §14.1 GR5 isolation level | SERIALIZABLE as itself; REPEATABLE READ as snapshot isolation; READ COMMITTED and READ UNCOMMITTED with a snapshot for each statement, kept while a cursor is open; each as SERIALIZABLE on an engine that does not read below it | `db/session.rs`, `txn.rs` |
| ISO/IEC 9075:1992 §14.1 GR6, GR7 diagnostics area limit | a statement reports one condition, its error, so any limit of 1 or more holds | `db/session.rs` |
| ISO/IEC 9075:1992 §4.30 the initial default schema | the session's authorization identifier, for every statement the session binds | `db/session.rs` |
| ISO/IEC 9075:1992 §21.2.2 the catalog's name | the setting CATALOG_NAME, `TURBOREL` by default | `settings.rs`, `system.rs` |
| ISO/IEC 9075:1992 §21.2.27, §21.2.28 SQL_IDENTIFIER, CHARACTER_DATA | CHARACTER(128); CHARACTER as long as the longest value the standard lists for the column, otherwise CHARACTER(4096), a longer text shown as null: of fixed length, not yet the CHARACTER VARYING the standard gives them | `system.rs`, `system/definition.rs` |
| ISO/IEC 9075:1992 §6.1 SR2 the national character set | SQL_TEXT, the one there is: NATIONAL CHARACTER is CHARACTER | `parser/data_type.rs`, `lexer.rs` |
| ISO/IEC 9075:1992 §6.10 GR5d, GR6d a bit string CAST to a character string | its bytes as UTF-8, the form-of-use, whose least character is 8 bits; bytes not UTF-8 are refused (SQLSTATE 22018), as a string holds characters | `value/types/character.rs` |
| ISO/IEC 9075:1992 §6.10 GR7, GR8 a character string CAST to a bit string | the bits of its UTF-8 | `value/types/bit.rs` |
| ISO/IEC 9075:1992 §6.1 SR26 the greatest `<time fractional seconds precision>` | 6: microseconds | `value/datetime.rs` |
| ISO/IEC 9075:1992 §4.5.1, §6.1 SR5, §16.5 GR1a an SQL-session's time zone displacement, and the one it begins with | the setting TIME_ZONE: the database's, `+00:00` by default, or the session's own, which SET TIME ZONE sets; LOCAL takes the database's again (RULES.md D54) | `settings/builtin.rs`, `db/clock.rs`, `db/time_zone.rs` |
| ISO/IEC 9075:1992 §10.1 SR3, SR4 the greatest `<interval leading field precision>` and `<interval fractional seconds precision>` | 9 and 6 | `value/interval.rs` |
| ISO/IEC 9075:1992 §6.15 GR4, GR5 the leading precision of an interval computed | of a sum or difference, one digit more than its operands' greatest; of a product or quotient, 9; a result past it raises 22015 | `value/interval/arith.rs`, `binder/interval.rs` |
| ISO/IEC 9075:1992 §6.15 GR6b whether the difference of two datetimes is rounded or truncated | truncated, toward 0, to the qualifier's least field | `value/interval/arith.rs` |
| ISO/IEC 9075:1992 §21.3.5 an interval's DATETIME_PRECISION | its `<interval fractional seconds precision>`, of SECOND; else 0 | `value/types/interval.rs` |
| ISO/IEC 9075:1992 §6.8 GR3 when a statement's datetime value functions are evaluated | as it begins, from the database's clock, the system's unless a host sets another (`Database::set_clock`) | `db/clock.rs`, `clock.rs` |
| ISO/IEC 9075:1992 §6.6 SR4 the precision of EXTRACT's result | 5 digits, and of SECOND the scale of the datetime's precision more | `binder/datetime.rs` |
| ISO/IEC 9075:1992 §9.2 GR3l, §4.6 a datetime's fraction stored in a type of fewer digits | truncated | `value/datetime.rs` |
| ISO/IEC 9075:1992 §21.3.23, §21.3.24 the character set SQL_TEXT | form-of-use UTF-8, 1,112,064 characters (every Unicode scalar value), PAD SPACE | `system/definition.rs` |
| ISO/IEC 9075:1992 §21.3.5 how data types are described | an exact number as NUMERIC, radix 10; an approximate one as REAL (24 bits) or DOUBLE PRECISION (53), radix 2; a character string's octet length 4n | `system/definition.rs` |
| ISO/IEC 9075:1992 §21.3.13 names of unnamed constraints | the table's name, the constraint's kind and a number: `T_PRIMARY1`, `T_NOTNULL2` | `db/constraints.rs` |
| Memory | MEMORY_BUDGET 256 MiB; a file database's buffer pool, STORAGE.POOL_BYTES, 32 MiB; the pool of an in-memory engine or index method 2 MiB; WORK_MEMORY, what an operator holds before it spills, 4 MiB, at least 64 KiB, a session's to set; SPILL_BATCH, the bytes a spilled run is written in, 16 KiB, from 4 KiB to 1 MiB (RULES.md D38) | `memory.rs`, `db/files.rs`, `engine.rs`, `spill.rs` |

---

## Limits

SQL-86 sets no limits beyond its implementor-defined choices, and §3.4
requires conforming SQL to be processed. A statement past one of these
limits is valid SQL that this implementation does not process: it gets
SQLCODE -900 (RULES.md D18).

| Limit | Value | Why | Where |
|---|---|---|---|
| BIT and BIT VARYING length | 262,136 bits (32,767 octets) | ISO/IEC 9075:1992 [§6.1 SR3] requires only a length above 0. | `value.rs`, `parser/data_type.rs` |
| CHARACTER length | 32,767 characters | [§5.5 SR2] requires only a length above 0. Values are stored padded to their length [§8.7 GR7b], so without a limit `CHAR(4000000000)` could allocate about 4 GB for each value. | `value.rs`, `parser/data_type.rs` |
| Exact precision | 18 digits, for NUMERIC, DECIMAL and exact literals | [§5.5 SR6, SR7] and [§5.2 SR4] set no maximum. | `value.rs`, `parser/data_type.rs` |
| FLOAT precision | 53 binary digits; approximate literals within the range of a double | [§5.5 SR10] allows more precision than asked for, but sets no maximum. | `parser/data_type.rs`, `parser.rs` |
| `<unsigned integer>` | 4,294,967,295 | [§5.2] sets no maximum. | `parser.rs` |
| Index entry | An encoded key plus an 8-byte row id. A B-link tree index holds 2,020 bytes at the default 8 KiB page, 996 at 4 KiB, and 16,356 at 64 KiB. A hash index holds 8,136 bytes at 8 KiB and 65,480 at 64 KiB. These figures assume the default 4-byte CRC-32C checksum. | A B-link node must hold at least four entries to split. A row's long values go out of line, but an index key is compared in its node, so it stays whole. A table's rows are not limited this way. STORAGE.PAGE_SIZE, chosen when the database is created, raises the limit. | `index/blink.rs`, `index/hash.rs`, `engine/tree.rs` |
| Nesting depth | 64 levels of the syntax tree: about 30 nested parentheses, 60 chained `AND`, `OR`, `+` or `UNION`, or 60 tables in a FROM clause | Without a limit, deep nesting overflowed the stack and aborted the process. 64 levels run in half of Rust's default 2 MiB thread stack. | `parser.rs` |

---

## Readings of ambiguous text

- **Exact against approximate numbers** [§5.11 GR4]: "Numbers are compared
  with respect to their algebraic value." An approximate number's value is
  the binary value it holds, so `0.10 = 0.1E0` is false: the double nearest
  0.1 is 0.1000000000000000055511151231257827…. Comparison is exact both
  ways.
- **`A -5`** [§5.2, §5.3 SR1]: a sign written against digits is part of a
  literal, so `-5` could be one token and `A -5` a syntax error. It is
  parsed as a subtraction; `- -5` is refused [§5.9 SR2].
- **WITH CHECK OPTION** [§6.5 GR3a, §8.7 GR2, §8.11 GR5, §8.12 GR4]: SQL-92's
  [ISO/IEC 9075:1992 §11.19 SR13, GR11]. It is CASCADED unless LOCAL is
  written, and an update is checked against every view with the option that
  it spans, including through a view without it. SQL-86 checked only the
  WHERE clause of the view a statement names.
- **§5.25 GR3** is read literally: a grouped query whose select list has
  only set functions returns one row even when there are no groups,
  including when HAVING removed the only group. Later standards changed
  this.
- **§7.3 SR4**: an unqualified name that is both a column and a declared
  parameter means the parameter.
- **A grant option given to PUBLIC** [§6.6 SR3b ii]: anyone may pass on
  a privilege through it, as SQL-92 has it [ISO/IEC 9075:1992 §10.3 GR2,
  §11.37 SR7a ii], where SQL-86 took only a grant naming the grantor
  itself; for GRANT and REVOKE alike (RULES.md D47).
- **A bit string's descriptor** [ISO/IEC 9075:1992 §21.3.5]: the
  description gives a bit string type its length in bits, and an octet
  length and a collation only to a character string type, but the check
  constraint TABLE_OR_DOMAIN_CHECK_COMBINATIONS requires all three of
  BIT and BIT VARYING. Its length in bits and the octets those take are
  given, as facts of it; no collation, as a bit string has none.
- **Bit strings brought together** [ISO/IEC 9075:1992 §9.3 SR3b]: the
  result of CASE, a set operation or a table value constructor of BIT(3)
  and BIT(4) is BIT(4), but no rule extends a value of 3 bits to it. Each
  keeps its bits, so a BIT(3) value inserted into a BIT(4) column through
  VALUES is refused (SQLSTATE 22026), as it is alone [§9.2 GR3h].
- **A time or timestamp without time zone** [ISO/IEC 9075:1992 §4.5.1,
  §6.1 SR5]: SQL-92 keeps it in UTC, the session's displacement implied,
  so it is kept so, and a value retrieved shows in the displacement of the
  session that retrieves it (RULES.md D54); stored in a column WITH TIME
  ZONE, it takes that displacement. A datetime literal in a `<default
  clause>` without one is of UTC [§5.3 SR18]: a definition is read again
  where no session is.
- **One datetime type** [ISO/IEC 9075:1992 §9.3 SR3e, §11.5 SR3c]: "the
  same datetime type" is read as of the same fields, WITH TIME ZONE or
  not, as store assignment takes either [§9.2 SR2]: the result of CASE, a
  set operation or VALUES is WITH TIME ZONE if one is, of the greatest
  precision; DEFAULT CURRENT_TIME is taken by a TIME column.
- **A datetime's or an interval's literal in CAST** [ISO/IEC 9075:1992
  §6.10 GR5e, GR6e, GR9a, GR12b]: "the shortest character string that
  conforms to the definition of `<literal>`" is read as the literal's
  string, the part between its quotes, an interval's sign before it:
  CAST(DATE '2024-01-02' AS CHAR(10)) is '2024-01-02', and CAST from a
  string takes that string. An interval's fields after the leading one
  are of any digits, `<datetime value>` being an `<unsigned integer>`
  [§5.3]: SR21's two digits are of a datetime literal's.
- **AT TIME ZONE** [ISO/IEC 9075:1992 §6.14 GR4]: the value is "DV - TZ",
  DV the datetime in UTC; it is read as the same instant shown in the
  displacement TZ, of the type WITH TIME ZONE, as a datetime WITH TIME
  ZONE keeps its own [§4.5.1].
- **OVERLAPS with an interval** [ISO/IEC 9075:1992 §8.11 SR3a]: "such that
  the interval can be added to the datetime" is read as §6.14 SR3 says
  of addition: the interval's fields are the datetime's.
