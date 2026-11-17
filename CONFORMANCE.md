# Conformance

This file says what turborel implements of the SQL standards, and which
choices it makes where the standards leave them open. The standards are read
as one language (RULES.md D32). **ANSI X3.135-1986** ("SQL-86", FIPS PUB 127)
comes first, extended by **ISO/IEC 9075:1992** ("SQL-92") and
**ISO/IEC 9075-2:2023** ("SQL:2023"). Where they differ, the newer one wins.

A citation like `§11.19 SR13` means clause 11.19, Syntax Rule 13. **F** is a
Format (the BNF), **SR** a Syntax Rule, **GR** a General Rule and **AR** an
Access Rule.

In [the SQL-86 sections](#sql-86-clause-by-clause) a bare citation is SQL-86.
Everywhere else a bare citation is SQL-92, and one marked `86` is SQL-86, as
in `86 §5.5 SR3`. One marked `2023` is ISO/IEC 9075-2:2023, as in
`2023 §9.15 SR4e`. The code spells them out, as `[X3.135-1986 §8.6 GR2]`,
`[ISO/IEC 9075:1992 §11.19 SR13]` and `[ISO/IEC 9075-2:2023 §9.15 SR4e]`.

Contents: [The claim](#the-claim) ·
[SQL-86, clause by clause](#sql-86-clause-by-clause) ·
[SQL-92](#sql-92) · [Collations](#collations) · [SQL:2023](#sql2023) ·
[SQL/PSM](#sqlpsm) · [Extensions](#extensions) ·
[Implementation-defined choices](#implementation-defined-choices) ·
[Limits](#limits) · [Readings of ambiguous text](#readings-of-ambiguous-text)

---

## The claim

The claim follows SQL-86 §3.4.

| Part | Claim |
|---|---|
| Type | Full SQL conformance to Level 2, for SQL-DDL (`<schema>`, through `Database::schema`) and SQL-DML. |
| Facilities | Direct processing of SQL statements through the Rust binding (`Session`, `Params`). §3.4 leaves the method and time of binding to the implementation. The module language and embedded SQL (COBOL, FORTRAN, Pascal, PL/I) are not claimed. |
| Environment [§4.6, §4.7] | A database kept in a directory (`Database::open_files`) keeps its schemas and committed changes, and is recovered after a failure [§4.16]. A database made with `Database::new` is held in memory and ends with the process. |
| Limits | SQL-86 sets one limit of its own, 18 characters in an `<identifier>` [§5.3 SR3], which SQL-92 raises to 128. A statement past one of this implementation's [limits](#limits) gets SQLCODE -900 ("valid, but not processed"). |

Every outcome has an SQLSTATE beside its SQLCODE [ISO/IEC 9075:1992 §22.1].

SQL-92 changes SQL-86 in several places, and here SQL-92 wins.

| SQL-86 | Here |
|---|---|
| Fewer reserved words | SQL-92's reserved words are key words, so SQL-86 text that names something `VALUE`, `ADD` or `COLUMN` (and others) writes it as a delimited identifier, `"VALUE"`. |
| Set functions of zero groups give one row [§5.25 GR3] | A query grouped by GROUP BY or HAVING gives no row. See [Queries and expressions](#queries-and-expressions). |
| A grant beyond what the grantor holds is refused [§6.6 SR5] | It grants what it can, with a warning. See §6.6 below. |
| WITH CHECK OPTION tests the named view's own WHERE clause | It cascades. See [Readings](#readings-of-ambiguous-text). |
| A grant option through PUBLIC counts only for a grant naming the grantor | It follows SQL-92. See [Readings](#readings-of-ambiguous-text). |

---

## SQL-86, clause by clause

**Done** means the Formats, Syntax Rules and General Rules are implemented
and tested. **Not claimed** means the clause is outside the claim above.

`cargo xtask trace '§8.6'` lists the tests that verify a clause. §3.2 is
notation, so it has none.

### 3. Overview, and 9. Levels

| Clause | Status | Notes |
|---|---|---|
| §3.2 Notation, §3.3 Conventions | Done | The BNF is followed production by production. A Format error is SQLCODE -102. A Syntax Rule error is -103, but -104 for a privilege, and -101 for the characters and tokens of §5.1 and §5.3. |
| §3.4 Conformance | Done | See [the claim](#the-claim). |
| §9 Levels | Done | Level 2 throughout. No Level 1 checking mode is provided. |

### 4. Concepts

| Clause | Status | Notes |
|---|---|---|
| §4.1 Sets, §4.2 Data types, §4.3 Columns, §4.4 Tables | Done | Base, viewed and grouped-view tables. |
| §4.5 Integrity constraints | Done | NOT NULL and UNIQUE, checked after each statement. |
| §4.6 Schemas, §4.7 The database | Done | Kept in a directory, or held in memory. |
| §4.8–§4.11 Modules, procedures, parameters, languages | Not claimed | A `Session` with `Params` is the binding §3.4 allows. Module text is not parsed, and a Rust library binds to no host language. |
| §4.12 Cursors | Done | Updatable cursors are working viewed tables, and read-only cursors working base tables. See [cursors](#cursors-1). |
| §4.13 Statements, §4.14 Embedded syntax | Done | Every `<SQL statement>` runs. Embedded SQL is not claimed. |
| §4.15 Privileges | Done | Owners and `<privilege definition>`s. |
| §4.16 Transactions | Done | Atomic, serializable, and atomic with respect to recovery for a database kept in a directory (RULES.md D25). Separate sessions run on many threads at once (D27). The built-in engines isolate transactions with versions (D29, D31). |

### 5. Common elements

| Clause | Status | Notes |
|---|---|---|
| §5.1 `<character>` | Done | Unicode. End of line is `\n` or `\r\n`. |
| §5.2 `<literal>` | Done | |
| §5.3 `<token>` | Done | Key words in any case, after SQL-92. |
| §5.4 Names | Done | Module and procedure names belong to the module language, not claimed. |
| §5.5 `<data type>` | Done | |
| §5.6 `<value specification>` | Done | Parameters with indicators, literals and USER. `<variable specification>` belongs to embedded SQL and gets SQLCODE -900, or -102 as a target of GET DIAGNOSTICS. |
| §5.7 `<column specification>` | Done | Outer references, looked up in the most local scope first. |
| §5.8 `<set function specification>` | Done | Over outer references too. |
| §5.9 `<value expression>` | Done | |
| §5.10 `<predicate>` | Done | |
| §5.11 `<comparison predicate>` | Done | A subquery gives at most one value. More is SQLCODE -811. |
| §5.12 `<between predicate>` | Done | |
| §5.13 `<in predicate>` | Done | Value lists and subqueries. |
| §5.14 `<like predicate>` | Done | With ESCAPE. A null escape character gives unknown, as ISO/IEC 9075:1992 §8.5 says. |
| §5.15 `<null predicate>` | Done | |
| §5.16 `<quantified predicate>` | Done | |
| §5.17 `<exists predicate>` | Done | |
| §5.18 `<search condition>` | Done | Three-valued logic, by the GR2 truth tables. |
| §5.19–§5.25 Table expressions and query specifications | Done | A subquery is effectively run for each row [§5.21 GR3] or group [§5.23 GR4]. Its rows are kept by outer values while they fit the work area, so it usually runs once for each set of them. §5.25 GR3 gives way to SQL-92, as [the claim](#the-claim) says. |

### 6. Schema definition language

| Clause | Status | Notes |
|---|---|---|
| §6.1 `<schema>` | Done | |
| §6.2 `<table definition>` | Done | |
| §6.3 `<column definition>` | Done | |
| §6.4 `<unique constraint definition>` | Done | Enforced with an index. |
| §6.5 `<view definition>` | Done | Not materialized. WITH CHECK OPTION as SQL-92 defines it. |
| §6.6 `<privilege definition>` | Done | On tables and views. A grantor holding no privilege on the table is refused, SQLCODE -104 [ISO/IEC 9075:1992 §11.36 AR1]. Any other grants what it holds with grant option, and a privilege it names beyond that is not granted, with a warning [§11.36 GR14], where SQL-86 refused one not applicable [SR5] (RULES.md D47). |

### 7. Module language

| Clause | Status | Notes |
|---|---|---|
| §7.1 `<module>` | Not claimed | `Session` plays the module's part. GR2 (the implicit COMMIT or ROLLBACK at the end) is the setting SESSION.COMMIT_AT_END. SR1 (one procedure opens each cursor) is not checked. |
| §7.2 `<module name clause>` | Not claimed | |
| §7.3 `<procedure>` | Not claimed | Parameters, the implicit transaction (GR2) and the SQLCODE outcomes (GR3) are implemented. Procedure text and the SQLCODE parameter declaration are not. |

### 8. Data manipulation language

| Clause | Status | Notes |
|---|---|---|
| §8.1 `<close statement>` | Done | |
| §8.2 `<commit statement>` | Done | Atomic across engines. |
| §8.3 `<declare cursor>` | Done | ORDER BY, UNION, updatability, and any SQL-92 query expression. |
| §8.4 `<delete statement: positioned>` | Done | |
| §8.5 `<delete statement: searched>` | Done | |
| §8.6 `<fetch statement>` | Done | Indicators, truncation, SQLCODE 100. |
| §8.7 `<insert statement>` | Done | VALUES and query expressions, and into updatable views. |
| §8.8 `<open statement>` | Done | |
| §8.9 `<rollback statement>` | Done | |
| §8.10 `<select statement>` | Done | |
| §8.11 `<update statement: positioned>` | Done | |
| §8.12 `<update statement: searched>` | Done | |

### Annexes A–F (embedded SQL)

Not claimed. Annex A says it "is not an integral part of the body of the
standard", and §3.4 lets a claim name its embedded languages. This one names
none.

---

## SQL-92

This section lists what of ISO/IEC 9075:1992 is implemented beyond SQL-86.
Where SQL-92 keeps a restriction of SQL-86 for Entry or Intermediate SQL
alone, the restriction is lifted.

### Lexical elements and errors

| Feature | Rules | What turborel does |
|---|---|---|
| Key words | §5.2 | SQL-92's reserved words, in any case. |
| Identifiers | §5.2 SR8 | Regular identifiers fold to upper case, up to 128 letters and digits, underscores not counted. Delimited identifiers too. |
| String literals | §5.3 | Empty and continued literals. |
| `<introducer>` | §5.3, §5.4 | Naming SQL_TEXT, of a literal or an identifier. |
| End of a statement | §20.1 | A direct statement ends in a semicolon, or none. |
| Valid SQL not implemented | RULES.md D18 | Each construct not taken is read by its Format in full and refused with SQLCODE -900, naming what is not implemented. SQL that breaks a Format is a syntax error (-102), whatever it names. The exception is `:P`, refused where it stands. |
| SQLSTATE | §22.1 | Beside every SQLCODE, with its subclass where the standard gives one. |

### Direct invocation of SQL

Direct SQL [§20.1, §20.2] is `Session::direct`, every statement of a server's
client (RULES.md D42), and the command-line client `turborel` (D46).

| Statement | Outcome |
|---|---|
| A query expression, with an optional ORDER BY | Its rows as they are fetched. None is "no data" [§20.2 GR3]. |
| A schema definition | Processed as the session's own. |
| Any other | Its completion. |

### Dynamic SQL

Prepared statements come in two forms, both with `<dynamic parameter
specification>`s [§6.2]. Their types are deduced as §17.6 GR5 says, and
refused where it gives none [GR4].

| Form | Rules | What turborel does |
|---|---|---|
| The Rust API | §17.6, §17.8–§17.10, §17.13, §17.14 | `Session::prepare`, `execute_prepared`, `open_prepared`. Takes a statement with a `<comment>`, as drivers send them (RULES.md D58). |
| A server's protocol | same | RULES.md D45. Takes a `<comment>` too. |
| SQL text | §17.1–§17.20 | Run by a session with its program's parameters, the session playing the module (D58). PREPARE refuses a `<comment>` [GR4q]. |

The statements of SQL text are these.

| Statements | Rules |
|---|---|
| PREPARE | §17.6 |
| DEALLOCATE PREPARE | §17.7 |
| DESCRIBE INPUT and OUTPUT | §17.8 |
| EXECUTE, with arguments or a descriptor area, and of a `<dynamic single row select statement>` INTO them | §17.9, §17.10 |
| EXECUTE IMMEDIATE | §17.11 |
| ALLOCATE, DEALLOCATE, GET and SET DESCRIPTOR, every field of Table 17 and the codes of Tables 18–20 | §17.1–§17.5 |
| `<dynamic declare cursor>`, through `Session::declare_cursor` | §17.12 |
| ALLOCATE CURSOR | §17.13 |
| Dynamic OPEN, FETCH, CLOSE, and positioned DELETE and UPDATE | §17.14–§17.18 |
| A positioned DELETE or UPDATE prepared, of no table too | §17.19, §17.20 |

Names of statements, cursors and descriptor areas are the session's, LOCAL
and GLOBAL ones apart [§5.4]. Direct SQL has no parameters [§20.1 SR1], so
there only the statements whose operands may be `<literal>`s are of use.

### Schemas and constraints

| Feature | Rules | What turborel does |
|---|---|---|
| Schema definitions | §11.1, §20.1 GR6b ii, iii | `CREATE SCHEMA S AUTHORIZATION U`, `CREATE SCHEMA S` owned by the session's authorization identifier [SR2b], or SQL-86's `CREATE SCHEMA AUTHORIZATION U`. The name may be qualified by the catalog's. What is in it is its owner's, and its unqualified names are of it. Run by a session, it is part of its transaction (RULES.md D33). |
| Names qualified by the catalog | §5.4, §6.4, §7.9 | A table's, view's, domain's, constraint's or assertion's, `CAT.S.T`, a column reference's qualifier's, `CAT.S.T.C`, and a `<qualifier>.*`'s. The catalog is the one there is. Another is refused (SQLCODE -103). |
| Constraints | §11.4, §11.6–§11.9, §10.6 | NOT NULL, UNIQUE (whose columns may be null), PRIMARY KEY, FOREIGN KEY with MATCH FULL, PARTIAL or neither and every referential action, RESTRICT too (ISO/IEC 9075-2:2023 §15.18, Feature T191), and CHECK, with subqueries too. Each is named, by the definition or the implementation. |
| A constraint's schema | §5.4 SR8 | A name written without a schema is of the `<schema definition>`'s schema, else of the session's default schema, which may differ from its table's or domain's (RULES.md D47). A declared local temporary table's is of MODULE. |
| When constraints are checked | §14.2 | At the end of each statement. Deferred ones, by SET CONSTRAINTS or INITIALLY DEFERRED, at commit. |
| Defaults | §11.5 | A literal, NULL, USER, CURRENT_USER, CURRENT_ROLE, SESSION_USER, SYSTEM_USER, or a datetime value function. Used by INSERT, DEFAULT VALUES, `SET ... = DEFAULT`, columns added, and the referential action SET DEFAULT. |
| DROP SCHEMA, DROP TABLE, DROP VIEW | §11.2, §11.18, §11.20 | CASCADE and RESTRICT, inside transactions (RULES.md D33). DROP SCHEMA RESTRICT is refused, and CASCADE drops them, while constraints named in the schema are of tables or domains of another (D47). RESTRICT is refused while the schema holds a collation [§11.2 SR3]. |
| ALTER TABLE | §11.10–§11.17 | ADD and DROP COLUMN, ALTER COLUMN defaults, ADD and DROP CONSTRAINT, CASCADE and RESTRICT, inside transactions (D33). |
| Views | §11.19, §11.11 GR2 | Of any query expression. WITH LOCAL or CASCADED CHECK OPTION, checked against every view an update spans [GR11]. A view keeps the columns it saw when defined, whatever columns are added later. |
| CREATE TABLE, CREATE VIEW, GRANT, REVOKE on their own | §11.3, §11.19, §11.36, §11.37, §20.1 | In a transaction (RULES.md D33, D47). |

Privileges follow these rules.

| Feature | Rules | What turborel does |
|---|---|---|
| The REFERENCES privilege | §10.3, §11.8 AR1, §11.9 AR1 | A constraint's condition needs REFERENCES on each column it names. Where it names no column, it needs REFERENCES on at least one column of each table it reads [§11.9 AR1a, AR1b]. |
| REFERENCES on a view | §11.19 GR6b, §4.9, §7.5 SR6d, §7.3 SR2, §7.9 SR5, §7.10 SR9a, SR16 | A view's owner holds REFERENCES on each column whose underlying columns it holds REFERENCES on, if it holds REFERENCES on some column of each leaf underlying table (RULES.md D52). So a CHECK or an assertion may read a view. |
| INSERT on columns | §10.3 GR4, §13.8 AR1 | Implemented. |
| A column added later | §10.3 GR3–GR5, §11.11 GR3 | INSERT, UPDATE and REFERENCES held on every column of a table extend to the column added. That is a grant on the table, or one listing all its columns, and the second goes beyond the standard. |
| GRANT | §11.36 GR14, GR15 | Gives what its grantor holds with grant option and warns of the rest (SQLSTATE 01007), in a schema definition too. |
| REVOKE | §11.37 SR18, GR1–GR6 | With or without GRANT OPTION FOR. It destroys what stood on what it revokes, the views, table and domain constraints and assertions its owners could no longer have, with CASCADE, or refuses with RESTRICT. |
| What REVOKE did not revoke | §11.37 GR8, GR9 | Warns (SQLSTATE 01006) for every grantee and action it did not revoke. |
| Cursor operation conflict | §11.8 GR8 | A referential action on a row a cursor still open updated warns (SQLSTATE 01001). |

### Queries and expressions

| Feature | Rules | What turborel does |
|---|---|---|
| CASE, NULLIF, COALESCE | §6.9 | Implemented. |
| Scalar and row subqueries | §7.11 | Implemented. |
| Row value constructors | §7.1 | In comparisons, BETWEEN, IN, IS NULL, quantified comparisons and MATCH. |
| Predicates | §8.5, §8.9, §8.10, §8.12 | LIKE of any character strings, IS TRUE, FALSE and UNKNOWN, UNIQUE and MATCH. |
| Joined tables | §7.5, §6.3 | CROSS, INNER, LEFT, RIGHT, FULL and UNION joins, with ON, NATURAL or USING, nested and in parentheses. Derived tables with AS and column lists. |
| Query expressions | §7.10, §7.2 | UNION, EXCEPT and INTERSECT, ALL or not, CORRESPONDING. Table value constructors of many rows. TABLE t. |
| Updatable queries with subqueries | §7.9 SR12d | Also searched UPDATE and DELETE that read the table they change. |
| String functions and `\|\|` | §6.6, §6.7, §6.13 | Implemented. |
| Store and retrieval assignment | §9.1, §9.2 | A string retrieved into a parameter too short for it is cut to fit, its indicator holding its length, with the warning 01004 [§9.1 GR2a, GR3b, GR3e, GR3g, GR3j]. |

**CAST** [§6.10] works among the supported types. A constant is not folded
where its CAST warns, so the warning belongs to the statement that runs it.

| Case | Outcome | Rules |
|---|---|---|
| Cuts a string, a character not a space | Warning 01004 | §6.10 GR5c ii, GR6c ii |

**Set functions** [§6.5] take a value expression holding no set function or
subquery, including over outer references [SR4, SR5].

| Case | Outcome | Rules |
|---|---|---|
| One that eliminates a null | Warning 01003, read by batches or row by row | GR1b |
| SUM | Its total alone is judged against the range of its result, 22003 out of it, whatever order the rows come in | GR2b iv |
| AVG | Its mean is judged the same way | GR2b ii |

**Grouped tables of 0 groups** follow SQL-92 [§7.9 GR1b i], not SQL-86.

| Query | Result | Rules |
|---|---|---|
| GROUP BY of no rows | No groups, so no row, even of a select list of only set functions | §7.7 GR1, §7.9 GR1b i |
| HAVING without GROUP BY | T is a single group, even of no rows, which HAVING keeps or removes | §7.8 GR1, GR2 |
| Not grouped | One row of its set functions, even of no rows | §7.9 GR1a i |

**GET DIAGNOSTICS** [§18.1] reads the diagnostics area [§4.19]. It runs only
in a program that embeds turborel, since its targets are parameters.

| Item | Rules | What it gives |
|---|---|---|
| Statement items | §18.1 GR1 | All of them. MORE is Y when the statement raised a condition the area does not hold. |
| Condition items | §18.1 GR3 | All of them. A name a condition does not have is a zero-length string. |
| CONSTRAINT_* | GR3f i | The constraint or assertion broken, and the foreign key of a 27000. Of a CAST, the domain's constraint. |
| CATALOG_NAME, SCHEMA_NAME, TABLE_NAME of a constraint | GR3f ii | A table constraint's table. Of an assertion, the one table the statement modified of those it reads, none if it modified several or the check came at commit. |
| Names of a syntax error or access rule violation | GR3g | One a query names that is not defined or has no column it names, a privilege is refused on, or a foreign key references. Else the table a statement inserts into, changes, defines, alters, drops, indexes or grants on, or a `<schema definition>` defines. COLUMN_NAME is the column of a privilege refused, or that a table lacks. |
| A declared local temporary table's CATALOG_NAME | GR3f ii, GR3g i | One space. |
| Withheld names | GR3j | A table the session holds no privilege on, and its constraints. An assertion unless the session owns its schema. |
| CONNECTION_NAME, SERVER_NAME | GR3n | `DEFAULT`. See [Readings](#readings-of-ambiguous-text). |

### Data types

**CHARACTER VARYING** [§6.1], with CHAR VARYING and VARCHAR, takes a length.
Written without one, as SQL:2023 allows (Feature T081), it is of the
longest string, 32,767 characters.

| Use | Behaviour | Rules |
|---|---|---|
| A value | Kept as assigned, unpadded | §9.2 GR3d, GR3e; §9.1 GR3d, GR3e |
| Comparing, indexing, uniqueness | Padded with spaces | §8.2 GR3a |
| CAST to it | Implemented | §6.10 GR6 |
| SUBSTRING, TRIM | Of a varying type | §6.7 SR2a, SR6e |
| UPPER, LOWER | Their operand's type | §6.7 SR3a |
| `\|\|` | Varying if either operand is | §6.13 SR3a |
| CASE, set operations | Varying if any is | §9.3 SR3a |
| A dynamic parameter in a string's place | CHARACTER VARYING(32767) | §17.6 GR5 |

**NATIONAL CHARACTER** [§6.1], NATIONAL CHAR and NCHAR, fixed or VARYING, and
the literal `N'...'` [§5.3] are the character string types and literals of the
national character set [§6.1 SR2].

**Datetimes** [§4.5.1, §6.1] are DATE, TIME(p) and TIMESTAMP(p), WITH TIME
ZONE or not (RULES.md D54).

| Use | Behaviour | Rules |
|---|---|---|
| Literals | `DATE '...'`, `TIME '...'`, `TIMESTAMP '...'` | §5.3 |
| Storing, comparing, indexing, uniqueness | Kept in UTC, by their instants | §8.2 GR5 |
| Assignment | To a datetime of their fields | §9.2 SR2, GR3l |
| CURRENT_DATE, CURRENT_TIME, CURRENT_TIMESTAMP | One instant in a statement | §6.8 |
| CAST | To and from character strings, and between datetimes | §6.10 SR4, GR5e, GR6e, GR9–GR11 |
| EXTRACT | A datetime's fields and displacement | §6.6 |
| CASE, set operations | Of one datetime type | §9.3 SR3e |
| Defaults | A datetime literal, of the defining session's displacement if it writes none, or a datetime value function | §5.3 SR18, §11.5 SR3a v, SR3c, GR1d |

**Intervals** [§4.5.2, §6.1, §10.1] are INTERVAL of a qualifier, year-month or
day-time (RULES.md D54).

| Use | Behaviour | Rules |
|---|---|---|
| Literals | `INTERVAL '...'` | §5.3 |
| Comparing, indexing, uniqueness | By their months or microseconds | §8.2 GR6 |
| Assignment | In a column's fields, 22015 if they leave it | §9.2 GR3m |
| Datetime plus or minus an interval | Of its fields, 22008 of no valid datetime | §6.14 |
| Interval arithmetic | Added, and multiplied and divided by numbers | §6.15 GR1–GR5 |
| `(A - B)` and a qualifier | The difference of two datetimes | §6.15 GR6 |
| AT LOCAL, AT TIME ZONE | Implemented | §6.14 GR4 |
| CAST | To and from strings, exact numbers of one field, and intervals of their class | §6.10 SR4–SR6, GR3c, GR12 |
| EXTRACT | An interval's fields | §6.6 |
| SUM, AVG | Of intervals | §6.5 SR9e |
| CASE, set operations | Of one class | §9.3 SR3f |
| Defaults | An interval literal | §11.5 SR3a vi |
| OVERLAPS | Implemented | §8.11 |
| SET TIME ZONE | Implemented | §16.5 |

**BIT and BIT VARYING** [§6.1], and the bit string literal `B'...'`
[§5.3], are not implemented. SQL:2003 dropped them and SQL:2023 has none,
so the newer text wins (RULES.md D32). `X'...'` is SQL:2023's binary
string literal, and BINARY and BINARY VARYING take the place of the bit
strings (see [Binary strings](#binary-strings)). So do its warning 01008,
implicit zero-bit padding, and its exception 22026, string data, length
mismatch, which nothing raises now. BIT_LENGTH stays, counting eight bits
to each octet of a character or binary string.

### Temporary tables

| Kind | Rules | What turborel does |
|---|---|---|
| Global and created local | §4.9, §11.3 | Each session has its own rows, ON COMMIT DELETE or PRESERVE ROWS [§14.3 GR3]. They are written in READ ONLY transactions too [§13.6–§13.10 GR1]. |
| Which tables each may refer to | §11.8 SR4, SR5; §11.9 SR3, SR4 | Implemented. |
| Their privileges | §10.3 SR2 | Implemented. |
| ALTER TABLE of one | §11.10, §11.11 GR2, §11.15 GR4, §11.16 GR2 | Allowed while no other transaction has a session's instance's rows open. A column added or dropped changes each session's rows, and any other change applies to every session's at once. A constraint added is checked of each session's rows. |
| CREATE INDEX on one | RULES.md D49 | Each session's instance indexes its own rows. |
| Declared local | §13.11 | `DECLARE LOCAL TEMPORARY TABLE MODULE.T`, a session's own and in no view of the Information Schema. Never altered, dropped, granted on or named by a view [§10.3 SR1, §11.10 SR5, §11.18 SR3, §11.19 SR5]. |

### Domains

Domains are §11.21–§11.27 (RULES.md D50).

| Feature | Rules | What turborel does |
|---|---|---|
| CREATE DOMAIN | §11.21 | In a schema definition too, with a default and CHECK constraints over VALUE. No domain is in usage by its own constraints [SR4]. |
| A column of one | §11.4 | Takes its type, default and constraints. |
| ALTER DOMAIN | §11.22–§11.26 | SET and DROP DEFAULT, ADD and DROP CONSTRAINT. The rows there are checked. |
| DROP DOMAIN | §11.27 | RESTRICT or CASCADE. |
| CAST to one | §6.10 GR13 | Its constraints checked of the value. |
| The USAGE privilege | §10.3, §11.36, §11.37 SR13–SR16 | Granted and revoked. A revoke abandons the views, CHECKs, domain constraints and assertions that CAST to it in a schema left without it. |
| Information Schema | §21.2 | DOMAINS, DOMAIN_CONSTRAINTS, the domain of each column, and USAGE_PRIVILEGES. |

### Assertions

Assertions are §11.34, §11.35 (RULES.md D51).

| Feature | What turborel does |
|---|---|
| CREATE ASSERTION | In a schema definition too. Checked as it is made. |
| When it is checked | At the end of each statement that changes a table it reads, or at commit if deferred. SET CONSTRAINTS sets it. |
| How it is checked | From the rows changed where its form allows: one table, an inner join or a grouping, and a SUM or AVG of exact numbers judged of totals it keeps. Else its whole query runs. |
| DROP ASSERTION | Implemented. |
| Dropping what it reads | A table or column it reads is dropped only with CASCADE, which takes it, as DROP SCHEMA does. |
| Information Schema | ASSERTIONS, CHECK_CONSTRAINTS and the constraint usage views. |
| \[EXTENSION\] ASSERTION_TOTALS | The totals kept, in tables of DEFINITION_SCHEMA that no statement names [§21.1]. |

### Cursors

Cursors follow §13.1, §13.3.

| Feature | What turborel does |
|---|---|
| SCROLL | FETCH's every orientation (NEXT, PRIOR, FIRST, LAST, ABSOLUTE, RELATIVE) and FROM. |
| INSENSITIVE | Implemented. |
| `<updatability clause>` | FOR READ ONLY or FOR UPDATE [OF columns]. FOR UPDATE with ORDER BY or SCROLL too, not of OFFSET or FETCH FIRST [2023 §7.17 SR7]. |
| Positioned UPDATE | Sets only the columns it names, and none a sort key of its ORDER BY reads, through an expression too [§13.9 SR4, SR8]. |

### Transactions, sessions and the Information Schema

| Feature | Rules | What turborel does |
|---|---|---|
| SET TRANSACTION | §14.1, §4.28 | READ ONLY or READ WRITE, and the isolation level, for the next transaction. See the table below. |
| SET CATALOG | §16.1, §5.4 SR20 | Of the one catalog's name. Another is no valid `<catalog name>`, refused (SQLSTATE 3D000) [GR2]. |
| SET SCHEMA | §16.2 | The default schema of unqualified names. |
| SET SESSION AUTHORIZATION | §16.4, §4.30 | The session's authorization identifier, the value trimmed and read as an `<authorization identifier>`. Refused in a transaction. The default schema stays as it was. |
| Connections | §15 | CONNECT TO a server AS a name, USER a user, or TO DEFAULT. SET CONNECTION to a name or DEFAULT. DISCONNECT a name, DEFAULT, CURRENT or ALL. The client library is the SQL-agent (`turborel_client::agent`), and so the command-line client is. A server or a session refuses them (SQLCODE -900) (RULES.md D46). No transaction spans servers (SQLSTATE 0A001). |
| COMMIT and ROLLBACK | §14.3, §14.4 | Without WORK too. |
| The Information Schema | §21 | Its 23 views, with the standard's own definitions, over the Definition Schema (RULES.md D37). Each also shows what the enabled roles own or hold (see Roles). |

Isolation levels behave as follows (RULES.md D29). No level reads uncommitted
changes, and constraints are checked serializably at every level.

| Level | Reads | Lost updates |
|---|---|---|
| SERIALIZABLE | Is serializable | None |
| REPEATABLE READ | One snapshot | None |
| READ COMMITTED, READ UNCOMMITTED | One snapshot for each statement | A statement loses no update. Under LOCKS.WAIT ON a searched UPDATE or DELETE writes a row another transaction changed once that transaction ends, in the version it committed, if its search condition still holds of it. A value read by one statement and written back by a later one can overwrite a change committed between them, which §4.28 allows. |

### Not implemented

| What | Why | Outcome |
|---|---|---|
| Leap seconds | By choice. Table 10 of §6.1 allows a SECOND of 60 or 61. A minute is always 60 seconds here, so datetime arithmetic, whose results over a leap second are implementation-defined [§6.1 Table 10, note], is exact. | A literal of one is SQLCODE -103, a CAST of one SQLSTATE 22018. |
| The module language and embedded SQL | RULES.md D32 | A parameter written with a colon, `:P`, is SQLCODE -900, in dynamic SQL's statements too, where it would be an `<embedded variable name>`. |
| Character sets other than SQL_TEXT, and translations | Never (RULES.md D32) | SQLCODE -900 for CHARACTER SET of another, its `<introducer>`, CONVERT, TRANSLATE, CREATE and DROP of a character set or translation, a privilege on one, a schema's DEFAULT CHARACTER SET of another, and SET NAMES. |

---

## Collations

These follow ISO/IEC 9075-2:2023 (RULES.md D60), with Features F690 (collation
support), F692 (extended collation support) and F693 (SQL-session
collations). Citations here are of that edition unless marked.

| Collation | Order | Pad | Where |
|---|---|---|---|
| UCS_BASIC | Unicode scalar value [§4.3.6] | PAD SPACE | `value/collation/folded.rs` |
| SQL_TEXT | As UCS_BASIC (IA003) | PAD SPACE | `value/collation/folded.rs` |
| UNICODE | ISO/IEC 14651:2020 without delta: UCA 13.0.0, Shifted, four levels | NO PAD | `value/collation/unicode.rs` |
| CASE_INSENSITIVE | \[EXTENSION\] Each character's simple case folding, Unicode 13.0 | PAD SPACE | `value/collation/folded.rs` |
| UNICODE_<LOCALE> | \[EXTENSION\] UNICODE tailored by CLDR 38's rules, one for every collation type of every locale but those for search. A locale's default type is UNICODE_<LOCALE>, such as UNICODE_SV, UNICODE_ZH and UNICODE_ZH_HANT. Another is UNICODE_<LOCALE>_<TYPE>, such as UNICODE_DE_PHONEBOOK and UNICODE_ZH_STROKE. The root's are UNICODE_EMOJI and UNICODE_EOR | NO PAD | `value/collation/tailored.rs`, `value/collation/reorder.rs` |

| Feature | Rules | What turborel does |
|---|---|---|
| `<collate clause>` | §10.7, §6.1 SR3, 92 §10.5 | In a column or domain definition, after its type or its constraints, in a value expression, ORDER BY and GROUP BY. It needs USAGE on a collation a definition made [AR1]. Built-ins are of INFORMATION_SCHEMA, and an unqualified name is of them [§5.4 SR13a]. |
| Which collation applies | §9.15 | Explicit, then the session's, then implicit. Two explicit, or two implicit that differ, are refused (SQLCODE -103). |
| A literal, parameter, CAST or USER | \[EXTENSION\] 92 §4.2.3 | Yields to a column's collation, as SQL-92's Coercible did. §9.15 SR4e alone would refuse `C = 'x'` of a collated column. A subquery's column of no collation of its own yields too. |
| Comparisons | §9.15 | =, <>, <, >, BETWEEN, IN, CASE, NULLIF, quantified comparisons, MATCH, UNIQUE and join conditions. |
| ORDER BY, GROUP BY, DISTINCT, set operations | §9.15, 92 §8.2 GR3d | By the collation. A group shows its first row's value. |
| MIN, MAX, COUNT(DISTINCT) | §9.15 | By the argument's collation. |
| Foreign keys | §9.15 SR4a | A column and the one it references have one collation. MATCH PARTIAL of a collated column is refused (SQLCODE -900). |
| CREATE COLLATION | §11.43 | FROM an existing collation, with NO PAD or PAD SPACE, or its pad. In a schema definition too. EXTERNAL, DESC, DEFAULT and TRANSLATION, SQL-92's sources, are refused (SQLCODE -900). |
| DROP COLLATION | §11.44 | RESTRICT or CASCADE. Columns and domains revert to the default [GR2], each table's rows copied, and a constraint that no longer holds fails the statement. RESTRICT is refused while a view or constraint names it [SR4], and CASCADE takes them. A table of another owner is reverted too [GR2]. A table referencing one reverted after it is checked again. |
| DROP SCHEMA | 92 §11.2, §11.2 | Takes its collations as DROP COLLATION CASCADE does [92 GR5; GR5], the columns of other schemas' tables reverted first. RESTRICT is refused while it holds one [92 SR3]. |
| USAGE | 92 §10.3, §11.36, §11.37, §12.7 | Granted and revoked. A column or domain of a schema whose owner is left without USAGE is impacted [GR12n]. RESTRICT refuses it (SQLCODE -103, SQLSTATE 2B000) [GR12q], and CASCADE reverts it to the default [GR12ad], each table's rows copied as DROP COLLATION copies them. That schema's views, constraints and assertions naming the collation are abandoned [GR12a, c, d, f] and go with CASCADE. A grant whose grantor loses the grant option falls [92 §11.37 SR7]. |
| SET COLLATION, SET NO COLLATION | §19.10, §9.15 SR4c | The session's collation (ID236: SQL_TEXT's). A bad name is 2H000. It applies to direct statements and to statements as they are prepared, never to views, constraints or defaults. |
| COLLATION FOR | §6.4 GR13 | The name of a value's own collation, which COLLATE takes. Null if its operands' conflict. |
| Information Schema | 92 §21 | COLLATIONS, USAGE_PRIVILEGES and each column's and domain's collation. A column of a domain with a COLLATE of its own shows the domain's. |

Module collations (§9.15 SR4d) do not apply, as the module language is not
claimed. Dynamic SQL's descriptors show SQL_TEXT for a character string,
whatever its collation.

| Rule | Choice | Where |
|---|---|---|
| IA060, §8.5 LIKE under a collation | By each character's folding, of UCS_BASIC, SQL_TEXT and CASE_INSENSITIVE. Under UNICODE and its tailorings, whose characters contract and expand, a run of the pattern's characters matches a substring equal to it under the collation [GR3d]. | `binder/collation.rs`, `value/like/collated.rs` |
| §9.11 POSITION | Exact, by characters, not by the collation | `functions/strings.rs` |
| §4.3.6 a key's length | About 16 bytes for each character under UNICODE and half as many again under a tailoring, so an index's limit holds less text | `value/collation/unicode.rs`, `value/collation/tailored.rs`, `index/blink.rs` |
| §12.7 GR12l, an impacted collation | A collation made FROM another made one keeps only the built-in it orders by, so none is impacted when USAGE on its source is revoked | `db/collations.rs` |

---

## SQL:2023

What SQL:2023 adds beyond collations, written from ISO/IEC 9075-2:2023
(RULES.md D32). Citations here are of that edition unless marked.

T285 (Enhanced derived column names) is claimed. A select list item
that is a single SQL parameter reference is named for the parameter
[§7.16 SR17c, CR6], and so is one of an SQL variable of
[SQL/PSM](#sqlpsm), so a FOR's row and a local cursor name it. Such a
column has no counterpart in a table, so a cursor of it is not simply
updatable [SR25c] and opens read-only. A column of any other expression
without an `<as clause>` is unnamed, the name SR17d leaves
implementation-dependent (UV087).

### Truth values

Feature T031 (BOOLEAN data type).

| Feature | Rules | What turborel does |
|---|---|---|
| BOOLEAN | §4.6, §6.1 | A data type of a column, a domain, a parameter and a result. Its values are True and False, Unknown its null value [§4.6.1], True greater than False [§4.6.2]. A value is one byte in a row and in a key. |
| `<boolean literal>` | §5.3 SR47, GR14 | TRUE, FALSE and UNKNOWN, UNKNOWN the null value. A column's default may be one [§11.5 SR4a vii]. |
| `<boolean value expression>` as a value | §6.29, §6.46 | Where a select list, a VALUES row, an UPDATE's SET, a CASE result, CAST, COALESCE, NULLIF or a set function takes a `<value expression>`, and in parentheses. A comparison's operands are `<common value expression>`s, so a condition compared is parenthesized. |
| `<boolean predicand>` | §6.46 SR3 | A value alone, no predicate's operator after it, is a condition, and of type BOOLEAN, else SQLCODE -103. IS TRUE, FALSE and UNKNOWN test it [§6.46], and IS NULL is its null predicate [§8.8]. |
| CASE and COALESCE | §9.5 SR3f | Their results are all BOOLEAN or none, else SQLCODE -103. |
| Assignment | §4.5.2, §4.6.2, §9.1 SR3, §9.2 SR3 | A truth value to a BOOLEAN site alone, and a number to a numeric one alone, else SQLCODE -103. |
| CAST | §6.13 GR11f, GR12f, GR21 | A character string casts to BOOLEAN as a `<boolean literal>`, its spaces trimmed and in any case, else SQLSTATE 22018. A truth value casts to a string as 'TRUE' or 'FALSE', 22018 if it is too short. No other type casts to or from BOOLEAN. |
| EVERY, ANY, SOME | §10.9 SR7e, GR7d | Set functions of a BOOLEAN argument, nulls left out, null of no values. ANY and SOME before a subquery are a quantifier [92 §8.7], before anything else the set function. |
| Descriptor areas | §20.1 Table 30, SR6j | TYPE 16, with no other field. |
| Information Schema | 92 §21 | A BOOLEAN column's DATA_TYPE is 'BOOLEAN'. |

SQL:2023 wins over SQL-92 here (RULES.md D32). A value alone as a
condition, `WHERE C`, was a syntax error (SQLCODE -104) and is now refused
only if C is not a BOOLEAN (SQLCODE -103). A condition's IS NULL,
`(C > 0) IS NULL`, was a syntax error and is now true where the condition
is unknown. ANY and SOME were quantifiers alone.

SQL:2023 reserves BOOLEAN and EVERY [§5.2], and SQL-92 does not, so both
stay identifiers, key words only where they are read: BOOLEAN as a data
type and EVERY before a parenthesis. A domain cannot be named BOOLEAN.

### Numeric literals

Feature T662 (underscores in numeric literals). An `<unsigned decimal
integer>` holds an `<underscore>` between two digits [§5.3], and is the same
without it [SR22], so it counts toward no precision. That holds of a
literal's whole part, fraction and exponent, and of a string CAST reads as
a number [§6.13 GR8b, GR9b]. A datetime's or an interval's string holds
none [SR44-SR46]. An underscore anywhere else ends the number, which a
`<separator>` or a `<delimiter token>` must follow (SQLCODE -101). The
non-decimal integers of Feature T661, `0X`, `0O` and `0B`, are not
implemented.

### Binary strings

Feature T021 (BINARY and VARBINARY data types), with Features T022
(advanced support for BINARY and VARBINARY data types), T023 (compound
binary literals) and T024 (spaces in binary literals).

| Feature | Rules | What turborel does |
|---|---|---|
| BINARY(n), BINARY VARYING(n), VARBINARY(n) | §4.4, §6.1 SR1, SR4, SR5, SR19-SR21 | Strings of octets. BINARY is BINARY(1), and VARBINARY is BINARY VARYING. A maximum length not written is the longest, 32,767 octets [SR6] (IL007), as Feature T081 allows. |
| `<binary string literal>` | §5.3 SR4-SR7, SR20 | X, in either case, then quoted pairs of hexits, spaces among them passed over. Parts separated by a newline are one literal. An odd number of hexits, or a character neither a hexit nor a space, is a lexical error (SQLCODE -101). Its type is BINARY of its octets (IV191). |
| Assignment | §9.2 GR3b viii-xii | BINARY(n) takes n octets, a shorter value padded with X'00', a longer one cut if the octets past n are all X'00', else 22001. BINARY VARYING(n) takes at most n, cut the same way. Only a binary string is assigned to one [SR3]. |
| Retrieved into a parameter | §9.1 GR5a, GR7h-GR7l | Cut to fit with 01004, the indicator its length, or a BINARY one padded with X'00' octets. |
| Comparing, indexing, uniqueness | §8.2 GR5 | Octet by octet, a string less than a longer one it begins (IA219), so trailing X'00' octets count (IA022). A key escapes each X'00' octet, so no key begins another. |
| `\|\|` | §6.32 SR6a, GR3 | BINARY if both operands are, else BINARY VARYING. A result past 32,767 octets keeps its first ones if the rest are X'00', else 22001. |
| SUBSTRING | §6.33 SR21, GR17 | Of BINARY VARYING, its octets. |
| TRIM, OVERLAY | §6.33 SR22, SR23, GR18, GR19 | BINARY VARYING of a binary string. TRIM takes X'00' if no trim octet is written, and a trim octet not of one octet is 22027. |
| `<octet like predicate>` | §8.5 SR2, GR4 | `_` and `%` are those characters' octets, and an escape octet makes either, or itself, stand for itself. An escape octet not of one octet is 2200D, a pattern with no partitioning by it 22025. A `?` among the operands is BINARY VARYING. |
| POSITION, OCTET_LENGTH | §6.31 GR3, GR6 | In octets. CHAR_LENGTH takes character strings alone. |
| CASE, set operations | §9.5 SR3b | BINARY VARYING if any is, BINARY if all are, of the greatest length. A binary string with any other type is SQLCODE -103. |
| CAST | §6.13 SR7, GR13 | From and to binary strings alone, else SQLCODE -103. Cut with 01004, or to BINARY padded with X'00' octets with no warning. |
| Defaults | §11.5 SR4a ii, GR2a iv, v | A binary string literal no longer than the column or domain, a BINARY one padded with X'00' octets, with no warning. |
| Descriptor areas | §20.1 Table 30, §20.5 GR4c iii 3, §20.10 GR7d ii | TYPE 60 of BINARY, 61 of BINARY VARYING. LENGTH and OCTET_LENGTH the length in octets. TYPE set sets LENGTH 1. |
| Information Schema | 92 §21 | DATA_TYPE 'BINARY' or 'BINARY VARYING', CHARACTER_MAXIMUM_LENGTH and CHARACTER_OCTET_LENGTH the length in octets. |

SQL:2023 wins over SQL-92 here (RULES.md D32). `X'...'` was a bit string
of four bits a hexit, and is now a binary string, so an odd number of
hexits is refused. CAST no longer takes a string's UTF-8 to bits and
back. BINARY and VARBINARY are no key words of SQL-92, so both stay
identifiers, read as data types where a data type is. A domain cannot be
named BINARY or VARBINARY. Binary large objects are
[large objects](#large-objects).

A collation's sort keys are binary strings, and an index on a collated
column keys them as BINARY VARYING (RULES.md D60).

### Large objects

Features T039 (CLOB locator: non-holdable), T040 (concatenation of
CLOBs), T041 (basic LOB data type support), T042 (extended LOB data type
support), T045 (BLOB data type), T046 (CLOB data type), T047 (POSITION,
OCTET_LENGTH, TRIM and SUBSTRING for BLOBs), T048 (concatenation of
BLOBs), T049 (BLOB locator: non-holdable), T050 (POSITION, CHAR_LENGTH,
OCTET_LENGTH, LOWER, TRIM, UPPER and SUBSTRING for CLOBs), T312 (OVERLAY
function) and T561 (holdable locators). No value of one is held whole,
in memory, in the log or on the wire. RULES.md D79 says how.

| Feature | Rules | What turborel does |
|---|---|---|
| CHARACTER LARGE OBJECT, NATIONAL CHARACTER LARGE OBJECT, BINARY LARGE OBJECT | §5.2 SR10, §6.1 SR1, SR10 to SR13, SR17 | CLOB, CHAR LARGE OBJECT, NCLOB, NCHAR LARGE OBJECT and BLOB too. A `<large object length>` of digits, with K, M or G as one token or two, of CHARACTERS or OCTETS of a CLOB. 4G if none is written (ID068), the longest (IL042, IL002, IL004), so T, P or more than 4G is SQLCODE -900, and 0 is -103. NATIONAL CHARACTER LARGE OBJECT is CHARACTER LARGE OBJECT, SQL_TEXT the one character set. |
| Assignment and CAST | §6.13 SR7, GR10 to GR13, §9.1, §9.2 GR3b, GR3e | To and from the strings of its family alone, else SQLCODE -103. Stored, a value longer than its site is 22001 unless what is past it is spaces, which are cut. Retrieved or cast, it is cut with 01004 unless what is cut is spaces. A dynamic parameter's value is cast to its type [§17.9 GR5g]. |
| Comparing | §4.1, §8.2, §9.11, §9.12 SR3, §9.14 SR3 | Equality alone. `=`, `<>`, IN, a quantified `=`, MATCH, EXISTS of an equality and a join's equality read both operands a chunk at a time. ORDER BY, GROUP BY, DISTINCT, a set operation not ALL, MAX, MIN, `<`, UNIQUE, and a key of a constraint or an index refuse one, SQLCODE -103. |
| `\|\|` | §6.32 SR3b i, SR6a i, GR2, GR3 | Of a large object and a string of its family, a large object of the sum of their lengths up to 4G, joined without a copy. |
| CHAR_LENGTH, OCTET_LENGTH | §6.30 GR4, GR5 | From the value's counts, nothing read. |
| POSITION | §6.31 SR2, GR2, GR3 | Searched a chunk at a time, of either operand, in characters or USING OCTETS. |
| SUBSTRING, OVERLAY | §6.33 SR4a ii, SR17, SR21b, SR23, GR3, GR17, GR19 | Large objects of its type, slices of the value, nothing copied. USING OCTETS of a CLOB takes whole characters (UV074). |
| TRIM, LTRIM, RTRIM, BTRIM, UPPER, LOWER, LPAD, RPAD | §6.33 SR9, SR14b ii, SR15a ii, SR16c ii, SR22, GR7, GR12, GR13, GR14, GR18 | Large objects of their argument's type, read from either end, or folded, a chunk at a time into the database's temporary store. A pad of a CLOB is a CLOB of 4G (IL046), its padding written to that store and joined to it uncopied, or the CLOB cut by a slice. |
| LIKE | §8.5 GR3d, GR4 | A CLOB or BLOB match value read a chunk at a time, under a collation of one character at a time too. Under a collation of contractions (IA060), a pattern, escape or match value longer than a chunk is SQLCODE -900, SQLSTATE 0A000, and so is a pattern or escape of a large object longer than one. SIMILAR is not implemented (T141). |
| Regular expressions | §6.31 GR18, GR19, §6.33 GR4b, GR9b, §8.7 | LIKE_REGEX, OCCURRENCES_REGEX, POSITION_REGEX, SUBSTRING_REGEX and TRANSLATE_REGEX read a CLOB subject a chunk at a time, and SUBSTRING_REGEX and TRANSLATE_REGEX of one are CLOBs. A pattern, flag or replacement longer than a chunk is SQLCODE -900. |
| LISTAGG | §10.9 SR11d, GR11g | Of CLOBs, CHARACTER VARYING of 32,767 characters (IA044), a longer value 22001 or left out under TRUNCATE. |
| Locators | §4.37.5, §14.17, §14.18 | A session's four-octet integer (UV065) of a value sent to a client or made by one a piece at a time. Valid until its transaction ends, FREE LOCATOR ending it sooner and HOLD LOCATOR keeping it past a commit. A rollback ends every one. FREE and HOLD take dynamic parameters [SR1]. One not valid is 0F001. |
| Descriptor areas | §20.1 Table 30 | TYPE 30 of BINARY LARGE OBJECT, 40 of CHARACTER LARGE OBJECT, LENGTH its length and OCTET_LENGTH its length in octets. The locator types 31 and 41 are not used, as no host language is. |
| Information Schema | 92 §21 | DATA_TYPE 'CHARACTER LARGE OBJECT' or 'BINARY LARGE OBJECT'. |
| Stored | RULES.md D79 | In chunks of 256 KiB, logged and checkpointed by chunk, collected with the versions that name them. The column store refuses a large object column. |

The non-holdable locators of T039 and T049 are the client protocol's
(D45). An embedded program reaches the same registry through the
`Session` API. T043 (multiplier T) and T044 (multiplier P) are not
claimed, as no length they write is processed.

### Simple CASE

Features F262 (extended CASE expression) and F263 (comma-separated
predicates in simple CASE expression). A simple CASE's WHEN takes a
`<when operand list>`, its `<when operand>`s separated by commas [§6.12],
and is true if any is, as `WHEN (CO EWO1) OR ... OR (CO EWOm)` [SR2i].

| Feature | Rules | What turborel does |
|---|---|---|
| `<case operand>` | §6.12 SR2c, CR1 | A `<row value predicand>`, a value or a row, or an `<overlaps predicate part 1>`. |
| A `<row value predicand>` as a `<when operand>` | SR2f i | True if the case operand equals it. A dynamic parameter among them takes the type of the first WHEN's first operand [92 §17.6 GR5k]. |
| A predicate's second part as a `<when operand>` | SR2f ii, CR2 | A comparison, BETWEEN, IN of a list or a subquery, LIKE, IS NULL, IS DISTINCT FROM, each with NOT where it takes one, a quantified comparison, MATCH and OVERLAPS, the case operand its first part. |
| OVERLAPS | SR2c | Every `<when operand>` of a CASE is OVERLAPS or none is, else SQLCODE -103. |

The case operand is evaluated once for each operand it is compared with,
as the equivalent searched CASE does. LIKE_REGEX is a `<when operand>`
too, with NOT or without. The predicates turborel has not, SIMILAR, IS
NORMALIZED, MEMBER, SUBMULTISET, IS A SET and IS OF, are no `<when
operand>`s either.

### DISTINCT predicate

Features T151 (DISTINCT predicate) and T152 (DISTINCT predicate with
negation). `X IS DISTINCT FROM Y` is true or false, never unknown
[§8.15 GR1], and IS NOT DISTINCT FROM is `NOT (X IS DISTINCT FROM Y)`
[SR6].

| Feature | Rules | What turborel does |
|---|---|---|
| Its operands | §8.15 SR1, SR3, SR5 | `<row value predicand>`s of one degree, each pair comparable and an operand of an equality operation [§9.11], else SQLCODE -103. An array may be one, as no ordering is needed. Strings compare under the collation §9.15 gives them, as for `=`. A bare NULL is no row value predicand [§7.2, §6.5], so it is refused as it is by `=`. |
| Its truth | §8.15 GR1 | Two nulls are not distinct and a null and a value are [GR1a, GR1b]. Two rows are distinct if a pair of fields is [GR1c iv], two arrays if their cardinalities differ or a pair of elements at one place is [GR1c v], other values if they are not equal [GR1c i, ii]. A distinct type's values compare as their source's. |
| A `<when operand>` | §6.12 SR2f ii | IS [NOT] DISTINCT FROM y, the case operand its first predicand. |
| Joins | §7.10 GR1 | A conjunct `a IS NOT DISTINCT FROM b` of a column of each operand is a hash key, on which a null matches a null, in memory, spilled and by batches, of collated strings by their sort keys. EXPLAIN shows it as `a IS NOT DISTINCT FROM b`. An index join probes on `=` keys alone. |

DISTINCT and FROM are key words of SQL-92 already, so nothing new is
reserved.

### GREATEST and LEAST

Feature T054 (GREATEST and LEAST). GREATEST and LEAST take two values or
more [§6.15], each comparable with each, else SQLCODE -103 [SR4]. Their
type is the values' [SR6, §9.5], and each is cast to it [SR7]. The result
is null if any value is [GR1], else the first value that is greater or
less than or equal to every other [GR3], so of two a collation holds
equal the first. Strings are ordered under the collation of all of
them [§9.14 SR1h, §9.15]. A dynamic parameter among the values takes the
first value's type, as one among COALESCE's does [92 §17.6 GR5j], which
§20 leaves unsaid; this is marked [EXTENSION]. GREATEST and LEAST are no
key words of SQL-92, so both stay identifiers, functions before a
parenthesis.

### Trims and pads

Features T055 (string padding functions) and T056 (multi-character TRIM
function). LTRIM, RTRIM and BTRIM remove each character of a set from
the start, the end or both of a string [§6.33 GR13], a space if no set
is written [SR15b]. LPAD and RPAD bring a string to a total length,
cutting it or adding the padding before or after it, repeated and cut, a
space if none is written [GR14, SR16a]. A total length is an exact
number of scale 0 [SR16b], else SQLCODE -103. A pad is CHARACTER VARYING
of 32,767 characters [SR16c] (IL046), of a CLOB a CLOB of 4G, and a trim
varying, as long as its string at most [SR15a]. A trim compares
characters under the collation of its string, TRIM of one character too,
and a trim or pad is of that collation [SR14d, SR15c, SR16e], whatever
that of its other strings. A dynamic parameter as a total length is
NUMERIC(P, 0), P the greatest precision, as SUBSTRING's length is [92
§17.6 GR5c], which §20 leaves unsaid; this is marked [EXTENSION]. The
five names are no key words of SQL-92, so each stays an identifier, a
function before a parenthesis.

### Optional maximum lengths

Feature T081 (optional string types maximum length). CHARACTER VARYING,
CHAR VARYING, VARCHAR, their national forms, BINARY VARYING and
VARBINARY may be written without a maximum length, and are then of the
longest string of their kind, 32,767 characters [§6.1 SR7] (ID069) or
32,767 octets [SR6] (IL007). That holds wherever a data type is written,
a column, a domain, a CAST's target. A varying value is stored at its own
length, so the longest maximum costs no space. SQL-92 required the
length [92 §6.1], and SQL:2023 wins here (RULES.md D32).

### BIGINT

Feature T071 (BIGINT data type).

| Feature | Rules | What turborel does |
|---|---|---|
| BIGINT | §4.5.2, §6.1 SR30 | Exact numeric of scale 0 and decimal precision 18 (IV032), no less than INTEGER's 10. It is NUMERIC(18, 0), one type with it whatever the name, so it compares, is assigned, cast and summed as any exact number, and a value past 18 digits is SQLSTATE 22003. A 64-bit integer of 19 digits does not fit. |
| Its name | §4.5.2 | Kept as written, in messages, the catalog and the Information Schema's DATA_TYPE 'BIGINT', NUMERIC_PRECISION 18, NUMERIC_PRECISION_RADIX 10, NUMERIC_SCALE 0. |
| Identity columns and sequences | §11.4 SR13, §11.72 SR4 | Of BIGINT, counting to 999999999999999999 if no maximum is written. |
| Descriptor areas | §20.1 Table 30, SR6g, SR7k, §20.10 GR7d | TYPE 25, PRECISION 18 and SCALE 0 as DESCRIBE sets them. An item of TYPE 25 takes a BIGINT value and gives one to a BIGINT target alone, else 22005. |

SQL:2023 reserves BIGINT [§5.2], and SQL-92 does not, so it stays an
identifier, read as a data type where a data type is. A column or a
domain may be named BIGINT, and BIGINT as a column's type is still the
type. A distinct type's source of BIGINT is NUMERIC(18, 0), its function
to the source named NUMERIC, as of any exact type (IV223).

### Decimal floating point

Feature T076 (DECFLOAT data type).

| Feature | Rules | What turborel does |
|---|---|---|
| DECFLOAT(p), DECFLOAT | §4.5.2, §6.1 SR22, SR23, SR35, SR36 | Decimal floating point of p digits, p from 1 to 34 (IA021), 34 if none is written (ID062). A precision above 34 is SQLCODE -900, of 0 SQLCODE -103. The adjusted exponent, that of the leading digit, is from -6143 to 6144 (IL054). A value has one form, so 1.5 and 1.50 are one value. It is 20 bytes in a row and 19 in a key, 1 of zero. |
| Rounding | §4.5.2, §9.2 GR3b xv | Half to even (IA192). A value past the range is 22003. |
| Arithmetic | §6.30 SR1a, GR4, GR7 | With a DECFLOAT operand the result is DECFLOAT(34) (IV134), the value of 34 digits nearest the exact one. An exact or approximate operand is taken as DECFLOAT, an approximate one from its exact binary value. A divisor of 0 is 22012, a result past the range 22003. An interval times or divided by a DECFLOAT is an interval [§6.44 GR5]. |
| Comparison, keys, uniqueness | §4.5.1, §8.2 GR3 | With any number, by value, an approximate one by its exact binary value. A key orders as the values compare, so an index on DECFLOAT is probed by DECFLOAT and by exact values. |
| Assignment | §9.2 GR3b xv | A number rounded to the precision. To an exact type, a DECFLOAT value is truncated to the scale (IA002), 22003 if it loses a leading digit. To an approximate type, its nearest, 22003 past the range. Only a number is assigned to one [SR3]. |
| Retrieved into a parameter | §9.1 GR5o | As assignment, rounded to a DECFLOAT parameter's precision. |
| CAST | §6.13 SR6, GR10-GR12 | A character string's `<signed numeric literal>`, its spaces trimmed, else 22018. To a string, plainly or in E notation (IV110), 22001 if too long. A number to and from DECFLOAT as assignment does. No datetime, interval, binary string or truth value casts to or from it (SQLCODE -103). |
| SUM, AVG | §10.9 SR7f, SR7g v | DECFLOAT(34) (IV170). The sum on the way is rounded to 34 digits at each row and judged against the range only once, as finished (RULES.md D15). MAX and MIN keep the argument's type. |
| CASE, set operations | §9.5 SR3c ii 1 | DECFLOAT of the greatest precision if any is DECFLOAT and all are numbers. |
| Defaults | §11.5 SR4a | A numeric literal, exact or approximate, rounded to the precision. |
| Descriptor areas | §20.1 Table 30, §20.5 GR4c iii 8, §20.10 GR7d v | TYPE 26, PRECISION the precision, 34 when TYPE is set (IA235). |
| Information Schema | 92 §21 | DATA_TYPE 'DECFLOAT', NUMERIC_PRECISION the precision, NUMERIC_PRECISION_RADIX 10, NUMERIC_SCALE null. |

DECFLOAT has no literal of its own. An `<approximate numeric literal>`
is approximate, not DECFLOAT (IV119), so a value is written as a numeric
literal, or `CAST('...' AS DECFLOAT)` where its type must show.
SQL:2023 reserves DECFLOAT [§5.2], and SQL-92 does not, so it stays an
identifier, read as a data type where a data type is. A column or a
domain may be named DECFLOAT, and DECFLOAT as a column's type is still
the type. SQL:2023 has no infinities or NaN of DECFLOAT, and neither has
turborel.

### Regular expressions

Features F841 (LIKE_REGEX predicate) and F847 (nonconstant regular
expression). `CVE LIKE_REGEX PAT FLAG FL` is true if a substring of CVE
matches the XQuery regular expression PAT under the option flag FL
[§8.7 GR1b], the empty string if no flag is written [SR2]. It is unknown
if any of the three is null [GR1a], and NOT LIKE_REGEX is its negation
[SR3]. Each is a character string [SR1], CVE a value and not a row, else
SQLCODE -103. A pattern and flag may be any character expressions, a
column's or a parameter's included. A `?` pattern or flag is CHARACTER
VARYING when the other of the two is not a `?`, the implicit flag
counting [§20.7 GR5a xxxvi], and a `?` value is refused.

| Feature | Rules | What turborel does |
|---|---|---|
| The pattern | §9.28 GR4, GR9a; F&O 3.1 §5.6.1 | XSD 1.1 Part 2's `regExp` with F&O's additions: `^` and `$`, reluctant quantifiers, back-references and non-capturing groups. Anything else is SQLSTATE 2201S. |
| The flag | §9.28 GR5; F&O 3.1 §5.6.2 | `s`, `m`, `i`, `x` and `q`, in any order, repeated or not. Any other character is 2201T. An invalid pattern is reported first [GR4]. |
| Newlines | §9.28 GR9a i | LF, VT, FF, CR, NEL, LINE SEPARATOR, PARAGRAPH SEPARATOR and CRLF [UTS #18 RL1.6]. `.` matches none but under `s`, which takes CRLF as one character. `^` and `$` under `m` are at each line's ends, never between CR and LF. |
| `\s` and `\S` | §9.28 GR9a ii, iii | `\s` matches a space, a tab or a newline sequence, CRLF one. `\S` matches one character `\s` does not. |
| Case | F&O 3.1 §5.6.2 | Under `i` a character matches its case variants, from Unicode 13.0's full case mappings, and a range those of each character in it. A back-reference compares case-blind. A category does not change. |
| Collations | F&O 3.1 §5.6.3 | Ignored. A CASE_INSENSITIVE column matches case and all unless the flag is `i`. |
| Where it is written | §8.7, §6.12 SR2f ii | A search condition, a value, a CHECK constraint, and a simple CASE's `<when operand>`. |

Several choices are turborel's own (RULES.md D61). Categories, blocks and
case variants are Unicode 13.0's, the version the collations have. `\i`
and `\c` are the name characters of XML 1.0 Fifth Edition. A block may
also be named by Unicode 3.1's superseded names, Greek,
CombiningMarksforSymbols and PrivateUse, which XSD lists; an unknown block
is 2201S [F&O 3.1 §5.6.1.5]. A range whose first character follows its
last is 2201S, as XSD 1.0 Second Edition, the one F&O 3.1 names, has it,
where XSD 1.1 would take it as empty. IA068, IA069, IV093 and IV094 do
not arise, as every string is in UCS. LIKE_REGEX and FLAG are no key
words of SQL-92, so each stays an identifier.

Features F842 (OCCURRENCES_REGEX function), F843 (POSITION_REGEX
function), F844 (SUBSTRING_REGEX), F845 (TRANSLATE_REGEX) and F846 (octet
support in regular expression operators). Each searches its subject from
a start position, 1 if none is written, for the match vectors of §9.28
GR11, each match beginning after the one before begins and not before
it ends, so empty matches count too.

| Feature | Rules | What turborel does |
|---|---|---|
| OCCURRENCES_REGEX | §6.31 SR3, GR18 | How many matches there are. -1 if the start position is less than 1 or past the string, so -1 of any empty string. |
| POSITION_REGEX | §6.31 SR4, GR19 | Where the occurrence-th match, 1 if none is written, or its group, 0 if none, begins, or with AFTER the position after it ends. 0 if there is no such match, the group matched nothing, the occurrence is less than 1, the group is none of the pattern's or the start position is not of the string. |
| SUBSTRING_REGEX | §6.33 SR8, GR4 | What that match or group matched, null where POSITION_REGEX would give 0. CHARACTER VARYING as long as its subject at most, of its collation. |
| TRANSLATE_REGEX | §6.33 SR12, GR9, §9.29 | Every match, or the occurrence-th, replaced from the last as fn:replace replaces it, `$N` a group. The subject itself if there is no such occurrence. As long as its subject at most if the replacement is not written or is the empty string, else CHARACTER VARYING of 32,767 characters (IL047), and a longer result is 22001. Of its subject's collation. A match of no characters is 2201U, and a replacement no XQuery replacement string 2201V, each only once a match is replaced. |
| USING CHARACTERS, USING OCTETS | §6.31 SR3d, SR4e, §6.33 SR8b, SR12b | Positions, lengths and the start position counted in characters, the default, or in octets of the string as UTF-8. A start position within a character names the next character (UV074). |
| Operands | §6.31 SR1, SR3, SR4, §6.33 SR3 | The pattern, flag, subject and replacement character strings, the start position, occurrence and group exact numbers of scale 0, else SQLCODE -103. A count or position is NUMERIC(18) (IV137). |
| Dynamic parameters | §20.7 GR5a xxxvi, xxxvii | A `?` string is CHARACTER VARYING when another of the four is not a `?`, an implicit flag or replacement counting. A `?` number is NUMERIC(18). |

Three readings fill gaps in the rules. A null start position makes the
result null, as a null of any other operand does, which GR18b and GR19b
leave unsaid. fn:replace refuses a pattern that matches the zero-length
string [F&O 3.1 §5.6.4], which §9.29 maps to no exception, so it is
2201U. POSITION_REGEX and SUBSTRING_REGEX compare the group with the
pattern's groups before the start position with the string [GR19b iii,
iv], so a pattern no regular expression is 2201S there, even with a
start position past the string. The four names are no key words of
SQL-92, so each stays an identifier, a function before a parenthesis.
START or AFTER is the word of POSITION_REGEX unless IN or FLAG follows
it, so a column of either name may be its pattern.

### Character length units

Feature T062 (character length units). A character string type's length
may be written in OCTETS, `VARCHAR(10 OCTETS)`, or in CHARACTERS, the
default [§6.1 SR9]. One in octets bounds the UTF-8 of each value [SR16b],
which is the form of use of the one character set, so the repertoire is
UCS as SR8 requires.

| Feature | Rules | What turborel does |
|---|---|---|
| Assignment | §9.2 GR3 | A value of more octets is 22001 unless only spaces are past them, and those are dropped. A fixed string is padded with spaces to its octets. |
| CAST | §6.13 GR5c, GR6c | A string is cut to whole characters of no more octets, with the warning 01004 if anything but spaces is lost. |
| Host variables | §9.1 GR3 | Cut and padded as CAST does, the indicator the value's length in octets. |
| Brought together | §9.5 SR3a, §6.33 SR3a | Of CASE, set operations and `||`, the length is in octets if every string's is, else in characters, which a length in octets bounds. A function's result, SUBSTRING's or a trim's, is in characters. |
| Descriptors | §20.10 GR7d, Information Schema | LENGTH and CHARACTER_MAXIMUM_LENGTH the length written, OCTET_LENGTH and CHARACTER_OCTET_LENGTH the same number (IA227). |
| CHAR_LENGTH, POSITION, SUBSTRING | §6.31 GR2, GR5, §6.33 GR3 | USING OCTETS counts octets of the string's UTF-8, USING CHARACTERS characters, the default. CHAR_LENGTH in octets is OCTET_LENGTH. |

A substring in octets whose ends cut a character takes only the whole
characters within them, as GR3 counts octets yet the result is a
character string; this is UV074's choice for a start position too. A
binary string's POSITION and SUBSTRING take no USING, their syntax having
none, so USING there is SQLCODE -103. CHARACTERS and OCTETS are no key
words of SQL-92, so each stays an identifier.

### Formatted CAST

Feature T839 (formatted cast of datetimes to/from character strings).
`CAST(v AS t FORMAT 'template')` writes a datetime as a character string
the template describes, or reads one back [§6.13 SR4]. The template is a
`<character string literal>`, read and checked as the statement is
bound. One of the two types is a datetime type and the other a character
string type, else SQLCODE -103, so NULL and a `?` take no FORMAT, a `?`
being of the target's type.

| Feature | Rules | What turborel does |
|---|---|---|
| The template | §9.52 SR2-SR15 | YYYY, YYY, YY, Y, RRRR, RR, MM, DD, DDD, HH, HH12, HH24, MI, SS, SSSSS, FF1 to FF9, A.M., P.M., TZH and TZM, written in upper case, and the delimiters `- . / , ' ; :` and space, each field the longest written at its place. Two delimiters in a row, a field twice, fields that do not go together and a field the datetime type has not are SQLCODE -103. |
| Its length | §9.50 SR3, §9.51 SR3, §9.52 SR16-SR19 | The string type is no shorter than the template's minimum length, else SQLCODE -103. A delimited field counts 1, TZH 2. |
| Datetime to string | §6.13 GR5e, GR6e, §9.50 | Each field its digits, the year its last ones, the time zone hour signed. A value without time zone is written in the session's displacement. The fraction is truncated to the template's digits (IA074). CHARACTER is padded with spaces, and a longer result is 22001. |
| String to datetime | §6.13 GR14-GR18, §9.51 | The template's regular expression matched against the whole string, its spaces kept. A delimited field takes 1 digit to its length, one not delimited exactly its length. No match is 22007, and so are fields that write no date or time. |
| Fields not written | §9.51 GR5-GR11 | The year and month the current, the day 1, the time and displacement 0. A year of fewer than four digits takes the current year's first ones. RR is read as YY (IV096). The fraction is truncated to the target's precision. |

Some readings fill gaps in the rules. A day of the year is read in the
year the template gives, as it is written of it, where GR7a iv names the
current year. A displacement is 22007 past -12:59 or +13:00, as any field
out of range is [GR12]. A digit of another script that `\p{Nd}` matches
is no `<unsigned integer>`, so it is 22007 too. Templates are upper case,
as §9.52 writes its key words, so `yyyy` is SQLCODE -103. A template is
not a key word, and FORMAT is no key word of SQL-92, so it stays an
identifier.

### Sequence generators

Features T176 (sequence generator support) and T177 (simple restart
option). CREATE, ALTER and DROP SEQUENCE define, change and drop a sequence
generator [§11.72-§11.74], in the session's transaction as other definitions
are, and NEXT VALUE FOR takes its next value [§6.14].

| Feature | Rules | What turborel does |
|---|---|---|
| Definition | §9.36, §11.72 | Each option at most once. The type exact numeric of scale 0, NUMERIC(18) by default (ID157). Each value whole and held by the type, the increment not 0, the maximum above the minimum, the start within them, else SQLCODE -103. Of NO MAXVALUE, the type's greatest value ascending and -1 descending (ID112); of NO MINVALUE, 1 ascending and the type's least value descending (ID113). |
| Values | §9.35 | The first of the current base value plus a multiple of the increment within the bounds. Past them, 2200H of NO CYCLE, else the minimum ascending, the maximum descending. |
| Alteration | §9.37, §11.73 | RESTART goes on from the start, RESTART WITH from its value, and neither from the current base value, which is checked to be within the new bounds. |
| Drop | §11.74 SR3, GR1, §11.2 GR12 | A trigger's action or a routine's body that draws from the generator refuses RESTRICT (SQLCODE -103). CASCADE drops each such trigger, and each such routine with CASCADE, then the generator. USAGE on it goes with it. DROP SCHEMA drops each of its generators with CASCADE. |
| Placement | §6.14 SR1, SR2 | In a select list of a cursor's query, a SELECT INTO's and an INSERT's, in an INSERT's VALUES and an UPDATE's SET; elsewhere SQLCODE -103, a sort key among them. A default takes no value expression, so there it is a syntax error. |
| One value a row | §6.14 GR1, §4.29.1 | Every NEXT VALUE FOR of one generator in a row gives the row's one value. |
| USAGE | §6.14 AR1, §11.72 GR2, §12.2, §12.7 | The schema's owner holds it grantably. GRANT and REVOKE ON SEQUENCE give and take it, and REVOKE with RESTRICT is 2B000 while a grant made by it would be abandoned [§12.7 GR12q]. INFORMATION_SCHEMA.USAGE_PRIVILEGES shows it, of OBJECT_TYPE SEQUENCE. |

A value is never given twice [§4.29.1]. The current base value is outside
every transaction, so a rollback keeps the values taken (RULES.md D62). Before
it gives out a value, a generator logs where to go on after a crash, a block
of values ahead, so a crash skips at most 1024 values and repeats none.

Some readings fill gaps in the rules. An ALTER without RESTART of a
generator that has given its last value is taken, whatever its bounds, as
its current base value is then past them by the generator's own doing. The
Information Schema of SQL:2023 is in ISO/IEC 9075-11, which this repository
does not hold, so INFORMATION_SCHEMA.SEQUENCES is an extension, its values
as text. SEQUENCE, INCREMENT, MAXVALUE, MINVALUE and RESTART are no key words
of SQL-92, so each stays an identifier, as do CYCLE and START, which
SQL:2023 reserves.

### Identity columns and generated columns

Features T174 (identity columns), T175 (generated columns), T178 (identity
columns, simple restart option), F384 (drop identity property clause),
F385 (drop column generation expression clause) and F386 (set identity
column generation clause). An identity column takes its values from a
sequence generator of its own [§4.17.12]. A generated column holds the
value of its generation expression, computed from its row [§4.17.13].

| Feature | Rules | What turborel does |
|---|---|---|
| Definition | §11.3 SR15, §11.4 SR10, SR11, SR13, SR16 | At most one identity column, of an exact numeric type of scale 0, its options as §9.36 says of that type. Its NOT NULL NOT DEFERRABLE is added unless written. A generation expression references only columns of its table that are not generated, and has no subquery, no set function and no potential source of non-determinism. Of no type written, the column is of its expression's type, else that type is assignable to the column's. A breach is SQLCODE -103. |
| INSERT | §14.11 SR11b to SR13, GR7e, §15.11 GR3a, GR6a | A generated column is given only DEFAULT in every row. A value given an identity column GENERATED ALWAYS needs an override clause, one GENERATED BY DEFAULT takes only OVERRIDING USER VALUE, and no override clause is written of no identity column given values. OVERRIDING USER VALUE ignores the values given. A row gives its identity column a value of the generator once the other columns have theirs, then its generated columns theirs. |
| UPDATE | §14.15 SR5, §15.14 GR4a, GR7a, GR9 | A generated column, or an identity column GENERATED ALWAYS, is set only to DEFAULT, which draws the generator's next value of an identity column. Every generated column is computed again of each row changed, and those that depend on a column set are counted among the columns it changed. |
| Referential actions | §11.8 SR14, GR5, GR6 | A foreign key of a generated referencing column has no SET NULL, no SET DEFAULT and no ON UPDATE CASCADE. A row an action changes has its generated columns computed again, and SET DEFAULT of an identity column draws its generator's next value. |
| ADD COLUMN | §11.11 SR3, GR2 | No second identity column. Each row there takes the generator's next value, or the expression's value. |
| ALTER COLUMN | §11.12 SR6 to SR9, §11.20, §11.21, §11.22 | The options change as §9.37 alters a generator. SET GENERATED changes ALWAYS and BY DEFAULT. DROP IDENTITY destroys the generator and keeps the NOT NULL. DROP EXPRESSION leaves the values. SET and DROP DEFAULT are refused of an identity column, SET DEFAULT of a generated one. |
| DROP COLUMN | §11.23 SR7f, GR3 | RESTRICT refuses a column a generation expression references. CASCADE first drops each such generated column with ALTER TABLE DROP COLUMN CASCADE, logged apart. |
| Domains and collations named | §11.40 SR2a, §12.7 GR12h, GR12z | DROP DOMAIN with RESTRICT refuses a domain a generation expression CASTs to. A generation expression that names a domain or collation dropped, or one whose USAGE its table's schema's owner loses, makes its column lost. DROP DOMAIN, DROP COLLATION, DROP SCHEMA and REVOKE USAGE refuse it with RESTRICT. With CASCADE they first drop each such generated column with ALTER TABLE DROP COLUMN CASCADE, logged apart. |

The values are stored with the row, as §15.11 and §15.14 compute them
(RULES.md D63). A generation expression is kept as written and is bound
once for each statement that writes the table, its names resolved in its
table's schema as of its definition. An identity column's generator is
named as its table is, apart from a sequence generator of that name, and
its values are never given twice, as a sequence generator's are (D62).

Some readings fill gaps in the rules. A column given values by a query
other than a table value constructor is not defaulted [§14.11 SR11b], so
an INSERT of a query into an identity column GENERATED ALWAYS needs an
override clause. If a DROP COLUMN CASCADE fails after it dropped the
generated columns on the column, those drops are not undone apart, and the
transaction can only roll back. §12.7 GR12h loses a generated column
only when its generation expression loses EXECUTE on a routine, and
§11.44 SR4 does not refuse a collation a generation expression names.
Either would leave an expression that no longer binds, so turborel loses
such a column as the row above says. A statement that fails after it
dropped such columns also leaves its transaction able only to roll back.
The Information Schema of SQL:2023 is in
ISO/IEC 9075-11, which this repository does not hold, so the columns of
COLUMNS that describe identity and generation are an extension, named as
that part names them, the generator's options as text. GENERATED, ALWAYS,
EXPRESSION and OVERRIDING are non-reserved words, and SYSTEM, which
SQL:2023 reserves and SQL-92 does not, stays an identifier too.

### TRUNCATE TABLE

Features F200 (TRUNCATE TABLE statement) and F202 (TRUNCATE TABLE: identity
column restart option). TRUNCATE TABLE deletes every row of a base table
without a triggered action [§14.10].

| Feature | Rules | What turborel does |
|---|---|---|
| The table | §14.10 SR2 to SR4, AR2 | A base table of a schema the session owns, which no foreign key references, its own included. A view, a declared local temporary table or a table referenced is SQLCODE -103, another's schema -104. |
| Its rows | GR5, GR7 | Every row goes, of a temporary table the session's alone. Of no row, the statement completes with no data. |
| Its identity column | SR5, GR8 | CONTINUE IDENTITY, written or not, leaves the generator. RESTART IDENTITY restarts it at its start value, with the rows, as ALTER COLUMN RESTART WITH would. A start value beyond the generator's bounds refuses it, SQLCODE -103. |
| READ ONLY | GR1 | A persistent table is refused, SQLSTATE 25006. A temporary one is not. |
| Cursors | GR2, GR3, GR6 | A cursor of the transaction that reads the table, directly or through a view, refuses the statement, SQLSTATE 36002. |

A persistent table's rows go at once, its storage replaced by an empty
generation as ALTER TABLE replaces it (RULES.md D64). A transaction that
began before the truncation committed is refused at its next use of the
table (SQLCODE -911). The read that tells whether the table had a row is
checked at commit at every isolation level, so a row another transaction
committed meanwhile fails the commit. Each CHECK with a subquery, and each
assertion, that reads the table is checked whole.

The rows ALTER TABLE copies to a new generation, or checks against a
constraint added, are no rows added either, so an assertion that reads its
table is checked whole after it too. TRUNCATE, which SQL:2023 reserves and SQL-92 does not, stays an
identifier, a key word only where TABLE follows it at the start of a
statement. RESTART is a non-reserved word.

### MERGE

Features F312 (MERGE statement), F313 (enhanced MERGE statement) and F314
(MERGE statement with DELETE branch). MERGE updates, deletes and inserts
rows of a table from one source, each row as the first WHEN clause that
applies to it says [§14.12].

| Feature | Rules | What turborel does |
|---|---|---|
| The target | §14.12 SR2 to SR4, SR7, SR8, AR1 | A base table, or an updatable view changed as its base table, insertable into if a WHEN NOT MATCHED clause is written. Its exposed name is the correlation name, if written. UPDATE on each column set, DELETE if a clause deletes and INSERT on each column inserted, else SQLCODE -104. A read-only target is SQLCODE -103. |
| The source | SR11, SR12, §7.6 AR1 | Any table reference but a joined table directly contained, whose exposed name differs from the target's, else SQLCODE -103. SELECT on what it reads. |
| Scope | SR9 | The target's columns are in scope in ON, in each WHEN condition and in SET, and not in an insert's values, which name the source's alone. |
| Evaluation | GR5 | The source joined to the target, as a LEFT join on ON, read whole before any row is changed. The rows are sorted by the target row's id, held up to the work area and spilled past it, as are the rows each clause will change. |
| WHEN MATCHED | GR6a i | A target row's clause is the first whose condition is true of a pair of it and a source row. Two such pairs of one target row are SQLSTATE 21000, duplicate source rows included. A target row no clause applies to is left alone. UPDATE computes its values from the pair, DELETE deletes the row. |
| WHEN NOT MATCHED | GR6a ii | As printed. A source row is inserted by a clause of condition SCj only if no target row makes `SC1 OR NOT SCj` true, read before any change, so an empty target inserts every source row and an SCj unknown for a row inserts it. A clause of no condition takes every source row ON matches to no target row. Each row is inserted by the first clause that takes it. |
| The insert | SR14 to SR18 | An insert column list, or every column, named once. As many values as columns, each NULL, DEFAULT or a value expression assignable to its column. A generated column takes DEFAULT alone, and an identity column follows OVERRIDING as INSERT does. Each rule broken is SQLCODE -103. |
| Order | GR6a | Clause by clause, in the order written, all the rows of one before the next. |
| Positioned changes | GR6a i 4 | An object row a positioned UPDATE or DELETE changed in the transaction raises warning 01001. |
| CHECK OPTION | GR7 | Every row updated or inserted through a view WITH CHECK OPTION is checked once all are written, SQLSTATE 44000. |
| READ ONLY | GR1 | A persistent table is refused, SQLSTATE 25006. A temporary one is not. |
| Completion | GR8, §23.1 GR2f ii | No data only if the source had no row. Otherwise success, however many rows changed. ROW_COUNT is as printed, of the target joined to the source before the MERGE, whatever the WHEN conditions. Of WHEN MATCHED clauses alone it counts the pairs ON matches, of WHEN NOT MATCHED clauses alone the source rows ON matches to no target row, and of both, the rows of the target RIGHT OUTER JOIN the source. COMMAND_FUNCTION is MERGE. |

At READ COMMITTED a target row another transaction changed after the
statement read it is matched again as that transaction left it, against
the same source row, by ON, the views' conditions and its clause's
condition (RULES.md D29). If they still hold, it is changed from that
version, and if not, it is left alone and not counted. It is never
inserted instead, since GR6a ii decides the inserts from the rows read
before any change. A row another transaction inserts meanwhile is not
read, so an insert of its key fails its unique constraint, and the
statement may be run again.

An error in a later WHEN MATCHED clause's condition is not raised for a
pair an earlier clause already takes, nor for one of a target row an
earlier clause took, since no later clause can be its row's clause.
MERGE, which SQL:2023 reserves and SQL-92 does not, stays an identifier,
a key word only where INTO follows it at the start of a statement.
MATCHED is a non-reserved word.

### Diagnostics

Features F122 (Enhanced diagnostics management), F123 (All diagnostics)
and T511 (Transaction counts). GET DIAGNOSTICS reads the diagnostics
area as [SQL-92's](#queries-and-expressions) does, with what SQL:2023
adds to it [§23.1].

| Feature | Rules | What turborel does |
|---|---|---|
| Targets | §23.1 SR1, CR3, §9.1 SR3, SR5 | An item goes to a target of any type its value is assignable to, a number to any numeric type, a character string to any character string type, as FETCH assigns it. A number too large for its target is SQLSTATE 22003, a string cut to fit the warning 01004. Any other target is SQLCODE -103. |
| ALL | SR2, GR6a, CR4 | ALL, ALL STATEMENT, ALL CONDITION and ALL CONDITION n put the items into one character string, each as `NAME = value`, joined by semicolons. An item whose value is a zero-length string, or spaces alone, which compare equal to one [§8.2 GR3b], is left out. A target of another type is SQLCODE -103, a condition number of no condition SQLSTATE 35000. |
| Transaction counts | GR2h, GR2i, GR2j, CR2 | TRANSACTIONS_COMMITTED and TRANSACTIONS_ROLLED_BACK count the transactions the statement committed and rolled back, by COMMIT, ROLLBACK, EXECUTE IMMEDIATE of either or the Rust API. A COMMIT that fails rolls back, and is counted so. TRANSACTION_ACTIVE is 1 while a transaction is active. |
| Codes | GR2c to GR2e, Table 39 | COMMAND_FUNCTION_CODE and DYNAMIC_FUNCTION_CODE give each statement's code, 0 of one that did not parse and of none prepared. |
| CONDITION | Format | CONDITION n reads a condition as SQL-92's EXCEPTION n does. |
| Routines and triggers | GR4g, GR4l, GR4q, GR5 | Of 09000, the trigger items name the trigger whose action failed, and CATALOG_NAME, SCHEMA_NAME and TABLE_NAME its table (see [Triggers](#triggers)). Of 2F005 and of an argument stored in its parameter, the routine items name the function or procedure, and the parameter items the parameter (see [SQL-invoked routines](#sql-invoked-routines)). Else the routine items are zero-length strings, and PARAMETER_ORDINAL_POSITION 0, a condition of a distinct type's function read as one of CAST (see [Readings](#readings-of-ambiguous-text)). |

COMMAND_FUNCTION and DYNAMIC_FUNCTION keep SQL-92's identifiers [92
§18.1 Table 22], and the codes are SQL:2023's, so a code tells apart
statements one identifier names. SELECT is 65 of SELECT ... INTO, 21 of
the Rust API's cursor and 41 of a statement prepared and executed INTO
targets. A positioned UPDATE or DELETE of a dynamic cursor is 42 or 38,
one prepared 55 or 54. A cursor specification prepared and opened, which
Table 22 has no row for, takes Table 39's identifier, SELECT CURSOR, and
code 85.

A transaction held in doubt (RULES.md D25) is counted neither committed
nor rolled back, as recovery decides it. The index and user statements
run outside a transaction, so they end none. A transaction that commits
and fails to publish its definitions after (SQLCODE -910) is counted
committed. CONDITION and STATEMENT are no key words of SQL-92, so each
stays an identifier, a key word only in GET DIAGNOSTICS, and a parameter
named CONDITION is still a target where = follows it.

### LIKE and AS in CREATE TABLE

Features T171 (LIKE clause in table definition), T172 (AS subquery clause
in table definition) and T173 (extended LIKE clause in table definition).
A `<like clause>` copies another table's columns, and an `<as subquery
clause>` defines a table from a query [§11.3].

| Feature | Rules | What turborel does |
|---|---|---|
| LIKE | §11.3 SR9b iii, v, AR2 | Each column of the table or view named, in the clause's place, with its name, its data type and collation, and NOT NULL if known not nullable. No other constraint, no domain and no row. SELECT on what it names, else SQLCODE -104. In a CREATE SCHEMA, a table defined before in it may be named. LIKE in DECLARE LOCAL TEMPORARY TABLE is taken too. |
| Its options | SR9b ii, iv | Each written once. None of a viewed table. EXCLUDING by default. INCLUDING IDENTITY makes a generator of the new table's own, of the same options, from the start value. INCLUDING DEFAULTS copies each column's own default. INCLUDING GENERATED copies each generation expression, each `<column reference>`'s table name replaced by the new table's. |
| AS | SR4, SR10 | Each column of the query, named as it or the `<column name list>` names it, of its declared type and collation, NOT NULL if known not nullable. A column unnamed, two of a name, a list of another length or of a name twice, or a dynamic parameter, is SQLCODE -103. NEXT VALUE FOR may be in its select list [§6.14 SR1a ii]. |
| WITH DATA | GR10 | The query's rows inserted in the same statement, before the definition is staged, so a failure defines no table. No row completes with no data. WITH NO DATA inserts none. |

A definition is logged as its text and made again from it at commit and
by recovery (RULES.md D65). So the table a LIKE names, and the tables,
views and sequence generators the query names, are held shared to the end
of the transaction, and neither names a session's declared local temporary
table, which no log keeps, but in a declared table's definition. For the
same reason, and as its rows are inserted in a transaction, which a
`<schema definition>` run apart from a session has none of, an `<as
subquery clause>` is refused in a CREATE SCHEMA, SQLCODE -103. Both are
this implementation's own. INCLUDING, EXCLUDING, DEFAULTS and DATA are
non-reserved words.

### UNIQUE null treatment

Feature F292 (UNIQUE null treatment). A `<unique null treatment>`, NULLS
DISTINCT or NULLS NOT DISTINCT, says whether a null counts as a value of
a unique constraint and of the UNIQUE predicate [§4.25.3.2, §8.11, §11.7].

| Feature | Rules | What turborel does |
|---|---|---|
| A unique constraint | §11.7 SR5, GR2d, GR3 | Written after UNIQUE in a table constraint. NULLS NOT DISTINCT refuses a second row of the same values, a null the same as a null, by any statement, at its end or at commit if deferred (SQLCODE -201). After PRIMARY KEY it is SQLCODE -103, and so in a column constraint [§11.4], whose grammar has none. Keys of both treatments on the same columns are two unique constraints on them [SR6c]. |
| The UNIQUE predicate | §8.11 GR2 | NULLS NOT DISTINCT is false of two rows equal but for nulls in the same columns. NULLS DISTINCT counts no row with a null. |
| Its description | §11.7 GR2d | INFORMATION_SCHEMA.TABLE_CONSTRAINTS gives NULLS_DISTINCT, as ISO/IEC 9075-11 names it: YES, NO, or null of a constraint not UNIQUE. |

The index of a key of NULLS NOT DISTINCT holds the keys with a null,
which no other index does, so a second null is found as a second value
is (RULES.md D66). No probe of it selects one, as no comparison with a
null is true. NULLS is a non-reserved word, a key word only after UNIQUE.

### WITH and WITH RECURSIVE

Features T121 (WITH (excluding RECURSIVE) in query expression), T122
(WITH (excluding RECURSIVE) in subquery), T131 (Recursive query), T132
(Recursive query in subquery) and T133 (Enhanced cycle mark values). A
`<with clause>` names queries for the query expression that follows it
[§7.17], and a `<search or cycle clause>` adds a recursive query's
sequence, cycle mark and path columns [§7.18].

| Feature | Rules | What turborel does |
|---|---|---|
| WITH | §7.17 SR3c, d, i, GR2a, b | Each query named once, its columns named once or by a `<with column list>` of as many names, else SQLCODE -103. Each read by the query expression's body and by the queries after it, where a table may be, correlated with outer rows as a subquery is. In a query, a subquery, a derived table, a view, INSERT, a CHECK constraint and an assertion. |
| WITH RECURSIVE | SR3h, j i to viii | Every query of the list has a column list, and none contains another WITH RECURSIVE list. The queries are bound stratum by stratum. Each that reads its stratum is a UNION, one operand of which reads none of the stratum, and every cycle of the stratum passes through such a query. Else SQLCODE -103. |
| Linear recursion | SR3j ix, x | A query specification reads its stratum once, in its FROM clause or a derived table there, beside no set function in its select list or HAVING, not on a side of an outer join that nulls give (UNION JOIN taken as FULL), under no INTERSECT ALL or EXCEPT ALL, not after EXCEPT. Else SQLCODE -103. |
| Types | SR19 b ii 2 | A recursive query's column is of its anchor's type, a recursive operand's value cast to it, so `VALUES (1)` gives NUMERIC(1,0) and 10 is SQLCODE -301. Every column of a recursive query may be null, where the standard may know some not to be. |
| Evaluation | GR2c | To a fixpoint, as the General Rules say, or of a single query over only the rows each iteration adds where that gives the same rows (RULES.md D67). The order of the rows is implementation-dependent (US036) without ORDER BY. |
| SEARCH and CYCLE | §7.17 SR3j xi, SR3k; §7.18 | An element with a `<search or cycle clause>` is expandable, recursive and referencing no other query of its list, a UNION whose first operand does not reference it and whose second is a `<query specification>` reading it in its FROM clause, else SQLCODE -103. Each Syntax Rule on the columns named and the mark values is SQLCODE -103. The element is its expansion [SR8], with its path and DEPTH FIRST sequence columns arrays of the greatest maximum cardinality and BREADTH FIRST's level INTEGER, so they grow past the anchor's types (RULES.md D70). The marks are TRUE and FALSE unless written. |
| Runaway recursion | GR2c ii 4, NOTE 322 | Refused past the session's EXECUTOR.MAX_ITERATIONS iterations, a million by default, 0 for no limit (SQLCODE -918, SQLSTATE TB918). This implementation's own, as the standard's loop does not end. A cancel or STATEMENT.TIMEOUT_MS stops it (SQLCODE -920). |

A `<search or cycle clause>` of a UNION CORRESPONDING, or of a recursive
operand that selects `*` of a NATURAL join or a join USING columns, is not
supported (SQLCODE -900). A path holds at most 65,535 rows. Neither is an
anchor operand of UNION CORRESPONDING without BY, nor a recursive query
whose anchor UNION is within its `<query expression body>` rather than
all of it, as `A UNION ALL B UNION ALL C` of two recursive operands. A query of a WITH is not
updatable, so no view of one is. RECURSIVE is a key word only after WITH,
SEARCH and CYCLE only after a query of the list.

A CHECK constraint whose subquery reads no table, such as one of a WITH of
VALUES, is checked of each row written, as one that reads a table is;
before, it was refused at its first check with SQLCODE -910.

### Partitioned joins

Feature F403 (Partitioned join tables). An operand of an outer join
PARTITION BY its columns is a `<partitioned join table>` [§7.10]: the
other operand is joined with each of its partitions, and a row a
partition does not match is given that partition's values [GR4].

| Feature | Rules | What turborel does |
|---|---|---|
| Where it is written | §7.10 F, SR9a, SR9c, SR9d | An operand of a qualified or natural join that is LEFT, RIGHT or FULL: the second of LEFT, the first of RIGHT, either or both of FULL. Else SQLCODE -103, or -102 of a CROSS or UNION join or of no join. The operand is a table, a derived table, a collection derived table that reads nothing of the first operand, or a joined table in parentheses. |
| Its columns | SR9b, SR10d | Each names one column of the partitioned operand, else SQLCODE -103, an outer reference included. A corresponding join column of NATURAL or USING is none of them. |
| Its partitions | GR4b, GR4c | Of values not distinct [§8.15], so the null value is a partition, and strings are partitioned by their collation. An array column partitions too, as arrays compare for equality. An empty operand has no partitions, so the join gives no row of the other [NOTE 239, NOTE 240]. |
| Its rows | GR4d to GR4k, GR5 | The rows of the join, then each row unmatched in a partition, of the other operand's partitioned columns the partition's values and nulls in the rest. A matched row keeps its own values. |

It is planned as an outer join of what its operands are made (RULES.md
D38), so it spills and reads by batches as any outer join does. The
partitioned operand's partitioning columns DISTINCT give its partitions,
and the other operand is joined with every one of them. The partitioned
operand's rows then match those on the join's condition and on each
partitioning column IS NOT DISTINCT FROM the partition's. The partitioned
operand is read twice, which gives the same rows, as a derived table
draws no sequence value. GR4h gives a filled row its partition's common
value. Of values a collation holds equal, 'a' and 'A' under
CASE_INSENSITIVE say, that is the one DISTINCT keeps, the first it reads.
PARTITION, which SQL-92 does not reserve, is read only before BY after an
operand of a join.

### Grouping sets

Features T431 (Extended grouping capabilities), T432 (Nested and
concatenated GROUPING SETS), T433 (Multi-argument GROUPING function) and
T434 (GROUP BY DISTINCT). One query gives subtotals and the grand total
[§7.13, §6.9].

| Feature | Rules | What turborel does |
|---|---|---|
| ROLLUP and CUBE | §7.13 SR13, SR14 | ROLLUP of n sets stands for n + 1 grouping sets, the longest first down to (). CUBE stands for 2 to the n, in the order of SR14's bits counted from the left. |
| GROUPING SETS | SR15, SR16 | Nested sets are promoted. Elements written one after another are concatenated, each set of the first with each of the second, in that order. |
| A simple GROUP BY | SR9 to SR11 | One of no ROLLUP, CUBE or GROUPING SETS loses its parentheses, so `GROUP BY (A, B), ()` is `GROUP BY A, B`. GROUPING is 0 there. |
| The empty grouping set | GR2a | `GROUP BY ()` makes one group even of no rows, as does each () among the sets. So COUNT(*) of an empty table grouped by ROLLUP (A) is one row, 0. |
| Each set's rows | SR17 | A column a set leaves out is null in that set's rows, in the select list, HAVING, a subquery's outer reference and a set function's argument. HAVING is judged of each set's groups. |
| GROUPING | §6.9 SR1 to SR10 | 1 of a column its set leaves out, else 0, and of several columns the bits read as a binary number, the first the highest. Each argument names a grouping column, else SQLCODE -103. It is written only in the select list or HAVING, never in a set function's argument, and one of outer references only is the outer query's. |
| GROUP BY DISTINCT | §7.13 SR17n | Sets of the same columns and collations are one. Rows alike of sets still apart are one, so a select column of an array type, which no operation groups [§9.12 SR3], is SQLCODE -103. GROUP BY ALL, the default, keeps every set. |

The Syntax Rules give a set function's argument the null value of a column
its set leaves out, as they do the select list [SR17m iii]. So in

```sql
SELECT P, SUM(P) FROM S GROUP BY ROLLUP (P)
```

the total row's SUM(P) is null, not the sum of every P, as the query of
that set alone is `SELECT CAST(NULL AS INTEGER), SUM(CAST(NULL AS INTEGER))
FROM S`. turborel gives the rows the rules define. A column no set
leaves out, or not a grouping column, is summed as it is.

The FROM and WHERE are read once, not once for each set. Each row is
given to every set, a left-out column's value null, and one aggregation
groups them all by the set's number and its GROUPING bits, so the groups
of two sets alike stay apart under ALL (RULES.md D38). It spills as any
grouping does. A grouping column of two collations among the sets is
grouped by each where its set names it. ROLLUP, CUBE and GROUPING, which
SQL-92 does not reserve, are key words only where they are read.

### LISTAGG and ANY_VALUE

Features T625 (LISTAGG) and T626 (ANY_VALUE), set functions of
ISO/IEC 9075-2:2023 §10.9.

| Feature | Rules | What turborel does |
|---|---|---|
| ANY_VALUE | §10.9 SR7f, GR7d v | `ANY_VALUE([DISTINCT \| ALL] x)` is one of the values of x not null, of x's type and collation, and null of none. Nulls left out give SQLSTATE 01003, as of any set function. |
| LISTAGG's syntax | §10.9 SR11 | `LISTAGG([DISTINCT \| ALL] x, 'sep' [ON OVERFLOW ...]) WITHIN GROUP (ORDER BY ...)`. Its separator and filler are character string literals, else SQLCODE -102, and no longer than its result, else -103. x is a character string, else -103. A set function or a subquery in x or a key is -103 [SR11a]. A key is an operand of an ordering operation [§9.14]. |
| Its order and nulls | GR11a, GR11b | The values in the order of the keys, each key as its collation orders it. Values of keys alike are in the order they were read. Null values are left out, with SQLSTATE 01003. |
| DISTINCT | GR11c | Values equal by x's collation are one, `'a'` and `'A'` under CASE_INSENSITIVE say. |
| Its result | SR11d, GR11e to GR11g | CHARACTER VARYING(32,767), of x's collation, null of no values. Values joined longer than 32,767 characters are SQLSTATE 22001 of ON OVERFLOW ERROR, written or implicit. ON OVERFLOW TRUNCATE takes the most values that fit with the separator, the filler (`...` if none is written) and, WITH COUNT, the count of those left out in parentheses, as GR11g iii defines, three periods where even the filler does not fit. |
| In a constraint | §9.16 SR1ap, SR1ar, SR1as, §11.9 CR2 | ANY_VALUE, ARRAY_AGG and LISTAGG are potential sources of non-determinism. A CHECK or an assertion containing one is SQLCODE -103, as F672 is not claimed. |

LISTAGG's argument is the row of x and its keys, null where x is, so the
executor leaves out a null value and warns of it as of any set function.
Of a group's values it holds only those its result can show. Once the
values in order are longer than 32,767 characters, those after cannot be
in its result, so without DISTINCT they are let go as they are passed,
and of ON OVERFLOW ERROR the error is raised then. Their total does not
depend on their order, so it is the error the result would raise
(RULES.md D15). ANY_VALUE, LISTAGG, WITHIN and OVERFLOW, which SQL-92 does
not reserve, are key words only where they are read.

### Null ordering

Feature T611 (Elementary OLAP operations), its `<null ordering>` of
ISO/IEC 9075-2:2023 §10.10. Window functions, the rest of T611, are
below.

| Feature | Rules | What turborel does |
|---|---|---|
| Its syntax | §10.10 | NULLS FIRST or NULLS LAST after a key's direction, in a cursor's ORDER BY, ARRAY(query ORDER BY ...), ARRAY_AGG's ORDER BY and LISTAGG's WITHIN GROUP. NULLS followed by neither is SQLCODE -104. |
| Where nulls go | GR1e ii | Under NULLS FIRST a null is before every value, under NULLS LAST after, ascending and descending alike. Two nulls are peers. Without one, nulls are last ascending and first descending (ID133). |
| Its plans | | A sort places nulls so in memory, spilled and by batches, a null's sort prefix the least or the greatest as its key places it. A column store kept in order of a column is read in that order only for the null ordering implied. An index read in key order gives no null, so it serves either. EXPLAIN shows a key's null ordering where it is not the one implied. |

NULLS, which neither SQL-92 nor SQL:2023 reserves, is a key word only
where it is read, so a column may be named NULLS.

### Functional dependencies

Feature T301 (Functional dependencies), of ISO/IEC 9075-2:2023 §4.26
and §6.7 SR7 b iv, SR8. A grouped query names, outside a set function,
a column functionally dependent on its grouping columns, in its select
list, HAVING, window clause, a hypothetical set function's values, an
inverse distribution function's fraction and an outer reference of a
subquery. DESCRIBE OUTPUT gives a query's primary key or preferred
candidate key as KEY_TYPE and KEY_MEMBER [§4.27, §20.10 GR6d, GR7c].

| Feature | Rules | What turborel does |
|---|---|---|
| Base tables | §4.26.3 | A non-deferrable PRIMARY KEY, or a non-deferrable UNIQUE of columns known not null, determines every column. A column is known not null of a non-deferrable NOT NULL or PRIMARY KEY. A generated column is determined by its parametric columns. A non-deferrable UNIQUE NULLS NOT DISTINCT determines every column too (IE015). |
| Equalities | §4.26.2, §4.26.12 | Of WHERE, each equality AND-component, of rows field by field: of two columns each determines the other, and of a column and a literal none determines it. A column compared with another deterministic expression of no column has one value too, and one compared with a deterministic expression of columns is determined by them (IE015, notes 68 and 69). A column compared by a collation not its own determines nothing. |
| Joins | §4.26.7 | An inner join keeps both operands' dependencies and adds its condition's equalities. Of the operand an outer join extends with nulls, a dependency holds only if a column of its determinant is known not null. A LEFT join on a deterministic condition adds that its condition's columns of the preserved operand determine each column of the other its equalities compare with a constant or a preserved column, and RIGHT the mirror. A NATURAL or USING join's common column is alike the column it is the value of, where that is known not null or null with the other. |
| Groups | §4.26.13 | The grouping columns determine the columns of the set functions. A grouping column written with COLLATE determines nothing. Of grouping sets, a column is dependent on each set's columns, else SQLCODE -103. |
| Queries | §4.26.4, §4.26.8, §4.26.15, §4.26.16 | A view, a derived table and a WITH query that is not recursive keep their query's dependencies, and a column of a select list is determined by the columns of its expression if it is deterministic. EXCEPT keeps its first operand's, INTERSECT both's. A collection derived table WITH ORDINALITY's ordinality column with its input's columns determines its elements, if its collections are deterministic. |
| Table value constructors | §4.26.6 | One of at most one row determines every column (IE015). One of more rows, a recursive query (IA204) and UNION (IA226) know none. |
| BUC-sets and BPK-sets | §4.26.2 to §4.26.16 | A non-deferrable PRIMARY KEY's columns are a BPK-set, a non-deferrable UNIQUE's of columns known not null, or NULLS NOT DISTINCT, a BUC-set [§4.26.2]. A select list, a view, a derived table, WHERE, HAVING and EXCEPT keep those of the columns they keep, INTERSECT either operand's, a join an operand's that determine every column and are known not null, a FROM of several table references those its WHERE makes determine every column [§4.26.11, §4.26.12]. A grouped table's grouping columns are a BUC-set if known not null and a BPK-set if within one. A collection derived table WITH ORDINALITY that reads no column before it has its ordinality column as a BPK-set. UNION, grouping sets, a table value constructor (IA203) and a recursive query (IA204) have none. |
| Candidate keys | §4.27, §20.10 GR6d, GR7c | KEY_TYPE is 1 of a candidate key within a BPK-set, which the deduction rule makes a BPK-set, the left-most of them the primary key. Else it is 2, the preferred candidate key the left-most within a BUC-set or within the grouping columns, a strong candidate key, else the left-most of all. KEY_MEMBER is 1 of the key's columns, 0 of the others, of a parameter and of a part of a column. DESCRIBE INPUT and a statement not a query set KEY_TYPE 0. SET DESCRIPTOR sets neither [§20.5 SR2, SR3a]. |
| A dependent column's value | §6.7 GR1e | The value of the group's first row read, kept as a set function would keep it, its null kept with no SQLSTATE 01003. Of a character string, a datetime with time zone or a user-defined type it is UV070. In a CHECK or an assertion such a column of a character string is a potential source of non-determinism, SQLCODE -103 [ISO/IEC 9075:1992 §11.9 SR6]. |

The candidate keys are found by a search of sets of the result's
columns in the order the left-most rule gives, the first found the
left-most. A table may have candidate keys exponentially many in its
columns, so the search stops after 4,096 sets, and then KEY_TYPE is 0,
no column a member. A query whose rows are a grouped query's, through a
select list, HAVING, a sort or a window, is taken as grouped, its
grouping columns of the result a strong candidate key.

A table constraint a grouped query needs to know a column dependent is
one the query depends on [§11.26 SR4]. So do a view, a CHECK, a domain
constraint, an assertion and a trigger that contain the query [SR5, SR7,
SR8]. DROP CONSTRAINT finds them by binding each that reads the table
again without the constraint. One that bound with it and binds no more
depends on it. With RESTRICT such a one refuses the drop, SQLCODE -103 citing SR13a, SR13c, SR13e or SR13f. With
CASCADE each is dropped as GR1 and GR3 to GR5 say, a domain constraint
from its domain as NOTE 568 says. A routine depends on none, since there
are no SQL routines [SR6, SR13b, SR13d].

A refused column is SQLCODE -103 citing §6.7 SR8. A partitioned join's
plan is read as the nodes it is made of, so its join partitioning
columns add no dependency. A set function of an enclosing query whose
argument has both its columns and the subquery's is still refused
[§6.9 SR8], so T301's rule for such an argument is not reached.

### Sort keys

Features E121-02 (ORDER BY columns need not be in select list), E121-03
(Value expressions in ORDER BY clause) and F868 (ORDER BY in grouped
table), the `<sort key>` of ISO/IEC 9075-2:2023 §10.10 and §7.17 SR26, in
a cursor's ORDER BY and ARRAY(query ORDER BY ...) [§6.49 NOTE 207].

| Feature | Rules | What turborel does |
|---|---|---|
| Its syntax | §10.10, §7.17 SR26b | A key is an unsigned integer, a position as SQL-92 has it, or any value expression, its COLLATE taken as the key's. A key that is not a position contains a column reference, one of a window's PARTITION BY among them, and no query expression, set function or NEXT VALUE FOR [§6.14 SR1], else SQLCODE -103. |
| A column left out | SR26 d i | A key of a simple table query may read the columns of its table expression the select list leaves out. Each such key is an extended sort key column, computed with the select list and taken away after the sort [GR5], so DESCRIBE and the rows show the result's columns alone. A key of the result's columns alone is computed of its rows [d ii]. |
| A key alike a select item | SR26 d i 6 | A key of the same operations on the same columns and values as an item of the select list sorts by that item's column, computed once. A key that SR26 d i 6's left normal form derivation would call alike, and this does not, sorts by a column of its own, of the same values. |
| A result's name | SR26 d i 6 | In a key, an unqualified name of one result column is that column, so of a key with a column left out the select item's value. |
| A grouped query | SR26 d i 9 B I, Feature F868 | A key of a grouped query reading a column its select list leaves out is computed over its groups. Each column it reads outside a window function is a grouping column or functionally dependent on the grouping columns, of each grouping set, by the rules T301 knows [§4.26], else SQLCODE -103 citing SR26 d i 9 B I. |
| What it refuses | SR26 d i 9 B II | A key reading a column left out of a query that is DISTINCT, or that directly contains a set function in its select list, HAVING or window clause, is SQLCODE -103. |
| Other query expressions | SR26 d i 1 | The keys of a set operation, a VALUES list or another query expression that is not a simple table query read its result's columns alone, else SQLCODE -103. A window function is in the ORDER BY of a simple table query alone [§6.10 SR2a]. |
| Window functions | §6.10 SR2a | A key of a simple table query may be a window function, computed with the select list's. |
| Updatable cursors | §14.3, Feature F833 | Ordered by its keys, each row's computed from its own values as OPEN reads it. One ordered by a window function, whose value is not of its row alone, is SQLCODE -900. |
| Its plans | | A key spills past MEMORY.WORK_BYTES with the rows, by batches or row by row. |

### ORDER BY, OFFSET and FETCH FIRST

Features F850 (Top-level ORDER BY in query expression), F851 (ORDER BY
in subqueries), F852 (Top-level ORDER BY in views), F855 (Nested ORDER
BY in query expression), F856 (Nested FETCH FIRST in query expression),
F857 (Top-level FETCH FIRST in query expression), F858 (FETCH FIRST in
subqueries), F859 (Top-level FETCH FIRST in views), F860 (Dynamic FETCH
FIRST row count), F861 (Top-level OFFSET in query expression), F862
(OFFSET in subqueries), F863 (Nested OFFSET in query expression), F864
(Top-level OFFSET in views), F865 (Dynamic offset row count in OFFSET),
F866 (FETCH FIRST clause: PERCENT option) and F867 (FETCH FIRST clause:
WITH TIES option), the `<order by clause>`, `<result offset clause>` and
`<fetch first clause>` of ISO/IEC 9075-2:2023 §7.17.

| Feature | Rules | What turborel does |
|---|---|---|
| Where the clauses are | §7.17 | After the body of any query expression: at the top of a cursor or a direct query, of a subquery, correlated too, a derived table, a view's query, a WITH element's query and its body, an INSERT's query, ARRAY's query [§6.49 NOTE 207] and a query in parentheses, an operand of a set operation among them. OFFSET, which SQL-92 does not reserve, is the clause only where a count and ROW or ROWS follow it, else an identifier. |
| Counts | SR27, SR28, §6.4 | A count is a `<simple value specification>`: a literal, signed too, a parameter or an embedded variable, of a type exact numeric of scale 0, else SQLCODE -103. \[EXTENSION\] A `?` of a prepared statement is a count too, typed INTEGER, as NTILE's count is [§20.7 GR5a xxxii], since §20.7 gives a row count no type. A count is evaluated once, as the rows are first asked for. |
| Their rows | GR5, GR6, GR7 | The body's rows sorted by ORDER BY, then OFFSET's count of them passed over, all past the last, then at most FETCH FIRST's count kept, one if it has none [GR7b iii]. No row past those kept is read, unless reading the body's rows can raise an exception, as a division can. Then every row is read, so one past those kept raises it as reading them all would [SQL-92 §3.3.4.4]. |
| Counts out of range | GR6b, GR7c | An offset below 0 is SQLSTATE 2201X, a count of FETCH FIRST below 1 2201W. A null count raises the same, since it is no number of rows. |
| Updatability | SR7, SR8, SR11 | A query expression of OFFSET or FETCH FIRST is neither updatable nor insertable-into, so a view of one is read-only and a cursor of one FOR UPDATE SQLCODE -103. One of ORDER BY alone is as its body is. |
| Recursive queries | SR3j ix 3, ix 4, GR2c ii | A recursive query has no FETCH FIRST, nor does a query primary in it that references its stratum, each SQLCODE -103. ORDER BY and OFFSET of a recursive query, or OFFSET of a query over its work table, are found as the General Rules find its fixpoint, of all its rows each time. |
| Functional dependencies | §4.26, NOTE 71 | A query of OFFSET or FETCH FIRST keeps the dependencies and candidate keys of its body, as its rows are some of the body's. |
| PERCENT | SR29, GR7b ii, NOTE 328, Feature F866 | A percentage is a `<simple value specification>` of a numeric type, else SQLCODE -103, or \[EXTENSION\] a `?` typed DOUBLE PRECISION. Its count is CEILING ( FFP * LOCT / 100.0E0 ), LOCT the rows before OFFSET, in SQL's arithmetic: approximate, or DECFLOAT of a DECFLOAT percentage, whose ceiling is exact. The rows are held to be counted, spilling past MEMORY.WORK_BYTES. A count below 1, of a percentage of 0 or below or of a table of no rows, is SQLSTATE 2201W, as GR7c says. |
| WITH TIES | SR30, GR7d i, Feature F867 | Of a query expression with an ORDER BY, else SQLCODE -103. After the last row its count keeps, the rows after it that are its peers by every sort key are kept [§10.10], compared as the sort compares them, by each key's collation, nulls peers of each other; no row past them is read, unless a row can raise an exception, as for ONLY. The peers OFFSET passed over stay passed over. |
| Its plans | GR5, GR7 | A Limit node over the sort, the sort spilling past MEMORY.WORK_BYTES, by batches or row by row. \[DESIGN\] As it runs, the sort is told how many of its first rows are asked for, OFFSET's and FETCH FIRST's counts together, and holds only those as it reads, with the peers of the last of WITH TIES, so it holds about that many rows however many it reads. Equal rows keep the order the whole sort gives them. Once those held outgrow MEMORY.WORK_BYTES, they and the rest are sorted whole, spilling. A scan already in the order ORDER BY asks for is limited without a sort. The planner prices a path in that order, an index's or a table's kept in it, by the rows it reads, those of OFFSET's and FETCH FIRST's constant counts of ONLY, so a few rows are read of an index in order and not sorted of the whole table. Through a filter, a join or a grouping, and of WITH TIES or PERCENT, it is priced by all its rows. A scan of the column store in COLUMNSTORE.ORDER's order is told the same of ONLY, and sorts no more than those first rows of the rows not yet frozen and of each group sorted otherwise. Neither is told through a filter, a percentage or a projection that can raise an exception. |

### Window functions

Features T611 (Elementary OLAP operations), T612 (Advanced OLAP
operations), T614 (NTILE function), T615 (LEAD and LAG functions), T616
(Null treatment option for LEAD and LAG functions), T617 (FIRST_VALUE and
LAST_VALUE functions), T618 (NTH_VALUE function), T619 (Nested window
functions), T620 (WINDOW clause: GROUPS option) and T627 (Window framed
COUNT DISTINCT), of ISO/IEC
9075-2:2023 §6.10, §6.11, §6.31, §7.15, §9.20, §9.23 and §10.9. T612 is
the window name, the WINDOW clause, an existing window name, frame
exclusion, PERCENT_RANK, CUME_DIST, ROW_NUMBER of no ordering,
WIDTH_BUCKET, the FILTER clause, and the hypothetical set and inverse
distribution functions.

| Feature | Rules | What turborel does |
|---|---|---|
| Window functions | §6.10 SR2 | ROW_NUMBER, RANK, DENSE_RANK, PERCENT_RANK, CUME_DIST, NTILE, LEAD, LAG, FIRST_VALUE, LAST_VALUE, NTH_VALUE and every set function, followed by OVER and a window name or specification. One is only in the select list of a query specification, never in WHERE, GROUP BY, HAVING, a set function's argument or a window's definition, else SQLCODE -103. Its arguments, keys and offsets hold no outer reference and no query expression [§9.23 SR3]. |
| Window names | §7.15 SR2, SR12, SR16 | The WINDOW clause after HAVING names windows, each name once. A window name in the select list, or an existing window name in a later definition, names one of them. A window built on an existing one adds no PARTITION BY, adds no ORDER BY where that one has one, and builds on none that has a frame. Each is SQLCODE -103 otherwise. |
| Partitions | SR4 to SR6, GR3 | PARTITION BY names columns of the table expression, grouping columns in a grouped query, each of a type a grouping operation takes. COLLATE is written only of a character string. Rows are of one partition where those columns are not distinct, each string by its collation. |
| Orderings | §10.10, GR4 | An ORDER BY of sort keys a sort takes, NULLS FIRST and LAST among them. Without one, every row of a partition is a peer of every other (US034). |
| Frames | SR10, SR11, SR15, GR5 | ROWS, RANGE and GROUPS, a start alone ending at CURRENT ROW. Bound pairs SR10 refuses are SQLCODE -103. An offset is a literal or a parameter, an exact integer of ROWS and GROUPS. RANGE with an offset orders by one key of a numeric, datetime or interval type, its offset numeric or an interval that key adds. GROUPS needs an ORDER BY. A frame is given whole as GR5 defines it, a null RANGE key's frame the rows of null keys. A date moved by a year-month interval to a day past its month's end takes the month's last day [GR5b i 1 A]. An offset negative or null is SQLSTATE 22013. |
| Exclusion | GR5b ii | EXCLUDE CURRENT ROW, GROUP, TIES and NO OTHERS. |
| Ranks | §9.23 SR5 | ROW_NUMBER is the row's place, RANK and DENSE_RANK count the rows and peer groups before its own, PERCENT_RANK and CUME_DIST are the fractions SR5g and SR5h define. The four ranks need an ORDER BY, and none of the five takes a frame, else SQLCODE -103. |
| NTILE | §6.10 GR1a, §9.23 SR5i | Each partition cut into the number of tiles given, the first MOD(rows, tiles) one row longer, a row's value its tile's number, of COUNT's type (IV084). The number is a `<simple value specification>` or `?`, exact numeric of scale 0 (a `?` INTEGER [§20.7 GR5a xxxii]), else SQLCODE -103. It is evaluated once, as the first partition is read, so of no rows it raises nothing. Null, each tile is null. Below 1, SQLSTATE 22014. |
| LEAD and LAG | §6.10 GR1b, §9.23 SR5j | The extent of the row the offset after or before the current one in its partition, of the extent's type and collation derivation. The offset is an `<unsigned integer>`, 1 if not written, and 0 takes the current row's own extent. Past the partition, the default, evaluated for the current row (RULES.md D15), null if not written. |
| Their default | SR5j iii | \[EXTENSION\] SR5j iii asks a default "compatible" with the extent, which would refuse `LAG(I, 1, 0)` of an INTEGER I, as 0 is NUMERIC. A default is taken where each of its type and the extent's is assignable to the other, and is cast to the extent's type, a string too long cut with SQLSTATE 01004 as CAST cuts it. Else SQLCODE -103. \[EXTENSION\] A `?` default takes the extent's type, where §20.7 GR5a gives it none. |
| Null treatment | §6.10 GR1b, GR1c, GR1d | IGNORE NULLS counts only the rows whose argument is not null, of LEAD, LAG, FIRST_VALUE, LAST_VALUE and NTH_VALUE. RESPECT NULLS, the default, counts every row. LEAD and LAG of offset 0 take the current row's extent under either. |
| FIRST_VALUE, LAST_VALUE and NTH_VALUE | §6.10 GR1c, GR1d, §9.23 SR6 | The first, the last and the nth argument of the row's frame, its exclusion applied, of the argument's type and collation derivation. NTH_VALUE counts from the frame's first row, or from its last under FROM LAST. Its `<nth row>` is as NTILE's number of tiles, null giving null and below 1 SQLSTATE 22016. A frame of no such row gives null. |
| Nested in an argument | §6.10 | A window function in the argument of LEAD, LAG, FIRST_VALUE, LAST_VALUE or NTH_VALUE is computed first, by its own window, or with the outer one's where they are alike and in one pass. |
| Nested window functions | §6.11 SR1 | ROW_NUMBER of a row marker and VALUE_OF at one are only in the aggregated argument of a set function a window function immediately contains, not in a set function of the groups within it, which §9.20 makes a column of the groups first. Elsewhere, SQLCODE -103. |
| ROW_NUMBER of a marker | SR2, GR2, GR3 | The place in the partition, from 1, of the partition's first or last row, the current row, the frame's row the set function takes, or the frame's first or last row as its exclusion leaves it, of COUNT's type (IV085). |
| VALUE_OF | SR3, SR4, GR4 | Its value expression read at the marker's row moved by the offset, after a plus or a minus sign. Past the partition, or of a null offset, the default, null if not written. Of the type §9.5 gives its value expression and default, each cast to it. The offset is a `<simple value specification>` or `?`, exact numeric of scale 0 (a `?` NUMERIC(18, 0), 18 the most digits of an exact number [§20.7 GR5a xxxiii]), evaluated once as the first partition is read, else SQLCODE -103. A `?` default takes the value expression's type [§20.7 GR5a xxxiv]. |
| Their columns | GR4e ii | A value expression's columns are read at the row its marker gives. GR4 says nothing of the default's, and the rest of the argument's are the frame's row's [GR1], so a default's are read there too. |
| An empty frame | NOTE 170 | Reads no argument, so its set function is of no rows. |
| In a constraint | §9.16 SR1c | No nested window function is a potential source of non-determinism, so a CHECK or an assertion may contain one. |
| Set functions | §9.23 SR8, §10.9 | Each is computed over the row's frame as of a group, nulls left out with SQLSTATE 01003, an empty frame giving what an empty group gives. One of a window of an ordering or a frame is not LISTAGG or an inverse distribution function, and is DISTINCT only if COUNT [SR8d], else SQLCODE -103. |
| Framed COUNT DISTINCT | §9.23 SR8d ii, §10.9 GR4b, Feature T627 | COUNT(DISTINCT x) over a window of an ordering or a frame counts each value of the frame's rows once, by the comparison of x's collation, its exclusion applied. Over a frame that moves, each value's count among the last frame's rows is kept in a work area that spills (RULES.md D38), and only the rows a frame gains or loses are counted. A frame's bounds move only forward, so a partition costs about its rows, not its rows times the frame's length. |
| Grouped queries | §9.20 | Windows of a grouped query are over its groups, after HAVING, so their arguments and keys may be set functions of the group, `SUM(SUM(X)) OVER ()`. SELECT DISTINCT removes groups alike before the windows, as SR22 builds the derived table of groups. |
| Their order of evaluation | §7.16 | Windows are computed over the rows of FROM, WHERE, GROUP BY and HAVING, before DISTINCT and the cursor's ORDER BY. A query of a window function in its select list makes a view or cursor not updatable. |
| In a constraint | §9.16 SR1c, §11.9 CR2 | ROW_NUMBER, NTILE, LEAD, LAG, FIRST_VALUE, LAST_VALUE, NTH_VALUE and a set function over a ROWS window are potential sources of non-determinism, so a CHECK or an assertion containing one is SQLCODE -103. |
| WIDTH_BUCKET | §6.31 SR21, GR17 | Each operand a number and the count exact of scale 0, else SQLCODE -103. The result is of the count's type. Null if any operand is. A count of 0 or below, or equal bounds, SQLSTATE 2201G. 0 before bound 1, the count plus 1 at or past bound 2, SQLSTATE 22003 if that is past the count's type, else the greatest whole number not above the count times the operand's distance from bound 1 over the range's width, plus 1. Of exact operands it is found exactly, of an approximate one in approximate arithmetic, and of a DECFLOAT one in DECFLOAT, kept within 1 to the count where rounding would pass either end. \[EXTENSION\] A `?` count is NUMERIC(18), a `?` operand or bound of the type of the first other operand or bound. |
| FILTER | §10.9 GR4 | `FILTER (WHERE cond)` after COUNT(*), a general set function, ARRAY_AGG or LISTAGG, of the groups or OVER a window, before OVER. Its set function is fed only the rows cond is true of, and of a row it is false or unknown of the argument is not computed, so a null of it is not left out and gives no SQLSTATE 01003. Over a window, each frame's rows cond is true of, of a frame growing, moving or computed again, and of nested window functions. A filtered set function is computed row by row, as one of DISTINCT is, not by batches. GROUPING takes no filter, so one written is SQLCODE -102. |
| Its condition | §6.9 SR5, §10.9 SR8, §6.11 SR1 | No set function, query expression, window function, nested window function or outer reference, else SQLCODE -103. |
| Of an enclosing query | §6.9 SR6, §10.9 SR8 | A set function whose argument is an outer reference is the enclosing query's, aggregated over its rows, so its filter would be too. One naming a column, which SR8 refuses of the query it is written in and the enclosing query's scope does not reach, is SQLCODE -103. One of no column is computed with it. |
| Hypothetical set functions | §10.9 SR10, GR9 | `RANK`, `DENSE_RANK`, `PERCENT_RANK` and `CUME_DIST` of values, `WITHIN GROUP (ORDER BY ...)` of as many sort keys, else SQLCODE -103 \[SR10b\]. Each is the value GR9's window function gives the hypothetical row among the group's rows, counted as each row is read, nothing held. A null key is placed as its null ordering says, never eliminated, so no SQLSTATE 01003. DENSE_RANK takes each distinct row once, by the values of the rows a DISTINCT set function has taken, which spill past the work area. RANK and DENSE_RANK are of COUNT's type (IV174), PERCENT_RANK and CUME_DIST DOUBLE PRECISION (IA043). Of a group of no rows, 1, 1, 0 and 1. They take FILTER. |
| Their values and keys | SR10a, SR10c, §9.14 SR3, §6.11 SR1 | No window function, set function, query expression or nested window function, else SQLCODE -103. Each value and its key of compatible types, else -103, compared as of the type §9.5 gives them, character strings by the collation §9.15 gives both. A key is an operand of an ordering operation. \[EXTENSION\] A `?` value takes its key's type, where §20.7 GR5a gives it none. |
| Not aggregated | SR14, §6.9 SR6 | The values are of the groups, so a column they name is a grouping column or an outer reference, else SQLCODE -103 as of a select list [§7.16 SR15]. Of a grouping set, a column it leaves out is null in them. Sort keys whose columns are all outer references, its aggregated column references, make it the query's of those columns, bound and computed there as a set function of an outer reference is, its values of that query's groups, in its select list or a subquery of its HAVING, else SQLCODE -103 [§6.9 SR6b, SR7]. A key of a column of its own query too keeps it there, the outer reference a value of the enclosing row. |
| Over a window | §9.23 SR8c | A hypothetical set function is no window function's aggregate, else SQLCODE -103. |
| Inverse distribution functions | §10.9 SR12, GR10 | `PERCENTILE_CONT` and `PERCENTILE_DISC` of a fraction, `WITHIN GROUP (ORDER BY ...)` of one sort key, else SQLCODE -103 \[SR12a\]. The key's values not null are sorted in its direction, nulls eliminated with SQLSTATE 01003. A null fraction, or no values, gives null. A fraction below 0 or above 1 is SQLSTATE 22003, even of a group of no rows. PERCENTILE_CONT interpolates between ROW0 and ROW1 by FACTOR in SQL's arithmetic, so of intervals an interval, and PERCENTILE_DISC takes the value at the ⌈NVE·N⌉th place, the first if NVE is 0, the first whose CUME_DIST is at least NVE. They take FILTER. |
| Their rows | GR10h | \[DESIGN\] Of an exact fraction ROW0, ROW1 and the place PERCENTILE_DISC takes are found exactly, so a CUME_DIST equal to the fraction is at least it, as GR10h iii's comparison of exact numbers has it. Of an approximate fraction they are found in approximate arithmetic, as NVE·(N−1) is. A DECFLOAT fraction is taken to 17 decimal places, the most an exact number of one digit before them holds. |
| Their values | GR10d, RULES.md D38 | \[DESIGN\] No accumulator holds them. A grouping sorts each group's values with the number of its first row and the call's place, in a part of its work area that spills, and each group, finished in that order, reads its values once and keeps the one or two its fraction picks. A window sorts each partition's values once, in a part that spills, and reads by place those each row's fraction picks. They are computed row by row, as a set function of DISTINCT is, not by batches. |
| Their key and fraction | SR12b, SR12d, SR14, §6.11 SR1 | No window function, set function, query expression or nested window function, else SQLCODE -103. The key is numeric or an interval, and the fraction numeric, else -103. The fraction is not aggregated, so of a grouped query it names grouping columns and outer references, as a hypothetical set function's values do, computed of each group's first row, or of a group of no rows of its grouping values. A key whose columns are all outer references makes it the enclosing query's, as of a hypothetical set function. \[EXTENSION\] A `?` fraction is NUMERIC(18,17), where §20.7 GR5a gives it none. |
| Their result | SR12e | Of an exact or approximate key DOUBLE PRECISION (IV177), of a DECFLOAT key DECFLOAT(34) (IV177), of an interval its type. |
| Over a window | §9.23 SR8d i | Only of a window of no ordering and no frame, so each row's frame is its partition, else SQLCODE -103. The fraction is computed of each row. |
| In a constraint | §9.16 SR1c | A filter is no potential source of non-determinism. An assertion of a filtered set function is checked by its query, not from the changes alone. |

A window reads its input sorted by its partition columns, then its
ordering, unless the input is ordered so, as a column store kept in that
order is. Windows of the same partitioning and ordering are computed in
one pass, and one sort orders peers alike for each of them (US035). A
partition is held while its rows are read, spilling past the work area,
and each frame bound is a pointer that only moves forward. A set
function over a frame starting at the partition's first row, and
excluding nothing, is fed each row once. Otherwise it merges states of a
tree of the partition's rows where the accumulator writes its state and
the tree fits the work area, and else reads each frame's rows again.
Of each row is held how many rows of its partition before it a function
under IGNORE NULLS counts, and the arguments not null are held apart in
order, spilling past their part of the work area. So LEAD, LAG and the
nth row of a frame are found by counting, not by reading the rows
between. Of a set function holding nested window functions, each row
holds the columns its argument reads. An argument that reads none of
CURRENT_ROW, BEGIN_FRAME and END_FRAME is of the frame's row alone, so
it is computed once for each row, held apart and spilling, and the set
function computed over it as over any argument. Any other is computed
for each row of each frame, so its cost is the rows times the frame's
length.

OVER, WINDOW, PARTITION, RANGE, GROUPS, the five rank names and NTILE,
LEAD, LAG, FIRST_VALUE, LAST_VALUE and NTH_VALUE, which SQL:2023 reserves
and SQL-92 does not, are key words only where they are read. So are
PRECEDING, FOLLOWING, UNBOUNDED, EXCLUDE, TIES, OTHERS, RESPECT and
IGNORE, which neither reserves. A function registered as RANK stays
callable, RANK read as the window function only where `() OVER` follows,
and one registered as LAG where OVER does not follow its arguments. VALUE_OF
and the six row markers, which SQL:2023 reserves and SQL-92 does not,
are read only in a nested window function, ROW_NUMBER so only of one
marker alone in its parentheses, and VALUE_OF only where AT and a marker
are among its arguments, so a function registered as VALUE_OF stays
callable. Within VALUE_OF's value, AT begins a time zone only before
LOCAL or TIME.

### Row pattern recognition

Features R010 (Row pattern recognition: FROM clause), R020 (Row pattern
recognition: WINDOW clause) and R030 (Row pattern recognition: full
aggregate support), of ISO/IEC 9075-2:2023 §4.19 to §4.23, §6.27, §7.7
to §7.9, §7.15 and §9.41.

| Feature | Rules | What turborel does |
|---|---|---|
| In FROM | §7.6, §7.7 SR3, SR11 | `<table primary> MATCH_RECOGNIZE (...)` of a table, a view or a derived table, its correlation name optional. Its input's name is seen only in its PARTITION BY and ORDER BY [§7.6 SR8b i 5]. PARTITION BY names columns of the input of a type grouping takes, COLLATE only of a character string, and ORDER BY sort keys an ordering takes, else SQLCODE -103. A result of no columns is -103. |
| Its output | §7.7 SR7 to SR10, GR5 | ONE ROW PER MATCH gives the partition columns and the measures, one row of each match. ALL ROWS PER MATCH gives the partition columns, the ordering columns, the measures and the rest of the input's columns, of each row of each match, SHOW EMPTY MATCHES the default. A match of no rows gives the row it was sought at, its measures of no rows, COUNT 0 and a navigation null. OMIT EMPTY MATCHES gives none of it, and WITH UNMATCHED ROWS gives each row no match covered, its measures null. Rows `{- -}` excludes give no row. |
| Variables | §7.9 SR4, SR7 to SR15, SR18 | The primary row pattern variables are those PATTERN names. SUBSET names a union of them, not a primary one, each once. DEFINE names primary ones, each once, and one it leaves out is true of every row. The universal variable, of every row, is the one a bare column names. Else SQLCODE -103. |
| The pattern | §7.9 SR19 to SR22, §9.41 | Concatenation, `|`, PERMUTE, `^`, `$`, `{- -}`, `()` and the quantifiers `*`, `+`, `?`, `{n}`, `{n,}`, `{,m}` and `{n,m}`, each reluctant with a `?` after it but `{n}`. A quantifier's m is at least its n and above 0, else -103. The match taken from a row is the first in §9.41's order of preference, the alternatives of `|` left to right, of PERMUTE in the lexicographic order of its permutations, a greedy quantifier the most iterations first and a reluctant one the fewest. An iteration of a quantifier that matches no rows ends it [§9.41 GR6b i]. WITH UNMATCHED ROWS with `{- -}` is -103 [SR22]. |
| Where the next match is sought | §7.9 SR23, GR2 | AFTER MATCH SKIP PAST LAST ROW, the default, TO NEXT ROW, TO FIRST v and TO LAST v, v a primary or union variable, and TO v as TO LAST v. Skipping to a variable that mapped no row is SQLSTATE 2202K, and to the match's first row 2202L. |
| References | §4.23, §6.27 | `v.c` is the column c of the last row v mapped, as far as the current row, and a bare `c` the universal variable's. Of a row no variable mapped, null. |
| Navigation | §6.27 SR1 to SR5, GR | PREV and NEXT move by a physical offset from a row, 1 by default, and FIRST and LAST by a logical offset among the rows a variable mapped, 0 by default. The two nest, a logical one within a physical one. The offset is a literal, its sign written against its number, or a parameter, exact of scale 0, a negative literal SQLCODE -103 and a negative or null parameter SQLSTATE 2202J. Its value expression names columns of one variable, and holds no navigation but that nesting, no set function and no query expression, else -103. A physical offset may pass the match within the partition. Past the partition, or a logical offset past the rows the variable mapped, null. |
| RUNNING and FINAL | §4.21, §6.27 SR4, §10.9 SR11 | RUNNING reads the match as far as the current row, the default, and FINAL the whole match. FINAL is only in MEASURES, else SQLCODE -103. Of ONE ROW PER MATCH both read the whole match. |
| CLASSIFIER and MATCH_NUMBER | §6.27, §6.31 SR22 | CLASSIFIER() is the name of the primary variable that mapped the row, a character string of the identifier type, null of a row of no variable. MATCH_NUMBER() is the number of the match in its partition, from 1, of COUNT's type. MATCH_NUMBER is not in a window, else -103. |
| Set functions | §10.9 SR6, SR11, SR15, Feature R030 | Every set function, `COUNT(v.*)` too, of the rows of one variable its arguments' columns name, or of the universal one, RUNNING or FINAL. Its arguments name columns of one variable, else -103. No GROUPING, no window function and no outer reference. A hypothetical set function's values and an inverse distribution function's fraction are of the partition, each column naming one of its partition columns or one they determine, else -103. |
| Subqueries | §7.8 SR3, §7.9 SR14 | A query expression in MEASURES or DEFINE references no row pattern variable, else SQLCODE -103. |
| In a window | §7.15 SR13, SR14, GR6, §9.23 SR9 | A window definition's MEASURES, AFTER MATCH SKIP, INITIAL or SEEK, PATTERN, SUBSET and DEFINE, after its frame. The frame is ROWS BETWEEN CURRENT ROW AND a bound, EXCLUDE NO OTHERS, and the pattern has no `^` and no `$`, else SQLCODE -103. Each row's frame is narrowed to the match starting at it, INITIAL the default, or of SEEK the first match starting in its frame, empty of a row with none or that a match skipped past. `m OVER w` names a measure of w's, else -103. |
| In a recursive query | §7.17 SR3f | A recursive query holds no row pattern recognition, else SQLCODE -103. |

| What | Why | Outcome |
|---|---|---|
| A window's row pattern recognition in a grouped query | \[DESIGN\] Its variables would be of groups, not rows. | SQLCODE -900. |
| Navigation or CLASSIFIER in an ordered set function's argument that is not aggregated | \[DESIGN\] It names no row of the partition. | SQLCODE -900. |
| A pattern past the matcher's limits | \[DESIGN\] PERMUTE of more than 8, a pattern compiled to more than 524,288 instructions, nested more than 256 deep, or a search past 10,000,000 steps from one row. | SQLCODE -900. |

A partition is read sorted by its partition columns, then its ordering,
unless its input is ordered so, and is held while its matches are
sought, spilling past the work area (RULES.md D38). The matcher
backtracks through the pattern in its order of preference, so the first
match it finds is the one taken.

MATCH_RECOGNIZE, MEASURES, PATTERN, DEFINE, SUBSET, PERMUTE, PREV,
CLASSIFIER, MATCH_NUMBER, RUNNING, FINAL, SKIP, PAST, INITIAL and SEEK,
which SQL:2023 reserves or does not and SQL-92 does not, are key words
only where they are read, so a column of one of those names stays a
column.

### Arrays and row types

Features S090 (Minimal array support), S099 (Array expressions), S301
(Enhanced UNNEST), S403 (ARRAY_MAX_CARDINALITY), S404 (TRIM_ARRAY), T051
(Row types) and T053 (Explicit aliases for all-fields reference). An
array is an ordered collection of elements of one type, at most its
maximum cardinality of them [§4.12.2]; a row is a sequence of fields, each
of a name and a type [§4.10].

| Feature | Rules | What turborel does |
|---|---|---|
| Array types | §6.1 SR43 to SR47 | `<data type> ARRAY [n]`, of any data type of the standard, rows and arrays among them, n from 1 to 65,535 (SQLCODE -900 past it), 65,535 if not written. No type of an extension is an element's. |
| Row types | §6.1 SR35 to SR42 | `ROW (name type, ...)`, no field named twice (SQLCODE -103). A column, a domain, a CAST's target and a dynamic parameter's type may be either. |
| Constructors | §6.49, §7.1 | `ARRAY[...]`, `ARRAY[]` where its type is known, `ARRAY(query)` of one column ordered by the query's ORDER BY, and `ROW(...)`, its fields named F1, F2 and so on. An empty array's type is its target's, so `ARRAY[]` alone is SQLCODE -103. |
| Element reference | §6.25 | `A[i]`, null of a null array or index, SQLSTATE 2202E for an index not of 1 to the cardinality. |
| Field reference | §6.16, §6.6 SR9b, SR11 | `R.F`, `(expr).F` and identifier chains. A chain of three is a column first, then a column's field. One of four names a catalog first, refused (SQLCODE -103) when it may also be a correlation name's fields. A longer one is fields of a column. A chain of four of no basis is SQLSTATE 3D000. |
| All fields | §7.16 SR2c, T053 | `R.*` in a select list is each field, and `R.* AS (a, b)` names them, as many names as fields. |
| CARDINALITY, ARRAY_MAX_CARDINALITY, TRIM_ARRAY | §6.31, §6.48 | CARDINALITY of a null array is null; ARRAY_MAX_CARDINALITY of one is its type's maximum [GR1a]. TRIM_ARRAY of a negative count or one past the cardinality is SQLSTATE 2202E. |
| Concatenation | §6.47 | `A \|\| B` of two arrays, of the greatest maximum of both summed, 65,535 at most; past it, SQLSTATE 2202F. |
| ARRAY_AGG | §10.9 SR7 | `ARRAY_AGG(x ORDER BY ...)` of each row's value, nulls among them, null of no rows; past 65,535 elements, SQLSTATE 2202F. Its grammar has no DISTINCT, so one written is SQLCODE -102. |
| Comparison | §8.2 GR1b, §9.11 | Arrays are compared for equality alone, of the same cardinality, element by element, three valued, so a pair known unequal makes them unequal though another is unknown. Rows are compared field by field and ordered by their first unequal field. A value of a row type is spread into its fields wherever a row value constructor is [§7.2]. |
| Ordering and grouping | §9.12 SR3, §9.14 SR3 | An array in ORDER BY, GROUP BY, DISTINCT, UNION but UNION ALL, EXCEPT, INTERSECT, MAX, MIN, a key, an index or `<`, `<=`, `>`, `>=`, BETWEEN, GREATEST or LEAST is SQLCODE -103. A named columns join (NATURAL or USING) of array columns is valid, its columns compared for equality (F402). Rows are ordered and grouped field by field. |
| Assignment and CAST | §9.2, §6.13 | Element by element and field by field, an array of more non-null elements than its target's maximum SQLSTATE 2202F. CAST also takes an array to a multiset and a multiset to an array ([Multisets](#multisets)). Nothing else is assigned or cast to or from an array or a row. |
| `SET A[i] = v` | §14.15 GR10b | Past the cardinality the array is extended with nulls; index 0 or past the maximum is SQLSTATE 2202E, a null array SQLSTATE 2200E. |
| Target `A[i]` | §6.4 SR7, SR8, §14.7 GR4b ii 1, §10.4 GR10b ii 1 | A SELECT INTO target or a CALL output argument in a procedure's body, of an SQL parameter's array, or in a BEFORE row trigger's action, of a new row column's. The array is of an array type or a distinct type whose source is one, and the position a literal or, in a body, an SQL parameter, exact numeric of scale 0 (SQLCODE -103). The element is assigned as `SET A[i] = v` assigns it, with the same SQLSTATEs. FETCH reads its targets alike, but neither a body nor an action may hold a FETCH. |
| UNNEST | §7.6 SR8, §7.10 | `UNNEST(a, ...) [WITH ORDINALITY] AS t [(names)]` of the rows of its arrays' elements, padded with nulls to the longest, each array of rows spread into its fields. The ordinality column is INTEGER (IV158). It reads the FROM items before it, after a comma or as the second operand of CROSS, INNER and LEFT joins; as that of a RIGHT or FULL join it may read none of the first (SQLCODE -103). |
| Dynamic SQL | §20.1, §20.9 to §20.12 | An item of a composite type is followed by items of its parts, of LEVEL one greater, with DEGREE, CARDINALITY and RETURNED_CARDINALITY, and KEY_MEMBER 0. TOP_LEVEL_COUNT counts the items of LEVEL 0. DESCRIBE takes WITH NESTING and WITHOUT NESTING. DATA of an item of a row or of an array's element is SQLSTATE 0700C to get and 0700D to set; a LEVEL set out of order SQLSTATE 0700E. |
| Its description | ISO/IEC 9075-11 | COLUMNS and DOMAINS give MAXIMUM_CARDINALITY and DTD_IDENTIFIER, and ELEMENT_TYPES and FIELDS describe the parts. A column's DTD_IDENTIFIER is its ordinal position, a domain's 1, and a part's its parent's, a point, and its field's place, or 1 for an element. |

An array's elements and a row's fields are kept in the row of their
table, and an array of a row type is unnested into the fields (RULES.md
D68). A collection derived table in a RIGHT or FULL join that reads a
table reference outside the join, or nested in a LEFT join's second
operand and reading its first, is valid SQL not supported (SQLCODE -900).
S091 (Basic array support) is claimed with S093 (Arrays of distinct
types), which it implies, and with it S095 (Array constructors by query),
S096 (Optional array bounds), S097 (Array element assignment) and S098
(ARRAY_AGG), each processed as the table says. ARRAY, ROW, UNNEST and
the functions are key words only where they are read (Appendix B).

### Multisets

Features S271 (Basic multiset support), S275 (Advanced multiset
support), S272 (Multisets of user-defined types), S281 (Nested
collection types), S402 (Distinct types based on multiset types) and
S202 (SQL-invoked routines on multisets). A multiset is an unordered
collection of elements of one type, each possibly there more than once
[§4.12.3].

| Feature | Rules | What turborel does |
|---|---|---|
| Multiset types | §6.1 SR48, §4.12.3 | `<data type> MULTISET`, of any data type of the standard, rows, arrays, multisets and distinct types among them. No type of an extension is an element's. A multiset holds at most 65,535 elements, as an array may (see [Limits](#limits)); past it, SQLSTATE 2200Q [§10.9 GR3b]. |
| Constructors | §6.52 | `MULTISET[...]`, of the type its elements bring together [§9.5], `MULTISET[]` where its type is known, and `MULTISET(query)` of one column and `TABLE(query)` of a row of each row's columns, named F1, F2 and so on (UV076). Either query with ORDER BY, or MULTISET's of more than one column, is SQLCODE -103. Of no rows, either is the empty multiset. |
| Element reference | §6.26 | `ELEMENT(m)`, null of a null or empty multiset, SQLSTATE 21000 of one of more than one element. |
| CARDINALITY and SET | §6.31, §6.51 | CARDINALITY counts the elements, null of a null multiset. SET keeps one element of each not distinct from one another, nulls among them. |
| Operators | §6.50 | `MULTISET UNION`, `EXCEPT` and `INTERSECT`, each ALL (implicit) or DISTINCT, null if an operand is. INTERSECT binds tighter, and UNION and EXCEPT go left to right. The result is a multiset of the element types brought together [§9.5]. |
| Comparison | §8.2 GR1b iii, §8.15 GR1c vi, §9.11 | Multisets are compared for equality alone. Two are equal if some pairing of their elements makes each pair equal, unknown if one makes each pair equal or unknown, else false, and distinct if some value is not distinct from more elements of one. A multiset compared with an array is SQLCODE -103. |
| Predicates | §8.16, §8.17, §8.18 | `x [NOT] MEMBER [OF] m`, `m [NOT] SUBMULTISET [OF] n` and `m IS [NOT] A SET`, each by its General Rules, nulls among the elements. A row value is a member of a multiset of rows. |
| Ordering and grouping | §9.12 SR3, §9.13 SR3, §9.14 SR3 | A multiset in ORDER BY, GROUP BY, DISTINCT, UNION but UNION ALL, EXCEPT, INTERSECT, MAX, MIN, a key, an index or `<` and the others is SQLCODE -103. So is a grouping operation, SET, IS A SET, SUBMULTISET, `=`, MEMBER and the operators but UNION ALL, over elements of a type grouped by nothing, an array, a multiset or a large object. |
| Aggregates | §10.9 SR7k to SR7n | COLLECT gathers its values, nulls among them, DISTINCT once each. FUSION gathers the elements of its multisets and INTERSECTION keeps as many of each as the multiset of fewest, each leaving out a null multiset. Each is null of no values. FUSION and INTERSECTION take no DISTINCT (SQLCODE -103). |
| Assignment and CAST | §9.2, §6.13 SR5, SR6 | Element by element, to a multiset alone. CAST takes a multiset to a multiset or an array and an array to a multiset; to an array of a maximum cardinality short of its elements it is SQLSTATE 2202F. A default or an SQL variable's may be `MULTISET[]` of a multiset type. |
| UNNEST | §7.6 SR8e | `UNNEST(m) AS t [(names)]` of a row of each element, one multiset alone and without WITH ORDINALITY (SQLCODE -103). |
| Nested and user-defined | S281, S272, S402 | Arrays of multisets, multisets of arrays and of multisets, and multisets of distinct types. A distinct type of a multiset source is taken wherever a multiset is, with ordering form NONE. |
| Routines | S202 | An SQL-invoked routine's parameters, result and SQL variables may be of multiset types. |
| Non-determinism | §9.16 SR1n, SR1o, SR1aq | A CHECK, assertion or generation expression holding a grouping operation, SET or INTERSECTION over elements of a character string, distinct or zoned datetime type is SQLCODE -103. |
| Dynamic SQL | §20.1 Table 30, §20.9 GR7 | DESCRIBE gives TYPE 55 and, WITH NESTING, the element's item after it. RETURNED_CARDINALITY counts the elements, and DATA of the element's item is SQLSTATE 0700C. |
| Its description | ISO/IEC 9075-11 | COLUMNS gives DATA_TYPE 'MULTISET' and no MAXIMUM_CARDINALITY, and ELEMENT_TYPES the element type. |

A multiset's elements are kept in the row of their table, as an array's
are, and paired by classes and a matching (RULES.md D80). MULTISET,
MEMBER, SUBMULTISET, ELEMENT, COLLECT, FUSION and INTERSECTION are key
words only where they are read, so a column may be named by any of them
(Appendix B). `TABLE(query)` is processed though T326 (Table functions)
is not claimed. S274 (Multisets of reference types) is claimed with
[reference types](#reference-types), and S233 (Multiset locators) waits
for host languages that hold them.

### Distinct types

Features S011 (Distinct data types), S028 (Permutable UDT options list)
and S401 (Distinct types based on array types), and S402 (Distinct types
based on multiset types) with [multisets](#multisets). CREATE TYPE
defines a distinct type of a predefined, an array or a multiset type
[§11.51], and DROP
TYPE drops one [§11.59]. Its values are its source type's, and compare
only with values of the same type [§4.9.5].

| Feature | Rules | What turborel does |
|---|---|---|
| Definition | §11.51 SR1 to SR7 | `CREATE TYPE n AS <predefined type> \| <array type> [FINAL]` and the two CAST options, in any order, each at most once. The schema is there (3F000) and is its schema definition's, and no type or domain of the schema has the name, else SQLCODE -103. A row type, another distinct type or a type of an extension as its source is SQLCODE -103, as is each option of a structured type but a method specification. |
| Source type | §11.51 SR7a | An exact type is kept as NUMERIC of its precision and scale and an approximate one as FLOAT of its precision (IV223), so INTEGER is NUMERIC(10, 0). |
| Comparison | §4.9.5, §8.2, §11.51 GR2c | Of values of one distinct type alone, as their source values compare. Ordered, grouped, made DISTINCT, kept unique and indexed so, unless the source is an array type, whose values compare with nothing. |
| Assignment and CAST | §4.13, §9.2 SR3, §6.13 SR12 | To a distinct type, of a value of the type, or of a type whose type precedence list [§9.7] holds the source's designator and that is assignable to the source. From one, to exactly its source type, of the same precision and scale. Any other is SQLCODE -103. |
| Operators | §6.30 SR3 and others | None but comparison. Arithmetic, a set function but COUNT, MAX and MIN, a string function, a predicate of strings, a Boolean operator, an element reference and CARDINALITY take no value of a distinct type, SQLCODE -103. |
| Result type | §9.5 SR3l | Values of one distinct type are brought together with values of that type alone, in CASE, COALESCE, a set operation and a VALUES of more than one row. |
| Functions | §11.51 GR2b, §9.18, §10.4 | FNUDT, named by the CAST (SOURCE AS DISTINCT) option or the type's name, and FNSDT, named by the CAST (DISTINCT AS SOURCE) option or the source type's name [§9.9], are invoked by name, a name not qualified found in the schema of the statement's names (ID047). They overload with the schema's SQL-invoked functions, and of the functions of one name the arguments' types choose, as an SQL-invoked function's invocation chooses. None that takes them, or two alike, is SQLCODE -103, and a ? argument of a name of two functions of one parameter is SQLCODE -103. A function of the name another function of the schema has, not told apart from it, is refused [§11.60 SR20b]. Their specific names are the type's name and _TO_DISTINCT or _TO_SOURCE. |
| Privileges | §6.1 AR1, §6.13 AR2, §9.18 SR6, §12.2, §12.3, §12.7 | Naming the type needs USAGE on it, and a CAST to or from it, or invoking a function, EXECUTE on the function, each SQLCODE -104 if not held. The schema's owner holds them grantably. GRANT and REVOKE take USAGE ON TYPE and EXECUTE ON SPECIFIC FUNCTION n or FUNCTION n [(types)]. REVOKE with RESTRICT is 2B000 where it would abandon a column of a type based on the type, a view, constraint, assertion, generated column, trigger or routine, or a grant. A routine is abandoned when its owner loses USAGE on a parameter's or its result's type [GR12o]. With CASCADE each goes, a column as ALTER TABLE DROP COLUMN drops it. UNDER is SQLCODE -103 of a distinct type. |
| Drop | §11.59 | RESTRICT refuses while a column, or another type's source, is based on the type, or a view, constraint, assertion or generated column references it or invokes its functions. CASCADE drops each, the types based on it among them, and is refused before it drops any if a table would lose every column [§11.23 SR4]. DROP SCHEMA CASCADE drops its types. |
| Methods | §11.51 SR10, §11.56, §6.18 | Original method specifications, instance or static, with their bodies by CREATE METHOD, added by ALTER TYPE ADD and dropped by DROP METHOD. A distinct type has no supertype, so no method of one is OVERRIDING, and `(v AS t).m()` names the type itself. |
| A default | §11.5 SR3, SR4e | A column of a distinct type has no default but NULL, and one of a type of an array source NULL or ARRAY[]. |
| Transforms | §4.9.6, §20.11, §20.12 | A value goes to the host and to a client as its source's, and comes from one as a value of its source made the type (ID048). A dynamic USING argument or INTO target is of a type its source is assignable to and from, else 07006. The protocol describes a column of a distinct type as its source type. |
| Dynamic SQL | §20.1, §20.10 GR7d viii | DESCRIBE gives TYPE 17 and USER_DEFINED_TYPE_CATALOG, _SCHEMA, _NAME and _CODE 1. Such an item is not valid [§20.1 SR6], so a FETCH or USING of it is 07002 or 07001 until its TYPE is set. |
| Its description | ISO/IEC 9075-11 | USER_DEFINED_TYPES, UDT_PRIVILEGES, ROUTINES, PARAMETERS, ROUTINE_PRIVILEGES and TRANSFORMS describe the types and their functions. COLUMNS, ELEMENT_TYPES and FIELDS give a column's or a part's DATA_TYPE as USER-DEFINED and the type's name in UDT_CATALOG, UDT_SCHEMA and UDT_NAME, ROUTINES in TYPE_UDT_CATALOG, TYPE_UDT_SCHEMA and TYPE_UDT_NAME. |

A value of a distinct type is its source's value, untagged (UV103,
RULES.md D69). A type is FINAL and never altered, so its descriptor is
kept in each column of it. What references a type is found by binding
each definition again without it, or without the privileges a REVOKE
leaves.

Some readings fill gaps in the rules. DROP TYPE with RESTRICT [§11.59
SR4e] refuses a type whose transforms are in a transform descriptor. The
transform its own definition makes [§11.51 GR2b] goes with it [§11.59
GR2], so only one a CREATE TRANSFORM made counts, as does a cast that
names the type [SR4f iv]. A CASCADE that would drop every column of a table is
refused before it drops any [§11.23 SR4]. TYPE, FINAL, INSTANTIABLE,
UNDER, SOURCE, REF, SPECIFIC and FUNCTION are no key words of SQL-92, so
each stays an identifier, a key word only where CREATE TYPE, DROP TYPE,
GRANT and REVOKE read it. A function named by a key word, as FNSDT of a
NUMERIC source is, is invoked delimited, `"NUMERIC"(p)`. The routines a
user defines are those of [SQL-invoked routines](#sql-invoked-routines),
and casts, orderings and transforms those of [structured
types](#structured-types).

### Structured types

Features S023 (Basic structured types), S024 (Enhanced structured types),
S025 (Final structured types), S026 (Self-referencing structured types),
S027 (Create method by specific method name), S028 (Permutable UDT
options list), S081 (Subtables), S111 (ONLY in query expressions), S151
(Type predicate), S161 (Subtype treatment), S211 (User-defined cast
functions), S241 (Transform functions) and S261 (Specific type method).
A structured type [§4.9.3] has attributes, methods and subtypes, and a
value names its most specific type (RULES.md D81).

| Feature | Rules | What turborel does |
|---|---|---|
| Definition | §11.51 | `CREATE TYPE n [UNDER s] AS (attributes) [NOT] INSTANTIABLE [NOT] FINAL`, its `<reference type specification>` and its method specifications, the options in any order. A supertype is FINAL or a type of no UNDER privilege, a cycle, an attribute of the type itself [SR8], or a name met twice is SQLCODE -103. |
| Values | §6.6, §6.18, §6.20, §14.15 | NEW and the constructor function, each attribute's observer `v.a` and mutator `v.a(x)`, and SET of an attribute in UPDATE [§14.15 SR7c]. A type NOT INSTANTIABLE has no constructor. |
| Methods | §6.18, §6.19, §9.18, §11.60 | Instance, static and constructor methods, CREATE METHOD by its signature or SPECIFIC METHOD [S027], OVERRIDING methods dispatched by the value's most specific type, `(v AS t).m()` invoking a supertype's, and SELF AS RESULT. |
| ALTER TYPE | §11.53 to §11.58 | ADD and DROP ATTRIBUTE, ADD and DROP METHOD, ADD OVERRIDING. A change of an attribute while a base table's column is of a type of the family is SQLCODE -103. |
| Predicates and treatment | §8.19, §6.17, §6.33 | `v IS [NOT] OF (t, ONLY u)`, `TREAT(v AS t)`, which raises 0D000 on a value of no subtype of t, and `v.SPECIFICTYPE` [S261]. |
| Orderings | §11.65, §11.66, §8.2, §9.12, §9.14 | CREATE ORDERING EQUALS ONLY or FULL, BY MAP, STATE or RELATIVE, and DROP ORDERING. By MAP a value compares, groups and sorts by its map function's value, by STATE by its attributes, by RELATIVE by its function's sign. A grouping or sort by RELATIVE, a key, a unique or referential constraint and an index of a structured type are SQLCODE -900. A type of no ordering is compared by none (SQLCODE -103). |
| Casts | §6.13 SR12, GR3, §9.2 GR2, §11.63, §11.64 | CREATE CAST and DROP CAST. CAST to a supertype is the value, any other by the cast whose source comes first in the type precedence list [§9.7], else SQLCODE -103. A cast AS ASSIGNMENT stores a value whose type is not assignable to its site. EXECUTE on the function is needed, else SQLCODE -104. |
| Transforms | §11.67 to §11.71, §19.12, §19.13, §6.4 | CREATE, ALTER and DROP TRANSFORM, SET DEFAULT TRANSFORM GROUP and SET TRANSFORM GROUP FOR TYPE, CURRENT_DEFAULT_TRANSFORM_GROUP and CURRENT_TRANSFORM_GROUP_FOR_TYPE, which falls back on a supertype's group and then the default. No host language or external routine uses them, so a value crosses to a client whole. |
| Schema elements | §11.1 | A method, a cast, an ordering and a transform are elements of CREATE SCHEMA, each made after the routines written before it. A method of a type of another schema is SQLCODE -103 [§11.60 SR9q]. |
| Functions dropped | §11.62 SR4a, SR4d | DROP FUNCTION RESTRICT of an ordering, transform or cast function is SQLCODE -103, and CASCADE drops what it is the function of. |
| Privileges | §12.3, §12.7, §6.23 AR1 | UNDER on a type, EXECUTE on a method, and the table/method privilege, granted by SELECT on a typed table or by a `<privilege method list>` [S024]. A method reference needs it on its scope. EXECUTE on an attribute's observer and mutator is granted and revoked by its specific name, or as METHOD of the attribute's name FOR its defining type [§10.6, §9.18 SR6], and held through each subtype. A grant of USAGE or UNDER whose grantor no longer holds it grantably is abandoned, at any remove [GR9]. RESTRICT refuses that, or a routine, constraint or view it abandons, with SQLSTATE 2B000 [GR12q], and CASCADE takes each. |
| Drop | §11.59 | RESTRICT refuses while a subtype, a typed table, a transform group, a cast, an attribute or a column names the type. CASCADE drops each. |
| Dynamic SQL | §20.1, §20.10 GR7d viii | DESCRIBE gives TYPE 17 and the type's name and USER_DEFINED_TYPE_CODE 2. |
| Its description | ISO/IEC 9075-11 | USER_DEFINED_TYPES, with each type's ordering, ATTRIBUTES, DIRECT_SUPERTYPES, METHOD_SPECIFICATIONS, METHOD_SPECIFICATION_PARAMETERS, UDT_PRIVILEGES, TRANSFORMS, TABLE_METHOD_PRIVILEGES, DIRECT_SUPERTABLES and REFERENCED_TYPES. |

SELF AS LOCATOR needs Feature S231 (Structured type locators), which is
not claimed (SQLCODE -900). A method's text is kept and bound again as
it is invoked, as a function's is.

### Typed tables

Features S051 (Create table of type), S081 (Subtables) and S111 (ONLY in
query expressions). A typed table [§4.17.9] has a column of each
attribute of its type and a self-referencing column, and a subtable
keeps its own rows, a superrow of each in each supertable (RULES.md
D81).

| Feature | Rules | What turborel does |
|---|---|---|
| Definition | §11.3 | `CREATE TABLE t OF type [UNDER s] (REF IS c ..., column options)`, in a schema definition too. Its type's columns are its own, so ALTER TABLE adds and drops none [§11.23 SR3]. |
| Reading | §7.6 | A table named without ONLY is read with each proper subtable's rows, cut to its columns. ONLY reads its own, which needs SELECT WITH HIERARCHY OPTION [AR1b ii 2]. |
| Changing | §14.9 to §14.15 | INSERT into a subtable writes the superrows too. A searched UPDATE or DELETE, a cursor, MERGE and TRUNCATE change each row where it is, ONLY the table's own. |
| Constraints | §4.17.9 | A supertable's constraints hold of its subtables' rows, and a key or referenced row is looked for across the family. |
| Privileges | §12.2 GR8 | SELECT WITH HIERARCHY OPTION, granted and revoked, gives SELECT on each proper subtable. |
| Drop | §11.31 | DROP TABLE RESTRICT of a supertable is refused, and CASCADE drops its subtables. |

### Reference types

Features S041 (Basic reference types), S043 (Enhanced reference types),
S094 (Arrays of reference types), S162 (Subtype treatment for
references) and S274 (Multisets of reference types). A reference
[§4.11] is its octets (RULES.md D81).

| Feature | Rules | What turborel does |
|---|---|---|
| Types | §6.1, §4.11 | `REF(type) [SCOPE table]`, of a column, an attribute, an array's or a multiset's element, a routine's parameter or result. |
| Generation | §11.51, §11.3, §14.11 | SYSTEM GENERATED from the family's sequence generator, never given twice, a crash included; USER GENERATED of its representation type, given by INSERT with OVERRIDING USER or SYSTEM VALUE; DERIVED from attributes under a unique constraint. |
| Dereference | §6.22, §6.23, §6.24 | `r -> a`, `r -> m(args)` and DEREF(r), each a lookup in the scope's family. One without a scope is SQLCODE -103. |
| Treatment | §6.17 [S162] | TREAT of a reference to a subtype's reference type, with or without a scope. |
| Scopes | §11.17, §11.18 | ALTER TABLE ... ADD SCOPE and DROP SCOPE, whose RESTRICT is refused while a dereference reads the column. |
| Referenceable views | §11.32, §11.33 | `CREATE VIEW v OF type [UNDER w] (REF IS c ...)`, read with its subviews, read-only, its references through its basis table's. |
| Comparison and CAST | §8.2, §6.13 SR13 | References compare for equality by their octets. CAST to a reference type of a compatible referenced type [GR4a], or by a user-defined cast. |

`REFERENCES ARE CHECKED`, which SQL:1999 had, is not in SQL:2023.

### Periods

Feature T181 (Application-time period tables). A period names two
columns of a table as its start, included, and its end, excluded
[§4.16].

| Feature | Rules | What turborel does |
|---|---|---|
| Definition | §11.3 SR1, SR2, GR4e | `PERIOD FOR name (start, end)` among a table's elements, one to a table. Its columns are two of the table's, written as DATE or one timestamp type, each NOT NULL NOT DEFERRABLE or of the primary key, neither identity nor generated, and no column bears the period's name. A temporary table has none, and a typed table's elements cannot write one. Each rule broken is SQLCODE -103. |
| Period constraint | §11.3 SR2e v 2 B, §11.27 GR2 | `CHECK (start < end)`, named as the table and the period with `_PERIOD` after, numbered past a name its schema has (UV100). A row that breaks it is SQLSTATE 23000. |
| ADD PERIOD | §11.27 | `ALTER TABLE t ADD PERIOD FOR name (start, end)` of a persistent base table that is not referenceable and has no period. The columns' declared types judge it, so a domain over DATE serves. The table's rows are checked by the period constraint, and the period is not added if one breaks it. |
| DROP PERIOD | §11.28 SR3d, GR2 | `ALTER TABLE t DROP PERIOD FOR name RESTRICT` or `CASCADE` drops the period and its constraint. A view, a CHECK, a domain constraint, an assertion, a routine or a trigger that references the period, one that bound before the drop and binds no more, refuses RESTRICT, SQLCODE -103 citing SR3d i to vi, and CASCADE drops it. A generation expression that references the period refuses both (SR3d vii). |
| Its columns | §11.23 SR6, §11.26 SR10 | While the period is, its columns are not dropped, its constraint is dropped only with it, and a NOT NULL or primary key whose going makes one of its columns possibly nullable is not dropped. Each is SQLCODE -103. |
| FOR PORTION OF | §14.9 SR7a, §14.14 SR7a, §15.8 GR9, §15.14 GR12 | `DELETE FROM t FOR PORTION OF p FROM x TO y` and `UPDATE t FOR PORTION OF p FROM x TO y SET ...` change the rows whose period overlaps x to y, an UPDATE's period cut to it. Each row's parts before x and after y are kept as rows of their own, of the old values, their identity column drawn and generated columns computed again. They are inserted once every row is changed, as an INSERT inserts them, its triggers executed, and the constraints judge them as the statement ends. The table has the period, an UPDATE sets neither of its columns, and x and y are datetimes comparable with them, reference no column of the table and invoke no SQL-invoked function that is not deterministic or modifies SQL-data outside a subquery. Each rule broken is SQLCODE -103. A `?` point in time is of the period's type. A routine or trigger that changes a table FOR PORTION OF a period refuses DROP PERIOD RESTRICT of it and is dropped by CASCADE (§11.28 SR3b, SR3c). |
| WITHOUT OVERLAPS | §4.25.3.2, §11.7 SR3b, SR6d i, §11.8 SR4b | `UNIQUE (c, ..., p WITHOUT OVERLAPS)` and `PRIMARY KEY (...)` refuse two rows of equal values in their columns, as their null treatment takes nulls, whose periods p overlap, SQLSTATE 23000, as the statement ends or as its deferral says. Its index is of its columns then the period's start and end, and a row is judged against the rows of its values whose period starts before its own ends. The table has the period, and a foreign key with no column list references no PRIMARY KEY WITHOUT OVERLAPS, SQLCODE -103. Its columns alone are no key, so a foreign key of them is refused too until temporal foreign keys. DROP PERIOD RESTRICT of the period is refused, and CASCADE drops the key (§11.28 SR3d ii, GR2f iii). |
| Foreign keys of a period | §4.25.3.3, §11.8 SR2, SR18a i, GR2d, GR2e | `FOREIGN KEY (c, ..., PERIOD p) REFERENCES t (c, ..., PERIOD q)` refuses a row whose period the periods of the rows of t its values match do not cover, SQLSTATE 23000, MATCH SIMPLE, FULL and PARTIAL taking nulls as of any foreign key. A row of t deleted, or its key or period changed, is refused while a row whose period overlapped its own is left uncovered. The rows of a key are read in order of their starts through the index of t's key WITHOUT OVERLAPS, stopping at a gap. Both periods are written, with the referenced columns, which are of a unique constraint WITHOUT OVERLAPS of q, each table has its period, the two of one type, and no action is CASCADE, SET NULL or SET DEFAULT. Each rule broken is SQLCODE -103. DROP PERIOD RESTRICT of either period is refused, and CASCADE drops the foreign key (§11.28 SR3d ii). |
| Its description | §4.16.1 | INFORMATION_SCHEMA.PERIODS gives the table, the period's name, its start and end columns and its constraint, of the tables the current user holds a privilege on. ISO/IEC 9075-11:2006 has no view of periods, so this one is an extension. |

PERIOD is a key word only before FOR, so a column may be named PERIOD.

Feature T180 (System-versioned tables). A table's system-time period,
SYSTEM_TIME, is given by the database, and WITH SYSTEM VERSIONING keeps
the rows it ends as historical system rows [§4.17.2.3]. They are kept in
a hidden table of their own (RULES.md D82), so the table is read, keyed
and constrained by its current rows alone.

| Feature | Rules | What turborel does |
|---|---|---|
| Definition | §11.3 SR1, SR2a, SR3, §11.4 SR6, SR14 | `PERIOD FOR SYSTEM_TIME (start, end)` of a column `GENERATED ALWAYS AS ROW START` and one `GENERATED ALWAYS AS ROW END`, each DATE or a timestamp type, NOT NULL NOT DEFERRABLE implicitly. A table has at most one system-time and one application-time period, and a temporary table none. `WITH SYSTEM VERSIONING` after the table's elements needs the system-time period, and no column is of a reference type. A ROW START or ROW END column of no such period, or a period of other columns, is refused. Each rule broken is SQLCODE -103. |
| Stamping | §15.11 GR3b, GR3c, §15.14 GR10a iii 1, §9.17 GR16a iii 1 | Each row inserted, updated or merged begins at the transaction timestamp, cast to the start column's type, and ends at the highest value of the end column's type. A row a later transaction updates or deletes is kept, of a versioned table, as a historical row ending at the transaction timestamp. One the transaction began itself is changed in place. One that begins after the transaction timestamp is SQLSTATE 2201H. |
| Given values | §14.11 SR13d, §14.12 SR18h iv, §14.15 SR3 | INSERT and MERGE give a system-time column a value only with OVERRIDING USER VALUE, which ignores it, else SQLCODE -103. DEFAULT needs none. No UPDATE or MERGE sets one, SQLCODE -103. |
| References | §11.7 SR6b, §11.8 SR10, §11.9 SR1, §11.47 SR5, §11.49 SR12, SR14c | No unique key, foreign key's referencing columns or CHECK is of a system-time column, no assertion references one of a versioned table, and no trigger's action one of its table, each SQLCODE -103. |
| ADD and DROP SYSTEM VERSIONING | §11.29, §11.30 | `ALTER TABLE t ADD SYSTEM VERSIONING` of a regular persistent base table, not referenceable, of a system-time period and no reference-type column, not versioned already. `DROP SYSTEM VERSIONING RESTRICT` or `CASCADE` of a versioned one deletes its historical rows. Under RESTRICT a view, table or domain constraint, assertion, routine or trigger reading the table FOR SYSTEM_TIME refuses it [SR3], and CASCADE drops each [GR3]. Each rule broken is SQLCODE -103. |
| ADD and DROP PERIOD | §11.27 SR4, GR1, §11.28 SR2, GR1 | `ADD PERIOD FOR SYSTEM_TIME (s, e) ADD COLUMN s ... ROW START ADD COLUMN e ... ROW END` adds both columns, each row there beginning at the transaction timestamp (IV181) and ending at the highest value. `DROP PERIOD FOR SYSTEM_TIME` of a table not versioned drops both columns, as DROP COLUMN does under the behavior given. ADD COLUMN adds no ROW START or ROW END column. |
| The versioned table kept | §11.3 SR9b i, §11.11 SR2, §11.12 SR2, SR9, §11.23 SR3, SR5, §14.10 SR3 | A versioned table's columns are not added, altered or dropped, its rows not truncated, and no LIKE copies it. A system-time column has no default and is not dropped while its period is. Each is SQLCODE -103. |
| Reading its history | §7.6 SR9c, SR9d, GR2c ii 1 | A versioned table read `FOR SYSTEM_TIME AS OF t` gives its current and historical rows whose period contains t; `BETWEEN [ASYMMETRIC \| SYMMETRIC] t1 AND t2` those current at a time from t1 to t2, both included, SYMMETRIC exchanging t1 and t2 if t1 is later; `FROM t1 TO t2` those current from t1 to before t2. Each point in time is a datetime value expression, cast to the start column's type, and a `?` is of that type. Read with none, or `AS OF CURRENT_TIMESTAMP`, it is its current rows. A table not versioned, a view, a query name or a transition table read so, or a point in time referencing a column, of the query or one it is in, or an SQL parameter, is SQLCODE -103. |
| History read only | §7.16 SR22d, §9.16 SR1s, §11.9 CR2 | A query reading a table FOR SYSTEM_TIME, but AS OF CURRENT_TIMESTAMP, is not updatable, and no constraint or assertion reads one so, SQLCODE -103. |
| Its description | §21.3.8, §4.16.1 | INFORMATION_SCHEMA.TABLES gives a versioned table's TABLE_TYPE as `SYSTEM VERSIONED`, which ISO/IEC 9075-11:2006 lacks, an extension. PERIODS gives SYSTEM_TIME and its columns, its constraint null. A history is in no view. |

Feature T502 (Period predicates) [§8.20]:

| Feature | Rules | What turborel does |
|---|---|---|
| Predicands | §8.20 SR1, SR2 | A `<period reference>` names a period of a table of the query, or of a query it is in, a qualifier choosing among tables. `PERIOD (start, end)` writes one of two values of one type, DATE or a timestamp type. After CONTAINS, a value naming no period is a point in time, DATE or a timestamp. TIME, two types, a period of two tables or an operand that is no period is SQLCODE -103. A `?` bound takes the other bound's type, a `?` point TIMESTAMP(6) WITH TIME ZONE. |
| Evaluation | §8.20 GR2 to GR4 | Every bound and point is cast to TIMESTAMP(6) WITH TIME ZONE (IL055) under the session's time zone and compared. A null bound of a written period makes the result unknown, judged before any period's order. A written period whose start is not before its end raises 22020. A period reference's null bounds, of an outer join's missing row, make it false. |
| Operators | §8.20 GR4 | OVERLAPS, EQUALS, CONTAINS of a period or a point, PRECEDES, SUCCEEDS, IMMEDIATELY PRECEDES and IMMEDIATELY SUCCEEDS. OVERLAPS of two row values stays the `<overlaps predicate>` [§8.14]. EQUALS, CONTAINS, PRECEDES, SUCCEEDS and IMMEDIATELY are no key words, so columns may bear those names. |

### Triggers

Features T200 (Trigger DDL), T211 (Basic trigger capability), T212
(Enhanced trigger capability), T213 (INSTEAD OF triggers), T214 (BEFORE
triggers), T215 (AFTER triggers), T216 (Ability to require true search
condition before trigger is invoked), T217 (TRIGGER privilege) and T218
(Multiple triggers for the same event executed in the order created).
CREATE TRIGGER defines a trigger [§11.49] and DROP TRIGGER drops one
[§11.50]. A statement's changes execute the triggers they activate
[§4.46, §15.19 to §15.21].

| Feature | Rules | What turborel does |
|---|---|---|
| Definition | §11.49 SR1 to SR9, SR15, AR1 | The name is new in its schema, and of the schema definition's in one. A BEFORE or AFTER trigger is on a persistent base table, an INSTEAD OF trigger on a view with no CHECK OPTION that no view WITH CHECK OPTION reads, one of each event, with no WHEN condition or column list. The column list's columns are the table's, once each. Each REFERENCING name once, none for the side the event has not, none the same, a transition variable only FOR EACH ROW. Each rule broken is SQLCODE -103, and a schema not owned -104. |
| The action | SR10, SR12 to SR14, IA046, IA047; ISO/IEC 9075-4:2006 draft §9.19 | Bound as the trigger is defined, as the schema's owner, so its rules and privileges are checked then. INSERT, searched UPDATE and DELETE, MERGE, SELECT INTO, GET DIAGNOSTICS, CALL, or an SQL control statement, SIGNAL or RESIGNAL of [SQL/PSM](#sqlpsm), a compound statement ATOMIC, Part 2's BEGIN ATOMIC list one, else SQLCODE -103: a schema, transaction, connection, session, dynamic or cursor statement, or a parameter, is refused as the trigger is defined. A BEFORE trigger's action changes no SQL-data and has no transition tables, and references no generated column of its new transition variable, else SQLCODE -103. |
| Privileges | AR2, AR3, §12.3, §12.7 | The schema's owner holds TRIGGER on the table, and SELECT if a transition name is given, else SQLCODE -104. TRIGGER is granted, revoked and held in ALL PRIVILEGES as the other table privileges are, and TABLE_PRIVILEGES shows it. A view's owner holds TRIGGER on it, not grantable [§11.32 GR5]. REVOKE with RESTRICT is refused where it would abandon a trigger, its TRIGGER or SELECT or a privilege its action needs, and CASCADE drops it. |
| Transition variables | §6.6 SR9, SR13, §6.4 SR7, §9.2 | OLD.C and NEW.C, where no table or correlation name of the statement is the qualifier, are the old and new row of the transition. A BEFORE row trigger's SELECT INTO and GET DIAGNOSTICS assign columns of NEW by store assignment, which the row then takes. |
| Transition tables | §7.6 SR6, §4.46.2 | OLD TABLE and NEW TABLE name, where no query name does, the old and new rows of the state change, held up to the work area and spilled past it, read in subqueries too. |
| Activation | §4.46.2 | A state change of each table, event and set of updated columns. A trigger of UPDATE OF its columns sees the transitions whose SET clauses name one of them, of every state change, the referential actions' included. A state change arises of no rows too, so a statement trigger executes for a statement that changes none. |
| BEFORE | §15.19, §15.21 GR4, §15.11 GR4, GR6a, §15.14 GR5, GR9, §15.8 GR2 | Once the rows are decided, before any is written: of an INSERT its rows given their values and identities, then computed again by their generated columns as the triggers left them; of an UPDATE its pairs of rows, the same; of a DELETE its rows. A statement trigger once for its state change, a row trigger for each transition. |
| AFTER | §15.20, §9.17 GR17b | At the end of the statement, after its referential actions and once its immediate constraints hold, over every state change it made, each trigger's transition tables whole. |
| Order | §4.46.1, §15.21 GR4, IS003, US048 | Triggers of one action time execute in the order created, their timestamps of creation never equal. A row trigger takes the transitions in the order the statement changed the rows. |
| Execution | §15.21 GR6, §9.17 GR5 | Each statement of the action runs as one of its own, with its referential actions, its immediate checks and its own triggers, as the schema's owner alone, CURRENT_USER that owner, or CURRENT_ROLE if it is a role, SESSION_USER the session's, under the triggering statement's instant, in a diagnostics area of its own. The WHEN condition is evaluated for each run, and its exception is 09000. |
| Failure | §15.21 GR6m, §23.1 GR4g, IA079 | SQLSTATE 09000, SQLCODE the cause's, the message naming the trigger and the cause's SQLSTATE and message. Nothing the statement did is kept. TRIGGER_SCHEMA and TRIGGER_NAME name the trigger, SCHEMA_NAME and TABLE_NAME its table, and MORE is Y, the cause's condition left out. |
| Referential actions | §15.17, §15.18 | The rows a referential action deletes or updates execute their table's BEFORE triggers before each change and its AFTER triggers with the statement's. |
| MERGE | §14.12 GR6a | Clause by clause, each clause's BEFORE triggers on its own rows before its changes, a statement trigger once for each event, then the AFTER triggers over all of them. |
| INSTEAD OF | §15.10, §15.13, §15.16, §11.49 GR6, §11.50 GR1 | An INSERT, searched UPDATE or searched DELETE of a view with an INSTEAD OF trigger of its event executes the trigger for the rows the view would give, before and after the SET clauses, or the rows inserted, and writes nothing through the view. No CHECK OPTION applies, and ROW_COUNT counts the transitions. The view's owner reads it, and the statement's user needs the privilege of the event. The trigger gives its schema's owner that privilege, grantable, with grantor _SYSTEM, and dropping it revokes it with CASCADE. |
| Views over trigger views | §15.12 GR2b, §15.15 GR3c, §15.10 GR3, §15.13 GR3, §15.16 GR3 | A change of an updatable view whose chain of underlying views reaches one with an INSTEAD OF trigger of its event executes that trigger, each row given as the row of the trigger's view it is, and writes nothing. The change is decided over the base table's rows when the chain reaches one. |
| MERGE | §14.12 SR2 to SR4, GR6a | Each WHEN clause whose event an INSTEAD OF trigger of the target, or of a view under an updatable target, takes gives that trigger its subject rows, before and after the SET clauses, or its candidate rows. Every clause is decided before any runs, and they run in order. A target that is not updatable is valid only when its triggers take every clause, and its rows are then those its owner reads through it. Any other clause writes as it does. ROW_COUNT counts the rows of every clause. A BEFORE trigger of a clause sees the target row as it was. |
| Positioned UPDATE and DELETE | §15.6 GR11b, GR13, §15.7 GR17b, GR12 | An UPDATE or DELETE WHERE CURRENT OF a cursor over a view whose chain reaches an INSTEAD OF trigger of its event executes the trigger for the cursor's current row, as a row of the trigger's view. A DELETE leaves the cursor before the next row, and an UPDATE leaves it on its row. |
| Defaults of a view's columns | §11.5 GR3a, GR3d, GR3e, §4.26.2, §15.12 GR2b iii 1 | A column of a view given no value by an INSERT or a MERGE whose rows an INSTEAD OF trigger takes, or given DEFAULT there or by SET, takes the default, its own or its domain's, of its single counterpart column, the base table column it is drawn from by column references alone, through projections, grouping columns, joins and views. A column of no such counterpart, as an expression, a set function or a set operation gives, is null, and so is one whose counterpart is an identity or generated column, which has no default and is left unassigned. A view over the trigger's view gives each column of the trigger's view it lacks that column's default. |
| CHECK OPTION | §11.32 SR19, §11.49 SR15d | A view WITH CHECK OPTION whose chain of underlying views holds one with an INSTEAD OF trigger is refused (SQLCODE -103), and so is an INSTEAD OF trigger on a view any such view reads down that chain. |
| Actions changing trigger views | §15.10, §15.13, §15.16, §15.21 GR6 | A triggered action may insert into, update or delete from a view that is not updatable but has an INSTEAD OF trigger of the event. It is bound against that view's rows when the trigger is defined, and the action's own transition variables and tables stay in scope within it. |
| Dropping | §11.2 SR4j, GR9, §11.23 SR7e, GR1, §11.26 SR13f, GR5, §11.31 SR7g, GR4b, §11.33 SR5c, GR3b | A trigger goes with its subject table or view. A table or view its action names, a column of its column list or of its action, and a constraint its action binds by, are dropped only with CASCADE, which drops the trigger, else SQLCODE -103. A schema holding a trigger is dropped only with CASCADE. |
| What it sees | §11.21 Note 561 | The action sees the columns its tables and its subject table had when it was defined: a column added later is not among those of its `*`, its INSERT without a column list or its transition rows. |
| Trigger descriptors described | 11:2006 draft §6.53 to §6.58 | The Definition Schema has a row of each trigger in TRIGGERS, of each column of a trigger column list in TRIGGERED_UPDATE_COLUMNS, and of each table, column, subject routine and sequence generator its action names in TRIGGER_TABLE_USAGE, TRIGGER_COLUMN_USAGE, TRIGGER_ROUTINE_USAGE and TRIGGER_SEQUENCE_USAGE. ACTION_ORDER counts from 1 among the triggers of one table, event, time and orientation in the order of creation. ACTION_TIMING is INSTEAD OF of a trigger on a view, which the draft precedes. ACTION_CONDITION and ACTION_STATEMENT are as written, null if longer than CHARACTER_DATA holds, so ACTION_STATEMENT may be null, where the draft has it NOT NULL. CREATED is null, as a trigger keeps an order of creation and no timestamp. |
| TRIGGERS, TRIGGERED_UPDATE_COLUMNS | 11:2006 draft §5.69, §5.64 | As the draft defines them: each trigger on a table, and each column of a column list, the user or an enabled role holds a privilege other than SELECT on, the condition and statement shown only where an enabled identifier owns the trigger's schema. |
| Trigger usage views | 11:2006 draft §5.65 to §5.68 | TRIGGER_TABLE_USAGE, TRIGGER_COLUMN_USAGE, TRIGGER_ROUTINE_USAGE and TRIGGER_SEQUENCE_USAGE, as the draft defines them, of the triggers of schemas an enabled identifier owns. |

The action is bound again only when the definitions change, and kept
for each level of nesting it runs at. A trigger whose action executes
it again recurses until its WHEN condition stops it or the limit of
nesting is passed (see [Limits](#limits)). The statements IA046 and
IA047 refuse are refused as Syntax Rules when the trigger is defined, so
0W000 is never raised as an action runs [§15.21 GR6e iii, GR6h]. 11000
and 0W001 are of system-time columns and data change delta tables,
which turborel has none of. TRUNCATE TABLE executes no trigger [§14.10
GR]. TRIGGER, BEFORE, AFTER, INSTEAD, REFERENCING, OLD, NEW,
ROW, EACH, STATEMENT and ATOMIC are no key words of SQL-92, so each
stays an identifier, a key word only where a trigger's statements read
it. The Information Schema's views of triggers follow the 2006 draft of
ISO/IEC 9075-11, its clause numbers cited with that edition.

### SQL-invoked routines

Features T321 (Basic SQL-invoked routines), its subfeatures 01 to 05,
T341 (Overloading of SQL-invoked functions and procedures), B033
(Untyped SQL-invoked function arguments), B128 (Routine language SQL),
S203 (Array parameters) and S204 (Array as result type of functions). CREATE FUNCTION and CREATE PROCEDURE define a
routine of an SQL routine body [§11.60], DROP ROUTINE, FUNCTION and
PROCEDURE drop one [§11.62], a function is invoked in a value expression
[§9.18, §10.4], a procedure by CALL [§16.1], and a function's body is a
RETURN [§16.2].

| Feature | Rules | What turborel does |
|---|---|---|
| Definition | §11.60 SR1 to SR9, SR13 to SR16, SR19, AR1, CR2 | The schema exists (3F000) and its user owns it (-104). Each characteristic at most once, the SQL-data access written, NO SQL refused. A function's parameters have no mode, and a procedure has no null-call clause, a function no DYNAMIC RESULT SETS. Each parameter named, no two alike, none of a domain or a row type. The body has no host, dynamic or embedded parameter. Each rule broken is SQLCODE -103. |
| Overloading | §11.60 SR20, SR22, §11.51 GR2b | A schema holds procedures of a name each of another number of parameters [SR20a], and functions of a name, its SQL-invoked functions and its distinct types' together, each told apart from the others [SR20b]. Of arguments of a function's own parameters' types, of the functions that take them, it must be the one subject routine, the values "arbitrarily selected" of SR20b ii taken to be of those types. Each exact numeric type is in NUMERIC's place of a precedence list and each approximate one in FLOAT's (RULES.md D74), so F(INTEGER) beside F(BIGINT) or F(DECIMAL(5, 2)) is refused, and beside F(DOUBLE PRECISION) is taken. CREATE TYPE checks its two functions so too. A regular function whose first parameter is of a user-defined type is told apart from each instance method of that type or a supertype of its name and schema, the others' types compatible [SR22]. Each refusal is SQLCODE -103. |
| Specific names | SR9s, SR9t, SR9u, UV102 | SPECIFIC names the routine apart from its name, of its schema, new in it. Without it, the name is the routine's name, an underscore and its creation's sequence number, with a further `_n` if that is taken. |
| The body | SR20, SR23, §16.2 SR1 to SR4, IA075, IA076, IA078 | Bound once as the routine is defined, as the owner of its schema, of that schema, so its rules and privileges are checked then and its names fixed [§4.35]. A function's body is a RETURN, of a type its result type stores, RETURN NULL a null of it. A procedure's is INSERT, searched UPDATE or DELETE, MERGE, SELECT INTO, GET DIAGNOSTICS or CALL, and no RETURN. Either may be an SQL control statement, SIGNAL or RESIGNAL, of [SQL/PSM](#sqlpsm), whose nested statements count for its SQL-data access. A schema, transaction, connection, session or dynamic statement is refused (IA078). A body of CONTAINS SQL that reads a table, or of READS SQL DATA that changes one, is refused (IA076, IA075). Each is SQLCODE -103. |
| Determinism | IA077 | DETERMINISTIC is taken as written: a body is not checked for a source of non-determinism. |
| Privileges | GR1, §12.3, §12.7 GR12o, GR12q | The owner holds EXECUTE from _SYSTEM, grantable if it holds grantable every SELECT, INSERT, UPDATE and DELETE on each table the body reads and EXECUTE on each routine it invokes. EXECUTE is granted and revoked as `<specific routine designator>` designates a routine, ALL PRIVILEGES on one being EXECUTE, and ROUTINE_PRIVILEGES shows it. A revoke that leaves the owner without a privilege the body needs, or EXECUTE on a routine it invokes, abandons the routine: RESTRICT refuses it (2B000), and CASCADE drops it and what invokes it. |
| Invocation | §9.18 SR5, SR6, SR10, §9.6, §9.7, GR3 to GR10, ID047 | Of the functions of the name in the qualifying schema, else the statement's default schema, its SQL-invoked functions and its distinct types' alike, those the statement's authorization identifier holds EXECUTE on, of as many parameters as there are arguments [SR10a], each typed argument's precedence list holding its parameter's designator [SR10d]. Left to right, those whose parameter's designator comes first in the argument's list are kept [§9.6], and one must be left [SR10h iv 3]. None of the arity is SQLCODE -103 citing SR10a, none of the types -103 citing SR10d, two alike -103, and functions that take the arguments but none with EXECUTE -104. A `?` argument decides nothing and takes its parameter's type [§20.7] (B033). A routine is found before a registered function of the name, which is searched only if no function of the schema takes the arguments [SR10h iv 1] (RULES.md D74). Each argument is stored in its parameter, then a null-call function given a null returns null without its body, else the body's value is stored in the result type. Its errors are those of store assignment, 22001 and 22003, the parameter named in the condition [§23.1 GR4q]. A body that ends without executing a RETURN raises 2F005 [GR8b ix 1]. A function that possibly modifies SQL-data is run by the session, as a statement nested in the one invoking it, its changes undone with that statement (RULES.md D76). |
| Parameters | §6.6 SR8, SR9, §6.8 | A parameter is named by its name, or by the routine's name and its name. A column of a table the body reads hides a parameter of its name. A function's body is a subquery of the row of its parameters, a VALUES of one value evaluated where it is, so a function that reads no table may be invoked where no subquery may run. |
| Where a function may be | §7.17 SR25, §11.9 SR6, §11.47 SR8, §11.4 SR10d, SR10e, §9.16 SR1am, §6.12 SR1a, SR2a | A function that possibly modifies SQL-data is invoked only where no query expression, CHECK, assertion, WHEN or search condition holds the invocation: in a SET clause of UPDATE or MERGE, a row of VALUES of INSERT or MERGE, a CALL argument, and a value of a program. Not in a CASE abbreviation, nor a simple CASE's operand. One that is not deterministic, or possibly reads SQL-data, is invoked in no CHECK, assertion or generation expression. Each is SQLCODE -103. |
| CALL | §16.1, §9.18 SR9, GR3 to GR14, §10.4 CR6, CR7 | The procedure of the name and arity, with EXECUTE, one at most by SR20a. An IN parameter's argument is a value expression it stores, a `?` of the parameter's type, an OUT or INOUT one's a target: a host parameter, a parameter of the procedure whose body the CALL is, or a column of a BEFORE row trigger's new transition variable, not an AFTER trigger's [SR9h iii 3]. NULL or DEFAULT of an IN parameter, and DEFAULT of an INOUT one, are of Features T522 and T523, SQLCODE -900. OUT parameters start null. The body runs as the schema's owner, of its schema, with a diagnostics area of its own, within the CALL statement and its instant (US037, US038). Each OUT and INOUT value is then assigned to its target [GR10b]. A BEFORE trigger's action calls no procedure that possibly modifies SQL-data [§11.49 SR14a], the procedure of the CALL's arity. |
| Subject routines kept | §4.35.2, §11.62 GR1, §12.7 GR12o | A view, CHECK, domain constraint, assertion, trigger, generated column or routine body invokes the routines it invoked as it was defined. Each records their specific names, and bound again chooses among those alone, so a function created later, or EXECUTE granted later, never changes what it invokes (RULES.md D73). One of them dropped makes it bind no more, never invoke another function of the name. |
| Dropping | §11.62, §10.6, §11.2 SR4l, GR10, §11.23 SR7c, §11.31 SR7e, §11.33 SR5d, §11.59 SR4g | DROP names the routine by its name, with its parameters' types or without, or by its specific name, of the routine type written, by the owner of its schema. A name alone of more than one routine of the type, or a list of types none or several are of, counting a distinct type's functions, is SQLCODE -103 [§10.6 SR3c]. RESTRICT refuses it if a view, constraint, assertion, generated column, trigger or routine invokes it. CASCADE drops each, and what invokes those. A table, view or column a body reads, a distinct type a routine is typed by or its body uses, and a schema holding a routine, are dropped only with CASCADE, which drops the routine. |
| Kept | §11.60 GR3, §4.35.5 | The descriptor is logged as its definition and kept in a checkpoint's snapshot, with its EXECUTE privileges. ROUTINES, PARAMETERS and ROUTINE_PRIVILEGES show it, ROUTINE_DEFINITION to the owner of its schema alone. |

SQL SECURITY (Feature T324), a parameter's default and named arguments
(T521, T522, T524, T525), a monomorphic table function's RETURNS TABLE (T326), DYNAMIC RESULT SETS
above 0 (T471), NEW SAVEPOINT LEVEL (T272) and a language other than SQL
are refused with SQLCODE -900. A routine's
behaviour is always DEFINER's, as SR23b leaves it without T324.
Routines do not recurse: a body binds before its routine exists, and a
chain of routines invoking routines is refused at 64. Procedures and
triggered actions nest at most 16 deep (see [Limits](#limits)). The
static rules leave nothing for 2F002, 2F003 and 2F004 to catch as a
routine runs [§9.18 GR6h, GR8b], so they are never raised. FUNCTION,
RETURNS, RETURN, CALL, SPECIFIC, ROUTINE, DETERMINISTIC, READS,
MODIFIES, CONTAINS, OUT and INOUT are no key words of SQL-92, so each
stays an identifier, read as a word where a routine's statements read
it.

### Polymorphic table functions

Features B200 (Polymorphic table functions), B201 (More than one PTF
generic table parameter), B202 (PTF copartitioning), B203 (More than one
copartition specification), B204 (PRUNE WHEN EMPTY), B205 (Pass-through
columns), B206 (PTF descriptor parameters), B207 (Cross products of
partitionings), B208 (PTF component procedure interface) and B209 (PTF
extended names). CREATE FUNCTION defines a polymorphic table function of
generic table and descriptor parameters, whose body names its component
procedures [§11.60], and a `<PTF derived table>` in FROM invokes it [§7.6,
§9.18]. RULES.md D78 says how.

| Feature | Rules | What turborel does |
|---|---|---|
| Definition | §11.60 SR4, SR7, SR10, AR3 | RETURNS TABLE of a column list, or of none, or ONLY PASS THROUGH. A generic table parameter is of row or set semantics, set semantics KEEP ON EMPTY or PRUNE ON EMPTY, PASS THROUGH or not. Private parameters, their defaults store-assignable to them. Each component procedure designated is one SQL-invoked procedure, of the parameter list §9.26 gives it, its modes and types alike [SR10h], of no MODIFIES SQL DATA nor more access than the function's [SR10j], describe deterministic [SR10k], each deterministic if the function is [SR10l]. A function of no column list and not ONLY PASS THROUGH has a describe component [SR10m]. The owner holds EXECUTE on each [AR3]. Each rule broken is SQLCODE -103. |
| Signatures | §9.26 | The private parameters first, INOUT, IN of the finish component. Then of each data parameter its value, of a descriptor parameter its area's name, of each generic table its full and requested row type's names to describe, its cursor row type's and its cursor's to fulfill, its partitioning and ordering descriptors' if of set semantics. Then the result row type's name, to describe only if it has no column list, and the status, INOUT CHARACTER(5). Each name VARCHAR(128) (IL058). Items viii and ix call the result name's type "of Pp", which names no parameter; it is read as VARCHAR(128) too. |
| Invocation | §7.6 SR7, SR12a, SR15a, §9.18 SR5e, SR6, SR12 | In FROM, TABLE ( a routine invocation ) and a correlation name, none of ONLY PASS THROUGH [SR7d]. A derived column list names each proper result column once. A table argument is TABLE ( name ), TABLE ( query ), or a nested invocation, up to 32 deep. Its range variable is its correlation name, else its table's name, else none (UV095), and no two are alike [SR12c]. PARTITION BY and ORDER BY are of set semantics alone, PRUNE or KEEP WHEN EMPTY of KEEP ON EMPTY alone [SR12b]. COPARTITION's tables have as many partitioning columns, each comparable [SR12d]. A descriptor argument is DESCRIPTOR ( columns ) or CAST ( NULL AS DESCRIPTOR ). A function of a generic parameter is no candidate of a value expression's invocation [SR5e]. EXECUTE on the function is needed, not on its components [SR6]. Each rule broken is SQLCODE -103, EXECUTE held not -104. |
| Compilation | §9.24, §9.18 SR12e to SR12n, IA062, IL056, IL057 | As the statement is bound. A data argument of no column reference, subquery, dynamic parameter, NEXT VALUE FOR or routine invocation is evaluated then (IA062), any other passed null to describe. The describe component runs on the PTF data area; an exception it raises, or a status other than 00000, is SQLCODE -103 [SR12i, SR12j]. The requested row types name distinct columns of their tables [SR12k], the initial result row type distinct named columns, at most 1000 (IL056, IL057). Without a describe component the requested row type is the full one. A describe component that reads SQL-data reads it in the statement's transaction, and in a definition bound where no statement runs is refused, SQLCODE -900. |
| Execution | §9.25, UA056, UV133 | As the cursor opens. An empty table of row semantics, or a pruned one, gives no row [GR4]. Of each co-partitioned set its distinct values, of each other partitioned table its own, then their cross product, one partition of nulls for an empty kept table [GR13 to GR16]. For each partition in turn, on a copy of compilation's data area, start runs, the cursors open on the partition's rows in its ordering, fulfill runs, they close, finish runs, the status checked after each [GR17 to GR20]. A table argument of row semantics is not split (UA056). A pass-through surrogate is BIGINT, its row's place in its table (UV133). |
| PTF statements | §5.4, §20.4 to §20.6, §20.13, §20.28, UV131 | In a component procedure, GET and SET DESCRIPTOR of a `<PTF descriptor name>`, COPY DESCRIPTOR to one, FETCH from a `<PTF cursor name>` into variables or a descriptor, and PIPE ROW. Each name is PTF and a value naming one of the area's (UV131), and a name of none is 33000, of a cursor 34000. PIPE ROW where no start, fulfill or finish runs is 0700H, of another than the intermediate result row type 33000, of a surrogate naming no row 0700G. GET DESCRIPTOR of SORT_DIRECTION and NULL_ORDERING reads an ordering descriptor. No other dynamic statement is in a routine's body (IA078), so DESCRIBE never names a PTF descriptor area. |
| The result | §7.6 SR12a, §20.28 GR7 to GR11 | Its proper result columns, then of each table argument of PASS THROUGH its columns, those of the row the surrogate names, null of a null one, its partitioning columns the partition's values. Of another, its partitioning columns, in the table's order. Each range variable names its own. |
| Kept | §11.60 GR3, §11.62 SR4b | The function, its components and what invokes it are logged and checkpointed as any routine. RESTRICT refuses to drop a component while a function names it, or a function a view invokes. ROUTINES shows it a FUNCTION of no data type. [EXTENSION] PARAMETERS shows a generic parameter's DATA_TYPE TABLE or DESCRIPTOR. |

### Roles

Features T331 (Basic roles) and T332 (Extended roles). CREATE ROLE
[§12.4], GRANT and REVOKE of roles [§12.5, §12.7], DROP ROLE [§12.6],
SET ROLE [§19.3], CURRENT_ROLE [§6.4], GRANTED BY of privileges and
roles [§12.8], and a role as the owner of a schema (RULES.md D75).

| Feature | Rules | What turborel does |
|---|---|---|
| Authorization cell | §4.42.1.2, §4.42.4 | A session's statements run as its user and the role it set. Their privileges are those of the user, the role, and each role applicable for the role: granted to it, to PUBLIC, or to a role found so. A definition, an abandonment and a grantor are checked with the privileges of the one identifier alone, which take in roles only if it is a role. A trigger or routine runs as its schema's owner alone, and SESSION_USER stays the session's. |
| CREATE ROLE | §12.4 SR1 to SR3, GR2, AR1, IW141 | The name is no authorization identifier known: no user's, role's, schema owner's, grantor's or grantee's, nor PUBLIC or _SYSTEM (-103). The role is granted WITH ADMIN OPTION by _SYSTEM to the current user, or with WITH ADMIN CURRENT_ROLE to the current role, one that is there (-103). The database's owner creates roles, and each user it allows by ALTER USER ... CREATE ROLE (-104). No user is created of a role's name. |
| GRANT of roles | §12.5 SR1, GR1 to GR5 | Each role exists. No grantee is the role or a role applicable for it, a cycle (-103). The grantor holds each WITH ADMIN OPTION, granted so to it or to a role applicable for it, else 0P000. A grant the same but for the admin option makes it grantable; one the same is not added. |
| REVOKE of roles | §12.7 AR1b, GR1b, GR5 to GR9, GR12q, GR12s ii | The revoker holds each role WITH ADMIN OPTION (-104). The role authorizations it granted of the roles to the grantees are destroyed, or with ADMIN OPTION FOR made not grantable. Those then reached from no independent one are abandoned. Every privilege of every kind is settled against what is left, and what needs one gone is abandoned as REVOKE of privileges finds it. RESTRICT refuses an abandoned privilege or object (2B000) and keeps an abandoned role authorization; CASCADE destroys them. No warning is raised for a role not granted, as GR12ai raises one only of privileges. |
| DROP ROLE | §12.6 AR1, GR1, GR2 | An enabled identifier holds the role WITH ADMIN OPTION (-104). The role is revoked from each grantee by any grantor, and so are the roles granted to it, the privileges granted to it, and the privileges and roles it granted, so no descriptor names it and its name is free again. RESTRICT refuses it while it granted any, or something would be abandoned (2B000). A role that owns a schema is not dropped (2B000), as no statement gives a schema another owner. |
| SET ROLE | §19.3 SR1, GR1 to GR5 | 25001 in a transaction. The value is a character string (-103), trimmed and read as an identifier. 0P000 if the cell holds no user, or no authorization of the role is granted to the user or to PUBLIC. NONE removes the role. SET SESSION AUTHORIZATION removes it too [§19.2 GR8], and takes no role's name (28000). |
| Grantor | §12.8, §12.2, §12.3 | GRANTED BY CURRENT_USER is the current user and CURRENT_ROLE the current role, each one that is there (-103). Omitted, the grantor is the current user, else the current role. A privilege a role granted stands while the role holds it grantably, through its applicable roles too [§12.7 GR2b i 2]. |
| Schemas | §11.1 SR2b, §4.42.4 | CREATE SCHEMA AUTHORIZATION names a role the session has enabled. A `<schema definition>`'s CREATE ROLE and GRANT of roles run as its owner, before its other elements. |
| Kept | §4.42.3 | Roles and role authorizations are logged with their statements, each definition's cell beside it, and kept in a checkpoint's snapshot. DEFINITION_SCHEMA.USERS lists each role. A server signs in no session as a role (28000). |
| Role authorizations described | 11:2006 draft §6.34 | DEFINITION_SCHEMA.ROLE_AUTHORIZATION_DESCRIPTORS has a row of each role authorization, PUBLIC the grantee of one granted to all and _SYSTEM the grantor of a creator's. |
| ENABLED_ROLES | 11:2006 draft §5.30, §4.42.4 | The current role, each role granted to it or to PUBLIC, and each granted to one of those in turn. The draft's text is corrected to §4.42.4 in two places. A null current role gives no row, since it enables no role. The roles granted to PUBLIC are enabled with the current role, as they are applicable for it. |
| APPLICABLE_ROLES, ADMINISTRABLE_ROLE_AUTHORIZATIONS | 11:2006 draft §5.9, §5.8 | As the draft defines them. A RECURSIVE view is written as the WITH RECURSIVE it is equivalent to [§11.32 SR9e]. |
| ROLE_ grants views | 11:2006 draft §5.38 to §5.40, §5.42, §5.43 | ROLE_TABLE_GRANTS, ROLE_TABLE_METHOD_GRANTS [§5.41], ROLE_COLUMN_GRANTS, ROLE_USAGE_GRANTS, ROLE_UDT_GRANTS and ROLE_ROUTINE_GRANTS, each privilege granted to or by an enabled role. ROLE_TABLE_GRANTS gives WITH_HIERARCHY of a SELECT. ROLE_ROUTINE_GRANTS takes the routine's name from the privilege table, which holds it, and its last subquery is closed as the draft leaves it open. |
| Other views | 11:2006 draft Clause 5 | Where a view of ISO/IEC 9075:1992 tests CURRENT_USER as a grantee, a grantor or a schema's owner, an enabled role passes the test too, as the draft's views read ENABLED_ROLES. The privilege views test grantee and grantor together before the catalog, as the draft parenthesizes them. |

The Information Schema's views of roles follow the 2006 draft of ISO/IEC
9075-11, its clause numbers cited with that edition.
ROLE, ADMIN, GRANTED and NONE are no key words of SQL-92, so each stays
an identifier, read as a word where a statement of roles reads it.
CURRENT_ROLE stays an identifier too, read as the value function where it
stands alone, so a column of that name is read qualified.

## SQL/PSM

Persistent stored modules, ISO/IEC 9075-4, written from its 2006 draft
(`5CD2-04-PSM-2006-01`), its clause numbers cited with that edition
(RULES.md D76, D77). Part 4 adds to Part 2's routines and triggers the
compound statement, local variables, conditions, cursors and handlers,
the control statements, SIGNAL and RESIGNAL, and SQL-server modules.

Features P002 (Computational completeness), P004 (Extended CASE
statement), P005 (Qualified SQL variable references), P006 (Multiple
assignment), P007 (Enhanced diagnostics management) and P008
(Comma-separated predicates in simple CASE statement) are claimed, and
P001 (Stored modules) and P003 (Information Schema views).

| Feature | Rules | What turborel does |
|---|---|---|
| Where a program stands | §4.10.7, §9.19, §11.2, §4.10.6 | An SQL control statement is a routine's body, a trigger's action, a statement of another one, or a statement run directly, alone a procedure of no parameters. A compound statement and the other control statements are not preparable, SQLCODE -103, but SIGNAL and RESIGNAL are. |
| COMMIT and ROLLBACK | §14.1 SR6, §14.13 SR9, §4.10.4; 2023 §11.60 SR23f | In an ATOMIC compound statement or a FOR, SQLCODE -103 [SR6, SR9]. In a routine's body, SQLCODE -103 (IA078). In a control statement NOT ATOMIC run directly, a non-atomic execution context, they end the transaction, its cursors closed [2023 §17.7 GR3], and the statements after run in a new one, begun as the first needs one. In a handler's action while an ATOMIC compound statement runs, an atomic execution context, SQLSTATE 2D000 [2023 §17.7 GR2, §17.8 GR2]. A statement after one that changes a definition sees it. |
| Compound statement | §14.1 SR1 to SR17, GR1 to GR5 | Its declarations in order, variables and conditions, then cursors, then handlers, its statements, and its labels, the ending one the beginning one's. No two variables, conditions, cursors or handlers' condition values alike in it. NOT ATOMIC is the default. ATOMIC, an exception no handler handles undoes it whole. UNDO handlers only in an ATOMIC one. Its variables are created, of their defaults, as it begins, its cursors destroyed as it ends. |
| Atomic statements | ISO/IEC 9075-2:2023 §4.41.5 | Each statement of a program but CALL, RETURN, a compound statement NOT ATOMIC and the control statements is atomic: one that fails is undone alone, its program's earlier changes kept for a handler. |
| Variables | §14.4, §6.2 SR3 to SR7, §6.3 | Of a data type or a domain, several to one declaration. The default is a `<default option>`, else the domain's, else null. A name is found in the innermost scope that has it: a query's own columns first, then each compound statement's variables inward to outward, then the routine's parameters. A compound statement's label, written or implicit, qualifies its variables (P005). |
| Assignment | §14.5 SR1 to SR6, GR1 | Of a variable, a parameter, a field of one, an array element as UPDATE's SET assigns it, or a BEFORE trigger's new row column. A list of targets takes the fields of a row value, or a subquery's one row, its targets null of no row and SQLSTATE 21000 of more (P006). |
| CASE and IF | §14.6, §14.7 | The first branch whose condition is true, else ELSE's. A simple CASE compares rows (P004) and lists of WHEN operands (P008). With no branch true and no ELSE, SQLSTATE 20000. |
| Iterations | §14.8 to §14.13 | LOOP, WHILE and REPEAT, with labels. LEAVE names any statement it is in, ITERATE an iteration. FOR declares and opens its cursor, named or not, its columns read-only variables qualified by its label, positioned UPDATE and DELETE of its named cursor taken. A loop checks for cancellation each iteration, so one that never ends fails with SQLCODE -920. |
| Local cursors | §12.1 to §12.4 | Declared in a compound statement, its scope, opened, fetched into variables and parameters, and closed, as Part 2's cursors are. Positioned UPDATE and DELETE of one are taken where the session runs the program. A cursor is named apart in each execution of its program, so one program run nested through triggers keeps its cursors apart. |
| Conditions and handlers | §14.2, §14.3 | A condition is declared for an SQLSTATE value or for none. A handler is CONTINUE, EXIT or UNDO, of SQLSTATE values, condition names, SQLEXCEPTION, SQLWARNING and NOT FOUND. Of a condition, the handler chosen is of the innermost compound statement with one appropriate, a specific condition value before a general one. CONTINUE goes on after the statement that raised it, EXIT leaves the compound statement, UNDO undoes its changes first. The action is in scope of the handlers outside its compound statement alone. A condition after which the transaction cannot go on, a cancellation, a conflict, class 40, is handled by none. |
| SIGNAL and RESIGNAL | §17.2, §17.3, §8.2 | SIGNAL raises its SQLSTATE value, five digits and capital letters not of class 00, or a declared condition, with the condition information items it sets. One of class 01 or 02 is a completion condition, and no handler handling it the program goes on [§17.3 GR6a iii]. An exception no handler handles fails the statement with SQLCODE -401 and its SQLSTATE, 45000 of a condition declared for none. RESIGNAL raises the condition its handler handles again, its items set anew, or another, and outside a handler is SQLSTATE 0K000. A RESIGNAL of a `<signal value>` raises its condition ahead of the one handled, with the items it sets [GR5]. The area holds one condition, so the one handled is left out and MORE is Y [GR5b]. |
| Diagnostics | §17.1, ISO/IEC 9075-2:2023 §23.2 | Each statement of a program empties the first diagnostics area and places its condition there. A handler's activation pushes the stack, so GET STACKED DIAGNOSTICS reads the condition handled and GET CURRENT, the default, its action's last statement (P007). STACKED where no handler is active is SQLSTATE 0Z002. |
| Functions | §11.2, ISO/IEC 9075-2:2023 §9.18 GR8b | A function's body may be a compound statement. One that does not possibly modify SQL-data runs where the query invoking it is evaluated, its local cursors over its queries' rows, and may CALL a procedure that changes none. One that possibly modifies SQL-data runs where the session runs its statements (D76), placed as [SQL-invoked routines](#sql-invoked-routines) says. A body that ends without RETURN raises 2F005. |
| Data access | ISO/IEC 9075-2:2023 §11.60 SR23 | The greatest of every statement in the body, nested ones and handlers' actions among them. |
| CREATE MODULE | §9.21 SR1 to SR8, GR1 to GR3, §9.24 | A module of a schema its user owns, in a `<schema definition>` of the schema defined, else SQLCODE -103 [SR1]. No second module of its name [SR2]. NAMES ARE SQL_TEXT, the one character set, another SQLCODE -900. Its routines are of its schema [§9.24 SR3], each defined as CREATE FUNCTION or CREATE PROCEDURE defines it, as the owner of the schema, and invoked by its name in the schema. Its definition is kept as written. |
| A module's routines | §9.24 GR1, ISO/IEC 9075-2:2023 §11.62 SR2 | No schema-level routines, so no DROP ROUTINE, GRANT or REVOKE names one, SQLCODE -103, and ROUTINES shows SCHEMA_LEVEL_ROUTINE NO and the module's name. |
| Names in a module | §9.21 SR4 to SR7, §8.1 SR2, SR4, ISO/IEC 9075-2:2023 §10.3 SR1 | A name of no schema other than a routine's is of the module's SCHEMA, else its own schema. A routine named of no schema is of the first schema of PATH that has one the invocation fits, PATH naming each schema once, of this catalog, else SQLCODE -103. With no PATH, the path is the module's schema alone. |
| Declared tables of a module | §12.8, ISO/IEC 9075:1992 §13.11 | Named MODULE.T within the module alone, each session's instance its own, ON COMMIT DELETE ROWS the default. In no view of the Information Schema. A module declaring one twice, SQLCODE -103. |
| EXECUTE on a module | §8.1 SR1, §10.1 to §10.3, §9.21 GR2 | What invokes a module's routine needs EXECUTE on the module, read so where §8.1 SR1 names the routine's privilege, as §10.3 GR5 abandons what invokes one at its revoke. Of a module, EXECUTE alone is granted or revoked [§10.2 SR1], else SQLCODE -103. The owner of its schema holds it of `_SYSTEM`, grantable once it holds grantably each privilege the routines need [§10.1 GR1]. A revoke abandons the views, constraints, generated columns, triggers and routines whose owners it leaves without it, RESTRICT refusing it with SQLSTATE 2B000. |
| DROP MODULE | §9.22 SR1 to SR3, GR1, GR2 | Of a module of a schema its user owns. RESTRICT refused while anything outside the module invokes a routine of it, SQLCODE -103. CASCADE drops those too. |
| What else drops a module | §9.2 GR1 to GR3, §9.25, §10.3 GR7 | A drop or revoke that takes one of its routines takes the whole module, its other routines with it. DROP SCHEMA with CASCADE drops the schema's modules, RESTRICT refused naming the first, and with either those of other schemas whose SCHEMA or PATH names it. |
| Views of modules | §18.1 to §18.4, §18.6, §18.7, §19.1 to §19.4; ISO/IEC 9075-11:2006 draft §5.47, §6.40 | MODULES, MODULE_PRIVILEGES, MODULE_TABLE_USAGE, MODULE_COLUMN_USAGE and ROLE_MODULE_GRANTS, with the draft's definitions, over the Definition Schema's MODULES, MODULE_PRIVILEGES, MODULE_TABLE_USAGE and MODULE_COLUMN_USAGE. ROUTINES gains MODULE_CATALOG, MODULE_SCHEMA and MODULE_NAME, and shows a module's routine to who holds EXECUTE on the module [§18.7], PARAMETERS following, as it reads ROUTINES. MODULE_AUTHORIZATION is null, as a server module has no `<module authorization clause>` [§19.4 Description 5]. CREATED and LAST_ALTERED are null, as no timestamp is kept. MODULES shows them as MODULE_CREATED and MODULE_LAST_ALTERED, the names its Conformance Rules give them. The usage views show a table or column as its routines' bodies bind now, not the module's own declared tables. |

The words of Part 4 that are no key words of SQL-92, IF, ELSEIF, LOOP,
WHILE, REPEAT, UNTIL, LEAVE, ITERATE, DO, HANDLER, EXIT, UNDO, SIGNAL,
RESIGNAL, SQLEXCEPTION, SQLWARNING, CONDITION and STACKED, stay
identifiers, read as words where a program reads them. Those Part 4
reserves are never a correlation name written without AS.

---

## Extensions

These facilities go beyond the standards, and the code marks them
`[EXTENSION]`.

### Data types beyond the standard

Plug-ins register these with `Database::register_type` (RULES.md D53).

| Use | Behaviour |
|---|---|
| Naming one | `name` or `name(a [, b])` in a column definition and in CAST. A domain of the name in the default schema is taken first. |
| CREATE DOMAIN | As its data type, in place of §11.21's `<data type>`. A name alone is the type, never a domain. |
| Values | CAST from a character string. Its DEFAULT is a character string literal. |
| Store assignment | Of a value comparable with it as its kind says, and a character string if its kind takes one (`SqlType::assigns_text`), in place of §9.2 SR2's rule for the standard types. |
| Function arguments | Declared of one (`ScalarFunction::argument_type`), each stored in it as a column's value is. |
| Privileges | None needed, being in no schema (§4.26). |
| Comparing | Compared, ordered, grouped, indexed and kept unique as its type's kind says, and comparable with values of its own type alone. |
| Durability | Logged, checkpointed and recovered. A database whose files hold one is not opened without it. |
| Clients | Sent as its type's name, its text and its bytes, and shown as its text by a client without the plug-in. |

### Indexes

These are RULES.md D34, of the methods `btree`, `hash` and `rtree`, or a
plug-in's. The statements run outside a transaction. The `rtree` method
indexes two approximate-numeric point coordinates or four rectangle
coordinates and answers independent SQL comparisons (`index/rtree.rs`,
`physical/access/columns.rs`, D72). Inner joins can bind these comparisons
from each outer row (`physical/indexed/coordinates.rs`). This does not add geometry types or
SQL/MM spatial routines.

```
CREATE INDEX name ON table (columns) [USING method] [WITH (...)]
DROP INDEX name
ALTER INDEX name SET (...)
```

Rust has `Database::create_index` and `drop_index`, and the views are
INFORMATION_SCHEMA.INDEXES and INDEX_OPTIONS. Index methods and other plug-ins
declare the options (`Database::declare_index_option`).

The Rust plug-in API also lets an index support a WHERE filter written as
a function call, such as `f(column, ?)` or `f(?, column)`. The registered
function takes two arguments and returns BOOLEAN or null; the other
argument is a constant or parameter. `IndexMethod::supports_predicate`
declares support, and `Probe::Predicate` carries the lookup request
(`src/index.rs`, RULES.md D34). This extension uses the existing
function-call syntax.

The function must return the same answer for the same inputs, have no
side effects, accept nulls and return null when either argument is null.
It must never raise an error for arguments its type checks accept
(`ScalarFunction::index_predicate`, `src/functions.rs`). A lookup may
return extra rows, called candidates: the executor fetches each visible
row and checks the full WHERE condition. If the function, an argument
expression or another part of the filter can raise, the table is read
whole so the lookup cannot hide the error (D15). The built-in `btree` and
`hash` methods retain their comparison lookups; a plug-in supplies the
function lookup.

### Users

Neither standard has these (RULES.md D43). They run outside a transaction, by
the database's owner.

```
CREATE USER name [WITH] PASSWORD 'literal'
ALTER USER name [WITH] PASSWORD 'literal'
ALTER USER name [NO] CREATE ROLE
DROP USER name
ALTER DATABASE OWNER TO name
```

ALTER USER ... CREATE ROLE allows the user to create roles, and NO CREATE
ROLE allows it no more (IW141). Rust has `Database::create_user`,
`set_verifier`, `allow_role_creator`, `drop_user` and `set_owner`, and
`Session::set_owner`. The view is INFORMATION_SCHEMA.USERS. A `Credentials`
plug-in makes a verifier of a password. The server's is PBKDF2-HMAC-SHA256,
keeping SCRAM-SHA-256's StoredKey and ServerKey.

### A server's connections

Neither standard has these (RULES.md D42). They run by the database's owner,
in a transaction or not. An ID of no connection served is SQLSTATE 08003.

```
END CONNECTION id      -- ends it, cancels what it runs, rolls back its session
CANCEL CONNECTION id   -- cancels what it runs
```

The ID is from INFORMATION_SCHEMA.CONNECTIONS. The server reaches them through
a `ServedConnections` it registers (`Database::set_served_connections`). CANCEL
is no key word.

### Replication

Neither standard has this (RULES.md D57). It runs outside a transaction, by
the database's owner.

```
ALTER DATABASE REPLICATION MEMBERS 'members'
```

The view is INFORMATION_SCHEMA.REPLICAS, a server's.

### Settings

These are RULES.md D36. INFORMATION_SCHEMA.SETTINGS lists every setting.

```
SET name = value                  -- a session's own value
SET name TO DEFAULT
SET DATABASE name = value         -- the database's, by its owner (D43), outside a transaction
SET DATABASE name TO DEFAULT
```

### Table settings

These are RULES.md D41. Engines and other plug-ins declare the options
(`Database::declare_table_option`), and INFORMATION_SCHEMA.TABLE_OPTIONS shows
them. [The manual](doc/appendix/g-table-options.md) lists each option.

```
CREATE TABLE ... USING engine WITH (name = value, ...)
ALTER TABLE ... SET (...)
ALTER TABLE ... USING engine WITH (...)
ALTER TABLE ... RESEAL
```

The row store takes OUT_OF_LINE and the options beginning STATISTICS. and
TRANSACTIONS. The column store, `columnstore`, takes them too, and those
beginning COLUMNSTORE., which say how a table of it freezes its rows into
compressed groups and writes them again (RULES.md D31).

The database declares ENCODINGS for every table. These are the registered
encodings its rows pass through, such as LZ4, or AES-256-GCM or
ChaCha20-Poly1305 with its key (RULES.md D48). RESEAL copies a table's rows
through those it names now, and is a non-reserved word.

### Views of the running system

| View | Shows |
|---|---|
| CONNECTIONS, SERVER, REPLICAS | A server's connections, its counts and its replicas |
| FILES, FILE_FORMATS | A database's files and their formats |
| JOBS | Its background jobs |
| MEMORY_RESERVATIONS | What holds its memory |

### Plans

`EXPLAIN <cursor specification>`, a direct statement, gives the plan of
a query as rows, one column PLAN of a line each, without running it.
The plan as bound, the rewrite rules that fired, the plan they left and
the physical plan follow in turn, as the library's `Session::explain`
gives them. EXPLAIN is no key word. It is read as the statement only
before SELECT, VALUES, TABLE, WITH or a parenthesis, so a table may be
named EXPLAIN (`db/explain.rs`).

### Plug-ins

Plug-ins cover storage engines, index methods, functions, optimizer rules,
system tables, the log, file systems, encodings, hash functions and
diagnostics. Most extension points are listed at the top of `src/db.rs`.
Encodings and hash functions are in `src/db/formats.rs`, and file systems in
`src/disk/io.rs`.

### Function call syntax

Registered functions may be called by name in sessions whose setting
SQL.FUNCTION_CALLS is ON. A name has one scalar function, or several that
each declare the types of their parameters and that no call could invoke two
of alike, chosen by the arguments' types as an SQL-invoked function is
(RULES.md D53, D74). Any other second registration of a name is refused. A
function of the default schema that takes the arguments is invoked before a
registered one. A call of a registered aggregate's name is the
aggregate's unless a function of the default schema of the name has as
many parameters as the call has arguments, as that is told before the
arguments are bound.

---

## Implementation-defined choices

These are the choices the standards leave to an implementation, grouped by
topic. The Where column names the file that implements each.

### Numbers and strings

| Rule | Choice | Where |
|---|---|---|
| 86 §5.1 SR1 character set; 86 §5.11 GR6 collating sequence | Unicode, in Unicode scalar order | `lexer.rs`, `value/comparison.rs` |
| 86 §5.3 end-of-line indicator | `\n` or `\r\n` | `lexer.rs` |
| 86 §5.5 SR3 precision of NUMERIC and DECIMAL when omitted | 18 | `value.rs` |
| 2023 §6.1 GR3 (IA227) a length in octets as characters | The same number, the most characters so many octets hold | `value/types/character.rs` |
| 86 §5.5 SR7–SR9 precision of DECIMAL, INTEGER, SMALLINT | As specified, 10, 5 | `value.rs` |
| 2023 §6.1 SR30 (IV032) the radix and precision of BIGINT | Decimal, as INTEGER's and SMALLINT's, of 18 digits, the most an exact value holds | `value.rs` |
| 86 §5.5 SR10–SR12 FLOAT, REAL, DOUBLE PRECISION | 24 bits if p ≤ 24, else 53. REAL 24, DOUBLE PRECISION 53, FLOAT with no precision 53. | `parser/data_type.rs`, `value.rs` |
| 2023 §6.9 SR10b (IV081), §7.13 SR17m iii (IV183) the type of the result of a grouping operation | Exact numeric of scale 0 and precision 18, the type BIGINT's, which holds GROUPING of up to 59 columns | `binder/grouping_sets.rs` |
| 2023 §10.9 SR11d (IA044) the type and maximum length of LISTAGG's result | CHARACTER VARYING of 32,767 characters, the longest varying character string, of CLOB arguments too | `functions/listagg.rs` |
| 2023 §9.23 SR5g, SR5h (IV082, IV083) the precision of PERCENT_RANK and CUME_DIST | 53 binary digits, DOUBLE PRECISION's | `binder/window.rs`, `exec/window.rs` |
| 2023 §9.23 SR5i ii (IV084) the type of NTILE | COUNT's, as IV166 below | `binder/window/value.rs` |
| 2023 §6.11 SR2 (IV085) the type of a `<nested row number function>` | COUNT's, as IV166 below | `binder/window/nested.rs` |
| 2023 §10.9 SR10d i (IV174) the precision of a hypothetical RANK or DENSE_RANK | COUNT's, as IV166 below | `binder/hypothetical.rs` |
| 2023 §10.9 SR10d ii (IA043) the type of a hypothetical PERCENT_RANK or CUME_DIST | Approximate numeric, DOUBLE PRECISION, as the window functions' (IV082, IV083) | `binder/hypothetical.rs` |
| 2023 §10.9 SR12e (IV177) the precision of an inverse distribution function of a number | DOUBLE PRECISION's 53 binary digits of an exact or approximate key, DECFLOAT(34)'s 34 decimal digits of a DECFLOAT key | `binder/distribution.rs` |
| 2023 §10.9 SR4 (IV166) the type of COUNT, and so of ROW_NUMBER, RANK and DENSE_RANK | Exact numeric of scale 0 and precision 18, as 86 §5.8 SR7 below | `functions.rs`, `binder/window.rs` |
| 86 §5.6 SR2 type of indicator parameters | Exact, precision 9, scale 0 | `value.rs`, `params.rs` |
| 86 §5.8 SR7, SR9b, c precision of COUNT and SUM; scale of AVG | 18. AVG of scale S has scale min(S + 2, 18). | `functions.rs` |
| 86 §5.9 SR4b, c precision of sums and products | 18 | `binder/expression.rs` |
| 86 §5.9 SR4d precision and scale of quotients | 18 digits, at the largest scale (at most 18) that fits, truncated toward zero, trailing zeros dropped down to max(s1, s2) | `value/decimal.rs`, `binder/expression.rs` |
| 86 §8.3 GR3b vii; 2023 §10.10 SR3, ID133, where nulls sort, the `<null ordering>` implied | Greater than every value, so last ascending and first descending, as NULLS LAST of ASC and NULLS FIRST of DESC | `value.rs`, `value/order.rs` |
| §6.1 SR2 the national character set | SQL_TEXT, the one there is. NATIONAL CHARACTER is CHARACTER. | `parser/data_type.rs`, `lexer.rs` |
| §6.13 SR3a VL and FL, the longest varying and fixed-length strings | 32767 characters each | `value.rs` |
| 2023 §4.4.1, IA022, whether trailing X'00' octets of a binary string are significant | They are | `value/binary.rs` |
| 2023 §8.2 GR5, IA219, a binary string beginning another, the rest X'00' | Less than it | `value/binary.rs` |
| 2023 §5.3 SR20, IV191, the type of a `<binary string literal>` | BINARY of its octets | `binder/values.rs` |
| 2023 §6.32 SR6a, IL003 and IL007, the longest fixed and varying binary strings | 32,767 octets each | `value.rs` |
| 2023 §6.1 SR6, SR7, IL007 and ID069, the maximum length of a varying string type written without one | 32,767 octets of a binary string, 32,767 characters of a character string | `parser/data_type.rs` |
| 2023 §6.33 SR16c, IL046, the maximum length of LPAD and RPAD | 32,767 characters, 4G of a CLOB | `functions/trim.rs`, `functions/lobs/pad.rs` |
| 2023 §6.33 SR12f, IL047, the maximum length of TRANSLATE_REGEX with a replacement | 32,767 characters of a character string subject, 4G of a CLOB | `functions/regex/translate.rs` |
| 2023 §6.1 SR17, IL042, IL002, IL004, the longest `<large object length>` and large object strings | 4G, 4,294,967,296 characters of a CLOB, octets of a CLOB of OCTETS or of a BLOB | `value/lob.rs`, `parser/data_type.rs` |
| 2023 §6.1 SR10, ID068, the length of a large object type written without one | 4G, the longest | `parser/data_type.rs` |
| 2023 §4.37.5, UV065, a locator's value | The next of the session's counter, from 1, never one in use | `db/lobs.rs` |
| 2023 §5.2 SR4, IV074, the `<truncating whitespace>` | The space alone | `functions/lobs/ops.rs` |
| 2023 §6.31 SR1, IV137, the type of OCCURRENCES_REGEX and POSITION_REGEX | NUMERIC(18) | `functions/regex.rs` |
| 2023 §11.72 SR6, ID157, the type of a sequence generator that names none | NUMERIC(18) | `db/sequences.rs` |
| 2023 §9.36 SR7, §9.37 SR7, ID112, the maximum of NO MAXVALUE | The type's greatest value ascending, -1 descending | `db/sequences.rs` |
| 2023 §9.36 SR8, §9.37 SR8, ID113, the minimum of NO MINVALUE | 1 ascending, the type's least value descending | `db/sequences.rs` |
| §9.1 GR3k, §9.2 GR3k a number stored or retrieved into an exact type | An exact number's extra fraction digits are truncated. An approximate number is rounded to the scale. | `value/types/exact.rs`, `value/types/approximate.rs` |
| 2023 §6.1 SR23, IA021, the greatest precision of DECFLOAT | 34 | `value.rs` |
| 2023 §6.1 SR22, ID062, the precision of DECFLOAT written without one | 34 | `value.rs`, `parser/data_type.rs` |
| 2023 §6.1 SR36, IL054, the range of DECFLOAT | Adjusted exponents from -6143 to 6144 | `value/decfloat.rs` |
| 2023 §4.5.2, IA192, how a DECFLOAT value is rounded | Half to even | `value/decfloat.rs` |
| 2023 §4.5.2, IV119, the type of an `<approximate numeric literal>` | Approximate numeric, never DECFLOAT | `binder/values.rs` |
| 2023 §6.30 SR1a, IV134, the precision of an arithmetic result with a DECFLOAT operand | 34 | `value/types/decfloat.rs`, `binder/expression.rs` |
| 2023 §10.9 SR7g v, IV170, the precision of SUM and AVG of DECFLOAT | 34 | `functions.rs` |
| 2023 §6.13 GR11c, GR12c, IV110, a DECFLOAT value as a character string | Plain if its adjusted exponent is from -6 to 33, else one digit, a point, the others, E and the exponent | `value/decfloat.rs` |
| 2023 §4.5.2, IA195, and §6.13 GR8a i, IA002, a DECFLOAT value assigned or cast to an exact type of a smaller scale | Truncated toward zero | `value/types/exact.rs` |
| 2023 §20.5 GR4c iii 8, IA235, PRECISION set with TYPE 26 | 34 | `db/dynamic/areas.rs` |

### Datetimes and intervals

| Rule | Choice | Where |
|---|---|---|
| §6.1 SR26 the greatest `<time fractional seconds precision>` | 6, microseconds | `value/datetime.rs` |
| §4.5.1, §6.1 GR5, §16.5 GR1a an SQL-session's time zone displacement, and the one it begins with | The setting SESSION.TIME_ZONE, the database's, `+00:00` by default, or the session's own, which SET TIME ZONE sets. LOCAL takes the database's again (RULES.md D54). | `settings/builtin.rs`, `db/clock.rs`, `db/time_zone.rs` |
| §10.1 SR3, SR4 the greatest `<interval leading field precision>` and `<interval fractional seconds precision>` | 9 and 6 | `value/interval.rs` |
| §6.15 GR4, GR5 the leading precision of an interval computed | Of a sum or difference, one digit more than its operands' greatest, at most 9. Of a product or quotient, 9. A result past it raises 22015. | `value/interval/arith.rs`, `binder/interval.rs` |
| §6.15 GR6b whether the difference of two datetimes is rounded or truncated | Truncated, toward 0, to the qualifier's least field | `value/interval/arith.rs` |
| §21.3.5 an interval's DATETIME_PRECISION | Its `<interval fractional seconds precision>`, of SECOND, else 0 | `value/types/interval.rs` |
| §6.8 GR3 when a statement's datetime value functions are evaluated | As it begins, from the database's clock, the system's unless a host sets another (`Database::set_clock`) | `db/clock.rs`, `clock.rs` |
| §6.6 SR4 the precision of EXTRACT's result | Of a datetime 5 digits, of an interval 9, and of SECOND the scale of its fraction more | `binder/datetime.rs` |
| §9.2 GR3l, §4.6 a datetime's fraction stored in a type of fewer digits | Truncated | `value/datetime.rs` |
| §9.50 GR4 (IA074) whether the seconds a template writes are truncated or rounded to its fractional precision | Truncated | `functions/formatted/format.rs` |
| §9.51 GR6b iv (IV096) the year RR's digits are read in | The current year, so RR is read as YY | `functions/formatted/parse.rs` |
| 2023 §8.20 GR3, IL055, the greatest timestamp precision a period predicate compares at | 6, microseconds, of TIMESTAMP WITH TIME ZONE | `binder/periods.rs` |
| 2023 §4.44 (IV072) the type of the transaction timestamp | TIMESTAMP(6) WITH TIME ZONE, the clock's instant as the transaction first changes data, at +00:00 | `db/write/system_time.rs` |
| 2023 §11.27 GR1c (IV181) the start given existing rows by ADD PERIOD FOR SYSTEM_TIME | The transaction timestamp, cast to the column's type | `db/ddl.rs` |
| 2023 §15.11 GR3c the highest value of a type | DATE 9999-12-31; a timestamp 9999-12-31 23:59:59.999999 cut to its precision, at +00:00 if zoned | `value/types/datetime.rs` |

### Cursors

| Rule | Choice | Where |
|---|---|---|
| 86 §4.12 working base or viewed table | Read-only cursors are base, created at OPEN (read as fetched where that gives the same rows, and a SCROLL one's kept as read). Updatable cursors are viewed, each row read when the cursor reaches it and kept only if it still satisfies the cursor's WHERE clause. | `db/cursor.rs`, `db/stream.rs`, `db/scroll.rs` |
| §4.21 a transaction's changes seen through a cursor not INSENSITIVE | Through a read-only one, none, as through an INSENSITIVE one. Through an updatable one, a row deleted, or changed so it no longer satisfies the cursor's WHERE clause, is passed over by every FETCH, and a row inserted is not seen. | `db/stream.rs`, `db/scroll.rs` |
| 86 §4.12 a row inserted at a cursor's position | No effect. A cursor covers only the rows its table had at OPEN. | `db/cursor.rs` |
| 86 §4.12 an error in a statement naming an open cursor | The cursor's state and position are unchanged. A FETCH failing to assign a value [86 §8.6 GR6] leaves the cursor on the row it fetched, the targets assigned before keeping their values. A failed OPEN leaves it closed. | `db/cursor.rs`, `db/assignment.rs` |
| 86 §8.6 GR5 the order targets are assigned in | The order of the `<fetch target list>`, stopping at the first that fails | `db/assignment.rs` |
| 2023 §14.10 GR2b, IA233, whether TRUNCATE TABLE's change is visible to a sensitive cursor held into a later transaction | No cursor of the transaction that reads the table may be open, so none sees it. The statement is refused, SQLSTATE 36002. | `db/truncate.rs` |
| 2023 §14.12 GR2b, IA233, whether MERGE's change is visible to a sensitive cursor held into a later transaction | As of UPDATE and DELETE. A read-only cursor that reads the table reads the rest of its rows before the change, so it sees none. | `db/merge.rs`, `db/cursor.rs` |

### Evaluation

| Rule | Choice | Where |
|---|---|---|
| §3.3.4.4, Annex C item 6 (implementation-dependent) whether the exception of an inessential part of a search condition is raised | An EXISTS reads its subquery's rows up to the first its condition keeps, and a scalar or row subquery reads two rows at most, the second failing it. So an error of a later row is not raised, by batches as row by row. Every other part of a condition is evaluated of every row its clause applies to (RULES.md D15). | `exec/eval.rs`, `exec/batch/stream.rs`, `exec/batch/joined/pairs.rs` |
| §3.3.4.4, Annex C item 5 (implementation-dependent) whether expressions are evaluated left to right | Conjuncts and rows are judged in the order the plan reads them. Where several would raise an error, which one a statement raises may differ by plan (the operand a hash join builds, the join order, an OPTIMIZER setting). Whether one is raised does not (RULES.md D15). | `physical/join.rs` |
| 2023 §7.13 SR17 (UV132) the column names and correlation name of the grouping sets' transformation | None is made. The sets are grouped in one pass, and nothing of the transformation can be named or seen. | `binder/grouping_sets.rs` |
| 2023 §10.9 GR7d v (UV062) the value ANY_VALUE gives | The first value not null it reads. So of several it may differ by plan, row by row, by batches or past the work area. | `functions/any_value.rs` |
| 2023 §6.7 GR1e (UV070) the value of a column functionally dependent on the grouping columns, of values not distinct in a group | The value of the group's first row read, so it may differ by plan, row by row, by batches or past the work area. | `functions/any_value.rs` |
| 2023 §10.9 GR11a, §10.10 the order of LISTAGG's values of sort keys alike | The order they were read | `functions/listagg.rs` |
| 2023 §7.15 GR4 (US034) the window ordering of a window of no ORDER BY | The order the rows are read, so ROW_NUMBER, FIRST_VALUE, LAST_VALUE and NTH_VALUE without an ordering may give values differently by plan | `exec/window.rs` |
| 2023 §7.15 GR4 (US035) the order of peers in a window partition | The order the window's sort leaves them in, the same for each window of that partitioning and ordering, as they are computed in one pass | `physical.rs`, `exec/window.rs` |
| 86 §6.5 GR1 whether a viewed table is materialized | No. Its query is bound into the plan of each query that names it. | `binder/views.rs` |
| §3.3.4.1 more than one condition of a statement | One is made available. That is an exception if one is raised, else no data if the statement found none, else a warning of its own rules, as of an assignment or a privilege, else the first warning an expression raised. A query's rows that can raise a condition are read as its cursor opens (RULES.md D15), so its warnings are its OPEN's, or, over the protocol, its Complete's once its rows are sent. | `error/warnings.rs`, `db/completion.rs` |

### SQLCODE

The rule is 86 §3.3, 86 §7.3 GR3b ii, and the choice is in `error.rs`.

| SQLCODE | Meaning |
|---|---|
| -101 | Lexical |
| -102 | Format |
| -103 | Syntax Rule |
| -104 | Privilege |
| -201 | Constraint |
| -301 | Data |
| -501 | Cursor |
| -401 | A condition SIGNAL or RESIGNAL raised that no handler handled [2023 §24.2] |
| -811 | Cardinality |
| -900 | Not implemented |
| -910 | API misuse |
| -911 | Conflict with another transaction |
| -912 | Transaction policy |
| -913 | Commit failed |
| -914 | Table damaged by a failed undo |
| -915 | I/O error |
| -916 | Corrupt stored data |
| -917 | Invalid transaction state |
| -918 | Memory budget spent or buffer pool full |
| -919 | Connection exception (a server's) |
| -920 | Statement cancelled |
| -921 | Dynamic SQL error |
| -922 | Commit in doubt |

### SQLSTATE and diagnostics

The SQLSTATE classes are §22.1, in `error/state.rs` and `db/completion.rs`.

| Condition | SQLSTATE |
|---|---|
| A Format, Syntax Rule or Access Rule error | Class 2A, statements being invoked directly [§20.1 GR5] |
| The same while preparing | Class 37 [§17.6 GR4a] |
| This implementation's own conditions | Class TB, the subclass the SQLCODE's digits |
| A server's conditions | Class 08, with 08004 for a connection refused and 08006 for one failed. 28000 for credentials refused. |
| A commit whose commit record could not be synced | 08007, transaction resolution unknown [§4.29], in doubt until the database is opened again |
| A conflict that undoes a statement | TB911 |
| A conflict that rolls back a commit | 40001 |
| A warning | A completion, its SQLCODE 0 [§22.2] |

| Rule | Choice | Where |
|---|---|---|
| §4.19 the size of the diagnostics area, and the order of its conditions | One condition, the statement's, successful completion too [§4.18] | `db/area.rs` |
| IA079, 2023 §9.17 GR22 whether condition areas past the first are occupied | None. Of 09000 [§15.21 GR6m], the cause's condition is left out, MORE Y, its SQLSTATE and message within the 09000's | `db/area.rs`, `db/triggers/run.rs` |
| §18.1 GR1e ROW_COUNT after other statements; Table 21's L; GR3b, GR3c an implementation's class origin; GR3k MESSAGE_TEXT | 0; the longest character string (L is never reached); TURBOREL for class TB; the error's message with its rule, empty for a completion | `db/area.rs` |
| §14.1 GR6, GR7 diagnostics area limit | A statement reports one condition, so any limit of 1 or more holds | `db/session.rs` |
| 2023 §23.1 Table 37, Table 38, IL205, the maximum length of a character string item | 32,767 characters, the longest character string varying | `db/area.rs` |
| 2023 §23.1 GR2d, GR2e, Table 39, IE005, IV190, IV212, the identifiers and codes of this implementation's statements | Each statement's first words, and a negative code of its own, -1 to -14, as the manual's page of GET DIAGNOSTICS lists them | `db/area/commands.rs` |
| 2023 §23.1 GR2e, IA156, whether a dynamic select statement prepared and not yet executed is described by its identifier and code | None is described. Only EXECUTE and the Rust API's open of a prepared statement set DYNAMIC_FUNCTION, and only as they run it | `db/execute.rs`, `db/prepared.rs` |
| 2023 §23.1 GR2g, UV121, ROW_COUNT after a statement that is no INSERT, searched UPDATE or DELETE, or MERGE | 0 | `db/area.rs` |
| 2023 §23.1 GR6a, US050, the order of the items of ALL | The order of the Format's lists of item names, statement items first, then each condition's CONDITION_NUMBER and items | `db/area.rs`, `ast/diagnostics.rs` |
| 2023 §23.1 GR6a, IL006, M, the maximum length of CHARFORM's CAST | 32,767 characters, ALL's whole string bounded so too [§6.32 GR2b ii 3 B] | `db/area/all.rs` |

### Dynamic SQL

| Rule | Choice | Where |
|---|---|---|
| §17.6 GR5b-d, s, u CHARACTER VARYING of a dynamic parameter | Of the longest length, 32767, its value CAST to it [§17.9 GR5g] | `params/dynamic.rs` |
| §17.6 GR5 a dynamic parameter as a CASE result, which GR5 gives no type | Refused, as one GR4 refuses (SQLSTATE 37000) | `binder/case.rs` |
| §17.6 GR10 a prepared statement in a later transaction | Valid. Its plan is kept between executions, and bound again against the definitions then whenever they changed (RULES.md D45). | `db/prepared.rs`, `db/prepared/plans.rs` |
| §17.2 SR2, GR3 the `<occurrences>` of an SQL descriptor area allocated without WITH MAX, the greatest, and how many areas a session holds at once | 100; 1,000, with more or fewer than 1 raising 07009 [GR3]; 64, one more refused (SQLCODE -900) | `db/dynamic/areas.rs` |
| §17.1 Table 18 the code of an implementation-defined data type | -1, of every plug-in's type (RULES.md D53). An item of it described keeps its type, which no other field gives. | `db/dynamic/descriptor.rs` |
| §17.2 GR2, §17.4 GR7c, §17.5 GR5, §17.9 GR3e, GR4e (implementation-dependent) a field not set, NAME of an unnamed column and of a dynamic parameter, the fields TYPE leaves | 0, or the empty string, DATA the null value; the empty string, UNNAMED 1; undefined | `db/dynamic/descriptor.rs`, `db/dynamic/areas.rs` |
| §17.5 GR7 restrictions on changing what DESCRIBE set | None. The fields set are those EXECUTE, OPEN and FETCH use. | `db/dynamic/areas.rs` |

### Polymorphic table functions

| Rule | Choice | Where |
|---|---|---|
| 2023 §9.18 SR12b ii (UV095) the range variable of a table argument of no correlation name nor table name | None. Its columns are named by their names alone. | `binder/ptf.rs` |
| 2023 §9.18 SR12e to SR12h, IA062, whether a value expression is evaluated during compilation | Yes, of one of no column reference, subquery, dynamic parameter, NEXT VALUE FOR or routine invocation | `binder/ptf/call.rs` |
| 2023 §9.24 GR2, IL056 and IL057, the most items of the initial and the intermediate result row types | 1,000 each | `db/dynamic/ptf.rs` |
| 2023 §9.26, IL058, the longest PTF extended name | 128 characters, VARCHAR(128) | `catalog/routines.rs` |
| 2023 §9.24 GR2 (UV131) the names of the private variables, the PTF extended names and the status variable | PTF_FULL_ROW_t, PTF_PARTITIONING_t, PTF_ORDERING_t, PTF_REQUESTED_ROW_t, PTF_CURSOR_ROW_t, PTF_CURSOR_t of the t-th generic table, PTF_DESCRIPTOR_p of the p-th parameter, PTF_INITIAL_RESULT and PTF_INTERMEDIATE_RESULT | `db/ptf/area.rs` |
| 2023 §9.25 GR10, GR11 (UV133) the names and declared type of the pass-through input surrogate columns | PTF_PASS_THROUGH_t, an underscore added until no column of the table has it; BIGINT, the row's place among its table's rows | `db/ptf/compile.rs`, `db/ptf/execute.rs` |
| 2023 §9.25 (UA056) the partitioning of a table argument of row semantics | One partition of all its rows, its virtual processors one | `db/ptf/execute.rs` |

### Sessions, users and connections

| Rule | Choice | Where |
|---|---|---|
| 86 §3.4 binding of programs to modules | `Session` and `Params` | `db.rs`, `params.rs` |
| 86 §7.1 GR2 the end of a session with a transaction open | COMMIT, or ROLLBACK, as the setting SESSION.COMMIT_AT_END says | `txn.rs`, `db.rs` |
| 86 §5.6 SR6 length of USER | The identifier's own length | `binder.rs` |
| ISO/IEC 9075-2:2023 §6.4 GR6 to GR9 CURRENT_USER, CURRENT_ROLE, SESSION_USER, SYSTEM_USER | CURRENT_USER is the top cell's user, null in a trigger or routine of a schema a role owns; CURRENT_ROLE its role. SESSION_USER and SYSTEM_USER are the session's user identifier, which SET SESSION AUTHORIZATION sets: a library runs under no operating system user of its own. | `catalog/roles.rs`, `binder/values.rs` |
| ISO/IEC 9075-2:2023 §12.4 IW141 who may create a role | The database's owner, and each user it allows by ALTER USER ... CREATE ROLE. | `db/roles.rs`, `db/users.rs` |
| §5.4; Annex B, item 41 which `<identifier>`s are valid `<authorization identifier>`s | Any, for a session the host program starts. A server's client is one of the users the database keeps (RULES.md D43), or one its authenticator accepts, the name it gives read as an `<identifier>`. | `db/users.rs`, `lexer.rs` |
| §4.30, §15.1 GR8a the authorization identifier a session begins with, and the default SQL-server | The user the client connects as, at a server. For a session the host program starts, the identifier it names. DEFAULT is the server the command line names, connected to as it starts. | `db.rs`, `cli/src/dial.rs` |
| §15.1 SR1, GR4 the implicit `<user name>`, and restrictions on one | The user the command line names. One named is proved as the command line says, a password typed asked for again, and the server accepts or refuses it (RULES.md D43). | `cli/src/dial.rs` |
| §15.1 GR8b how an `<SQL-server name>` determines the server | It is an address, 'host', 'host:port', '[IPv6 address]:port', on Unix a socket's path beginning '/', or on Windows a named pipe's name beginning '\\'. It uses TLS when the default server's connection does, trusting the same certificates. An address is no `<identifier>`, so CONNECT to one names its connection with AS [GR6]. | `cli/src/dial.rs` |
| §16.4 GR4 whether a session sets another authorization identifier | A session begun by the database's owner sets any, a user or not. Any other session sets only the one it began with (SQLSTATE 28000) (RULES.md D43). | `db/session.rs` |
| §20.1 GR2a whether direct invocation terminated unexpectedly | A server's connection that ends without its client terminating it, lost, failed or cut by the server stopping. Its transaction is rolled back. | `server/src/native.rs` |
| §14.1 SR2 default isolation level | READ COMMITTED, where the standard says SERIALIZABLE, following PostgreSQL. TRANSACTION.ISOLATION = SERIALIZABLE conforms. | `db/session.rs` |
| §14.1 GR5 isolation level | SERIALIZABLE as itself. REPEATABLE READ as snapshot isolation. READ COMMITTED and READ UNCOMMITTED with a snapshot for each statement, kept while a cursor is open. Each as SERIALIZABLE on an engine that does not read below it. | `db/session.rs`, `txn.rs` |
| §4.30 the initial default schema | The session's authorization identifier, for every statement the session binds | `db/session.rs` |
| 2023 §14.10 GR4, IA234, the changes TRUNCATE TABLE disallows that are not significant | Any other transaction's write of the table until the truncating one ends, and any use of the table by a transaction or statement that began before it committed (SQLCODE -911) | `db/truncate.rs`, `txn/locks.rs` |
| 2023 §14.12 GR4, IA234, the changes MERGE disallows that are not significant | As of UPDATE, DELETE and INSERT. The rows it changes, and under a coordinator's locks the tables it reads, are another transaction's to write only once it ends. | `db/merge.rs`, `txn/access.rs` |

### Schemas and the catalog

| Rule | Choice | Where |
|---|---|---|
| 86 §3.3, §4.6, §4.7 how schemas are created and stored | `Database::schema` and the `Catalog` trait. In memory, or kept in a directory with its catalog file and log. | `db.rs`, `catalog.rs`, `db/files.rs` |
| §5.4 SR8b, §10.6, §11.6, §11.21 who may name a constraint in a schema, which SQL-92 does not restrict | Only the schema's owner, who owns the constraint's table's or domain's schema too. Another is refused (SQLCODE -104) (RULES.md D47). | `db/constraints.rs`, `db/domains.rs` |
| §11.1 AR1 the privileges a schema definition needs | Through a session, as a server's clients run it, a schema of any name, its authorization identifier the session's, written or implicit [SR2b]. `Database::schema`, the host program's, creates any, naming its AUTHORIZATION. | `db/direct.rs`, `db/schema.rs` |
| §5.4 SR20 which `<identifier>`s are valid `<catalog name>`s | The one catalog's, the setting CATALOG.NAME's, checked as a statement is read. A definition read again keeps the names it was checked with, so if the catalog is renamed, its views and constraints read as before. | `parser.rs`, `db/schema.rs` |
| §21.2.2 the catalog's name | The setting CATALOG.NAME, `TURBOREL` by default | `settings.rs`, `system.rs` |
| §21.2.27, §21.2.28 L and ML, the lengths of SQL_IDENTIFIER and CHARACTER_DATA | CHARACTER VARYING(128) and CHARACTER VARYING(32767), the longest identifier and character string. A longer text, as a view's definition may be, is shown as null [§21.3.9 Description 3], its definition completing with a warning, 01009 of a search condition, 0100A of a query expression [§11.9 GR2, §11.19 GR13, §11.34 GR4]. This implementation's own system tables are of CHARACTER VARYING too. | `system.rs`, `system/definition.rs` |
| §21.3.23, §21.3.24 the character set SQL_TEXT | Form-of-use UTF-8, 1,112,064 characters (every Unicode scalar value), PAD SPACE | `system/definition.rs` |
| §21.3.5 how data types are described | A number's by the name it was written with, INTEGER, SMALLINT, BIGINT, DECIMAL or NUMERIC, radix 10, or REAL, DOUBLE PRECISION or FLOAT, radix 2, of the binary precision it holds values of, 24 or 53. A type computed is NUMERIC or FLOAT. The name is no part of the type, so INTEGER and NUMERIC(10,0) are one type. A character string's octet length is 4n. | `value.rs`, `value/types/exact.rs`, `value/types/approximate.rs` |
| §21.3.13 names of unnamed constraints | The table's name, the constraint's kind and a number, as `T_PRIMARY1`, `T_NOTNULL2` | `db/constraints.rs` |
| 2023 §11.3 SR2e v 2 B (UV100) the name of a period constraint | The table's name, the period's and PERIOD, joined by underscores, as `T_P_PERIOD`, an underscore and a number after if its schema has the name | `db/constraints/periods.rs` |
| ID106, ISO/IEC 9075-2:2023 §8.11 SR3, §11.7 SR5 the `<unique null treatment>` not written | NULLS DISTINCT, as every key and UNIQUE predicate was before one could be written | `ast/schema.rs` |
| IA201, ISO/IEC 9075-2:2023 §4.25.3.2 whether two rows all null are unique with nulls not distinct | Not: under NULLS NOT DISTINCT they are the same, a key refusing the second and UNIQUE false of them | `db/integrity/keys.rs`, `exec/eval.rs` |
| IE015, ISO/IEC 9075-2:2023 §4.26.3, §4.26.6, §4.26.12 known functional dependencies the rules leave to the implementation | A non-deferrable UNIQUE NULLS NOT DISTINCT determines its table's columns. A table value constructor of at most one row determines its columns. A column compared with a deterministic expression of no column has one value, and one compared with a deterministic expression of columns is determined by them (notes 68 and 69). | `binder/dependencies.rs`, `binder/dependencies/tables.rs`, `binder/dependencies/conditions.rs` |
| IA203, ISO/IEC 9075-2:2023 §4.26.6 the BPK-sets and BUC-sets of a table value constructor | None | `binder/dependencies.rs` |
| IA204, ISO/IEC 9075-2:2023 §4.26.16 the known functional dependencies, BPK-sets and BUC-sets of a recursive query | None but the axiomatic | `binder/dependencies.rs` |
| IA226, ISO/IEC 9075-2:2023 §4.26.16 known functional dependencies of a UNION | None but the axiomatic | `binder/dependencies.rs` |
| IA046, ISO/IEC 9075-2:2023 §11.49 SR13 whether a triggered action may contain a schema, transaction, connection, dynamic or session statement, and whether that is a Syntax Rule | It may not, nor a cursor statement or a parameter, refused as a Syntax Rule as the trigger is defined (SQLCODE -103) | `db/triggers/define.rs`, `parser/triggers.rs` |
| IA047, ISO/IEC 9075-2:2023 §11.49 SR14a whether a BEFORE trigger's action may contain a statement that possibly modifies SQL-data | It may not: INSERT, UPDATE, DELETE and MERGE are refused as the trigger is defined (SQLCODE -103) | `db/triggers/define.rs`, `parser/triggers.rs` |
| IS003, ISO/IEC 9075-2:2023 §4.46.1 the order of triggers of one timestamp of creation | None share one: each is one more than the greatest of the catalog's as it is defined | `db/triggers/define.rs` |
| US048, ISO/IEC 9075-2:2023 §15.21 GR4a the order a row trigger takes its transitions in | The order the statement changed the rows | `db/triggers/changes.rs`, `db/triggers/fire.rs` |
| IV223, ISO/IEC 9075-2:2023 §11.51 SR7a the source type of a distinct type of an exact or an approximate type | NUMERIC of its precision and scale, or FLOAT of its precision | `db/user_types.rs` |
| ID047, ISO/IEC 9075-2:2023 §9.18 SR10b the SQL-path | The schema of the statement's names, the session's default schema or a schema statement's own | `binder/routines.rs` |
| ID048, ISO/IEC 9075-2:2023 §4.9.6 a session's transform group of a distinct type | The one its definition makes, named FNUDT, in every session | `db/dynamic/using.rs`, `protocol/src/fields.rs` |

### Memory

The standards leave memory to the implementation (RULES.md D38). This is in
`memory.rs`, `db/files.rs`, `engine.rs`, `spill.rs` and
`db/temporary/pages.rs`.

| Setting | Default | Holds |
|---|---|---|
| MEMORY.BUDGET_BYTES | 1 GiB | The database's budget |
| STORAGE.POOL_BYTES | 128 MiB | A database's buffer pool, of its data file, or for a database in memory shared by its engine and index methods and made with its first table |
| (none) | 2 MiB | The pool of an engine or index method made in memory on its own |
| MEMORY.WORK_BYTES | 4 MiB, at least 64 KiB, a session's to set | What an operator, or a temporary table's instance (D49), holds before it spills |
| SPILL.BATCH_BYTES | 16 KiB, from 4 KiB to 1 MiB | The bytes a spilled run is written in |

A set function's accumulator holds its group's values in memory, past
the work area too. LISTAGG's holds the values that fit in the 32,767
characters its result can show, of which empty strings joined by an
empty separator may be any number, and under DISTINCT each distinct
value, to count them.
ARRAY_AGG's holds at most 65,535 elements.

A window's partition is held as its rows are read, and spills past the
work area, so a partition of any size is computed in it. A set function
over a moving frame keeps a tree of states only if the tree fits the
work area, else it reads each frame's rows from the held partition
again. The work area is shared among the window's set functions.

---

## Limits

SQL-86's §3.4 requires conforming SQL to be processed. A statement past one of
these limits is valid SQL that this implementation does not process, and gets
SQLCODE -900 (RULES.md D18).

| Limit | Value | Why | Where |
|---|---|---|---|
| BINARY and BINARY VARYING length | 32,767 octets | 2023 §6.1 SR4 requires only a length above 0. | `value.rs`, `parser/data_type.rs` |
| CHARACTER length | 32,767 characters | 86 §5.5 SR2 requires only a length above 0. Values are stored padded to their length [86 §8.7 GR7b], so without a limit `CHAR(4000000000)` could allocate about 4 GB for each value. | `value.rs`, `parser/data_type.rs` |
| Exact precision | 18 digits, for NUMERIC, DECIMAL and exact literals | 86 §5.5 SR6, SR7 and 86 §5.2 SR4 set no maximum. | `value.rs`, `parser/data_type.rs`, `parser/literal.rs` |
| FLOAT precision | 53 binary digits, and approximate literals within the range of a double | 86 §5.5 SR10 allows more precision than asked for, but sets no maximum. | `parser/data_type.rs`, `parser/literal.rs` |
| `<unsigned integer>` | 4,294,967,295 | 86 §5.2 sets no maximum. | `parser.rs` |
| A sum of SUM or AVG on the way to its total | 128 bits, past some 10^20 exact values of 18 digits, or 10^18 intervals of 9 digits of days | §6.5 GR2b iv judges only the total against the range of the result, so whether a sum raises does not depend on the order of its rows (RULES.md D15). Each sum before it is kept wider than the result, but not without bound. | `functions/sum.rs` |
| Multiset cardinality | 65,535 elements | §4.12.3 sets no maximum, and §10.9 GR3b names multiset value overflow, 2200Q, for a result past what its site holds. The limit is an array's maximum cardinality, so a multiset casts to an array of it. | `value/types/multiset.rs` |
| SQL descriptor areas | 64 at once, of a session | §17.2 GR3 leaves the number to the implementation, and names no exception past it. Direct SQL allocates them, a server's clients among it. | `db/dynamic/areas.rs` |
| Regular expression size | 8,192 instructions compiled, about as many characters and quantified repeats, and groups 64 deep | §9.28 sets no limit. A counted repeat is compiled once for each count, so `a{9000}` passes it. | `regex/compile.rs`, `regex/parse.rs` |
| Regular expression steps | 10,000,000, of a search with back-references | A back-reference matches no automaton, so its search goes back and can take time exponential in the string. One without any takes time linear in it, with no limit. | `regex/backtrack.rs` |
| Grouping sets | 4,096 of a GROUP BY, and GROUPING of 59 columns | 2023 §7.13 SR14 and SR16 set no maximum, and a CUBE of n sets stands for 2 to the n. GROUPING's value is of 18 digits [§6.9 SR10b], which 2 to the 59 fits and 2 to the 60 does not. | `binder/grouping_sets.rs`, `binder/grouping_sets/expansion.rs` |
| Trigger and procedure nesting | 16 levels of triggered actions and procedures executed within each other, past which the statement fails with SQLCODE -900, of a trigger's action SQLSTATE 09000 | §15.21 and §9.18 set no limit, and a trigger whose action executes it again would recurse until the stack overflowed. 16 levels run in half of Rust's default 2 MiB thread stack. | `db/nested.rs` |
| Nesting depth | 64 levels of the syntax tree, about 30 nested parentheses, 60 chained `AND`, `OR`, `+` or `UNION`, or 60 tables in a FROM clause | Without a limit, deep nesting overflowed the stack and aborted the process. 64 levels run in half of Rust's default 2 MiB thread stack. | `parser.rs` |

An index entry is an encoded key plus an 8-byte row id, and has its own limit.
These figures assume the default 4-byte CRC-32C checksum.

| Page size | B-link tree index | Hash index |
|---|---|---|
| 4 KiB | 983 bytes | |
| 8 KiB (default) | 2,007 bytes | 8,116 bytes |
| 64 KiB | 16,343 bytes | 65,460 bytes |

A B-link node must hold at least four entries to split, and each counts a value
of 8 bytes. A row's long values go out of line, but an index key is compared in
its node, so it stays whole. A table's rows are not limited this way.
STORAGE.PAGE_BYTES, chosen when the database is created, raises the limit. The
code is in `index/blink.rs`, `index/hash.rs` and `engine/tree.rs`.

---

## Readings of ambiguous text

Where the standards are unclear, turborel reads them as follows.

### SQL-86

| Text | Reading |
|---|---|
| Exact against approximate numbers [§5.11 GR4] | "Numbers are compared with respect to their algebraic value." An approximate number's value is the binary value it holds, so `0.10 = 0.1E0` is false, as the double nearest 0.1 is 0.1000000000000000055511151231257827…. Comparison is exact both ways. |
| `A -5` [§5.2, §5.3 SR1] | A sign written against digits is part of a literal, so `-5` could be one token and `A -5` a syntax error. It is parsed as a subtraction. `- -5` is refused [§5.9 SR2]. |
| WITH CHECK OPTION [§6.5 GR3a, §8.7 GR2, §8.11 GR5, §8.12 GR4] | SQL-92's [ISO/IEC 9075:1992 §11.19 SR13, GR11]. It is CASCADED unless LOCAL is written, and an update is checked against every view with the option that it spans, including through a view without it. SQL-86 checked only the WHERE clause of the view a statement names. |
| §7.3 SR4 | An unqualified name that is both a column and a declared parameter means the parameter. |
| A grant option given to PUBLIC [§6.6 SR3b ii] | Anyone may pass on a privilege through it, as SQL-92 has it [ISO/IEC 9075:1992 §10.3 GR2, §11.37 SR7a ii], where SQL-86 took only a grant naming the grantor itself. This holds for GRANT and REVOKE alike (RULES.md D47). |

### SQL-92

| Text | Reading |
|---|---|
| LISTAGG DISTINCT's copy kept [2023 §10.9 GR11c] | GR11c removes the redundant duplicates of values ordered by WITHIN GROUP, but does not say which copy stays. The first in that order stays, in its place, so `'X'` keyed 1 and `'x'` keyed 2 give `X` under CASE_INSENSITIVE, and `x` of ORDER BY the key DESC. |
| Binary strings brought together [2023 §9.5 SR3b] | The result of CASE, a set operation or a table value constructor of BINARY(2) and BINARY(3) is BINARY(3), but no rule extends a value of 2 octets to it. Each keeps its octets until it is assigned, where a BINARY(3) column pads it [§9.2 GR3b x]. |
| A time or timestamp without time zone [§4.5.1, §6.1 GR5] | SQL-92 keeps it in UTC, the session's displacement implied, so it is kept so, and a value retrieved shows in the displacement of the session that retrieves it (RULES.md D54). Stored in a column WITH TIME ZONE, it takes that displacement. A datetime literal in a `<default clause>` without one is of UTC [§5.3 SR18], as a definition is read again where no session is. |
| UNICODE_<LOCALE> and CLDR [UTS #35 Part 5 §3.13.2] | The rules tailor UNICODE, not CLDR's root collation, which orders 105 of its 24,412 characters otherwise. These are currency symbols, length and repetition marks, and a few Cyrillic letters. The script groups `[reorder]` moves are laid over UNICODE's primaries from CLDR's own, as UTS #35 describes for allkeys.txt. So the digits of scripts, which UNICODE puts before the currency symbols, are in the group of digits but stay where UNICODE has them. |
| PAD SPACE under UNICODE [2023 §4.3.2, §11.43] | A space weighs only at the fourth level, so trailing spaces are taken away first. A character below a space then orders as past the string's end at that level, where padding would order it before. Only `CREATE COLLATION ... FROM UNICODE PAD SPACE` reaches this, as UNICODE itself is NO PAD. |
| One datetime type [§9.3 SR3e, §11.5 SR3c] | "The same datetime type" is read as of the same fields, WITH TIME ZONE or not, as store assignment takes either [§9.2 SR2]. The result's type of CASE, a set operation or VALUES is WITH TIME ZONE if one is, of the greatest precision, and each value keeps its own form until it is assigned. DEFAULT CURRENT_TIME is taken by a TIME column. |
| A datetime's or an interval's literal in CAST [§6.10 GR5e, GR6e, GR9a, GR12b] | "The shortest character string that conforms to the definition of `<literal>`" is read as the literal's string, the part between its quotes, an interval's sign before it. So CAST(DATE '2024-01-02' AS CHAR(10)) is '2024-01-02', and CAST from a string takes that string. An interval's fields after the leading one are of any digits, `<datetime value>` being an `<unsigned integer>` [§5.3]. SR21's two digits are of a datetime literal's. |
| AT TIME ZONE [§6.14 GR4] | The value is "DV - TZ", DV the datetime in UTC. It is read as the same instant shown in the displacement TZ, of the type WITH TIME ZONE, as a datetime WITH TIME ZONE keeps its own [§4.5.1]. |
| DATA of a datetime or an interval [§17.1 SR2] | The rule says when a value's or a target's type matches an item descriptor area's, but lists no datetime or interval type, so none would match. One matches the type the item describes, as a plug-in's type does. |
| An invalid SQL statement identifier [§17.6 GR8] | Table 23 has no such condition. It is read as invalid SQL statement name, 26000, the condition of a statement's name. |
| A `<statement name>` and a LOCAL extended statement name [§5.4 GR12, GR13] | Both are of the module, here the session, but the one is an `<identifier>` and the other a value. They are names apart, as a `<cursor name>` and an extended cursor name are [§17.6 GR2a]. |
| OVERLAPS with an interval [§8.11 SR3a] | "Such that the interval can be added to the datetime" is read as §6.14 SR3 says of addition, that the interval's fields are the datetime's. |
| Which collation a session's statement is prepared under [2023 §9.15 SR4c] | Its Syntax Rules apply as it is prepared, so it keeps that collation at every execution and rebind, unlike the time zone (RULES.md D45). |
| A `?` beside a TIME [§17.6 GR5f ii, §6.14 SR3] | GR5f gives the `?` INTERVAL DAY TO SECOND, though §6.14 SR3 lets a time take no day field. The type is kept as GR5f says, and the days do not move a time of day. |
| The default connection's names [§18.1 GR3n ii, §12.3 GR6c i] | A session the host program starts effectively ran CONNECT TO DEFAULT, whose text gives no name of a connection or server. CONNECTION_NAME and SERVER_NAME are each read as `DEFAULT`, the word that stands for them. |
| ALL CONDITION of a number no condition has [2023 §23.1 GR3, GR6a iii] | GR3's exception is written of a `<condition information>`, and GR6a iii reads condition area CN as GR3 does. A number of no condition is read as GR3 reads it, SQLSTATE 35000. |
| The items of ALL [2023 §23.1 GR6a ii 2, GR6a iii] | "CIN is the `<condition number>` of CII" is read as the `<condition information item name>` of CII, as SIN is a statement item's name. CONDITION_NUMBER is presented once for each condition, before its items and not among them, and ALL CONDITION with a number presents none, as GR6a iii names no COND. |
| A key WITHOUT OVERLAPS of a period dropped [2023 §11.28 SR3d ii, GR2f iii] | The search condition of a key WITHOUT OVERLAPS, §11.7 SR6d i, writes the period's start and end columns, not the period's name, and the descriptor names the period (GR2c). It is read as one that references the period, so DROP PERIOD RESTRICT is refused while it is, and CASCADE drops it, as GR2f iii drops a constraint. |
| RESTRICT of a foreign key of a period [2023 §15.18 GR6, GR9a iv, GR10a iv] | The matching rows of GR6 are those whose referencing values equal the referenced row's, and nothing there names a period. So RESTRICT refuses a referenced row deleted, or its key changed, while any row of its values is there, whatever their periods, and a change to its period alone is no update of a referenced column, which NO ACTION's check as the statement ends judges. |
| A referenced period alone [2023 §11.8 SR2a] | SR2a asks a `<referenced period specification>` of a foreign key that writes a `<referencing period specification>`, and says nothing of the one written alone, which SR18a then never reads. It is refused, SQLCODE -103, as a period that would judge nothing. |
| The rows FOR PORTION OF keeps [2023 §15.8 GR9c ii, §15.14 GR12c ii] | "The following `<insert statement>` is effectively executed without further Access Rule and constraint checking." An INSERT executed has its triggers executed [§15.11], so the rows kept execute the table's INSERT triggers, in the state changes of the DELETE or UPDATE. Constraints judge them as the statement ends, as NOTE 707 and NOTE 714 say. |
| A generation expression referencing a period dropped with CASCADE [2023 §11.28 SR3d vii, GR2f] | SR3d vii forbids the reference under RESTRICT, but GR2f names nothing to drop for it under CASCADE, and dropping the period would leave the column's expression unbound. The drop is refused under CASCADE too, until the column is dropped. |
| TRANSACTION_ACTIVE [2023 §23.1 GR2j] | "Currently active" is read as GET DIAGNOSTICS runs, which begins no transaction, so it tells whether the statement before left one active. |
| The routine a condition is raised in [2023 §23.1 GR4l, GR4q] | The only SQL-invoked routines are the functions a distinct type's definition makes, which take a value of their parameter's type and give it back as their result's [§11.51 GR2b]. A condition raised as one is invoked is read as one of the CAST it stands for, so no condition names a routine, and ROUTINE_CATALOG, ROUTINE_SCHEMA, ROUTINE_NAME, SPECIFIC_NAME, PARAMETER_MODE and PARAMETER_NAME are zero-length strings, PARAMETER_ORDINAL_POSITION 0. |
