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
[Extensions](#extensions) ·
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
| Constraints | §11.4, §11.6–§11.9, §10.6 | NOT NULL, UNIQUE (whose columns may be null), PRIMARY KEY, FOREIGN KEY with MATCH FULL, PARTIAL or neither and every referential action, and CHECK, with subqueries too. Each is named, by the definition or the implementation. |
| A constraint's schema | §5.4 SR8 | A name written without a schema is of the `<schema definition>`'s schema, else of the session's default schema, which may differ from its table's or domain's (RULES.md D47). A declared local temporary table's is of MODULE. |
| When constraints are checked | §14.2 | At the end of each statement. Deferred ones, by SET CONSTRAINTS or INITIALLY DEFERRED, at commit. |
| Defaults | §11.5 | A literal, NULL, USER, CURRENT_USER, SESSION_USER, SYSTEM_USER, or a datetime value function. Used by INSERT, DEFAULT VALUES, `SET ... = DEFAULT`, columns added, and the referential action SET DEFAULT. |
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
| `<updatability clause>` | FOR READ ONLY or FOR UPDATE [OF columns]. FOR UPDATE with ORDER BY or SCROLL too. |
| Positioned UPDATE | Sets only the columns it names, and none its ORDER BY reads [§13.9 SR4, SR8]. |

### Transactions, sessions and the Information Schema

| Feature | Rules | What turborel does |
|---|---|---|
| SET TRANSACTION | §14.1, §4.28 | READ ONLY or READ WRITE, and the isolation level, for the next transaction. See the table below. |
| SET CATALOG | §16.1, §5.4 SR20 | Of the one catalog's name. Another is no valid `<catalog name>`, refused (SQLSTATE 3D000) [GR2]. |
| SET SCHEMA | §16.2 | The default schema of unqualified names. |
| SET SESSION AUTHORIZATION | §16.4, §4.30 | The session's authorization identifier, the value trimmed and read as an `<authorization identifier>`. Refused in a transaction. The default schema stays as it was. |
| Connections | §15 | CONNECT TO a server AS a name, USER a user, or TO DEFAULT. SET CONNECTION to a name or DEFAULT. DISCONNECT a name, DEFAULT, CURRENT or ALL. The client library is the SQL-agent (`turborel_client::agent`), and so the command-line client is. A server or a session refuses them (SQLCODE -900) (RULES.md D46). No transaction spans servers (SQLSTATE 0A001). |
| COMMIT and ROLLBACK | §14.3, §14.4 | Without WORK too. |
| The Information Schema | §21 | Its 23 views, with the standard's own definitions, over the Definition Schema (RULES.md D37). |

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

T285 (Enhanced derived column names) is not claimed. It names a column
of a single SQL parameter reference for the parameter [§7.16 SR17c,
CR6], and turborel has no SQL parameters. A column of any other
expression without an `<as clause>` is unnamed, the name SR17d leaves
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

Feature T021 (BINARY and VARBINARY data types), with Features T023
(compound binary literals) and T024 (spaces in binary literals), and of
Feature T022 SUBSTRING on binary strings.

| Feature | Rules | What turborel does |
|---|---|---|
| BINARY(n), BINARY VARYING(n), VARBINARY(n) | §4.4, §6.1 SR1, SR4, SR5, SR19-SR21 | Strings of octets. BINARY is BINARY(1), and VARBINARY is BINARY VARYING. A maximum length not written is the longest, 32,767 octets [SR6] (IL007), as Feature T081 allows. |
| `<binary string literal>` | §5.3 SR4-SR7, SR20 | X, in either case, then quoted pairs of hexits, spaces among them passed over. Parts separated by a newline are one literal. An odd number of hexits, or a character neither a hexit nor a space, is a lexical error (SQLCODE -101). Its type is BINARY of its octets (IV191). |
| Assignment | §9.2 GR3b viii-xii | BINARY(n) takes n octets, a shorter value padded with X'00', a longer one cut if the octets past n are all X'00', else 22001. BINARY VARYING(n) takes at most n, cut the same way. Only a binary string is assigned to one [SR3]. |
| Retrieved into a parameter | §9.1 GR5a, GR7h-GR7l | Cut to fit with 01004, the indicator its length, or a BINARY one padded with X'00' octets. |
| Comparing, indexing, uniqueness | §8.2 GR5 | Octet by octet, a string less than a longer one it begins (IA219), so trailing X'00' octets count (IA022). A key escapes each X'00' octet, so no key begins another. |
| `\|\|` | §6.32 SR6a, GR3 | BINARY if both operands are, else BINARY VARYING. A result past 32,767 octets keeps its first ones if the rest are X'00', else 22001. |
| SUBSTRING | §6.33 SR21, GR17 | Of BINARY VARYING, its octets. |
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
named BINARY or VARBINARY. TRIM and OVERLAY on binary strings, the rest
of Feature T022, and the binary large objects of Feature T045 are not
implemented.

A collation's sort keys are binary strings, and an index on a collated
column keys them as BINARY VARYING (RULES.md D60).

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
cutting it or adding the padding before or after it, repeated and cut,
a space if none is written [GR14, SR16a]. A total length is an exact
number of scale 0 [SR16b], else SQLCODE -103. A pad is CHARACTER
VARYING of 32,767 characters [SR16c] (IL046), and a trim varying, as
long as its string at most [SR15a]. A trim compares characters under
the collation of its string, TRIM of one character too, and a trim or
pad is of that collation [SR14d, SR15c, SR16e], whatever that of its
other strings. A dynamic parameter as a total length is NUMERIC(P, 0),
P the greatest precision, as SUBSTRING's length is [92 §17.6 GR5c],
which §20 leaves unsaid; this is marked [EXTENSION]. The five names are
no key words of SQL-92, so each stays an identifier, a function before
a parenthesis.

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
| Drop | §11.74 | Nothing can depend on a generator, so RESTRICT and CASCADE are alike. USAGE on it goes with it. |
| Placement | §6.14 SR1, SR2 | In a select list of a cursor's query, a SELECT INTO's and an INSERT's, in an INSERT's VALUES and an UPDATE's SET; elsewhere SQLCODE -103. ORDER BY and a default take no value expression, so there it is a syntax error. |
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
| Assignment and CAST | §9.2, §6.13 | Element by element and field by field, an array of more non-null elements than its target's maximum SQLSTATE 2202F. Nothing else is assigned or cast to or from an array or a row. |
| `SET A[i] = v` | §14.15 GR10b | Past the cardinality the array is extended with nulls; index 0 or past the maximum is SQLSTATE 2202E, a null array SQLSTATE 2200E. |
| UNNEST | §7.6 SR8, §7.10 | `UNNEST(a, ...) [WITH ORDINALITY] AS t [(names)]` of the rows of its arrays' elements, padded with nulls to the longest, each array of rows spread into its fields. The ordinality column is INTEGER (IV158). It reads the FROM items before it, after a comma or as the second operand of CROSS, INNER and LEFT joins; as that of a RIGHT or FULL join it may read none of the first (SQLCODE -103). |
| Dynamic SQL | §20.1, §20.9 to §20.12 | An item of a composite type is followed by items of its parts, of LEVEL one greater, with DEGREE, CARDINALITY and RETURNED_CARDINALITY. TOP_LEVEL_COUNT counts the items of LEVEL 0. DESCRIBE takes WITH NESTING and WITHOUT NESTING. DATA of an item of a row or of an array's element is SQLSTATE 0700C to get and 0700D to set; a LEVEL set out of order SQLSTATE 0700E. |
| Its description | ISO/IEC 9075-11 | COLUMNS and DOMAINS give MAXIMUM_CARDINALITY and DTD_IDENTIFIER, and ELEMENT_TYPES and FIELDS describe the parts. A column's DTD_IDENTIFIER is its ordinal position, a domain's 1, and a part's its parent's, a point, and its field's place, or 1 for an element. |

An array's elements and a row's fields are kept in the row of their
table, and an array of a row type is unnested into the fields (RULES.md
D68). A collection derived table in a RIGHT or FULL join that reads a
table reference outside the join, or nested in a LEFT join's second
operand and reading its first, is valid SQL not supported (SQLCODE -900).
S091 (Basic array support) is claimed with S093 (Arrays of distinct
types), which it implies, and with it S095 (Array constructors by query),
S096 (Optional array bounds) and S098 (ARRAY_AGG), each processed as the
table says. S097 (Array element assignment) is not claimed. Its target's
array is an SQL parameter or a new transition variable's column [§6.4
SR7], and turborel has neither SQL-invoked routines with parameters nor
triggers. ARRAY, ROW, UNNEST and the functions are key words only where
they are read (Appendix B).

### Distinct types

Features S011 (Distinct data types), S028 (Permutable UDT options list)
and S401 (Distinct types based on array types). CREATE TYPE defines a
distinct type of a predefined type or an array type [§11.51], and DROP
TYPE drops one [§11.59]. Its values are its source type's, and compare
only with values of the same type [§4.9.5].

| Feature | Rules | What turborel does |
|---|---|---|
| Definition | §11.51 SR1 to SR7 | `CREATE TYPE n AS <predefined type> \| <array type> [FINAL]` and the two CAST options, in any order, each at most once. The schema is there (3F000) and is its schema definition's, and no type or domain of the schema has the name, else SQLCODE -103. A row type, another distinct type or a type of an extension as its source is SQLCODE -103, as is each option of a structured type. A structured type is SQLCODE -900. |
| Source type | §11.51 SR7a | An exact type is kept as NUMERIC of its precision and scale and an approximate one as FLOAT of its precision (IV223), so INTEGER is NUMERIC(10, 0). |
| Comparison | §4.9.5, §8.2, §11.51 GR2c | Of values of one distinct type alone, as their source values compare. Ordered, grouped, made DISTINCT, kept unique and indexed so, unless the source is an array type, whose values compare with nothing. |
| Assignment and CAST | §4.13, §9.2 SR3, §6.13 SR12 | To a distinct type, of a value of the type, or of a type whose type precedence list [§9.7] holds the source's designator and that is assignable to the source. From one, to exactly its source type, of the same precision and scale. Any other is SQLCODE -103. |
| Operators | §6.30 SR3 and others | None but comparison. Arithmetic, a set function but COUNT, MAX and MIN, a string function, a predicate of strings, a Boolean operator, an element reference and CARDINALITY take no value of a distinct type, SQLCODE -103. |
| Result type | §9.5 SR3l | Values of one distinct type are brought together with values of that type alone, in CASE, COALESCE, a set operation and a VALUES of more than one row. |
| Functions | §11.51 GR2b, §9.18, §10.4 | FNUDT, named by the CAST (SOURCE AS DISTINCT) option or the type's name, and FNSDT, named by the CAST (DISTINCT AS SOURCE) option or the source type's name [§9.9], are invoked by name, a name not qualified found in the schema of the statement's names (ID047). Of the functions of one name, the argument's type chooses. None that takes it, or two alike, is SQLCODE -103, and a ? argument of a name of two functions is SQLCODE -103. A function of the name another function of the schema has, of the same type designator, is refused [§11.60 SR20b]. Their specific names are the type's name and _TO_DISTINCT or _TO_SOURCE. |
| Privileges | §6.1 AR1, §6.13 AR2, §9.18 SR6, §12.2, §12.3, §12.7 | Naming the type needs USAGE on it, and a CAST to or from it, or invoking a function, EXECUTE on the function, each SQLCODE -104 if not held. The schema's owner holds them grantably. GRANT and REVOKE take USAGE ON TYPE and EXECUTE ON SPECIFIC FUNCTION n or FUNCTION n [(types)]. REVOKE with RESTRICT is 2B000 where it would abandon a column of a type based on the type, a view, constraint, assertion or generated column, or a grant. With CASCADE each goes, a column as ALTER TABLE DROP COLUMN drops it. UNDER is SQLCODE -103 of a distinct type. |
| Drop | §11.59 | RESTRICT refuses while a column, or another type's source, is based on the type, or a view, constraint, assertion or generated column references it or invokes its functions. CASCADE drops each, the types based on it among them, and is refused before it drops any if a table would lose every column [§11.23 SR4]. DROP SCHEMA CASCADE drops its types. |
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
GR2], so only one a CREATE TRANSFORM made counts, and turborel has none. A CASCADE that would drop every column of a table is
refused before it drops any [§11.23 SR4]. TYPE, FINAL, INSTANTIABLE,
UNDER, SOURCE, REF, SPECIFIC and FUNCTION are no key words of SQL-92, so
each stays an identifier, a key word only where CREATE TYPE, DROP TYPE,
GRANT and REVOKE read it. A function named by a key word, as FNSDT of a
NUMERIC source is, is invoked delimited, `"NUMERIC"(p)`. Structured types,
SET TRANSFORM GROUP and SQL-invoked routines a user defines are not
implemented.

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

These are RULES.md D34, of the methods `btree` and `hash` or a plug-in's. The
statements run outside a transaction.

```
CREATE INDEX name ON table (columns) [USING method] [WITH (...)]
DROP INDEX name
ALTER INDEX name SET (...)
```

Rust has `Database::create_index` and `drop_index`, and the views are
INFORMATION_SCHEMA.INDEXES and INDEX_OPTIONS. Index methods and other plug-ins
declare the options (`Database::declare_index_option`).

### Users

Neither standard has these (RULES.md D43). They run outside a transaction, by
the database's owner.

```
CREATE USER name [WITH] PASSWORD 'literal'
ALTER USER name [WITH] PASSWORD 'literal'
DROP USER name
ALTER DATABASE OWNER TO name
```

Rust has `Database::create_user`, `set_verifier`, `drop_user` and `set_owner`,
and `Session::set_owner`. The view is INFORMATION_SCHEMA.USERS. A `Credentials`
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

### Plug-ins

Plug-ins cover storage engines, index methods, functions, optimizer rules,
system tables, the log, file systems, encodings, hash functions and
diagnostics. Most extension points are listed at the top of `src/db.rs`.
Encodings and hash functions are in `src/db/formats.rs`, and file systems in
`src/disk/io.rs`.

### Function call syntax

Registered functions may be called by name in sessions whose setting
SQL.FUNCTION_CALLS is ON. Each name has one scalar function, none chosen by its
arguments' types, and a second registration of a name is refused (RULES.md
D53).

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
| 86 §5.6 SR2 type of indicator parameters | Exact, precision 9, scale 0 | `value.rs`, `params.rs` |
| 86 §5.8 SR7, SR9b, c precision of COUNT and SUM; scale of AVG | 18. AVG of scale S has scale min(S + 2, 18). | `functions.rs` |
| 86 §5.9 SR4b, c precision of sums and products | 18 | `binder/expression.rs` |
| 86 §5.9 SR4d precision and scale of quotients | 18 digits, at the largest scale (at most 18) that fits, truncated toward zero, trailing zeros dropped down to max(s1, s2) | `value/decimal.rs`, `binder/expression.rs` |
| 86 §8.3 GR3b vii where nulls sort | Greater than every value, so last ascending and first descending | `value.rs` |
| §6.1 SR2 the national character set | SQL_TEXT, the one there is. NATIONAL CHARACTER is CHARACTER. | `parser/data_type.rs`, `lexer.rs` |
| §6.13 SR3a VL and FL, the longest varying and fixed-length strings | 32767 characters each | `value.rs` |
| 2023 §4.4.1, IA022, whether trailing X'00' octets of a binary string are significant | They are | `value/binary.rs` |
| 2023 §8.2 GR5, IA219, a binary string beginning another, the rest X'00' | Less than it | `value/binary.rs` |
| 2023 §5.3 SR20, IV191, the type of a `<binary string literal>` | BINARY of its octets | `binder/values.rs` |
| 2023 §6.32 SR6a, IL003 and IL007, the longest fixed and varying binary strings | 32,767 octets each | `value.rs` |
| 2023 §6.1 SR6, SR7, IL007 and ID069, the maximum length of a varying string type written without one | 32,767 octets of a binary string, 32,767 characters of a character string | `parser/data_type.rs` |
| 2023 §6.33 SR16c, IL046, the maximum length of LPAD and RPAD | 32,767 characters | `functions/trim.rs` |
| 2023 §6.33 SR12f, IL047, the maximum length of TRANSLATE_REGEX with a replacement | 32,767 characters | `functions/regex/translate.rs` |
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

### Cursors

| Rule | Choice | Where |
|---|---|---|
| 86 §4.12 working base or viewed table | Read-only cursors are base, created at OPEN (read as fetched where that gives the same rows, and a SCROLL one's kept as read). Updatable cursors are viewed, each row read when the cursor reaches it and kept only if it still satisfies the cursor's WHERE clause. | `db/cursor.rs`, `db/stream.rs`, `db/scroll.rs` |
| §4.21 a transaction's changes seen through a cursor not INSENSITIVE | Through a read-only one, none, as through an INSENSITIVE one. Through an updatable one, a row deleted, or changed so it no longer satisfies the cursor's WHERE clause, is passed over by every FETCH, and a row inserted is not seen. | `db/stream.rs`, `db/scroll.rs` |
| 86 §4.12 a row inserted at a cursor's position | No effect. A cursor covers only the rows its table had at OPEN. | `db/cursor.rs` |
| 86 §4.12 an error in a statement naming an open cursor | The cursor's state and position are unchanged. A FETCH failing to assign a value [86 §8.6 GR6] leaves the cursor on the row it fetched, the targets assigned before keeping their values. A failed OPEN leaves it closed. | `db/cursor.rs`, `db/assignment.rs` |
| 86 §8.6 GR5 the order targets are assigned in | The order of the `<fetch target list>`, stopping at the first that fails | `db/assignment.rs` |
| 2023 §14.10 GR2b, IA233, whether TRUNCATE TABLE's change is visible to a sensitive cursor held into a later transaction | No cursor of the transaction that reads the table may be open, so none sees it. The statement is refused, SQLSTATE 36002. | `db/truncate.rs` |

### Evaluation

| Rule | Choice | Where |
|---|---|---|
| §3.3.4.4, Annex C item 6 (implementation-dependent) whether the exception of an inessential part of a search condition is raised | An EXISTS reads its subquery's rows up to the first its condition keeps, and a scalar or row subquery reads two rows at most, the second failing it. So an error of a later row is not raised, by batches as row by row. Every other part of a condition is evaluated of every row its clause applies to (RULES.md D15). | `exec/eval.rs`, `exec/batch/stream.rs`, `exec/batch/joined/pairs.rs` |
| §3.3.4.4, Annex C item 5 (implementation-dependent) whether expressions are evaluated left to right | Conjuncts and rows are judged in the order the plan reads them. Where several would raise an error, which one a statement raises may differ by plan (the operand a hash join builds, the join order, an OPTIMIZER setting). Whether one is raised does not (RULES.md D15). | `physical/join.rs` |
| 2023 §7.13 SR17 (UV132) the column names and correlation name of the grouping sets' transformation | None is made. The sets are grouped in one pass, and nothing of the transformation can be named or seen. | `binder/grouping_sets.rs` |
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
| §18.1 GR1e ROW_COUNT after other statements; Table 21's L; GR3b, GR3c an implementation's class origin; GR3k MESSAGE_TEXT | 0; the longest character string (L is never reached); TURBOREL for class TB; the error's message with its rule, empty for a completion | `db/area.rs` |
| §14.1 GR6, GR7 diagnostics area limit | A statement reports one condition, so any limit of 1 or more holds | `db/session.rs` |

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

### Sessions, users and connections

| Rule | Choice | Where |
|---|---|---|
| 86 §3.4 binding of programs to modules | `Session` and `Params` | `db.rs`, `params.rs` |
| 86 §7.1 GR2 the end of a session with a transaction open | COMMIT, or ROLLBACK, as the setting SESSION.COMMIT_AT_END says | `txn.rs`, `db.rs` |
| 86 §5.6 SR6 length of USER | The identifier's own length | `binder.rs` |
| §6.2 SR4, GR10, GR11 CURRENT_USER, SESSION_USER, SYSTEM_USER | Each is the session's authorization identifier. A session has no other, and a library runs under no operating system user of its own. | `binder.rs`, `catalog.rs` |
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
| ID106, ISO/IEC 9075-2:2023 §8.11 SR3, §11.7 SR5 the `<unique null treatment>` not written | NULLS DISTINCT, as every key and UNIQUE predicate was before one could be written | `ast/schema.rs` |
| IA201, ISO/IEC 9075-2:2023 §4.25.3.2 whether two rows all null are unique with nulls not distinct | Not: under NULLS NOT DISTINCT they are the same, a key refusing the second and UNIQUE false of them | `db/integrity/keys.rs`, `exec/eval.rs` |
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
| SQL descriptor areas | 64 at once, of a session | §17.2 GR3 leaves the number to the implementation, and names no exception past it. Direct SQL allocates them, a server's clients among it. | `db/dynamic/areas.rs` |
| Regular expression size | 8,192 instructions compiled, about as many characters and quantified repeats, and groups 64 deep | §9.28 sets no limit. A counted repeat is compiled once for each count, so `a{9000}` passes it. | `regex/compile.rs`, `regex/parse.rs` |
| Regular expression steps | 10,000,000, of a search with back-references | A back-reference matches no automaton, so its search goes back and can take time exponential in the string. One without any takes time linear in it, with no limit. | `regex/backtrack.rs` |
| Grouping sets | 4,096 of a GROUP BY, and GROUPING of 59 columns | 2023 §7.13 SR14 and SR16 set no maximum, and a CUBE of n sets stands for 2 to the n. GROUPING's value is of 18 digits [§6.9 SR10b], which 2 to the 59 fits and 2 to the 60 does not. | `binder/grouping_sets.rs`, `binder/grouping_sets/expansion.rs` |
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
