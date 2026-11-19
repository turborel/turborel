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
[SQL/PSM](#sqlpsm) · [SQL/XML](#sqlxml) · [SQL/MM Spatial](#sqlmm-spatial) ·
[Extensions](#extensions) · [Features claimed](#features-claimed) ·
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
| SQL/MM Full-Text | The public types and routines of ISO/IEC 13249-2:2002 (CD, SC32 N0640) §12.1, built in, with the choices that part leaves to an implementation listed [below](#sqlmm-full-text) [§12.2]. |
| SQL/MM Spatial | The types and routines of ISO/IEC 13249-3:2016 (DIS, SC32 N2593), with topology, networks, shortest paths and linear referencing, built in. What is not built yet, and the choices that part leaves to an implementation, are listed [below](#sqlmm-spatial). No claim of its §23 is made yet. |
| Simple feature access | The SQL option of OGC 06-104r4 (ISO 19125-2), "SQL with Geometry Types", conformance class (gS, f, 3D, M, tS) of Annex A.4, listed [below](#simple-feature-access-sql-option). |
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

### The native protocol

A server's client asks for capabilities in Hello and is granted those the
server serves in Ready, the protocol staying version 1 (RULES.md D185,
[The Native Protocol](doc/client/05-protocol.md)). What each adds to a
statement's outcome, as the client library exposes it, follows.

| Capability | Rules | What turborel does |
|---|---|---|
| `diagnostics` | §18.1 GR3 | An Error carries its severity, detail, hint, context, and the CONSTRAINT_NAME, SCHEMA_NAME, TABLE_NAME and COLUMN_NAME of the condition, and the data type it is about (D186). |
| `conditions`, `notices` | §18.1, §3.3.4.1 | Every condition a statement raised, in order, in its completion, or each a Notice before it, a SIGNAL's of class 01 or 02 and its completion's warning (D187). |
| `row-counts` | §18.1 GR1e | A completion's rows are ROW_COUNT's for an INSERT, UPDATE, DELETE or MERGE, and a query's rows returned (D197). It carries the server's time for the statement too (D240). |
| `generated-values` | ISO/IEC 9075-2:2023 §11.4 SR10, §15.10 | An INSERT's identity values, in the order of its rows, none of its triggers' statements, none of a MERGE (D198). |
| `describe`, `cursors` | §17.7, §17.13, §13.1 to §13.4 | A statement or an open cursor described again, and several cursors of one statement open at once (D188). |
| `fetch-ends` | §4.28 | A cursor opened under autocommit commits as its last row is sent or it is closed, and rolls back as its rows fail (D199). |
| `copy` | §13.8 GR4b iii | Rows copied in, each value assigned as INSERT assigns it, all or nothing (D195). |
| `status`, `ping`, `reset`, `change-user`, `query-attributes`, `connection-attributes`, `multi-statement`, `compression`, `commit-position` | none | Extensions of the protocol, RULES.md D189 to D194 and D196. |

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

A time zone displacement is from -12:59 to +14:00. SQL-92 bounds it at
-12:59 and +13:00 [§5.3 SR25, §16.5 GR1b]; ISO/IEC 9075-2:2023 leaves
the least (IL061) and the greatest (IL062) implementation-defined, no
greater than -12:00 and no less than +14:00 [ISO/IEC 9075-2:2023
§4.7.2, §6.1 Table 13], so the newer text wins (RULES.md D32). A
literal past them is SQLCODE -103 [ISO/IEC 9075-2:2023 §5.3 SR43];
AT TIME ZONE, SET TIME ZONE and the setting SESSION.TIME_ZONE refuse
one with SQLSTATE 22009 [ISO/IEC 9075-2:2023 §6.42 GR5b ii 2 A, §19.4
GR1b, §6.1 GR8]; a CAST of a string, as of any string that is no
datetime, with 22018. A displacement is stored, logged and sent as a
number of minutes of 16 bits or more, so no format bounds it.

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
| Columns of ISO/IEC 9075-11 | ISO/IEC 9075-11:2006 draft Clause 5, §6.11, §6.20, §6.21, §6.28, §6.30, §6.31, §6.40, §6.41, §6.50, §6.52 | Each view of ISO/IEC 9075:1992 and of ISO/IEC 9075-11 here has every column the draft gives it. A data type's descriptor has INTERVAL_TYPE, an interval type's fields as the draft's check constraint names them, and INTERVAL_PRECISION, its leading field precision [§6.21 Description 8, 9]. TABLES has IS_INSERTABLE_INTO, YES of a base table and of an updatable view, and VIEWS has it as INSERTABLE_INTO, as Feature T111 is not supported [§6.50 Description 7b]; COLUMNS has IS_UPDATABLE, YES of each column of such a table [ISO/IEC 9075-2:2023 §4.17.6]; the Information Schema's own tables and views are neither. COMMIT_ACTION is a temporary table's ON COMMIT [§6.50 Description 4]. SCHEMATA's SQL_PATH is null, as no `<schema definition>` here has a `<schema path specification>` [§6.41 Description 5b]. KEY_COLUMN_USAGE's ORDINAL_POSITION is a column's position in its constraint, a foreign key's too [§6.28 Description 3], where SQL-92 makes a foreign key's column's that of the column it references [ISO/IEC 9075:1992 §21.3.14 Description 3], which POSITION_IN_UNIQUE_CONSTRAINT shows now, null of a UNIQUE or PRIMARY KEY constraint's column [§6.28 Description 4]. CHARACTER_SETS's CHARACTER_REPERTOIRE is SQL_TEXT [ISO/IEC 9075-2:2023 §4.3.7 Table 1]. A routine's SQL_PATH is its module's PATH, or its own schema [ISO/IEC 9075-2:2023 §11.60 GR3v], each written `catalog.schema`, a comma between; MAX_DYNAMIC_RESULT_SETS is 0; AS_LOCATOR NO of a function, null of a procedure; IS_UDT_DEPENDENT YES of a routine a type's definition makes [ISO/IEC 9075-2:2023 §4.35.1]. The columns of an external routine, a transform's from-sql and to-sql routines, and a `<result cast>` are null, as every routine is an SQL routine, which has none [ISO/IEC 9075-2:2023 §4.35.5, §11.60 SR9m i]; the views join a `<result cast>`'s descriptor from DATA_TYPE_DESCRIPTOR, as the draft does, which none matches. |
| Timestamps | ISO/IEC 9075-11:2006 draft §5.6, §6.30 Description 13, 14, §6.40 Description 24, 25, §6.58 Description 13, §7.3 Table 1; ISO/IEC 9075-2:2023 §4.9.7, §4.35.5, §4.46.1, §11.49 GR5j, §11.51 GR1g, GR1h, §11.56 GR1, §11.60 GR4, GR5 | Feature T011 (Timestamp in Information Schema), with Features F411 (Time zone specification) and F555 (Enhanced seconds precision), which it implies [§7.3 Table 1]. CREATED and LAST_ALTERED of ROUTINES, METHOD_SPECIFICATIONS, ROUTINES_S and METHOD_SPECS, and CREATED of TRIGGERS and TRIGGERS_S, are of TIMESTAMP(2) WITH TIME ZONE, the domain TIME_STAMP's type [§5.6]: the value of CURRENT_TIMESTAMP, its instant and the session's time zone displacement, as the statement that made what they describe ran, its fraction of a second cut to two digits. A routine a type's definition makes, its observers, mutators, constructor and distinct type's functions, is of the CREATE TYPE, or of the ALTER TYPE that added its attribute; a method's body is a routine descriptor of CREATE METHOD's timestamps [§11.60 GR4, GR5], and its specification keeps those of the statement that specified it. No statement here alters a routine, a method specification or a module, as ALTER ROUTINE is of an external routine alone [§11.61 SR5], so LAST_ALTERED is CREATED [§6.40 Description 25]. The timestamps are kept in the descriptors, and a definition's log record carries its statement's CURRENT_TIMESTAMP, so recovery and a standby's replay make them the same, reading no clock (RULES.md D37). *(Extension)* A registered XQuery module keeps both too, which no view shows, ALTER XQUERY MODULE moving the last-altered one [§11.61 GR4]. |
| Views of usage | ISO/IEC 9075-11:2006 draft §5.13, §5.16, §5.17, §5.20, §5.24, §5.44, §5.46 to §5.48, §5.74, §6.13, §6.17, §6.18, §6.35, §6.37 to §6.39, §6.63 | Features F341 (Usage tables) and F231 (Privilege tables), each of their views defined. CHECK_CONSTRAINT_ROUTINE_USAGE, COLLATION_CHARACTER_SET_APPLICABILITY, COLUMN_COLUMN_USAGE, COLUMN_UDT_USAGE, DATA_TYPE_PRIVILEGES, ROUTINE_COLUMN_USAGE, ROUTINE_ROUTINE_USAGE, ROUTINE_SEQUENCE_USAGE, ROUTINE_TABLE_USAGE and VIEW_ROUTINE_USAGE, with the draft's definitions, over Definition Schema tables made from what the catalog keeps of each definition for RESTRICT: a routine's tables, subject routines and sequence generators, a view's and a check constraint's subject routines, fixed as each was defined. ROUTINE_COLUMN_USAGE shows the columns its Description 1 lists, as the routine's body binds now: those a searched UPDATE's or DELETE's search condition or an UPDATE's source references, and those an INSERT's column list or an UPDATE's SET names [§6.35 Description 1a to 1d]; not a column a query alone references, which MODULE_COLUMN_USAGE shows of a module's routine [ISO/IEC 9075-4:2006 draft §19.1 Description 2]. COLUMN_COLUMN_USAGE binds each generation expression now. Corrected: COLUMN_UDT_USAGE joins a column's descriptor by the column's name, as DATA_TYPE_DESCRIPTOR keys it here; DATA_TYPE_PRIVILEGES passes over a column of a domain, which has no DTD_IDENTIFIER of its own here, and reads a method specification parameter's object as its specific name [§6.29 Description 5]. |
| Conformance views | ISO/IEC 9075-11:2006 draft §5.52 to §5.57, §6.43 to §6.46 | Features F501 (Features and conformance views) and F502 (Enhanced documentation tables): SQL_FEATURES, SQL_PARTS, SQL_PACKAGES, SQL_SIZING, SQL_IMPLEMENTATION_INFO and SQL_SIZING_PROFILES, with the draft's definitions. SQL_CONFORMANCE has a row of each part ISO/IEC 9075-2:2023 names, and of each feature and subfeature of the taxonomies of ISO/IEC 9075-2:2023 (Annexes D and H) and of the 2006 drafts of ISO/IEC 9075-4, 9075-11 (Annex E) and 9075-14, IS_SUPPORTED YES exactly where [Features claimed](#features-claimed) claims it, or claims the feature a subfeature is of [§6.43 Description 7], COMMENTS the section claiming it, IS_VERIFIED_BY null, as no conformance test has verified a claim [Description 6]; `cargo xtask features` writes its rows (`system/definition/conformance.tsv`) from those texts and that table, and `cargo xtask verify` fails if they differ. No part is claimed whole, and no text defines a package, so SQL_PACKAGES has no row. SQL_SIZING has a row of each implementation-defined limit ISO/IEC 9075-2:2023 Annex B lists, IL001 to IL208, SIZING_ID its number, SIZING_NAME Annex B's words, SUPPORTED_VALUE read from the constant the code keeps it in, 0 of a limit there is none of (IL039 to IL041, IL063's digits), null of IL066, of Ada (`system/definition/sizing.rs`). ISO/IEC 9075-11 and 9075-2 name no implementation information item, so SQL_IMPLEMENTATION_INFO has no row, and no profile is defined, so SQL_SIZING_PROFILES has none. The implementation information and sizing items ISO/IEC 9075-3 (SQL/CLI) adds [§6.44, §6.45] are not defined yet; they will be added, read from that part's text. |
| Short name views | ISO/IEC 9075-11:2006 draft §5.77, §4.2 | The short name view of each view defined here, with the draft's definition, each column named by an `<as clause>` as its `<view column list>` names it [§7.9 SR9a, §11.19 GR1], so it shows the rows its view shows the user: CATALOG_NAME, SCHEMATA_S, DOMAINS_S, TABLES_S, COLUMNS_S, REF_CONSTRAINTS, KEY_COLUMN_USAGE_S, CHARACTER_SETS_S, COLLATIONS_S, SEQUENCES_S, TRANSLATIONS_S, CONSTR_TABLE_USAGE, CONSTR_COL_USAGE, COL_DOMAIN_USAGE, ELEMENT_TYPES_S, FIELDS_S, UDT_S, ROUTINES_S, PARAMETERS_S, ATTRIBUTES_S, METHOD_SPECS, METHOD_SPEC_PARAMS, REFERENCED_TYPES_S, TABLE_METHOD_PRIVS, ADMIN_ROLE_AUTHS, ROLE_ROUT_GRANTS, ROL_TAB_METH_GRNTS, TRIGGERS_S, TRIG_UPDATE_COLS, TRIG_TABLE_USAGE, TRIG_COLUMN_USAGE, TRIG_ROUT_USAGE_S, TRIG_SEQ_USAGE_S, COLLATION_APPLIC_S, COL_COL_USAGE, CONSTR_ROUT_USE_S, ROUT_ROUT_USAGE_S, ROUT_SEQ_USAGE_S, ROUTINE_COL_USAGE, ROUT_TABLE_USAGE, SQL_IMPL_INFO and SQL_SIZING_PROFS; those of [SQL/PSM](#sqlpsm) and [SQL/XML](#sqlxml) with them. None is a feature of its own: each is of its view's, as the draft's Conformance Rules give them [§5.77 CR1 to CR53], and every name is of 18 characters at most, as an SQL-implementation without Feature F391 (Long identifiers) needs [ISO/IEC 9075-2:2023 §5.2 CR1, CR2]. Each has every column of its view's the draft gives it. Corrected: ATTRIBUTES_S's select list, which names IS_NULLABLE where its `<view column list>` names ATTRIBUTE_NAME, read as the list; METHOD_SPECS's USER_DEFINED_TYPE_CATALOG, _SCHEMA and _NAME, read as the UDT_CATALOG, UDT_SCHEMA and UDT_NAME of METHOD_SPECIFICATIONS [§5.34]; ROUTINES_S's RESULT_CAST_CHAR_SET_NAME, read as ROUTINES's RESULT_CAST_CHARACTER_SET_NAME [§5.49]. |

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
| Stored | RULES.md D79 | In chunks of 256 KiB, logged and checkpointed by chunk, collected with the versions that name them; a distinct type's as its source's. One inside an array, a multiset, a row or a structured value is held whole in that value's row, at most a chunk, a longer one SQLCODE -900. The column store refuses a large object column, and one of a type holding a large object. |

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
current year. A displacement is 22007 past -12:59 or +14:00, as any field
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

### Data change delta tables

Feature T495 (combined data change and retrieval). A `<data change delta
table>`, OLD, NEW or FINAL TABLE of an INSERT, a searched UPDATE or
DELETE, or a MERGE, is read as a table by the query that runs its
statement [§7.6].

| Feature | Rules | What turborel does |
|---|---|---|
| Where it stands | §7.5 SR3, §7.17 SR2, SR3d iv, GR1a, §7.19 SR1, §11.32 SR7 | As the one table reference of the FROM clause of a query specification that is the body of a cursor's query, directly run, prepared or declared, of a SELECT INTO, in a routine too, of a WITH list element, one each, or of an INSERT's query; or as a MERGE's source. WITH list elements' statements run in the order written, then the query's own (RULES.md D174, D175). In a subquery, a derived table, beside another table reference, in a set operation's operand or in a view, SQLCODE -103, its statement not run. |
| Its statement | SR11, SR11a | Not FOR PORTION OF, nor positioned. OLD of an INSERT, or of a MERGE that neither updates nor deletes, and NEW or FINAL of a DELETE, or of a MERGE that neither updates nor inserts, are SQLCODE -103. |
| Its subject table | SR11b | A base table, or a view of CASCADED CHECK OPTION, its rows the base table's change made, in the view's columns; any other view is SQLCODE -103. Of a typed table, the rows of its subtables an UPDATE or DELETE reaches, by their superrows [§14.9 GR4, §14.14 GR6] (RULES.md D175). |
| Its rows | GR5b | The old or new delta table of the operation, each row as the statement writes it, after its BEFORE triggers, its defaults, identity and generated columns given, before its referential actions and AFTER triggers. A MERGE's OLD has the rows it updated and deleted, its NEW those it inserted and updated. A derived column list renames the columns [SR12b ii]. |
| FINAL | GR5a, §15.18 GR15, §15.21 GR6i | A referential action of the statement that changes the subject table is SQLSTATE 27001; an AFTER trigger's statement that does is 0W001, told by the trigger's 09000 [§15.21 GR6m]. |
| Failure | | A statement that fails gives no rows, and the query fails with it. The query's rows are read as its cursor opens, so its own error undoes the statement too. |

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

### Enhanced numeric functions

Feature T621 (Enhanced numeric functions): the numeric value functions
LN, EXP, POWER, SQRT, FLOOR and CEIL or CEILING of ISO/IEC 9075-2:2023
§6.31, and the set functions STDDEV_POP, STDDEV_SAMP, VAR_POP, VAR_SAMP
and the binary set functions of §10.9.

| Feature | Rules | What turborel does |
|---|---|---|
| LN, EXP | §6.31 SR16, SR17, GR12, GR13 | Of a number, DOUBLE PRECISION, or DECFLOAT(34) of a DECFLOAT. LN of 0 or below is SQLSTATE 2201E. EXP past the range of the result is 22003. Null of a null. |
| POWER, SQRT | §6.31 SR18, SR19, GR14 | DOUBLE PRECISION, or DECFLOAT(34) if either operand is DECFLOAT. 0 to a negative power, and a negative base to a power not a whole number, are 2201F; 0 to the power 0 is 1, to a positive power 0; a negative base to a whole power is signed as the power is odd or even. SQRT(x) is POWER(x, 0.5), so SQRT of a negative number is 2201F. Past the range, 22003. |
| FLOOR, CEIL, CEILING | §6.31 SR20, GR15, GR16 | Of an exact number, exact of scale 0, found exactly, of precision enough for its whole part and one digit more (IV151); of an approximate number or a DECFLOAT, of its type. |
| The names | §6.31, §10.9 | LN, EXP, POWER, SQRT, FLOOR, CEIL, CEILING, STDDEV_POP, STDDEV_SAMP, VAR_POP, VAR_SAMP, COVAR_POP, COVAR_SAMP, CORR and the REGR_ names, which SQL-92 does not reserve, are the functions only before "(", so a column may still be named one. |
| VAR_POP, VAR_SAMP, STDDEV_POP, STDDEV_SAMP | §10.9 SR2, SR7h, SR7i, SR7j, GR7d ix | Of a number; DISTINCT or ALL written is SQLCODE -103 [SR2]. DOUBLE PRECISION, or DECFLOAT(34) of a DECFLOAT. VAR_POP is (SXS − SX²/N)/N; VAR_SAMP divides by N − 1 and is null of one value; STDDEV_ is the square root. Null of no values. |
| The binary set functions | §10.9 SR9, GR8 | COVAR_POP, COVAR_SAMP, CORR, REGR_SLOPE, REGR_INTERCEPT, REGR_COUNT, REGR_R2, REGR_AVGX, REGR_AVGY, REGR_SXX, REGR_SYY and REGR_SXY of Y and X, each a number, else -103. A pair with a null Y or X is left out, with SQLSTATE 01003. REGR_COUNT is NUMERIC(18), 0 of no pairs; the others are DOUBLE PRECISION, or DECFLOAT(34) if Y or X is, and null of no pairs. COVAR_SAMP is null of one pair. |
| CORR, REGR_R2, REGR_SLOPE, REGR_INTERCEPT | §10.9 GR8c x to xiii | Null where X is all alike. CORR is null where Y is all alike, REGR_R2 1. CORR is SQRT(POWER(N·SXY − SX·SY, 2) / ...), as GR8c x 3 writes it, so it is never below 0: of Y falling as X rises it is 1. A result past the range is null. |
| Their uses | §10.9 GR4, §6.10, §7.9 | Each takes FILTER, is computed for each group of GROUP BY, ROLLUP, CUBE and GROUPING SETS, and is a window function with OVER. |
| Out of range | §10.9 GR3e | A sum on the way past the range of the result's type is 22003, judged once the sum is finished. |

Of numbers not DECFLOAT the statistical set functions keep the count,
the means and the sums of squared deviations and of their products by
Welford's updates, so no digits are lost to the subtraction SXS − SX²/N.
Their state is written, and two states merged by Chan's formulas, so a
window frame is computed from a tree of states. SXX and SYY only grow,
so whether one passes the range does not depend on the order of the
rows (RULES.md D15). Of DECFLOAT operands they find SX, SXS and the rest
in DECFLOAT arithmetic, as the General Rules write them, and write no
state, as SUM of DECFLOAT writes none. A variance rounding leaves below 0
is 0. CORR and REGR_R2 rounding puts past 1 are 1.

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
| Array types | §6.1 SR43 to SR47 | `<data type> ARRAY [n]`, of any data type of the standard, rows and arrays among them, n from 1 to 65,535 (SQLCODE -900 past it), 65,535 if not written. The geometry types may be an element's. No other type of an extension may. |
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
| Multiset types | §6.1 SR48, §4.12.3 | `<data type> MULTISET`, of any data type of the standard, rows, arrays, multisets and distinct types among them. The geometry types may be an element's. No other type of an extension may. A multiset holds at most 65,535 elements, as an array may (see [Limits](#limits)); past it, SQLSTATE 2200Q [§10.9 GR3b]. |
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
| Bitemporal tables | §4.17.2, §15.8 GR9, §15.14 GR12, §11.7 SR6d i, §11.8 SR18a i | A versioned table may have an application-time period too, defined with the table or added either way about. UPDATE and DELETE FOR PORTION OF it keep each row they change as a historical row, and the parts they keep begin at the transaction timestamp. Its keys WITHOUT OVERLAPS and temporal foreign keys judge its current rows alone. A period predicate references either period, of current and historical rows read FOR SYSTEM_TIME. |
| SYSTEM_TIME no application-time period | §11.7 SR3, §11.8 SR18a i, §14.9 SR7a, §14.14 SR7a, §11.28 SR2d, GR1d | SYSTEM_TIME, a reserved word, is no `<application time period name>`: a key WITHOUT OVERLAPS, a foreign key's PERIOD or FOR PORTION OF naming it is SQLCODE -102. A view, constraint, assertion, routine or trigger referencing the system-time period of a table not versioned refuses DROP PERIOD FOR SYSTEM_TIME RESTRICT, SQLCODE -103, and CASCADE drops it. |
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
| The action | SR10, SR12 to SR14, IA046, IA047; ISO/IEC 9075-4:2006 draft §9.19 | Bound as the trigger is defined, as the schema's owner, so its rules and privileges are checked then. INSERT, searched UPDATE and DELETE, MERGE, SELECT INTO, GET DIAGNOSTICS, CALL, or an SQL control statement, SIGNAL or RESIGNAL of [SQL/PSM](#sqlpsm), a compound statement ATOMIC, Part 2's BEGIN ATOMIC list one, else SQLCODE -103: a schema, transaction, savepoint, connection, session, dynamic or cursor statement, or a parameter, is refused as the trigger is defined. A BEFORE trigger's action changes no SQL-data and has no transition tables, and references no generated column of its new transition variable, else SQLCODE -103. |
| Privileges | AR2, AR3, §12.3, §12.7 | The schema's owner holds TRIGGER on the table, and SELECT if a transition name is given, else SQLCODE -104. TRIGGER is granted, revoked and held in ALL PRIVILEGES as the other table privileges are, and TABLE_PRIVILEGES shows it. A view's owner holds TRIGGER on it, not grantable [§11.32 GR5]. REVOKE with RESTRICT is refused where it would abandon a trigger, its TRIGGER or SELECT or a privilege its action needs, and CASCADE drops it. |
| Transition variables | §6.6 SR9, SR13, §6.4 SR7, §9.2 | OLD.C and NEW.C, where no table or correlation name of the statement is the qualifier, are the old and new row of the transition. A BEFORE row trigger's SELECT INTO and GET DIAGNOSTICS assign columns of NEW by store assignment, which the row then takes. |
| Transition tables | §7.6 SR6, §4.46.2 | OLD TABLE and NEW TABLE name, where no query name does, the old and new rows of the state change, held up to the work area and spilled past it, read in subqueries too. |
| Activation | §4.46.2 | A state change of each table, event and set of updated columns. A trigger of UPDATE OF its columns sees the transitions whose SET clauses name one of them, of every state change, the referential actions' included. A state change arises of no rows too, so a statement trigger executes for a statement that changes none. |
| BEFORE | §15.19, §15.21 GR4, §15.11 GR4, GR6a, §15.14 GR5, GR9, §15.8 GR2 | Once the rows are decided, before any is written: of an INSERT its rows given their values and identities, then computed again by their generated columns as the triggers left them; of an UPDATE its pairs of rows, the same; of a DELETE its rows. A statement trigger once for its state change, a row trigger for each transition. |
| AFTER | §15.20, §9.17 GR17b | At the end of the statement, after its referential actions and once its immediate constraints hold, over every state change it made, each trigger's transition tables whole. |
| Order | §4.46.1, §15.21 GR4, IS003, US048 | Triggers of one action time execute in the order created, and those of one timestamp of creation, as those one statement creates, in the order they were created. A row trigger takes the transitions in the order the statement changed the rows. |
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
| Trigger descriptors described | 11:2006 draft §6.53 to §6.58 | The Definition Schema has a row of each trigger in TRIGGERS, of each column of a trigger column list in TRIGGERED_UPDATE_COLUMNS, and of each table, subject routine and sequence generator its action names in TRIGGER_TABLE_USAGE, TRIGGER_ROUTINE_USAGE and TRIGGER_SEQUENCE_USAGE. TRIGGER_COLUMN_USAGE has a row of each column its Description 1 lists, found by binding the definition again: those any search condition of it references, its WHEN condition's among them, those an UPDATE's source references, and those an INSERT's column list or an UPDATE's SET names [§6.54 Description 1a to 1e]. The triggered action column set DROP COLUMN RESTRICT reads holds these and each other column the action references, a query's select list's among them [ISO/IEC 9075-2:2023 §11.49 GR5i]. ACTION_ORDER counts from 1 among the triggers of one table, event, time and orientation in the order of creation. ACTION_TIMING is INSTEAD OF of a trigger on a view, which the draft precedes. ACTION_CONDITION and ACTION_STATEMENT are as written, null if longer than CHARACTER_DATA holds, so ACTION_STATEMENT may be null, where the draft has it NOT NULL. CREATED is its timestamp of creation, the CURRENT_TIMESTAMP of its CREATE TRIGGER [§6.58 Description 13] (Feature T011, see [Timestamps](#transactions-sessions-and-the-information-schema)); its order of creation, which ACTION_ORDER follows, is kept apart. |
| TRIGGERS, TRIGGERED_UPDATE_COLUMNS | 11:2006 draft §5.69, §5.64 | As the draft defines them: each trigger on a table, and each column of a column list, the user or an enabled role holds a privilege other than SELECT on, the condition and statement shown only where an enabled identifier owns the trigger's schema. |
| Trigger usage views | 11:2006 draft §5.65 to §5.68 | TRIGGER_TABLE_USAGE, TRIGGER_COLUMN_USAGE, TRIGGER_ROUTINE_USAGE and TRIGGER_SEQUENCE_USAGE, as the draft defines them, of the triggers of schemas an enabled identifier owns. |

The action is bound again only when the definitions change, and kept
for each level of nesting it runs at. A trigger whose action executes
it again recurses until its WHEN condition stops it or the limit of
nesting is passed (see [Limits](#limits)). The statements IA046 and
IA047 refuse are refused as Syntax Rules when the trigger is defined. A
savepoint statement of a procedure an action calls raises 0W000 as it
runs [§15.21 GR6e iii], and the action fails with 09000; nothing else
raises it [GR6h]. 11000
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
| The body | SR20, SR23, §16.2 SR1 to SR4, IA075, IA076, IA078 | Bound once as the routine is defined, as the owner of its schema, of that schema, so its rules and privileges are checked then and its names fixed [§4.35]. A function's body is a RETURN, of a type its result type stores, RETURN NULL a null of it. A procedure's is INSERT, searched UPDATE or DELETE, MERGE, SELECT INTO, GET DIAGNOSTICS or CALL, and no RETURN. Either may be an SQL control statement, SIGNAL or RESIGNAL, of [SQL/PSM](#sqlpsm), whose nested statements count for its SQL-data access, or a savepoint statement [§9.18 GR8b iv 2]. A schema, connection, session or dynamic statement, COMMIT, or ROLLBACK but to a savepoint, is refused (IA078). A body of CONTAINS SQL that reads a table, or of READS SQL DATA that changes one, is refused (IA076, IA075). Each is SQLCODE -103. |
| Determinism | IA077 | DETERMINISTIC is taken as written: a body is not checked for a source of non-determinism. |
| Privileges | GR1, §12.3, §12.7 GR12o, GR12q | The owner holds EXECUTE from _SYSTEM, grantable if it holds grantable every SELECT, INSERT, UPDATE and DELETE on each table the body reads and EXECUTE on each routine it invokes. EXECUTE is granted and revoked as `<specific routine designator>` designates a routine, ALL PRIVILEGES on one being EXECUTE, and ROUTINE_PRIVILEGES shows it. A revoke that leaves the owner without a privilege the body needs, or EXECUTE on a routine it invokes, abandons the routine: RESTRICT refuses it (2B000), and CASCADE drops it and what invokes it. |
| Invocation | §9.18 SR5, SR6, SR10, §9.6, §9.7, GR3 to GR10, ID047 | Of the functions of the name in the qualifying schema, else the statement's default schema, its SQL-invoked functions and its distinct types' alike, those the statement's authorization identifier holds EXECUTE on, of as many parameters as there are arguments [SR10a], each typed argument's precedence list holding its parameter's designator [SR10d]. Left to right, those whose parameter's designator comes first in the argument's list are kept [§9.6], and one must be left [SR10h iv 3]. None of the arity is SQLCODE -103 citing SR10a, none of the types -103 citing SR10d, two alike -103, and functions that take the arguments but none with EXECUTE -104. A `?` argument decides nothing and takes its parameter's type [§20.7] (B033). A routine is found before a registered function of the name, which is searched only if no function of the schema takes the arguments [SR10h iv 1] (RULES.md D74). Each argument is stored in its parameter, then a null-call function given a null returns null without its body, else the body's value is stored in the result type. Its errors are those of store assignment, 22001 and 22003, the parameter named in the condition [§23.1 GR4q]. A body that ends without executing a RETURN raises 2F005 [GR8b ix 1]. A function that possibly modifies SQL-data is run by the session, as a statement nested in the one invoking it, its changes undone with that statement (RULES.md D76). |
| Parameters | §6.6 SR8, SR9, §6.8 | A parameter is named by its name, or by the routine's name and its name. A column of a table the body reads hides a parameter of its name. A function's body is a subquery of the row of its parameters, a VALUES of one value evaluated where it is, so a function that reads no table may be invoked where no subquery may run. |
| Where a function may be | §7.17 SR25, §11.9 SR6, §11.47 SR8, §11.4 SR10d, SR10e, §9.16 SR1am, §6.12 SR1a, SR2a | A function that possibly modifies SQL-data is invoked only where no query expression, CHECK, assertion, WHEN or search condition holds the invocation: in a SET clause of UPDATE or MERGE, a row of VALUES of INSERT or MERGE, a CALL argument, and a value of a program. Not in a CASE abbreviation, nor a simple CASE's operand. One that is not deterministic, or possibly reads SQL-data, is invoked in no CHECK, assertion or generation expression, nor is a registered function that says it is not deterministic (RULES.md D133). Each is SQLCODE -103. |
| CALL | §16.1, §9.18 SR9, GR3 to GR14, §10.4 CR6, CR7 | The procedure of the name and arity, with EXECUTE, one at most by SR20a. An IN parameter's argument is a value expression it stores, a `?` of the parameter's type, an OUT or INOUT one's a target: a host parameter, a parameter of the procedure whose body the CALL is, or a column of a BEFORE row trigger's new transition variable, not an AFTER trigger's [SR9h iii 3]. NULL or DEFAULT of an IN parameter, and DEFAULT of an INOUT one, are of Features T522 and T523, SQLCODE -900. OUT parameters start null. The body runs as the schema's owner, of its schema, with a diagnostics area of its own, within the CALL statement and its instant (US037, US038). Each OUT and INOUT value is then assigned to its target [GR10b]. A BEFORE trigger's action calls no procedure that possibly modifies SQL-data [§11.49 SR14a], the procedure of the CALL's arity. |
| Subject routines kept | §4.35.2, §11.62 GR1, §12.7 GR12o | A view, CHECK, domain constraint, assertion, trigger, generated column or routine body invokes the routines it invoked as it was defined. Each records their specific names, and bound again chooses among those alone, so a function created later, or EXECUTE granted later, never changes what it invokes (RULES.md D73). One of them dropped makes it bind no more, never invoke another function of the name. |
| Dropping | §11.62, §10.6, §11.2 SR4l, GR10, §11.23 SR7c, §11.31 SR7e, §11.33 SR5d, §11.59 SR4g | DROP names the routine by its name, with its parameters' types or without, or by its specific name, of the routine type written, by the owner of its schema. A name alone of more than one routine of the type, or a list of types none or several are of, counting a distinct type's functions, is SQLCODE -103 [§10.6 SR3c]. RESTRICT refuses it if a view, constraint, assertion, generated column, trigger or routine invokes it. CASCADE drops each, and what invokes those. A table, view or column a body reads, a distinct type a routine is typed by or its body uses, and a schema holding a routine, are dropped only with CASCADE, which drops the routine. |
| Named arguments | §10.4, §9.18 SR4, SR5d | `name => value` in a CALL (T521) and a polymorphic table function's invocation (T524), each put at its parameter's place. All named or none, no name twice (-103). |
| Kept | §11.60 GR3, §4.35.5 | The descriptor is logged as its definition and kept in a checkpoint's snapshot, with its EXECUTE privileges. ROUTINES, PARAMETERS and ROUTINE_PRIVILEGES show it, ROUTINE_DEFINITION to the owner of its schema alone. |

SQL SECURITY (Feature T324), a parameter's default (T522, T525), a
named argument of a function other than a polymorphic table function
(T524), a monomorphic table function's RETURNS TABLE (T326), DYNAMIC RESULT SETS
above 0 (T471) and a language other than SQL
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

### SQL/JSON

The SQL/JSON query operators JSON_EXISTS [§8.23], JSON_VALUE [§6.28]
and JSON_QUERY [§6.35] are implemented, over JSON text in character
strings, with the SQL/JSON data model [§4.48.3], the parsing and
serializing of JSON text [§9.42, §9.43] and the SQL/JSON path language
[§9.45, §9.46] they read (`turborel::json`, RULES.md D84). The
operators' General Rules are the library's (`json/query.rs`), with the
rules they apply: the serializing of an item [§9.43], the array wrappers
[§9.44], the processing of the common syntax [§9.47], and the casting
and serializing of a sequence [§9.48, §9.49]; the binder makes each call
of an operator a function that applies them to its operands' values. So
are the `<JSON predicate>` IS JSON [§8.22], the constructor functions
JSON_OBJECT and JSON_ARRAY [§6.34], and the `<JSON aggregate function>`s
JSON_OBJECTAGG and JSON_ARRAYAGG [§10.11], set functions.

Features T823 (SQL/JSON: PASSING clause), T828 (JSON_QUERY) and T829
(JSON_QUERY: array wrapper options) are claimed, and so are the path
language's T831 (strict mode), T832 (item method), T833 (multiple
subscripts), T834 (wildcard member accessor), T835 (filter expressions),
T836 (starts with predicate), T837 (regex_like predicate), T840 (hex
integer literals) and T865 to T878 (the item methods bigint() to
timestamp_tz(), with precisions and scales), which §9.46's Conformance
Rules 8 to 21 ask of the methods alone. Features T811 (Basic SQL/JSON
constructor functions), T812 (SQL/JSON: JSON_OBJECTAGG), T813 (SQL/JSON:
JSON_ARRAYAGG with ORDER BY), T814 (Colon in JSON_OBJECT or
JSON_OBJECTAGG), T822 (SQL/JSON: IS JSON WITH UNIQUE KEYS predicate) and
T830 (Enforcing unique keys in SQL/JSON constructor functions) are
claimed. Features T821 (Basic SQL/JSON query operators), T824
(JSON_TABLE: specific PLAN clause), T825 (SQL/JSON: ON EMPTY and ON
ERROR clauses), T826 (General value expression in ON ERROR or ON EMPTY
clauses), T827 (JSON_TABLE: sibling NESTED COLUMNS clauses) and T838
(JSON_TABLE: PLAN DEFAULT clause) are claimed, as JSON_TABLE [§7.11] is
implemented whole (RULES.md D90). T803 (String-based JSON) is
implemented for character strings, every input and output clause of the
query operators, the constructors and IS JSON; JSON text in a binary
string, FORMAT JSON ENCODING, is SQLCODE -900, so it is not claimed.

Features T801 (JSON data type) and T802 (Enhanced JSON data type) are
claimed (RULES.md D89). These cover JSON columns, JSON ( ) of either key
uniqueness, JSON_SCALAR and JSON_SERIALIZE. A value of JSON is the input
of every query operator, constructor, aggregate and IS JSON. JSON_QUERY,
the constructors and the aggregates take RETURNING JSON, implicit of a
JSON input. T851 (optional keywords for default syntax) is claimed, as
FORMAT JSON after a JSON input, in RETURNING JSON and in JSON ( ) and
JSON_SERIALIZE. T860, T862, T863 and T864 (the simplified accessor of a
column, the wildcard member accessor, single-quoted member names and any
value expression primary) are claimed. T861 (case-sensitive member
accessor) is not. A member name after a column is an SQL identifier,
upper case unless delimited, and a quoted string is read as written.
T879 to T882 (JSON in equality, grouping, ordering and multiset element
grouping) are claimed.

The library holds the rules of the `<JSON predicate>` IS JSON [§8.22]
and of the constructor functions JSON_OBJECT and JSON_ARRAY [§6.34] and
JSON_OBJECTAGG and JSON_ARRAYAGG [§10.11] (`json/predicate.rs`,
`json/construct.rs`), string-based (T803): their inputs are character
strings, FORMAT JSON marking one as JSON text, and they return character
strings. IS JSON is Unknown of the null value and False of text that is
not JSON, or that has two members of one key under WITH UNIQUE KEYS. A
constructor keeps a character string, a number and a truth value as its
scalar and casts any other value, a datetime too, to a character string
(ID100, IV179). A null name is 22004 and two members of one name under WITH
UNIQUE KEYS 22030. An aggregate of no rows is the null value. An object
has at most 1,048,576 members (IL051) and an array as many elements
(IL052), past them 2203E and 2203D.

| Feature | Rules | What turborel does |
|---|---|---|
| JSON_EXISTS | §8.23 SR1, GR | A predicate, in a search condition or as a BOOLEAN value: True if the path gives an item, False if it gives none, Unknown if the context item is null. |
| JSON_VALUE | §6.28 SR1 to SR5, GR; §9.48 | The one scalar the path gives, cast to the returned type. Null if the context item is null, and of the SQL/JSON null. An array or object is 2203F, two items 22034, an item of a type that does not cast to the returned type 2203G, and a cast's exception is its own, 22018 say. |
| JSON_QUERY | §6.35 SR1 to SR9, GR; §9.49 | The one item the path gives, serialized as JSON text, FORMAT JSON, to the returned type. Two items unwrapped are 22034, and a text longer than the type 22001. |
| JSON_TABLE | §7.11 SR1, GR1 | A table of the rows its row pattern gives, of regular, ordinality and formatted columns, NESTED [PATH] COLUMNS to any depth with siblings, path names, PLAN and PLAN DEFAULT, the table's ERROR or EMPTY ON ERROR, and PASSING. The row pattern and every NESTED path may name PASSING variables, and a column's own path may not, SQLCODE -103. A column of the JSON type without a formatted column's clause is a formatted column, so DEFAULT ON EMPTY or ON ERROR of it is -103. A DEFAULT of a regular column is evaluated only when it is the value. JSON_TABLE in a CHECK constraint is -103, as JSON_VALUE and JSON_QUERY are [§9.16 SR1u]. |
| JSON values | §4.8, §6.1, §6.37, §6.40, §6.41, §6.36 | JSON is a type of columns, parameters and results, stored parsed in a binary form read in place. JSON ( ) parses text WITH or WITHOUT UNIQUE KEYS, 22032 of text that is no JSON text and 22037 of two members of one key under WITH. JSON_SCALAR makes a scalar of an SQL value, JSON_SERIALIZE writes text of a character string type. A value of JSON is compared, grouped, ordered and indexed by its comparison key. `doc.items[0].name` and `doc.*` are JSON_QUERY's lax paths [§6.36 SR3]. A document is at most 256 MiB, past it SQLCODE -900. |
| Common syntax | §10.14 SR1 to SR5; §10.12 | The context item is a character string, FORMAT JSON implicit or written, else SQLCODE -103. Its text is parsed WITHOUT UNIQUE KEYS before the path is evaluated (IA050, IV185), so a text that is no JSON text is 22032 whether the path reads it or not. The path specification is a character string literal, parsed once as the statement is prepared: a path that is none is refused then, -101, -102 or -103. AS a table path name is -103 [SR2a]. |
| PASSING | §10.14, §9.46 SR4, GR11b; §10.12 SR3, SR6, SR7 | An argument without FORMAT JSON is the item its value is: a character string, a number, a truth value or a datetime as it is, an interval cast to CHARACTER VARYING(32767) (IV165), a null value the SQL/JSON null. One with FORMAT JSON is a character string's JSON text, parsed first (IA054); of a null value it is the empty sequence. A binary string without FORMAT JSON, an array, a multiset or a row is -103; a user-defined type, a reference or a type beyond the standard is -900. A named variable no argument has is -103 as the statement is prepared, whatever the path does with it. Of two arguments of one name, the first is the variable's. |
| Identifiers of PASSING | §9.46 SR4, GR11b; ISO/IEC 9075:1992 §5.2 SR10 | An argument's `<identifier>` is equivalent to a named variable's `<JSON path identifier>` if its value, as SQL defines it, is the variable's name: a regular identifier in upper case, a delimited identifier its body. The path language is case-sensitive, so `PASSING 1 AS x` is the variable `$X`, and `PASSING 1 AS "x"` is `$x`. |
| starts with of a variable | §9.46 SR8 | The variable is an argument of a character string without FORMAT JSON, else -103. |
| RETURNING | §6.28 SR1, SR2; §6.35 SR1b; §10.13 SR2 | JSON_VALUE returns a predefined character string, numeric, BOOLEAN or datetime type, else -103; JSON_QUERY a character string type, else -103. Without RETURNING each returns CHARACTER VARYING(32767) (ID074, ID101). A binary string type for JSON_QUERY, and FORMAT JSON ENCODING of a binary string anywhere, are valid SQL not implemented, -900; ENCODING of a character string is -103 [§10.12 SR8, §10.13 SR2a]. A cast that cuts a string completes with the warning 01004, as CAST does. |
| Wrappers and quotes | §6.35 SR6 to SR9; §9.49 GR4, GR5 | WITHOUT ARRAY is the default. WITH ARRAY WRAPPER is UNCONDITIONAL. WITH CONDITIONAL ARRAY WRAPPER leaves a sequence of one array or object as it is and wraps any other, a lone scalar included, as §9.49 GR4a iv says. An empty sequence is never wrapped, so ON EMPTY applies. OMIT QUOTES casts a lone character string to the returned type, KEEP QUOTES, the default, serializes it in quotation marks. A quotes behavior WITH a wrapper is -103 [SR8]. |
| ON EMPTY and ON ERROR | §6.28 SR4, SR5; §6.35 SR4, SR5; §8.23 SR1; §9.48 GR4, GR5; §9.49 GR4c, GR6; §9.43 GR2a iii | NULL ON EMPTY and NULL ON ERROR are implicit, and FALSE ON ERROR of JSON_EXISTS. ON ERROR replaces an exception condition, which is an error of SQLSTATE class 22, data exception, alone: a limit of turborel's, SQLCODE -900, and a Syntax Rule broken are raised whatever ON ERROR says. ERROR ON EMPTY makes an empty sequence 22035, an exception ON ERROR then replaces, so ERROR ON EMPTY with NULL ON ERROR, written or implicit, is null [§9.48 GR4c i, GR5c]. DEFAULT is any value expression, a column's or a `?` too, cast to the returned type, which CAST is to take, else -103; a cast of the DEFAULT ON EMPTY that fails is an exception ON ERROR replaces [GR4c iii], and of the DEFAULT ON ERROR one raised [GR5d i]. A DEFAULT's value expression is evaluated only for a row that takes it [GR4c iii, GR5d]: one that can raise an exception is bound as a CASE of which behavior the row takes, one that cannot is evaluated with the other operands, as no row could tell. NEXT VALUE FOR in a DEFAULT draws of every row, as §6.14 GR1a draws of each row whatever is taken, and is in no CASE [§6.14 SR2], so the operator's DEFAULTs are then all arguments. An exception the value expression raises as it is evaluated, before its cast, is raised. JSON_QUERY's text longer than its returned type is 22001 whatever ON ERROR says [§9.43 GR2a iii]. |
| Dynamic parameters | §20.7 GR5a, GR5b | No rule types a `?` here. A `?` context item or argument FORMAT JSON is CHARACTER VARYING(32767) and a DEFAULT the returned type [EXTENSION]; an argument without FORMAT JSON is refused. |
| Where written | §6.3, §8.1; §9.16 SR1g, SR1h, SR1al; §11.9 CR2 | JSON_VALUE, JSON_QUERY and the constructors wherever a value is; JSON_EXISTS and IS JSON wherever a predicate is: in queries and views. JSON_VALUE, JSON_QUERY and JSON_EXISTS are potential sources of non-determinism, so a CHECK constraint or a generation expression holding one is SQLCODE -103; the constructors, the aggregates and IS JSON may be there. |
| Key words | §5.2 | JSON_EXISTS, JSON_VALUE and JSON_QUERY are no key words of SQL-92, so each stays an identifier, the operator only before a parenthesis. PASSING, RETURNING, FORMAT, JSON, ENCODING, ERROR, EMPTY, ARRAY, OBJECT, WITHOUT, CONDITIONAL, UNCONDITIONAL, WRAPPER, KEEP, OMIT, QUOTES, SCALAR and STRING are read only where the operators' Formats have them, so a column of any of those names stays one. |
| IS JSON | §8.22 SR1 to SR4, GR | A predicate wherever one is written. True if the character string is JSON text of the kind named, VALUE, the default, ARRAY, OBJECT, or SCALAR, neither, and, WITH UNIQUE KEYS, of no object of two members of one key; False otherwise, of a text that is no JSON text too; Unknown of the null value. IS NOT JSON is its negation [SR4]. FORMAT JSON is implicit [SR1]. An input of no character string is SQLCODE -103, a binary string -900. A text beyond turborel's limits is -900, no answer. |
| JSON_OBJECT | §6.34 SR5, SR6, GR2, GR5a | Members written `[KEY] name VALUE value` or `name : value` (T814). A name is a character string, else -103; a null one is 22004. A value of a string, number or truth value is kept, one of another type cast to CHARACTER VARYING(32767) (ID100, IV179), a datetime among them; FORMAT JSON parses it under the key uniqueness constraint. NULL ON NULL is the default; ABSENT ON NULL leaves a member bound to the SQL/JSON null out. WITH UNIQUE KEYS makes two members of one name 22030, those left out too. Of no members, `{}`. |
| JSON_ARRAY | §6.34 SR7, GR3, GR4, GR5a | By enumeration, its elements in order; by query, of a query of degree 1, else -103 [SR7c], its column's values in the order of its ORDER BY, `[]` of no rows. A lone scalar subquery makes it by query [SR7d]. ABSENT ON NULL is the default. An element FORMAT JSON is parsed WITHOUT UNIQUE KEYS (IV156). |
| JSON_OBJECTAGG, JSON_ARRAYAGG | §10.11 SR1 to SR5, GR | Set functions, with FILTER and OVER, of a group's rows, nulls kept; the null value of no rows. JSON_OBJECTAGG is NULL ON NULL and WITHOUT UNIQUE KEYS by default, and a null name is 22004. JSON_ARRAYAGG's ORDER BY (T813) orders the elements, its keys orderable, else -103 [§9.14 SR3]; without one, the order the rows are read (US044). ABSENT ON NULL is its default. A grouping past its work area spills them as it spills every set function. They are no potential source of non-determinism, so a constraint may hold them. |
| Inputs and outputs of the constructors | §10.12 SR3, SR4a, SR6 to SR8; §10.13 SR2 | As the query operators': a value FORMAT JSON of no string is -103 [SR6], a binary string without FORMAT JSON -103 [SR7], an array, a multiset or a row -103 [SR3], ENCODING of a character string -103 [SR8]. A JSON-returning function, a constructor, an aggregate or JSON_QUERY, as an input is FORMAT JSON implicitly [SR4a], so constructors nest, as a PASSING argument too. RETURNING is a character string type, else -103, a binary string type -900; without it CHARACTER VARYING(32767) (ID098, ID134). A text longer than the type is 22001. |
| Key words of the constructors | §5.2 | JSON_OBJECT, JSON_ARRAY, JSON_OBJECTAGG and JSON_ARRAYAGG are no key words of SQL-92, each the function only before a parenthesis. ABSENT, KEYS, SCALAR and the words the query operators share are read only where the Formats have them. RETURNING within a constructor is the output clause unless VALUE, a colon, a comma or ")" follows it, so a column RETURNING may be a name. IS [NOT] JSON, and FORMAT JSON [ENCODING ...] before IS, after a value are IS JSON, so `SELECT ID JSON FROM T` and `SELECT ID FORMAT FROM T` still name columns. |
| SQL/JSON items | §4.48.3, §8.2 GR10 | The SQL/JSON null, a scalar that is a character string, a number, a truth value or a datetime, an array, or an object. Two are equivalent if their comparison keys are equal (RULES.md D89): scalars equal as SQL values, strings code point by code point, arrays element by element, objects by their members sorted by key and value. |
| JSON text | §9.42 GR3a; RFC 8259 §2 to §7 | A string that is no JSON text is SQLSTATE 22032, and so is one whose string escapes a surrogate that is not of a pair. Members of one key are all kept (UA057), in the order written (US039); under WITH UNIQUE KEYS they are 22037. Arrays and objects nested more than 128 deep are SQLCODE -900. |
| Numbers | §9.42 GR3a iii 3 A IV, §9.45 GR1a; §5.3 SR29 | A number without an exponent is exact, of its fraction's scale; one with an exponent is approximate. An exact one of more than 18 digits, or a fraction of more than 18, is exact still if its significant digits fit, leading zeros and a fraction's trailing zeros not being significant, else DECFLOAT of at most 34 significant digits. A number of more significant digits, or out of its kind's range, an approximate one too small to be other than zero among them, is 22003, IS JSON raising it too. |
| Lexical elements | §9.45 SR1 to SR4, GR1; ECMAScript 5.1 §7 | Identifiers of Unicode 13.0's letters, with `\u` escapes; string literals in single or double quotation marks, with ECMAScript's escapes and line continuations; decimal numeric literals, and hex integer literals (T840), `0x` or `0X` and hexadecimal digits, each the exact number of its value, of more than 18 digits a DECFLOAT of at most 34. A literal past those limits is SQLCODE -900. The language is case-sensitive and reserves no word: `$.type` is a member, `$.type()` the method [NOTE 483]. A character that begins no token is SQLCODE -101. |
| Format | §9.46 F, SR3, SR5 to SR7, SR10 | A path begins with `strict` or `lax`. Text that does not conform is SQLCODE -102; `@` outside a predicate and `last` outside a subscript are -103. A like_regex pattern and its flags are read when the path is parsed, 2201S and 2201T. The precision and scale of decimal() and the precisions of time(), time_tz(), timestamp() and timestamp_tz() are unsigned integers, a precision of 0 or a scale above the precision -103 [§6.1 SR23, SR25], a precision above 18, or above 6 for a time or timestamp, -900. A `<JSON datetime template>` is read as the path is parsed [SR10]: one that is no datetime template, or is neither dated nor timed, or zoned and not timed, is SQLCODE -103, and one of FF7 to FF9 SQLCODE -900. A path nested more than 64 deep, or of more than 63 accessors or operators in a row, is SQLCODE -900. No analysis for certain exceptions is made (IA223) [SR9]. |
| Modes | §9.46 GR7, GR8; §4.48.5 | Lax mode wraps and unwraps one level, and takes what it still cannot access for nothing. Strict mode raises each structural error. |
| Member accessors | §9.46 GR11g ii 1, 2 | In strict mode an item that is no object with the member is 2203A, and for `.*` one that is no object 2203C. Members of one key raise no exception (IA071): `.key` takes the first (UA075), and `.*` gives the bound values of all (UA058) in the order written (US040). |
| Array accessors | §9.46 GR11g ii 3, 4 | An item that is no array is 22039. A subscript that is not one number is 22033; a fraction is truncated toward zero (IA073). In strict mode a subscript outside the array, or a range whose first is past its last, is 22033; in lax mode the elements there are are taken. `last` is the current array's last subscript. |
| Item methods | §9.46 GR11g ii 6 | type() names an item's kind, a datetime's by its type. size() is an array's elements or 1, in strict mode 22039 of what is no array. double() casts a number or a character string to DOUBLE PRECISION, 22036 of another item and 22018 of a string that is no number. ceiling(), floor() and abs() are of numbers alone, else 22036; ceiling() and floor() never give negative zero. datetime() reads a character string as a date, a time with and without time zone and a timestamp with and without time zone in turn, with time zone where it writes a displacement, of the precision of its fraction; else, or of another item, 22031, and a fraction of more than 6 digits is -900. datetime() of a template reads the string by it, as CAST with FORMAT does [§9.51], to the datetime type the template implies. A datetime of no time zone is taken in the SQL-session's time zone displacement as the path was parsed, with a template and without alike. keyvalue() gives an object of `key`, `value` and `id` for each member of each object, in the order kept, members of one key all (IA072, UA058); an item that is no object is 2203C. |
| keyvalue() id | §9.46 GR11g ii 6 C VIII 2 b | `id` uniquely identifies the object a member is of, as the rule asks (IA224): each item of the context item, then of the PASSING variables in order, is numbered in preorder from 0, and an object the path reaches there has its number however it is reached, so the members of one object share an id and two objects never do. An object the path makes, as of keyvalue() itself, is numbered past them, in the order it is reached. |
| Casting item methods | §9.46 GR11g ii 6 C IX to XIX | bigint(), integer(), decimal() and number() take numbers and character strings, else 22036; boolean() truth values and strings, else 2202V; date() strings, else 2202W; string() strings, numbers and truth values, else 2202X; time() and timestamp() strings, else 2202Y and 2202Z; time_tz() and timestamp_tz(), as printed, numbers and strings, else 2202Y and 2202Z. Each casts by CAST, its exception the path's: decimal() to DECIMAL of the precision and scale written, of none 18 and 0; string() to CHARACTER VARYING(32767) (IL006); the time and timestamp methods to the precision written, of none 0 and 6. number()'s type (IV186) is a number's own, so a number is kept as it is, and of a string that of the `<signed numeric literal>` it is. A datetime without time zone is taken in the SQL-session's time zone displacement as the path was parsed. |
| Arithmetic | §9.46 GR11h to GR11j, §6.31 SR12 | A sign is of numbers alone, else 2203B. An operand that is not one number is 22038. `%` is MOD, of whole exact or DECFLOAT numbers, a zero divisor 22012; a whole exact number of any scale, `4.0`, is taken. An approximate operand, or one with a fraction, breaks MOD's Syntax Rule, which the path checks only as it runs, so it is data exception 22000, an exception ON ERROR replaces. An exception of the arithmetic is the path's. |
| Filters and predicates | §9.46 GR11g ii 5, GR12 | A filter keeps the items its predicate is True of. Predicates are True, False or Unknown, an exception within one making it Unknown, a strict mode's structural error too. Character strings compare by code point. The SQL/JSON null is comparable with any item. Where one pair is in error and another succeeds, in lax mode, the result is True (UA073). A like_regex match the matcher gives up on, past its steps, is an error of its item. |
| Named variables | §9.46 SR4, GR11b | A variable's value is the item passed under its name, as the PASSING row above says. One that is not passed is SQLCODE -103, found before anything is evaluated, so no filter hides it. |

Where the text of §6.28, §9.45, §9.46 and §9.48 cannot be followed as
printed, it is read as Readings of ambiguous text says.

### Savepoints

Features T271 (Savepoints) and T272 (Enhanced savepoint management).
SAVEPOINT [§17.5], RELEASE SAVEPOINT [§17.6] and ROLLBACK TO SAVEPOINT
[§17.8] at the savepoint levels of §4.43.2 (RULES.md D88).

| Feature | Rules | What turborel does |
|---|---|---|
| SAVEPOINT | §17.5 GR1 to GR4, §4.41.2.4, IL208 | A savepoint of the name at the current level is destroyed, then one is established here. Past 1,024 in the transaction, 3B002, the statement failing alone. With no transaction active, it initiates one. |
| RELEASE SAVEPOINT | §17.6 GR2, GR3 | The savepoint of the name at the current level and every one established after it at that level are destroyed, the changes kept. A name of none, with no transaction active too, is 3B001. |
| ROLLBACK TO SAVEPOINT | §17.8 SR1, SR2, GR3a to GR3j, IA167, IA168, IA211 | AND CHAIN with it is a syntax error (2A000), AND NO CHAIN its default. A name of no savepoint of the current level is 3B001, as is one established before the most recent atomic execution context began. Every change to SQL-data since is undone, in tables, temporary tables, every index and large objects, through the same undo a failed statement uses, and every change of definitions staged since is cancelled. Later savepoints are destroyed, it stays. Locators given since are invalid, those before stay valid. Every open cursor is closed. The transaction goes on, holding every lock it took. |
| Savepoint levels | §4.43.2, §9.18 GR7, GR15, §11.60 SR9j to SR9l, GR3k, §15.21 GR6c, GR6j; ISO/IEC 9075-4:2006 draft §14.1 GR1, GR6 | A function, a procedure of NEW SAVEPOINT LEVEL, a triggered action and an ATOMIC compound statement each establish a level as they begin and destroy it with its savepoints as they end. A procedure of OLD SAVEPOINT LEVEL, written or implicit, runs at its caller's. A statement names only the savepoints of the current level. COMMIT and ROLLBACK destroy every savepoint [§17.7 GR7, §17.8 GR2c]. |
| In routines and triggers | §9.18 GR8b iv 2, §15.21 GR6e iii, IA046; ISO/IEC 9075-4:2006 draft §14.13 SR9 | A routine's body may hold the savepoint statements, so 2F003 is never raised. A FOR holds no ROLLBACK TO SAVEPOINT (2A000). A trigger's action that is one is refused as the trigger is defined (2A000). One run while a triggered action executes, in a procedure it calls, raises 0W000, which fails the action with 09000. |
| Atomic execution contexts | §4.41.5, §17.8 GR3c | A function, a triggered action and an ATOMIC compound statement are atomic execution contexts, a procedure's CALL is not. A savepoint statement of a program is not atomic itself, so ROLLBACK TO goes back past the start of the statement running. |
| Recovery | §17.8 GR3d | Going back logs no record of its own. The changes since the savepoint are marked undone as a failed statement's are, so recovery and a standby replaying the log leave out the rows and the changes of definitions cancelled. |
| ROUTINES | 11:2006 draft §5.49, §6.40 Description 21 | NEW_SAVEPOINT_LEVEL is YES of a function and of a procedure of NEW SAVEPOINT LEVEL, NO of another procedure. |

SAVEPOINT and RELEASE are no key words of SQL-92, so each stays an
identifier, read as a word where a savepoint statement reads it. A
ROLLBACK TO SAVEPOINT in a routine's body that would cancel a change of
definitions staged before the statement invoking the routine began is
refused, SQLCODE -900, as that statement runs on those definitions. Run
directly, it cancels them. The command codes are SAVEPOINT 63, RELEASE
SAVEPOINT 57 and ROLLBACK WORK 62 [§23.1 Table 39].

### Bracketed comments

Feature T351 (Bracketed comments) is claimed [§5.2 SR11, SR12]. A
`<bracketed comment>` is a separator, as a `<simple comment>` is, and
one begun within it nests, so each needs its own `*/`. One not ended is
SQLSTATE 2A000, SQLCODE -101. \[EXTENSION\] One that begins a statement
and whose contents begin with `+` holds hints, of which
VECTOR_WAIT_FRESH is read (RULES.md D105).

## SQL/PSM

Persistent stored modules, ISO/IEC 9075-4, written from its 2006 draft
(`5CD2-04-PSM-2006-01`), its clause numbers cited with that edition
(RULES.md D76, D77). Part 4 adds to Part 2's routines and triggers the
compound statement, local variables, conditions, cursors and handlers,
the control statements, SIGNAL and RESIGNAL, and SQL-server modules.

Subfeatures P002-01 to P002-15 of P002 (Computational completeness) are
claimed: the compound statement, the handler, condition and SQL variable
declarations, the assignment, CASE, IF, ITERATE, LEAVE, LOOP, REPEAT,
WHILE and FOR statements, and SIGNAL and RESIGNAL [Annex F Table 5].
P002 itself is not. A feature claimed is claimed with every subfeature
it has [ISO/IEC 9075-11:2006 draft §6.43 Description 7], and P002-16,
`<control statement>`s as the SQL-statement of an externally-invoked
procedure, needs the module language, which is not claimed (RULES.md
D32). So conformance to this part is not claimed either, as it requires
P002 [§21.1].

P001 (Stored modules), P004 (Extended CASE statement), P005 (Qualified
SQL variable references), P006 (Multiple assignment) and P007 (Enhanced
diagnostics management) are implemented, and P003 (Information Schema
views) as its four views, P003-01 to P003-04; its short name views
MODULES_S and MODULE_COL_USAGE [§18.8] are not. None of them is
claimed: each implies P002, P003 through P001 [§21.3 Table 4], and a
feature is claimed only with the features it implies [ISO/IEC
9075-2:2023 §25.3]. P008 (Comma-separated predicates in simple CASE
statement), which implies none, is claimed.

| Feature | Rules | What turborel does |
|---|---|---|
| Where a program stands | §4.10.7, §9.19, §11.2, §4.10.6 | An SQL control statement is a routine's body, a trigger's action, a statement of another one, or a statement run directly, alone a procedure of no parameters. A compound statement and the other control statements are not preparable, SQLCODE -103, but SIGNAL and RESIGNAL are. |
| COMMIT and ROLLBACK | §14.1 SR6, §14.13 SR9, §4.10.4; 2023 §11.60 SR23f | In an ATOMIC compound statement or a FOR, SQLCODE -103 [SR6, SR9], but ROLLBACK TO SAVEPOINT in an ATOMIC one, which goes back to a savepoint established in it [2023 §17.8 GR3c]. In a routine's body, SQLCODE -103 (IA078), but ROLLBACK TO SAVEPOINT. In a control statement NOT ATOMIC run directly, a non-atomic execution context, they end the transaction, its cursors closed [2023 §17.7 GR3], and the statements after run in a new one, begun as the first needs one. In a handler's action while an ATOMIC compound statement runs, an atomic execution context, SQLSTATE 2D000 [2023 §17.7 GR2, §17.8 GR2]. A statement after one that changes a definition sees it. |
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
| Views of modules | §18.1 to §18.4, §18.6, §18.7, §19.1 to §19.4; ISO/IEC 9075-11:2006 draft §5.47, §6.40 | MODULES, MODULE_PRIVILEGES, MODULE_TABLE_USAGE, MODULE_COLUMN_USAGE and ROLE_MODULE_GRANTS, with the draft's definitions, over the Definition Schema's MODULES, MODULE_PRIVILEGES, MODULE_TABLE_USAGE and MODULE_COLUMN_USAGE. ROUTINES gains MODULE_CATALOG, MODULE_SCHEMA and MODULE_NAME, and shows a module's routine to who holds EXECUTE on the module [§18.7], PARAMETERS following, as it reads ROUTINES. MODULE_AUTHORIZATION is null, as a server module has no `<module authorization clause>` [§19.4 Description 5]. CREATED and LAST_ALTERED are the CURRENT_TIMESTAMP of its CREATE MODULE, the same, as no statement alters a module [§19.4 Description 7, 8] (Feature T011). MODULES shows them as MODULE_CREATED and MODULE_LAST_ALTERED, the names its Conformance Rules give them. The usage views show a table or column as its routines' bodies bind now, not the module's own declared tables. |
| Short name views of modules | §18.8 | MODULE_COL_USAGE and MODULES_S, the draft's definitions over MODULE_COLUMN_USAGE and MODULES, each column named by an `<as clause>` as the draft's `<view column list>` names it. Corrected: MODULE_COL_USAGE, which has no `<from clause>` in the draft, reads MODULE_COLUMN_USAGE, whose columns it names; MODULES_S's DEFAULT_SCHEMA reads MODULES's DEFAULT_SCHEMA_NAME [§18.4]. |

The words of Part 4 that are no key words of SQL-92, IF, ELSEIF, LOOP,
WHILE, REPEAT, UNTIL, LEAVE, ITERATE, DO, HANDLER, EXIT, UNDO, SIGNAL,
RESIGNAL, SQLEXCEPTION, SQLWARNING, CONDITION and STACKED, stay
identifiers, read as words where a program reads them. Those Part 4
reserves are never a correlation name written without AS.


## SQL/XML

XML-related specifications, ISO/IEC 9075-14, written from its 2006 draft
(`5CD2-14-XML-2006-01`), its clause numbers cited with that edition, and
from the W3C texts the draft names, at the editions it names (RULES.md
D106). Part 14 adds the XML type, whose values are XQuery sequences, and
the operators that make them and write them.

Features X010 (XML type), X016 (Persistent XML values), X020
(XMLConcat), X025 (XMLCast), X030 (XMLDocument), X031 (XMLElement), X032
(XMLForest), X034 (XMLAgg), X035 (XMLAgg: ORDER BY option), X036
(XMLComment), X037 (XMLPI), X038 (XMLText), X040 to X049 (Basic table
mapping, with nulls absent and as nil, a table as a forest and as an
element, a target namespace, data and metadata, and base64 and hex
encodings of binary strings), X050 to X059 (Advanced table mapping, of
schemas and the catalog, with the same), X060 and X061 (XMLParse:
character string input and
CONTENT, DOCUMENT option), X065 and X066 (XMLParse: BLOB input and
CONTENT, DOCUMENT option), X070 to X072 (XMLSerialize: character string
serialization), X073 to X075 (XMLSerialize: BLOB serialization), X076
(XMLSerialize: VERSION), X077 (XMLSerialize: explicit ENCODING option),
X078 (XMLSerialize: explicit XML declaration), X080 (Namespaces in XML
publishing), X081 to X084 (Query-level XML namespace declarations, XML
namespace declarations in DML, in DDL and in compound statements), X085
(Predefined namespace prefixes), X090 and X091 (XML
document and content predicates), X170 and X171 (XML null handling
options, NIL ON NO CONTENT option), X181 and X182 (XML(DOCUMENT(UNTYPED)) and
XML(DOCUMENT(ANY)) types), X131 to X135 (Query-level XMLBINARY clause,
XMLBINARY clause in DML, in DDL, in compound statements and in
subqueries), X190 (XML(SEQUENCE) type), X211 (XML 1.1
support), X231 and X232 (XML(CONTENT(UNTYPED)) and XML(CONTENT(ANY))
types), X160 and X161 (Basic and Advanced Information Schema for
registered XML Schemas), their short name views XML_SCHEMAS_S,
XML_SCH_NSPACES_S and XML_SCH_ELEMENTS_S among them [§20.15 CR1 to
CR4], X241 and X242 (RETURNING CONTENT and RETURNING SEQUENCE in XML
publishing), X251 to X255 (Persistent XML values of each XML type), X011
(Arrays of XML type), X012 (Multisets of XML type), X013 (Distinct types
of XML type), X014 (Attributes of XML type), X015 (Fields of XML type) and
X120 (XML parameters in SQL routines), X086 (XML namespace declarations in
XMLTable), X096 (XMLExists), X200 to X206 (XMLQuery, its RETURNING CONTENT
and SEQUENCE, a context item, variables, EMPTY ON EMPTY and NULL ON
EMPTY), X221 and X222 (XML passing mechanism BY VALUE and BY REF) and X300
to X305 (XMLTable, its derived column list, ordinality column, column
defaults, context item and variables), X191 and X192
(XML(DOCUMENT(XMLSCHEMA)) and XML(CONTENT(XMLSCHEMA)) types), with X260,
X261, X263 and X264 (their ELEMENT clause, NAMESPACE without ELEMENT, NO
NAMESPACE with ELEMENT and schema location), X256 and X257 (Persistent
XML values of them), X271 to X274 (XMLValidate: data-driven case,
ACCORDING TO clause, ELEMENT clause, schema location), X281 to X284 and
X286 (XMLValidate with DOCUMENT, CONTENT and SEQUENCE options, NAMESPACE
without ELEMENT, NO NAMESPACE with ELEMENT), and X141 to X145, X151 to
X153, X155 and X157 (IS VALID: data-driven case, ACCORDING TO clause,
ELEMENT clause, schema location, outside check constraints, with
DOCUMENT, CONTENT and SEQUENCE options, NAMESPACE without ELEMENT, NO
NAMESPACE with ELEMENT), and X400 (Name and identifier mapping), by
which an application invokes the mapping of XML Names to identifiers
[§9.3], are claimed. X121 (XML parameters in
external routines) is not, as turborel has no external routine. Every
expression of XQuery 1.0 is evaluated. Every function of XQuery 1.0
Functions and Operators is built in, with the optional parts the rows
below say. Its Minimal Conformance [XQuery
§5.1] is claimed: all of XQuery but its optional features, the
implementation-defined items each defined below, the data model, and
every function of XQuery 1.0 Functions and Operators. Of
its optional features [XQuery §5.2] [§6.18 SR6 NOTE 28] the Full Axis
Feature, the Schema Import Feature [§5.2.1], the Schema Validation
Feature [§5.2.2], the Static Typing Feature [§5.2.3], in effect as
XQUERY.STATIC_TYPING or a query's option asks, the Module Feature
[§5.2.5] and the Serialization Feature [§5.2.6] are supported,
importing and validating by registered XML Schemas, importing
registered XQuery modules, and serializing a query's result by
XMLSERIALIZE with the parameters of [XQuery Appendix C.3].

| Feature | Rules | What turborel does |
|---|---|---|
| The XML types | §4.2.2, §6.1 SR1 to SR4, GR1 | XML(DOCUMENT), XML(CONTENT), each UNTYPED, ANY or XMLSCHEMA, and XML(SEQUENCE). XML alone is XML(SEQUENCE), and DOCUMENT or CONTENT alone is ANY [SR2, SR3b]. A type of XMLSCHEMA's descriptor holds the indicated registered XML Schema, by its name [§4.2.6], and the indicated namespace and global element declaration's NCName, if any [SR4, GR1d to GR1f]; it is written with ID and the full name, kept in the catalog and described over a connection with them. |
| What a definition names | §4.2.6 | The registered XML Schema an `<XML valid according to what>` of a view, a CHECK, a domain constraint, an assertion, a trigger or a routine indicates is the one it indicated as it was created, kept with the definition and taken whenever it is used, invoked or evaluated; the definition depends on it (RULES.md D106). So are the in-scope schema definitions of a validate expression of its query [§10.20 SR2e], and \[EXTENSION\] the registered XQuery modules its queries import [XQuery §4.11]: DROP XQUERY MODULE RESTRICT is refused while one does, CASCADE drops it, a generated column refusing even CASCADE. |
| `<XML valid according to clause>` | §11.6 SR1 to SR4, AR1, GR1 | ID a `<registered XML Schema name>` [SR1]; URI a target namespace, NO NAMESPACE the zero-length one, LOCATION a schema location too, of the registered XML Schemas the user may use, the one the choice below takes [SR2b ii]; of none, SQLCODE -103 [SR2c]. The element clause's namespace, else the target namespace [SR4a], and its ELEMENT, an `<identifier>`'s text, must be the registered XML Schema's, else SQLSTATE 2201M or 2201N as the clause is bound [SR4b, SR4d iii, GR1]. Its user, or of an SQL schema statement not under SQL SECURITY INVOKER its schema's owner, may use it [AR1] (`catalog::xml_schemas::may_use`), else SQLCODE -104. One permanently registered is SQLCODE -900, as turborel holds none of its components. |
| XMLVALIDATE | §6.20, §10.22 | DOCUMENT, CONTENT or SEQUENCE of an XML value [§6.9 SR1], of the type SR3 gives: of a DOCUMENT or CONTENT type, of XMLSCHEMA of what the clause indicates, else ANY; of XML(SEQUENCE), XML(SEQUENCE). The null value of the null value [GR2]; DOCUMENT of no document SQLSTATE 2200M, CONTENT of no one document node 2200N, an atomic value, attribute or text node 2201J [GR3]; the empty sequence of the empty sequence [GR4]. Each element is validated by §10.22, XQuery's `validate strict` (`xml::schema::validate`): of DOCUMENT the document node, its identity constraints and IDs checked; else each element child of a document node, and each element, alone [GR6a iii 2 C, GR6b iii]. Its registered XML Schema is the clause's, else its namespace's the user may use, as the choice below takes, none 2201L [GR6a iii 2 A, GR6b i]; an element not of the element clause's QName 2201P, namespace 2201Q [B, ii]; one not valid 2201R [D, iv]; a document node's child no element, comment or processing instruction 2201K [GR6a iii 1]. Comments and processing instructions copied [§10.18] [GR6a iii 3, GR6c]. The nodes made are typed, as XQuery's data model types them [XQueryDM §3.3]. |
| IS VALID | §8.5, §6.4 | Of one XML value [SR1], IS NOT its negation [SR3]; a `<when operand>` of a simple CASE. Unknown of the null value [GR2]; False of a value not of the shape DCS asks [GR3 to GR5b], True of an empty sequence of SEQUENCE [GR5a]; else True AND each item's truth: an element of no registered XML Schema found Unknown [GR7a iii 3 B I, GR7b ii 1], one of another QName or namespace than the element clause's, or not valid by §10.22, False [GR7a iii 1, 3 B II, GR7b ii 2], a comment or processing instruction True [GR7c]. Anywhere a predicate is, a CHECK too. |
| XML values | §4.2.3 | An XQuery sequence of nodes and atomic values. A value held in a column is its stored form (RULES.md D106): in the row when short, else as the table's large objects, logged and recovered as large objects are. One inside an array, a multiset, a row or a structured value is held whole in that value's row, its stored form at most 64 MiB, else SQLCODE -900. |
| Assignment | §4.2.4, §10.1 SR1, GR1, §10.2 SR1, GR1 | An XML value to an XML site alone, an UNTYPED one's of an UNTYPED type, one of XMLSCHEMA of a type of XMLSCHEMA of the same registered XML Schema, of the namespace and the element the site's includes, if any [§10.2 SR1b ii], else SQLCODE -103. A DOCUMENT site takes a document, else SQLSTATE 2200L; a CONTENT site one document node, else 2200U. A value stored is a copy. |
| XML within other types | §6.1 CR2, CR3, §6.2 CR1, §12.5 CR1, §12.6 CR1, §12.7 CR1, §10.4 SR1, §10.5 SR1 | An array or multiset of an XML type or of a distinct type of one; a distinct type whose source is an XML type, its function to its source named XML [§10.5 SR1], of no ordering; an attribute of a structured type, and a field of a row type, of either; a parameter or result of an SQL routine of any of them. Each is stored, read, assigned and cast part by part, an XML part checked as §10.2 GR1a and §6.5 GR1a check one. Every XML type's type precedence list is XML [§10.4 SR1], so a routine of an XML parameter takes an argument of any XML type that is assigned to it. |
| Not comparable | §4.2.4, §10.8 SR1 to §10.11 SR1 | No equality, grouping or ordering operation takes an XML value: comparisons, IN, quantified comparisons, MATCH, UNIQUE, DISTINCT, IS DISTINCT FROM, NULLIF, a simple CASE, GROUP BY, ORDER BY, set operations but UNION ALL, MIN and MAX, a NATURAL or USING join's common column, keys, and indexes but the XML methods' ([Indexes](#indexes)), SQLCODE -103. |
| Types brought together | §10.3 SR1 | XML types with XML types alone, of the type SR1b gives: SEQUENCE if one is, the one type, DOCUMENT(UNTYPED) if all are, CONTENT(UNTYPED) if all are UNTYPED; of XMLSCHEMA if all are, of one registered XML Schema, of the namespace and the element all include, DOCUMENT if all are [III to VIII]; else CONTENT(ANY) [IX]. |
| IS DOCUMENT, IS CONTENT | §8.2, §8.3, §6.4 | Of one XML value, else SQLCODE -103 [SR1]. IS DOCUMENT is True of one document node whose children are one element and comments and processing instructions, IS CONTENT of one document node; Unknown of the null value; else False [GR2]. IS NOT is the negation [SR2]. Either is a `<when operand>` of a simple CASE. |
| XMLCAST | §6.6, §9.5 GR8, §9.8 | Of or to an XML type [SR4], neither of nor to a collection, row, structured or reference type [SR5, SR10b i]; a passing mechanism of XML types alone [SR9]; NULL and `?` CAST to an XML type [SR10a, SR11]; to a domain, its data type [SR3a]. Of XML to XML, CAST [SR10b ii]. Of an SQL value to XML(CONTENT), XMLPARSE of its lexical form by §9.8, `&`, `<`, `>` and carriage returns mapped, a character no XML 1.1 character SQLSTATE 0N002 [GR3a to GR3c]; to XML(SEQUENCE), that cast as the built-in XML Schema type §9.5 maps its type to, one atomic value, of a binary string `xs:hexBinary` or, in the scope of XMLBINARY USING BASE64, `xs:base64Binary` [GR3d, §9.5 GR8b ii]; to XML(DOCUMENT), SQLCODE -103 [SR7]. Of an XML value to an SQL type: document nodes replaced by their children, atomized, none the null value, more than one SQLSTATE 10000 [GR4b to GR4d]; a date or time adjusted to UTC for a type without time zone [GR4j i to iii]; cast as the type §9.5 maps the SQL type to, restricted by its facets: a CHARACTER's length, a CHARACTER VARYING's, a CLOB's and a binary string's maxLength, a binary string `xs:hexBinary` or, in the scope of XMLBINARY USING BASE64, `xs:base64Binary` [§9.5 GR8b ii], a NUMERIC's or DECIMAL's totalDigits and fractionDigits, an INTEGER's, SMALLINT's or BIGINT's bounds, a datetime's or interval's pattern [§9.5 GR8a to GR8l]; a value not of its lexical forms, or one a facet refuses, 10000 [GR4e, GR4j]; INF, -INF and NaN 22003 [GR4k v]; a year past 0001 to 9999 in UTC 22007 [GR4k vi to viii]; then CAST to the SQL type [GR4l]. A time or timestamp is checked against TIME(p)'s or TIMESTAMP(p)'s pattern as the choice below says, so each value of the SQL type is taken and one of more than p digits of fraction is not. The types mapped are the character and binary strings, large objects too, the exact and approximate numbers, BOOLEAN, the datetimes and the intervals; DECFLOAT, which §9.5 does not map, SQLCODE -103. |
| XMLDOCUMENT | §6.13, §11.4 | Of an XML value [§6.9 SR1]: XQuery's `document { $EXPR }`, a new document node of copies of the value's nodes, a document node's children in its place, adjacent atomic values one text node a space apart, an attribute SQLSTATE 10000, XQuery error [GR8]. Construction mode preserve keeps the ID and IDREFS properties [GR5]; an element copied from below another declares the bindings in its scope. The null value of the null value [GR2]. RETURNING CONTENT or SEQUENCE [SR3]; of XML(CONTENT(UNTYPED)), the document node unvalidated, its elements `xdt:untyped`, its attributes `xdt:untypedAtomic` [GR9a, §10.19]. |
| XMLELEMENT | §6.14, §10.13 | An element of the name, a QName whose prefix XMLNAMESPACES or the predefined prefixes bind [SR5, SR6]; its attributes those of XMLATTRIBUTES whose values are not null, each named by AS or its column's name mapped to an XML Name fully escaped [§9.1] [SR3d], none `xmlns` nor of its prefix, none twice [SR3f, SR4], of a type §9.5 maps, not XML [SR3c], its value mapped by §9.8 [GR3]; its content each value not null, an XML value's nodes copied, another's §9.8 text parsed as CONTENT [§10.13 GR4c i]; XQuery's `element { $QNAME } { $SEQ }` [GR4c viii]: atomic values a text node a space apart, an attribute after other content or two attributes of one name SQLSTATE 10000 [XQuery §3.7.1.3], each name's namespace declared, a prefix bound to another URI changed [XQuery §3.7.4]. NULL ON NULL, EMPTY ON NULL, implicit, ABSENT ON NULL, NIL ON NULL and NIL ON NO CONTENT [SR8, §10.13 GR4]; no attribute `xsi:nil` with content [SR9]. Content of a collection type, or a distinct type, not based on an XML unmappable type, XML(SEQUENCE), a structured or a reference type [§4.10.8], nor on a type §9.5 maps to nothing, else SQLCODE -103; a row type, none of SR7's types, SQLCODE -103 [SR7]. |
| Collections and rows mapped | §9.8 GR1a, GR9i to GR9k | An array or a multiset an element `element` of each element, of a null one `<element xsi:nil="true"/>` [GR9j], a multiset's in the order it holds them; a row an element of each field not null, named by its name mapped to an XML Name fully escaped [GR9i, §9.1], NULLS absent as every caller gives it [§6.6 GR3a, §10.13 GR4c i 1 B]; an XML value its serialization as CONTENT [GR9k]; a distinct type's value its source's [GR1a]; a datetime without time zone in the session's displacement [GR7]. Values nested deeper than types nest, SQLCODE -900. XMLELEMENT's and XMLFOREST's content, and XMLCAST's operand of a distinct type whose source is a collection type, to XML(CONTENT) [§6.6 SR10b i, GR3c]; to XML(SEQUENCE), SQLCODE -900, as GR3d casts to an atomic type and §9.5 maps no collection to one. XMLCAST of a collection or a row type itself is refused [SR5, SR10b i]. |
| XMLFOREST | §6.15 | An element of each value, named as XMLELEMENT's attributes are [SR2c], §10.13's of no attribute, NULL ON NULL implicit [SR3], the elements concatenated [§10.14] [GR5 to GR7]. |
| XMLNAMESPACES | §11.3, §10.12 | In XMLELEMENT and XMLFOREST, in scope within it, a subquery of it too [§6.14 SR1, §6.15 SR1]; among a scope's options (below). At most one default [SR2]; each prefix an XML 1.1 NCName, once, not `xml` nor `xmlns`, of a URI not empty [SR3 to SR5, SR7]; no URI the namespace of `xml` or `xmlns` [SR6]. A prefix is the innermost declaration's, else `xml`, `sqlxml`, `xs` or `xsi`'s, else SQLCODE -103; a name of no prefix is in the innermost default namespace, NO DEFAULT none [§10.12 SR2]. |
| XML lexically scoped options | §11.3, §7.2, §12.1, §12.2, §12.4, §14.3 to §14.7, §15.1 | XMLNAMESPACES and XMLBINARY \[USING\] BASE64 or HEX, at most one of each [§11.3 SR1]. Of a `<with clause>`, alone or before its `<with list>`, a comma between them and none else [§7.2 SR1, SR4], in scope of its `<query expression>`: its subqueries, derived tables, `<with list element>`s and set operations' operands, a subquery's own XMLBINARY too [SR2, SR3, CR3]. Of DELETE searched, INSERT, MERGE and UPDATE positioned and searched after WITH, in scope of the statement [§14.3 to §14.7 SR1, SR2]; prepared too; not of a positioned DELETE nor of a dynamic cursor's UPDATE, whose productions §14 does not modify. Of a generation expression, a CHECK, of a domain too, and an assertion, in scope of it, kept with its text, so read again the same, after the database is opened again too [§12.1, §12.2, §12.4 SR1, SR2]; CHECK_CLAUSE shows the `<search condition>` alone [ISO/IEC 9075-11:2006 draft §6.14 Description 2a]. Of DECLARE in a compound statement, in scope of all it holds, a trigger's action and a routine's body too [§15.1 SR1, SR2]. XMLNAMESPACES is of XMLELEMENT's and XMLFOREST's names and of an XQuery's statically known namespaces and default element namespace [§10.12, §10.20 SR2d]. The innermost declaration of a prefix, the default namespace or the encoding is taken: an XMLELEMENT's or XMLFOREST's, then each `<with clause>`'s from the inside out, then the statement's or the definition's, then each compound statement's [§10.12 SR2a ii 2 A, §6.6 SR1a]. A view's query and a routine's body see none of what reads or invokes it. XMLBINARY is of XMLCAST, XMLELEMENT, XMLFOREST and XMLTEXT [§6.6 SR1, §6.14 GR1, §6.15 GR1, §6.19 GR3]; XMLPI writes hex [§6.17 GR3]. |
| XMLCONCAT | §6.12, §10.14 | Of two XML values or more: every item of each not null, in order; the null value if each is [GR1 to GR4]. RETURNING CONTENT is of XML(CONTENT(UNTYPED)) of DOCUMENT(UNTYPED) and CONTENT(UNTYPED) values, of XML(CONTENT(XMLSCHEMA)) of what values of XMLSCHEMA of one registered XML Schema have in common, else XML(CONTENT(ANY)) [SR2a]; its document node is made of that type, so the nodes' types are kept as SR4's cast to XMLSCHEMA needs. |
| XMLCOMMENT, XMLPI, XMLTEXT | §6.11, §6.17, §6.19 | Of a character string: a comment, holding no two hyphens together nor a hyphen last, else 2200S [§6.11 GR3]; a processing instruction of a target, an XML 1.1 NCName not `xml` in any case [§6.17 SR2, SR3], its text holding no `?>`, else 2200T [GR4], its leading white space removed [XQuery §3.7.3.5]; a text node, empty of a zero-length string [XQuery §3.7.3.4]. A character no XML 1.1 character is 2200S of XMLCOMMENT, else 0N002 [§9.8 GR9a ii]. The null value of the null value. |
| XMLAGG | §11.2 | A set function of an XML value [§6.9 SR1], ordered by a `<sort specification list>` whose keys an ordering operation takes, else SQLCODE -103: the values of the group not null, concatenated [§10.14] in the keys' order, stably; the null value of none [GR2]. RETURNING CONTENT is of XML(CONTENT(UNTYPED)) of an UNTYPED value, else XML(CONTENT(ANY)) [SR2a], made by `document { $EXPR }` within the aggregate, so it is one aggregate over a window too [SR5]. Values holding more than 64 MiB together, SQLSTATE 2200R [GR1]. |
| RETURNING in publishing | §6.11 SR3, §6.12 SR4, §6.14 SR16, §6.15 SR8, §6.17 SR7, §6.19 SR3, §11.2 SR2, SR5 | RETURNING CONTENT is XMLDOCUMENT of the value made RETURNING CONTENT, of XML(CONTENT(UNTYPED)); RETURNING SEQUENCE the value made, of XML(SEQUENCE). |
| CAST | §6.5 SR1 to SR4, GR1 | Of an XML type to an XML type alone [SR3]. BY REF is implicit [SR2], to an UNTYPED type of an UNTYPED one [SR4a], to one of XMLSCHEMA of one assignable to it [SR4b]. To a DOCUMENT or CONTENT type, as assignment checks it [GR1a]. BY VALUE to a type of XMLSCHEMA is XMLVALIDATE of DOCUMENT or CONTENT according to it [GR1b i 1]; to ANY or SEQUENCE a copy [§10.18] [GR1b i 2]; to UNTYPED an unvalidated document node [§10.19] [GR1b i 3]. |
| XMLPARSE | §6.16, §10.16 | Of a character string or a binary string, large objects too. DOCUMENT parses a textual XML 1.0 or XML 1.1 document, else 2200M; CONTENT a document or an external parsed entity, else 2200N [GR6a]. The internal DTD subset's entities are expanded and its attribute defaults applied [GR7c, GR7d]. STRIP WHITESPACE removes text of white space alone by `xml:space` [GR8]. Its type is XML(DOCUMENT(UNTYPED)) or XML(CONTENT(UNTYPED)) [SR1]. |
| XMLSERIALIZE | §6.8, §10.15 | To a character or a binary string, by the XML output method of XSLT 2.0 and XQuery 1.0 Serialization, after its sequence normalization. CONTENT if neither is written. DOCUMENT of a document alone, else 2200L. VERSION '1.0' or '1.1'. A binary string in UTF8 or UTF16. INCLUDING and EXCLUDING XMLDECLARATION. The result is stored in its type as assignment stores it, too long 22001. A value with no serialization, an attribute alone among others, is 2200W. |
| Over a connection | RULES.md D106 | A column of an XML type is described with its modifiers. A value is sent as its serialization as CONTENT, refused with 2200W if it has none, and a client parses it as CONTENT when it needs its nodes. A value sent as a parameter is kept as sent. |
| USAGE on registered XML Schemas | §4.7.1, §11.6 AR1 | Granted and revoked by GRANT and REVOKE USAGE ON XML SCHEMA, as on a sequence generator: USAGE or ALL PRIVILEGES alone, else SQLCODE -103; its schema's owner holds it with grant option; a grant needs it held grantably, else warning 01007, and none held is -104; REVOKE takes the grants named or their grant option, with the grants they abandon and, of each schema whose owner it leaves without USAGE, what names it as DROP XML SCHEMA finds it (registered XML Schemas using it; views, constraints, assertions, triggers and routines indicating it), dropped as DROP XML SCHEMA CASCADE drops them, RESTRICT refusing that with SQLSTATE 2B000; a column, domain or user-defined type of an XML type of it refuses even CASCADE, 2B000, as DROP XML SCHEMA refuses it. CREATE XML SCHEMA's library is the registered XML Schemas its schema's owner may use, by these grants [§11.6 AR1a]. `catalog::xml_schemas::may_use` is §11.6 AR1's check, for the statements that name one. |
| Descriptor areas | §17.1 GR1, Table 12 | XML is code 137. An item of it matches any XML type. |
| XMLQUERY | §6.18, §11.5 | A query of a `<character string literal>`, parsed by XML 1.1's lexical rules [GR2a] and analysed as the statement is bound in the static context §10.20 makes [SR5, SR6]. PASSING and its default mechanism, then each argument a `<value expression>`, the context item at most once [SR5b i], a variable AS an XML 1.1 NCName, none twice [SR5a i, ii]; an XML value BY REF its nodes, BY VALUE a copy [§10.18]; another value XMLCAST to XML(SEQUENCE), of no mechanism [SR5a iv, v, SR5b iii, iv]; else SQLCODE -103. A null context item makes the null value [GR1a ii 1], a null variable the empty sequence [GR1b i 1], a context item of more than one item SQLSTATE 2200V [GR1a ii 2 B III]. RETURNING CONTENT, of no returning mechanism [SR4a], is XMLDOCUMENT of the value RETURNING SEQUENCE BY VALUE [SR10a]; RETURNING SEQUENCE of its mechanism or the default passing mechanism, else SQLCODE -103 [SR4b, SR4c], of XML(SEQUENCE). NULL ON EMPTY gives the null value of the empty sequence [GR4a]; a returning mechanism BY VALUE a copy [GR4b]. Every XQuery error, static, type or dynamic, is SQLSTATE 10000, its message naming the error's QName [GR3]. |
| XMLEXISTS | §8.4 | A boolean value of the arguments XMLQUERY takes: True if the query's value is not empty, False if it is, Unknown of a null context item [GR2]; read where a `<boolean predicand>` is, so a search condition and a select list both take it. Of no argument list, its value is only tested, so no returning mechanism is needed [SR3, §6.18 SR4c]. |
| XMLTABLE | §7.1 | A `<table primary>` with its correlation name, a derived column list optional, its arguments those of XMLQUERY, BY REF of none [SR4c ii]; lateral, as XMLITERATE of its row pattern's value RETURNING SEQUENCE BY REF EMPTY ON EMPTY [SR4h]: a row for each item, none of the null value or the empty sequence [GR1]. Its XMLNAMESPACES are in scope of its patterns [SR4a]. One FOR ORDINALITY column at most, of the item's place from 1, exact numeric of scale 0 [SR4d]. Each regular column's pattern is its PATH, else its name [SR4e ii 4], evaluated with the item as context item: of an empty value its default, else null [SR4e ii 3, 8]; of XML(SEQUENCE) the value, by the column's mechanism, else the argument list's, else SQLCODE -103 [SR4e ii 2 A]; of another type the value made a document node, which an attribute of the value fails with SQLSTATE 10000 [§6.18 SR10a, §6.13 GR8], then XMLCAST to the type, BY VALUE of an XML type [SR4e ii 7]; a mechanism of such a column SQLCODE -103 [SR4e ii 2 B]. Two columns of one name SQLCODE -103. |
| XQuery: static context | XQuery §2.1.1, Appendix C.1, §4; §10.20 SR2 | The predeclared namespaces `xml`, `xs`, `xsi`, `fn`, `xdt`, `local`, and `sqlxml` [SR2b], those the `<XML namespace declaration>`s in scope declare overriding them and their default the default element namespace [SR2d], then the prolog's. Every QName resolved: an unbound prefix err:XPST0081, of a name test err:XPST0008 [XQuery §3.2.1.2]; a variable not in scope err:XPST0008; a function of no signature of its name and arity err:XPST0017; an atomic type not in scope err:XPST0051, a cast to none err:XPST0080. A prolog's setters, namespace and default namespace declarations, variables (a variable declared twice err:XQST0049, depending on itself through functions err:XQST0054), functions (in no namespace err:XQST0060, a reserved one err:XQST0045, twice err:XQST0034, recursive), options (`static-typing` of `urn:turborel:option`, others ignored, their names resolved). A pragma is recognized of none, so an extension expression of no expression in braces is err:XQST0079. A module import imports a library module of its target namespace, its functions and variables, each module of a query analysed in a static context of its own [XQuery §4.11]: a namespace imported twice err:XQST0047, one of no module err:XQST0059, an imported function or variable declared again err:XQST0034 or err:XQST0049, a library module's declaration outside its target namespace err:XQST0048, one of a schema type the importing module does not import err:XQST0036; a library module given as a query err:XPST0003. The in-scope schema definitions are registered XML Schemas the current user may use [SR2e], which validate expressions validate by, and those a schema import imports [XQuery §4.10], whose type definitions, element declarations and attribute declarations name sequence types [XQuery §2.5.4.3 to §2.5.4.6]; a definition's, those in scope or imported as it was created [§4.2.6]. |
| XQuery: static typing | XQuery §5.2.3, §2.2.3.1, §2.3.1; XQueryFS §3.5.4, §4, §5.14, §5.15, §7, §8, Appendix B.2; §6.18 SR6, §10.21 | In effect for a query if XQUERY.STATIC_TYPING is ON for the statement preparing it, or its prolog declares the option `static-typing` "on" of `urn:turborel:option` [XQuery §4.16] (`xquery/statics.rs`). Each expression is given the static type of XQuery 1.0 and XPath 2.0 Formal Semantics, the Candidate Recommendation of 3 November 2005, by its normalization's typing rules: literals, variables, paths by the axes and node tests over the types the in-scope schema definitions expand to [XQueryFS §8.1.9, §8.2], arithmetic and comparisons after atomization and `fs:convert-operand` by the operator mapping, logic and conditions by the effective boolean value, FLWOR, quantified, typeswitch, instance of, treat, cast and castable, constructors, validate, and function calls by their signatures and [XQueryFS §7.2]'s rules, a declared function's body against its result type. An operand of a type not fit for it is err:XPTY0004, or the type error XQuery names (err:XPTY0018, XPTY0019, XPTY0020, XQTY0024, XQTY0030); an expression of the empty type but `()` and `fn:data(())` err:XPST0005; the context item where none is passed, or in a function body, err:XPST0001; each raised as the statement is prepared, SQLSTATE 10000 [SR6, GR3]. A value passed is of its formal type [§10.21]. The library modules a query imports are typed with it, each function body and variable in its own module's static context, a library module's variable of no focus, and a call of an imported function by its declaration [XQuery §4.11]; a library module is not typed as it is registered. A definition is typed as it is made, as XQUERY.STATIC_TYPING of the session making it or its option asks, and keeps that typing whenever it is bound again. |
| XQuery: evaluation | XQuery §2.4, §3, Appendix B | Paths on all twelve axes, each step's nodes in document order with duplicates removed, a reverse axis's positions from the context node; numeric and boolean predicates; sequences, ranges, union, intersect and except; arithmetic and comparisons by Appendix B.2's operator mapping, its operands atomized, a node to its typed value, of an element or attribute a validation typed the value its type annotation reads [XQueryDM §3.3.1.2], and promoted [B.1]: numbers, strings by collation, booleans, dates, times and the two durations ordered, Gregorian values, `xs:duration`, binaries and QNames equal or not; general comparisons converting untyped values [§3.5.2]; node comparisons; logic by effective boolean value; FLWOR with positional variables, where and order by (stable, empty least or greatest, by collation); quantified, conditional and typeswitch expressions; instance of, treat, cast, castable and the constructor functions by [XQueryFO §17]; validate, strict and lax, by the in-scope schema definitions [§10.20 SR2e], err:XQTY0030, err:XQDY0061, err:XQDY0084 and err:XQDY0027 [§3.13]; direct and computed constructors, boundary whitespace stripped by default [§3.7.1.4], each node copied by the construction and copy-namespaces modes, each name's namespace declared, a prefix taken renamed [§3.7.4]; ordered and unordered expressions; prolog variables evaluated once, as first read; functions with their arguments and results converted by the function conversion rules [§3.1.5]. |
| XQuery: built-in functions | XQueryFO §2 to §16 | Every function of [XQueryFO], of each number of arguments its signatures take (`xquery/library.rs`, one file of each section): the accessors, fn:error and fn:trace, fn:dateTime, the functions on numbers, strings, URIs, booleans, durations, dates and times, QNames, nodes and sequences, and the context functions; each converts its arguments by its signature's function conversion rules [XQuery §3.1.5]. A constructor function of each atomic type is its cast [XQuery §3.12.5]. The operators' functions [XQueryFO §1.2] back up XQuery's arithmetic, comparisons, ranges, node comparisons, union, intersect and except [XQuery Appendix B.2]. \[EXTENSION\] A plug-in registers more (`Database::register_xquery_function`), called by name and arity or declared `external` [XQuery §4.15]. |
| Mappings of tables, schemas and catalogs | §4.10.8 to §4.10.11, §9.4 to §9.17 | \[EXTENSION\] Through XMLMAP ([XMLMAP](#xmlmap)), as the draft gives them no syntax [§4.10.9 NOTE 13]. TABLE, SCHEMA and CATALOG map to the data [§9.10, §9.13, §9.16], the XML Schema document [§9.9, §9.12, §9.15, §9.7], or both [§9.11, §9.14, §9.17], with each of NULLS absent and nil, TABLEFOREST, TARGETNS and ENCODING. A column is visible with SELECT on its table and, of a distinct type, EXECUTE on its cast to its source type; XML visible if its type maps, so not of XML(SEQUENCE), a structured or reference type, nor based on one [§4.10.8], nor, \[EXTENSION\], of DECFLOAT or a plug-in's type, which §9.5 maps to no type, nor based on one. A row, an array or a multiset is mapped as §9.8 maps it [GR9i, GR9j], a null field nilled of NULLS nil, its type a complex type [§9.5 GR8n, GR8p, GR8q], defined with the types it is made of [§9.7 GR3b, GR3d]. A schema's and the catalog's mappings hold their XML visible tables and schemas alone, those there are as they run. A view, routine or trigger that maps a schema depends on it: DROP SCHEMA RESTRICT is refused while it is there, CASCADE drops it. A mapping of a schema or the catalog leaves out a view that maps a schema or the catalog, or reads one that does, as it could hold itself, and is refused (SQLCODE -103) where a routine would. A visible column that is not XML visible completes the statement with the warning column cannot be mapped, 01010 [§9.11 GR3, §9.14 GR3, §9.17 GR3]. A table's names, columns' and types' are mapped by §9.1 and §9.2, fully escaped; its values by §9.8. |
| Information Schema | §20.3 to §20.11, §21.1 to §21.3 | ATTRIBUTES, COLUMNS, DOMAINS, ELEMENT_TYPES, FIELDS, METHOD_SPECIFICATIONS, METHOD_SPECIFICATION_PARAMETERS, PARAMETERS and ROUTINES show an XML type's XML_PRIMARY_MODIFIER, DOCUMENT, CONTENT or SEQUENCE, and XML_SECONDARY_MODIFIER, UNTYPED or ANY, null of SEQUENCE [§21.1 Description 1a]. The XML_SCHEMA_ columns show a type of XMLSCHEMA's registered XML Schema, its catalog, schema and name, and its XML namespace and element if it names them [§21.1 Description 1a iii], from the type's descriptor (`value::types::Descriptor::xml_schema`); all seven are null of another type. PARAMETERS shows XML_PASSING_MECHANISM and ROUTINES XML_RETURN_MECHANISM, BY REF of a parameter or result of an XML type, else null; the associated string type's columns and XML_OPTION are null, as an external routine alone has them [§12.7 SR1]. ROUTINES and METHOD_SPECIFICATIONS show the RESULT_CAST_XML_ columns of a `<result cast>`'s type, null, as no SQL routine has one; ROUTINES shows XML_CHAR_TYPE once, among the associated string type's columns, as ROUTINES_S does, which reads it there [§20.15]. NCNAME and URI are not defined as domains, as CHARACTER_DATA is not [§20.1, §20.2]. The views of registered XML Schemas [§20.12 to §20.14] are the next row's. |
| Registered XML Schemas | §4.2.6, §5.2, §11.6 AR1a | Registered by CREATE XML SCHEMA, of a `<registered XML Schema name>` in a schema its creator owns [§5.2], from one document, a character string, binary string or XML value; dropped by DROP XML SCHEMA, RESTRICT refused while a registered XML Schema includes, redefines or imports it, or a routine has a parameter or result of a type of XMLSCHEMA of it, or a view, constraint, assertion, trigger or routine names it, CASCADE dropping those, and refused even so while a column, a domain or a user-defined type is of such a type (RULES.md D106). Two of one name are refused, SQLCODE -103 [§4.2.6]. The owner of its schema may use it, and anyone a permanently registered one [§11.6 AR1a] (`catalog/xml_schemas/privileges.rs`); a permanently registered one is never registered again nor dropped. Its descriptor holds its target namespace, schema location, name, whether permanent, whether non-deterministic, and its namespaces with their global element declarations, the last three read from its schema [§4.2.6]. |
| The views of registered XML Schemas | §20.12 to §20.14, §21.4 to §21.7, §4.2.6 | XML_SCHEMAS, XML_SCHEMA_NAMESPACES and XML_SCHEMA_ELEMENTS, the draft's definitions over Definition Schema tables of the same names, show the registered XML Schemas the user, PUBLIC or an enabled role holds USAGE on, the permanently registered ones among them; USAGE_PRIVILEGES shows USAGE on each, of OBJECT_TYPE XML SCHEMA [§21.4]. A schema's namespaces are its target namespace and each of its top-level components', those it imports among them; an element declaration is non-deterministic if its type, or that of an element in its content at any depth, has a wildcard of `##any` or `##other` processed strictly or laxly, a namespace if one of its element declarations is, a schema if one of its namespaces is [§4.2.6]. |
| Short name views | §20.15 | XML_SCH_ELEMENTS_S, XML_SCH_NSPACES_S and XML_SCHEMAS_S, the draft's definitions over XML_SCHEMA_ELEMENTS, XML_SCHEMA_NAMESPACES and XML_SCHEMAS, each column named by an `<as clause>` as the draft's `<view column list>` names it [ISO/IEC 9075:1992 §7.9 SR9a, §11.19 GR1]. ATTRIBUTES_S, COLUMNS_S, DOMAINS_S, ELEMENT_TYPES_S, FIELDS_S, METHOD_SPECS, METHOD_SPEC_PARAMS and PARAMETERS_S end with the seven columns of an XML type, and ROUTINES_S has them after DTD_IDENTIFIER, the names the draft gives them there; PARAMETERS_S and ROUTINES_S end with the associated string type's columns and XML_PASSING_MECH or XML_RETURN_MECH. The draft's COLUMN_S, DOMAIN_S, METHOD_SPECS_S and METHOD_SPEC_PARAMS_S are read as COLUMNS_S, DOMAINS_S, METHOD_SPECS and METHOD_SPEC_PARAMS, the views ISO/IEC 9075-11 defines. ROUTINES_S and METHOD_SPECS have the RC_XML_ columns too. |
| Registered XQuery modules | XQuery §4.2, §4.11, §5.2.5 | \[EXTENSION\] Registered by CREATE XQUERY MODULE, of a `<schema qualified name>` in a schema its creator owns, from one library module, a `<character string literal>`, of a target namespace of nonzero length, else SQLCODE -103; one not a library module err:XPST0003, any static error of it or of what it imports SQLSTATE 10000. Analysed as it is registered in a static context of its own, its schema imports and module imports importing among the registered XML Schemas and XQuery modules its schema's owner may use; those imported kept, depended on and imported again whenever it is imported. A duplicate name SQLCODE -103. ALTER XQUERY MODULE replaces its text and location, analysed and its imports recorded again, refused, SQLCODE -103, if a module importing it or a definition keeping it would no longer analyse or bind. DROP XQUERY MODULE RESTRICT is refused while a module imports it, CASCADE drops those, a cycle whole; DROP XML SCHEMA CASCADE drops the modules importing it, DROP SCHEMA CASCADE the schema's. Logged as its statement's text, its drop as a drop record, checkpointed with the catalog, replayed by recovery and a standby. |
| USAGE on registered XQuery modules | XQuery §5.2.5 | \[EXTENSION\] Granted and revoked by GRANT and REVOKE USAGE ON XQUERY MODULE as on a registered XML Schema: USAGE or ALL PRIVILEGES alone, else SQLCODE -103; its schema's owner holds it with grant option; a grant needs it held grantably, else warning 01007, and none held is -104; REVOKE takes the grants named or their grant option, with the grants they abandon and, of each schema whose owner it leaves without USAGE, the registered XQuery modules that imported it, dropped as DROP XQUERY MODULE CASCADE drops them, RESTRICT refusing that with SQLSTATE 2B000; a grant not made warns 01006. A query imports, and a module registered imports, among the modules its user, or its schema's owner, holds USAGE on (`catalog::xquery_modules::may_use`). USAGE_PRIVILEGES shows each, of OBJECT_TYPE XQUERY MODULE. |

| Choice | Rules | turborel |
|---|---|---|
| XML alone; DOCUMENT and CONTENT alone | §6.1 SR2, SR3b | XML(SEQUENCE); ANY. |
| The type of XMLPARSE | §6.16 SR1 | UNTYPED, as every node a parse makes is. |
| CAST's passing mechanism | §6.5 SR2 | BY REF. |
| A routine's passing mechanism | §12.7 SR8b i, iii | BY REF, of each parameter and result of an XML type, as none is written. |
| A namespace of none | §21.5, §21.6, §11.6 SR2a i | The zero-length string, as NO NAMESPACE names it, in XML_SCHEMA_TARGET_NAMESPACE and XML_SCHEMA_NAMESPACE, which are of primary keys. |
| The lengths of NCNAME and URI | §20.1 Description 2, §20.2 Description 2 | CHARACTER_DATA's, 32767 characters: XML_SCHEMA_ELEMENT and XML_SCHEMA_NAMESPACE are CHARACTER VARYING(32767). |
| XMLCAST's passing mechanism | §6.6 SR10b ii 1 | BY REF. |
| The binary encoding | §6.6 SR1b, §6.14 GR1b, §6.15 GR1b, §6.19 GR3b | HEX, of XMLCAST, XMLELEMENT, XMLFOREST, XMLTEXT and XMLMAP of no ENCODING in the scope of no XMLBINARY: a binary string is an `xs:hexBinary`, in upper case. |
| The publishing operators' returning clause | §6.11 SR1, §6.12 SR1, §6.14 SR10, §6.15 SR4, §6.17 SR4, §6.19 SR1, §11.2 SR1 | RETURNING CONTENT, as XMLDOCUMENT's, so what they make is stored in a DOCUMENT or CONTENT column; XMLAGG's, implementation-dependent, too. |
| XMLAGG of no ORDER BY | §11.2 GR2d | The order in which the rows are read. |
| The registered XML Schema of a URI | §11.6 SR2b | turborel identifies registered XML Schemas by name [§4.2.6], so SR2b ii: of those of the target namespace, and of the schema location if written, that the user may use, the one of the least name, schema then identifier; the permanently registered ones last. (`db/definitions/xml_schemas/according.rs`) |
| Whether the namespace and element must be there | §11.6 SR4b, SR4d iii | They must: one not there is SQLSTATE 2201M or 2201N as the clause or type is bound, as GR1 would raise. |
| The registered XML Schema of an element, data-driven | §6.20 GR6a iii 2 A II 2, GR6b i 2, §8.5 GR7a iii 3 A II 2, GR7b i 2 | Of the registered XML Schemas of the element's namespace that the user may use, the one of the least name, the permanently registered ones last, chosen as the statement is bound (`binder/xml/valid.rs`). |
| A prefix XQuery makes | XQuery §3.7.4 | `ns1`, `ns2` and on, of an attribute in a namespace with no prefix; a prefix bound to another URI on the element gets its next free number, as `p1`. |
| The namespace of `sqlxml` | §4.9 Table 2, §10.12 SR2a ii 2 B I | `http://standards.iso.org/iso/9075/2003/sqlxml`, as Table 2 writes it; §10.12 and §10.20 write it without the slash after `iso`. |
| SQL_TEXT's mapping to Unicode | §9.1 GR3, §9.3 GR4, §10.12 SR3 | The identity: SQL_TEXT is Unicode. |
| How an application invokes the mappings of names | §4.10.3 NOTE 10, §4.10.4 NOTE 11, §9.1, §9.3; Feature X400 | \[EXTENSION\] In SQL, `XMLNAME_TO_IDENTIFIER ( <character value expression> )`, the delimited identifier an XML Name maps to [§9.3 GR5], in double quotes, each double quote doubled; and `IDENTIFIER_TO_XMLNAME ( <character value expression> [ , { PARTIALLY \| FULLY } ESCAPED ] )`, the XML Name an identifier's characters map to [§9.1], PARTIALLY ESCAPED by default. §9.2's multi-part names are not taken, as a string is no sequence of identifiers. Each gives CHARACTER VARYING of the longest mapping its argument's declared length allows, at most 32,767 (twice it and 2, nine times it), a longer result SQLSTATE 22001, null of null. No error of the draft arises: §9.3 GR4's unmappable XML Name needs a character SQL_TEXT lacks (`functions/xml/names.rs`, `parser/xml/names.rs`). In Rust, `turborel::xml::{name_of, name_of_parts, identifier_of, delimited}` (`xml/identifiers.rs`). |
| A null element mapped | §9.8 GR9j iii 2 A, §4.9 Table 2 | `<element xsi:nil="true"/>` declares the prefix `xsi`, so the text, parsed as CONTENT [§10.13 GR4c i 1 B, §6.6 GR3b], is namespace-well-formed. |
| How USAGE on a registered XML Schema is granted | §4.7.1; Annex B 2a | GRANT and REVOKE USAGE ON XML SCHEMA `<registered XML Schema name>`, as ISO/IEC 9075-2:2023 §12.2, §12.3 and §12.7 grant and revoke USAGE on a sequence generator (`db/usage/xml_schemas.rs`). On a permanently registered one every user holds it, not grantably, so a GRANT grants nothing (01007) and a REVOKE revokes nothing (01006). |
| The permanently registered XML Schemas | §4.2.6 Table 1; Annex B 1b | Named INFORMATION_SCHEMA.XS, XSI and SQLXML, after Table 1's prefixes; each one's schema location URI its target namespace URI followed by `.xsd`. XS has the 41 global element declarations of the Schema for Schemas and its datatypes [Schema1 Appendix A, Schema2 Appendix A], each non-deterministic, and the namespaces of XML Schema and of `xml`, which it imports; XSI no element declaration; SQLXML `sqltype` and `sqlname` [§22.1], deterministic. They are constants, not catalog objects, and every user holds USAGE on them (`catalog/xml_schemas/permanent.rs`). Their components are read from documents kept in the tree (`xml/schema/permanent/`): XS's from the Schema for Schemas [Schema1 Appendix A] with the schema of the datatypes [Schema2 Appendix A] and turborel's schema of the XML namespace's `xml:lang` and `xml:space` [XML 1.0 §2.10, §2.12], which no text on hand prints; XSI's none of its own [Schema1 §3.2.6, §3.2.7]; SQLXML's from §22.1, which a registered XML Schema's document may import. The Schema for Schemas' restrictions are trusted, not checked by Particle Valid (Restriction), which three of them break by its letter [Schema1 §3.9.6]. |
| The modules a module import imports | XQuery §4.11, §5.2.5 | One registered XQuery module of its target namespace (`xquery/analyze/modules.rs`): of a main module's, among those the user may use, of a library module's, among those it imported as it was registered; of these, the one whose location, or name alone or after its schema's, is a location hint, else the one of the least name. A library module importing its own target namespace imports another of it. Modules of one namespace import one another, ALTER XQUERY MODULE making a module import one registered after it; a cycle of modules not all of one namespace is err:XQST0073. |
| NIL ON NO CONTENT | §10.13 GR4c ii 2 | Read of the content as mapped: an element whose content holds no element, text node or atomic value is nilled. |
| CAST of a distinct type of XML | §6.5 SR3, ISO/IEC 9075-2:2023 §11.51 GR2b, §6.13 SR12 | By the type's own casts to and from its source, as Part 2 makes them for every distinct type, though §6.5 SR3's table writes N between XML and a user-defined type: without them no value of the type could be made (X013). |
| The XML Schema types of SQL's numbers and BOOLEAN | §9.5 GR8d iv, GR8e viii, GR8f | INTEGER, SMALLINT and BIGINT restrictions of `xs:integer`, an XML Schema document's by their greatest and least values; REAL and FLOAT of 24 bits or fewer `xs:float`, DOUBLE PRECISION and other FLOATs `xs:double`; BOOLEAN `xs:boolean`, each named by a restriction of no facet [§9.6 GR6b iv]. |
| Annotations of XML Schema types | §4.10.5, §9.5 GR8 | None is written, so no type an XML Schema document defines uses the namespace of `sqlxml` (`functions/xml/map/types.rs`). |
| XMLMAP's clauses not written | RULES.md D106 | NULLS ABSENT, as XMLFOREST leaves a null value out; AS ELEMENT, TABLEFOREST False; TARGETNAMESPACE '', no namespace [§9.10 GR1]; DATA; ENCODING of the XMLBINARY in scope, HEX outside every one, as of XMLCAST, XMLELEMENT and XMLFOREST [§11.3]. |
| The version and encoding of a mapping's documents | §9.11 GR7b, GR8, §9.14 GR8b, GR9, §9.17 GR8b, GR9 | XML 1.0, unless a name of the XML Schema document is of XML 1.1 alone [GR7a]; UTF-8, so XMLDECL is written of XML 1.1 alone, and its encoding never (`functions/xml/map.rs`). |
| SLOC, the location of `sqlxml`'s XML Schema | §9.11 GR10c, §9.14 GR11c, §9.17 GR11c | None: no type uses `sqlxml`, so XSDIMP and XSDECL are the zero-length string (`functions/xml/map/documents.rs`). |
| IDI, a row type's identifier | §9.4 GR19 (implementation-dependent) | The multi-part name [§9.2] of each field's name and its type's XML Name in turn, so `Row.X.INTEGER.Y.VARCHAR_5` (`functions/xml/map/composites.rs`). |
| Annotations of tables, schemas and the catalog | §9.9 GR5f i 7, GR9, §9.12 GR8, §9.15 GR7 | None (`functions/xml/map/documents.rs`). |
| The orderings of rows, tables and schemas | §9.10 GR6a, §9.12 GR5a, §9.13 GR5a, §9.16 GR5a | Rows as the table is read; a schema's tables, and the catalog's schemas, in order of name (`binder/xml/map.rs`). |
| A character set's mapping to Unicode in XML Schema types | §9.4 GR2c, §9.5 GR8a ii | The identity. Of a length in characters it is homomorphic, N characters of N; of a length in octets not, as N octets hold at most N characters, so a CHARACTER of one is restricted by `xs:maxLength`, and named VARCHAR_N_N. |
| Limits of XQuery's atomic values | XQueryFO §10.1.1, §17.1.1 | A date's year from 0001 to 9999, else err:FODT0001; seconds, of times and of `xdt:dayTimeDuration`, to the microsecond, more digits dropped; a duration within 128 bits of months or microseconds, else err:FODT0002; `xs:decimal` and `xs:integer` of any digits, an SQL exact number of them its fraction cut to 38 digits. A float or double cast to a decimal is the shortest decimal that reads back as it; a double cast to a float its mantissa truncated to 24 bits, as [XQueryFO §17.1.3.1] says, below a float's least normal value its subnormal, the bits truncated further. A zero time zone is written Z. |
| The types §9.5 maps SQL's types to | §9.5 GR8 | Without annotations [GR8a vii and the like]. CSM, the mapping of a character set to Unicode, is the identity [GR8a ii], so CHARACTER(n) is `xs:string` of the length n, and of a length in OCTETS of the maxLength n. |
| The pattern of TIME(p) and TIMESTAMP(p) | §9.5 GR8h to GR8k, §6.6 GR4j | The patterns ask for exactly p digits of fraction, which no canonical form of a value has, as its seconds end in no zero [XQueryFO §17.4]; so a value, adjusted to UTC first of a type without time zone [GR4j i to iii], is checked against XT written as §9.8 GR9f and GR9g write an SQL value of the type: its fraction padded with zeros to p digits, its time zone ±hh:mm. A value, or text, of more than p digits of fraction is refused, SQLSTATE 10000; zeros ending the text's fraction are not counted, as they are no digits of its value, so `12:00:00.5000` is a TIME(3). |
| The time zone of §9.5's patterns | §9.5 GR8i ii, GR8k iii | TZ, printed `(+\|-)\p{Nd}{2}:\p{Nd}{2}`, is no regular expression, `+` being a metacharacter [Schema2 Appendix F]; it is read as `(\+\|-)`, the sign it names. |
| XMLDOCUMENT's returning clause | §6.13 SR1 | RETURNING CONTENT. |
| How far a URI is checked | XQueryFO §17.1.1 | Not at all: every string is a lexical form of `xs:anyURI`. |
| The literals of `xs:base64Binary` | Schema2 §3.2.16 | Groups of four characters of RFC 2045's alphabet, spaces among them ignored, the last group alone padded with `=`; another character is refused, not ignored. Its string has no spaces. |
| The names of `xs:Name`, `xs:NCName`, `xs:NMTOKEN` and their kin | Schema2 §3.3.4 to §3.3.11 | XML 1.1's, the names of XQuery's evaluations under XML 1.1's lexical rules [§6.6 GR3d vi, GR4j] and those `\i` and `\c` match in a pattern. |
| `xs:gMonth` | Schema2 §3.2.14.1 | Written `--MM--`, as the edition SQL/XML names writes it. |
| A restriction's bounds | Schema2 §4.3.7.2, §4.3.9.4 | A bound's value is of its base's value space, the base's bounds aside, so an exclusive bound may be restated; a bound a step specifies replaces its base's on that side; minInclusive and minExclusive, as maxInclusive and maxExclusive, are refused together in one step. |
| How deep lists and unions nest | RULES.md D106 | At most 32 deep (`MAX_TYPE_NESTING`), deeper SQLCODE -900. |
| A QName's value | Schema2 §3.2.18 | Its prefix and local name as written, and its namespace URI [XQueryDM §3.3.3], kept in the stored form; two are equal if their URIs and local names are. A literal cast makes a QName of no namespace, as an atomic value holds no namespace bindings [XQueryFO §17.1.1]. A length facet constrains no QName or NOTATION, as length is measured in no unit of theirs [Schema2 §4.3.1.3]. |
| The canonical form of the types with none | XQueryFO §17.1.2 | The string XQuery writes: of `xs:date`, the Gregorian types and the durations their local value or canonical duration; of `xs:base64Binary` no spaces. The seconds of a date and time or a time end in no zero, as XQuery writes them. |
| XMLDOCUMENT's type of CONTENT | §6.13 SR3a | XML(CONTENT(UNTYPED)), so what it makes is unvalidated [GR9a, §10.19], and an UNTYPED column takes what the publishing operators make; XMLCONCAT's document node is of its own declared type (`binder/xml/publish.rs`). |
| XMLSERIALIZE's version and encoding | §6.8 SR2d, SR4d, SR4e | '1.0'. The encodings are UTF8, the default, and UTF16. |
| A byte order mark | §10.15 GR4a ii | Before UTF16 alone. |
| The standalone declaration | §10.15 GR7 | None is written. |
| An XML declaration not asked for | §10.15 GR8c ii | None, of a document of XML 1.0 in UTF8 or UTF16; one of a document of XML 1.1 [GR8c i]. |
| Prefixes undeclared | §10.15 GR9b | None, of XML 1.1 too: undeclare-prefixes is no, its default for XQuery's Serialization Feature [XQuery §5.2.6, Appendix C.3], which SQL/XML gives no way to set (`functions/xml.rs`). A value sent over a connection in XML 1.1 undeclares them, so a client reads back its tree (RULES.md D106). |
| How an XML Schema is registered | §4.2.6 | \[EXTENSION\] CREATE XML SCHEMA and DROP XML SCHEMA (RULES.md D106, `db/definitions/xml_schemas.rs`). A registered XML Schema is identified by its `<registered XML Schema name>` alone. Its target namespace is its document's targetNamespace, the zero-length string of none; its schema location is the LOCATION written, the zero-length string if none. Its `<include>`, `<redefine>` and `<import>` find the documents of registered XML Schemas the owner of its schema may use, as §11.6 AR1a asks of one an SQL-schema statement names; those it read are what it depends on. |
| Schema documents read | Schema1 §2.4, §4.2, §4.3.2, §5.1 | A schema is read from schema documents, each a `<schema>` that is its tree's element (`xml/schema/read.rs`). `<include>`, `<redefine>` and `<import>` find documents among those the schema is read with alone, by schema location, or for an import by namespace; none is fetched, so turborel is a processor in conformance to the XML Representation of Schemas, not a fully conforming one. A location naming no such document adds no components [§4.2.1]. The XML Schema namespace's components are its built-in ones, named without an `<import>`. |
| Missing sub-components | Schema1 §5.3 | None: a reference that resolves to nothing is refused as the schema is read. |
| The Schema for Schemas | Schema1 §5.1, Appendix A | Each element of a schema document is checked against the attributes of no namespace and the order of children its XML Representation Summary gives (`xml/schema/read/shape.rs`), not validated against the Schema for Schemas. |
| Simple type definitions | Schema1 §3.14, Schema2 §4.1.2 | The built-in datatypes of XML Schema Part 2, the lists `xs:NMTOKENS`, `xs:IDREFS` and `xs:ENTITIES` among them, and each `<simpleType>` of the schema documents, by restriction with facets, by list or by union, read whole when first named (`xml/schema/read/simple.rs`); a restriction of simple content with facets or a `<simpleType>` makes its own. A simple type's {final} is read by [Schema1 §3.14.2], so `#all` of final or finalDefault is extension, restriction, list and union. finalDefault lists extension, restriction, list and union: [Schema1 Appendix A] lets it list the first two alone, but [Schema2 §4.1.2] reads list and union from it into a simple type's {final}, so turborel takes all four, a complex type's and an element declaration's {final} keeping extension and restriction of them [Schema1 §3.4.2, §3.3.2]. blockDefault gives an element declaration's {disallowed substitutions} substitution, extension and restriction, and a complex type's {prohibited substitutions} extension and restriction [Schema1 §3.3.2, §3.4.2]. `xs:NOTATION` named as a type is refused, and a type restricting it must give an enumeration, as Part 2 asks [Schema2 §3.2.19]. |
| Typed values of unions | XQueryDM §3.3.1.2 | A node of a union type, or of a list of one, is annotated with the union, its typed value read as the kind of the atomic values validating it gave: one of an atomic type, more of a list of that type. A list whose items are of several kinds is annotated with each item's kind in order, the member type that validated it, so its typed value is each item of its own type [XQueryDM §3.3.1.3]. |
| Typed values of qualified names | XQueryDM §3.3.1.3, §3.3.3 | A node of `xs:QName` or `xs:NOTATION`, of a type derived from one, or of a list of them, has a typed value whose namespace URI is the one its prefix, or the default namespace of none, is bound to among the namespaces in scope of its element [Schema2 §3.2.18 NOTE]; a parentless attribute's has none. |
| Unique Particle Attribution | Schema1 §3.8.6, Appendix H | Checked on the automaton of each content model (`xml/schema/automaton.rs`), its transitions labelled by particle: one particle reached by two paths, as a group referred to twice, is one particle. Occurrence ranges unfold to at most 4,096 particles and the automaton to 16,384 states, SQLCODE -900 past either. |
| Type Derivation OK from the ur-type | Schema1 §3.4.6, §3.14.6 | Every type is validly derived from `xs:anyType` [§2.2.1.1]. Clause 1.5 of Derivation Valid (Extension) is not checked. |
| The validation of an element node | XQuery §3.13, §10.22 | Its identity constraints and ID/IDREF table are not checked, as XQuery's validate does not check them of an element node; of a document node they are. |
| What is not assessed | XQuery §3.13, XQueryDM §3.3.1.1 | An element skipped by a wildcard, or laxly assessed with no declaration, is annotated `xs:anyType`; an attribute not assessed is untyped, `xdt:untypedAtomic`. The first failure ends validation. |
| Anonymous types' names | XQueryDM §3.3 | `#anonymous-` and a number of a complex type, `#anonymous-simple-` and a number of a simple one, in the type's target namespace: no NCName, so no named type has it. |
| An element of simple content | XQueryDM §6.2.4 | One text node, its schema normalized value, absent if empty, before its comments and processing instructions. |
| Default values | Schema1 §3.4.5, §3.3.5 | An absent attribute takes its use's value constraint, else its declaration's. An empty element of mixed content takes its declaration's default as text too. An attribute added in a namespace takes a prefix bound to it in scope, if one is. |
| Entities and notations | XQuery §3.13 | `xs:ENTITY` and `xs:NOTATION` values are not checked against unparsed entities and notations, as XQuery's validate does not check them; a NOTATION value is a QName whose prefix is in scope [Schema1 §3.15.3]. |
| Limits of reading a schema | RULES.md D18, D106 | Declarations, groups and types nested 64 deep where reading recurses; 64 types of a derivation or declarations of a substitution group, or simple types read by way of one another; 256 documents. Past one, SQLCODE -900. |
| The other serialization parameters | §10.15 GR10 | No CDATA section, document type declaration, indentation, URI escaping, content type, Unicode normalization or character map. |
| The defaults of the serialization parameters | XQuery §5.2.6, Appendix C.3 | byte-order-mark yes of a binary string in UTF16, else no; encoding UTF-8, or the UTF16 a binary string names; media-type `application/xml`, which changes nothing written, as no result is annotated with one [Serialization §3]; normalization-form none; omit-xml-declaration as §10.15 GR8 and the choice above give it; standalone omit; version 1.0. The rest are C.3's: no CDATA section element, no doctype-public or doctype-system, no indentation, the method `xml`, no prefix undeclared, no character map. XMLSERIALIZE is the means by which a result is serialized, its errors SQLSTATE 2200W, the code of [Serialization Appendix B] in the message (`xml/serialize.rs`). |
| Notations and unparsed entities | §10.16 GR7a ii, iii | Not kept. |
| Base URI | §10.16 GR7b | None. |
| External DTD | §10.16 GR7e | Not read, nor any external entity, so no parse opens a file or a URL. A reference that needs one, or a declaration not read, is the exception GR6 raises, as the data model has every entity expanded. |
| The encoding of a binary string parsed | XML §4.3.3 | UTF-8, UTF-16 with its byte order mark, UTF-16BE and UTF-16LE that declare themselves, ISO-8859-1 and US-ASCII; another is refused. |
| XMLQUERY's returning clause | §6.18 SR3 | RETURNING CONTENT, as XMLDOCUMENT's (`binder/xml/query.rs`). |
| The subset of XQuery | §6.18 SR6, NOTE 28 | All its grammar; its static rules, the Static Typing Feature's when in effect (`xquery/statics.rs`); the built-in functions above; schema imports and validation; serialization by XMLSERIALIZE; library modules, registered XQuery modules imported (`xquery/analyze.rs`). |
| When static typing is in effect | §6.18 SR6, NOTE 29; XQuery §5.2.3, §4.16 | Off by default, as it refuses queries that would evaluate without error. XQUERY.STATIC_TYPING ON, of a session or the database, puts it in effect for the statements prepared. A definition (a view, a CHECK, an assertion, a trigger, a routine, a generation expression, a domain constraint) is typed as it is made by the value of the session making it, logged with its statement so it is made again alike after a crash; it keeps whether it was typed, as it keeps the registered XML Schemas it indicates [§4.2.6], and is typed so whenever it is bound again, whatever the session reading it asks. The option `static-typing` of `urn:turborel:option`, the namespace of turborel's options, puts it in effect for its query, of the value `on`; `off` takes nothing out of effect, as an option suppresses no static error [XQuery §4.16]; another value, or another option of that namespace, is SQLCODE -103. An option of another namespace is ignored. |
| The formal types of what is passed | §10.21 SR3c, SR5h, SR6 | A variable passed of an XML(DOCUMENT) or XML(CONTENT) type or an SQL type is one if its value expression is known not null, else optional; the context item is one [SR3a]. Known not null [SR3c] is a column reference to a known not null column [X3.135-1986 §5.25 SR10], and, by turborel's implementation-defined rule, a literal but UNKNOWN. An SQL type is typed by the primitive type of the XML Schema type §9.5 maps it to, binary strings hexBinary; a type §9.5 maps to none as XML(SEQUENCE). An XMLTABLE column's pattern's context item is of the type of an item of the row pattern's value, if that was typed, else one of XML(SEQUENCE). |
| Static typing extensions | XQuery §5.2.3.1; XQueryFS §6.1.1 | Three, each giving a type that is a subtype of the one [XQueryFS] gives: a quantifier counts the least and most items of a type, so the empty type adds none to a sequence [§8.4]; the root of a focus of documents is of the focus's type, a document being its own root, where `fn:root` gives `document` [§4.2, §7.2]; and a schema wildcard's name admits the names its namespace constraint allows alone, an attribute wildcard's not those of the attribute uses beside it, where [Appendix C.11] gives `*` (below). |
| Readings of [XQueryFS] | XQueryFS §3.5.4, §4.7, §7.1.1, §7.2.10, §8.6.1, Appendix B.2 | Where the Candidate Recommendation's rules leave a type out or disagree with XQuery: `element()`, `node()` and `document-node()` admit nilled elements, as XQuery's matching does [XQuery §2.5.4.3]; `fs:convert-operand` returns an atomic operand neither untyped nor numeric unchanged, as its dynamic semantics says; the operator mapping is [XQuery Appendix B.2]'s, with its type promotion [B.1], where Appendix B.2 of [XQueryFS] leaves rows out; a direct element's content is converted by `fs:item-sequence-to-node-sequence` as a computed element's is; fn:sum's zero is promoted, as its other values are, and fn:sum of one argument, of no value, gives `xs:integer` too; validate makes of a document a document of the element validated, and of an element of no declaration in scope `xs:anyType`'s. Subtyping is decided for the shapes the rules compare with; any other shape is not a subtype, so a query is refused, never admitted, where it cannot be decided. [XQueryFS Appendix C.11] does not map a wildcard's namespace constraint, and its types name a wildcard `*` alone; as an extension a wildcard's name admits the names of the namespaces its {namespace constraint} allows alone, none for not but neither that namespace nor none [Schema1 §3.10.4], and an attribute wildcard's not the names of the attribute uses beside it nor `xsi`'s four, which are typed by those [Schema1 §3.4.4 Element Locally Valid (Complex Type) 3]; a name test of it keeps the names both match. Its element type is `xs:anyType`'s, its attribute type `xs:anySimpleType`'s, of each {process contents}. |
| Schema imports | XQuery §4.10, Appendix C.1 | The schema of a target namespace is the registered XML Schema of it the current user may use whose schema location is a location hint, else the one of the least name, the permanently registered ones last; it brings the components of every namespace it has (`xquery/analyze/import.rs`). err:XQST0058 of a namespace imported twice, err:XQST0059 of one no such schema has, err:XQST0035 of a name two imported schemas define from different schema documents. err:XQST0012 is never raised: each registered XML Schema is valid, and what two imported share comes from the same documents, else err:XQST0035. A simple type of the atomic variety of an imported schema is an atomic item type [XQuery §2.5.4.2], a cast's target [§3.12.3] and a constructor function [§3.12.5]; a list, union or complex type is none, err:XPST0051, and no cast's, err:XPST0080. |
| User-defined atomic types | XQueryDM §2.6.1, §3.3.1.2; XQuery §2.5.4.2, §3.1.5, §3.12.3, §3.12.5; XQueryFO §17.2 to §17.5 | A value of a user-defined atomic type is a value of the built-in type it restricts, annotated with its type's name, an anonymous type's made from its place in its schema (`xml::Atomic::annotation`). A node's typed value is of its type: an atomic type's, a list's item type's, a union's member's that validated each item; kept when stored (`xml/stored/atomic.rs`). It matches an atomic item type of an imported schema by that schema's Type Derivation OK of the type of its name there, none if the schema has none of the name, and a built-in type by the hierarchy of its built-in type. A cast to one checks its facets: a string or untyped value validated as a literal, its whitespace normalized and patterns matched first; another value cast as the built-in type, its patterns matched against its canonical form; refused, err:FORG0001. A cast to one derived from `xs:QName` or `xs:NOTATION` is of a string literal alone (err:XPST0083), evaluated as the query is analysed. A cast from one to its built-in type keeps the value, of that type alone. The function conversion rules cast an untyped value to a parameter's user-defined type. Arithmetic, comparisons and functions take the value as one of its built-in type and give built-in types (`xquery/types.rs`, `xquery/eval/types.rs`). Statically typed, it is an atomic type of its own: a node's typed value of it, a cast to it and its constructor function are of it; it is a subtype of a user-defined type its name's type derives from in that type's schema, and of the built-in types above the one it restricts; a built-in type is a subtype of none of it; operators take it as its built-in type (`xquery/statics.rs`). |
| XMLTABLE's determinism | §7.1 SR1 | Every XMLTABLE is deterministic, as a query reads no clock but the statement's and no source but its arguments. |
| XQuery's static context | XQuery Appendix C.1 | Default element/type namespace none, unless an XMLNAMESPACES in scope declares one; default function namespace `fn`; boundary-space strip; construction preserve; ordering ordered, and unordered gives the same order; default order for empty sequences least (`xquery::EMPTY_ORDER`); copy-namespaces preserve, inherit; base URI none; the default collation the Unicode codepoint collation; the statically known collations it and each of the database's, as `urn:turborel:collation:` and its name (`xquery/collation.rs`); in-scope schema types the built-in types of `xs` and `xdt`; the in-scope schema definitions [ISO/IEC 9075-14:2006 draft §10.20 SR2e], of each target namespace the registered XML Schema the current user may use of the least name, as XMLVALIDATE's data-driven case chooses, chosen as the statement is bound, none permanently registered; a validate expression's effective schema the one of the namespace of the element it validates, a schema of no components if none, and no name of a query resolves to their components (`xquery/eval/validate.rs`, `binder/xml/query.rs`); no statically known documents or collections. |
| XQuery's dynamic context | XQuery Appendix C.2 | The current dateTime the statement's instant in the session's time zone, the implicit timezone the session's displacement; no available documents, so fn:doc fails with err:FODC0005 of any URI; no collection. |
| XQuery's limits | XQuery Appendix D | Expressions nested 64 deep; operations and function calls nested 256 deep as a query is evaluated, expressions 256 deep as it is statically typed; a static type nested 256 deep, an axis's closure of 4,096 item types; a sequence of 1,048,576 items; the nodes one evaluation constructs 64 MiB; past them SQLCODE -900 (`xquery/eval.rs`, `xquery/statics.rs`). |
| Order of nodes of two trees | XQuery §2.4.1 | By where each tree is held in memory, stable while a query runs, every node of a tree together (`xml/sequence.rs`). |
| XQuery's warnings | XQuery §2.3.1, Appendix D | None is raised. |
| Type annotations | XQuery §2.2.1, §3.7.1.3, §3.7.3.1, §4.6 | A validated node has the annotation its validation gives; a parsed one `xdt:untyped` of an element and `xdt:untypedAtomic` of an attribute. An element a query constructs is annotated `xs:anyType` in construction mode preserve, the default, and `xdt:untyped` in strip; preserve keeps a copied node's annotation, strip makes every copied element `xdt:untyped` and attribute `xdt:untypedAtomic`; an attribute a query constructs is `xdt:untypedAtomic`. XMLELEMENT and XMLFOREST RETURNING SEQUENCE make an element of `xs:anyType`, as `element { $QNAME } { $SEQ }` does in the default context of §10.20 [§10.13 GR4c viii]; a value of XML(CONTENT(UNTYPED)), as RETURNING CONTENT makes, has every element `xdt:untyped` [§10.19]. |
| Copy-namespaces no-preserve | XQuery §3.7.1.3 | Each copied element declares the bindings its name and attributes' names use; one copied below another keeps too those its copied ancestors declare, as a tree holds bindings where they are declared. |
| A prefix made for a constructed name | XQuery §3.7.4 | `ns1`, `ns2` and on, or the prefix taken followed by its next free number. |
| XQuery's Unicode and XML versions | XQuery §2, §3.1.1, §3.7.1.1 | Names, characters, character references and attribute normalization of XML 1.1 and Namespaces in XML 1.1, as XMLQUERY reads by XML 1.1's lexical rules [§6.18 GR2a]. |
| The functions and operators of XQuery | XQueryFO §1.1 | The built-in functions of every query, and the operators its evaluation calls (RULES.md D106). Unicode 13.0, the version of the collations and regular expressions. |
| The precision of `xs:decimal` and `xs:integer` operations | XQueryFO §6.2 | Any digits to 1,024, past which an operation overflows, err:FOAR0002; no integer is of limited precision, so none wraps. A quotient of decimals is exact if it ends within 18 digits after the period, or the scale of either operand, and is truncated there otherwise. |
| Overflow and underflow of floats and doubles | XQueryFO §6.2 | IEEE 754's: an infinity, or a zero or subnormal value. A float's operation is rounded to single precision. |
| fn:error and fn:trace | XQueryFO §3, §4 | The error is SQLSTATE 10000, its message the QName as `NS#LP` and the description; an error object is not returned. The trace is discarded. |
| A node's base URI and document URI | XQueryFO §2.5, §2.6 | What the `xml:base` attributes of its element and that element's ancestors make, each resolved against the one above it, else none, as no tree keeps a base URI of its own; no document node has a document URI. |
| fn:resolve-uri | XQueryFO §8.1 | RFC 3986 §5.2, strict; a base of no scheme is err:FORG0009. |
| Collations of XQuery's functions | XQueryFO §7.3.1 | The code point collation and each collation of the database, named `urn:turborel:collation:` and its name, as `urn:turborel:collation:CASE_INSENSITIVE`; another is err:FOCH0002. Strings compare by sort keys with no padding. |
| Collation units | XQueryFO §7.5 | Characters, of the code point collation and of the collations that compare one character at a time, by their folds. Of `UNICODE` and its tailorings, each character their steps begin at whose normalized form begins with a starter, and what follows it up to the next: a character and its marks, a contraction, a Hangul syllable; each weighed at every level the collation compares as it is weighed in the whole string (`value/collation/units.rs`). A unit is ignorable if it has no weight at any level. A minimal match is a run of units whose weights at each level, joined, are those of the string sought, beginning and ending at a unit that is not ignorable (`xquery/library/substrings.rs`). err:FOCH0004 is raised only of a collation that does not say how it splits a string, which no built-in one is. |
| Normalization forms | XQueryFO §7.4.6; XML 1.1 Appendix B | NFC, NFD, NFKC, NFKD and FULLY-NORMALIZED, and the zero-length string for none. A fully normalized string is its NFC, as a string holds no character escape or include, and it is its one relevant construct; one whose NFC begins with a composing character, of a canonical combining class not zero or second in the canonical decomposition mapping of a primary composite, has no fully normalized form, err:FOCH0003 (`xquery/library/normalize.rs`). |
| Case mappings | XQueryFO §7.4.7, §7.4.8 | Each character's full mapping, SpecialCasing's unconditional one or UnicodeData's simple one; none that a context or language governs. |
| Regular expressions of fn:matches, fn:replace and fn:tokenize | XQueryFO §7.6.1 | XQuery 1.0's: a newline a line feed alone, `\s` XML Schema's, no `(?:` and no flag `q`, by the matcher of LIKE_REGEX (RULES.md D61) and its limits. |
| Durations times or divided by a number | XQueryFO §10.6.3, §10.6.8 | Exact, the double as the shortest decimal that reads back as it, rounded to the month or microsecond, a half up. |
| The current dateTime and the implicit time zone | XQueryFO §16.3, §16.6 | The instant the statement read its clock at (RULES.md D54), in the implicit time zone the evaluator gives. |
| fn:doc, fn:doc-available and fn:collection | XQueryFO §15.5.4 to §15.5.6 | No document or collection is available and there is no default collection, as no query reads an external resource: fn:doc of a URI is err:FODC0005, fn:doc-available false, fn:collection err:FODC0002 or err:FODC0004. |
| The order of fn:distinct-values and fn:unordered | XQueryFO §15.1.6, §15.1.11 | The first of each set of equal values, in the order they came; fn:unordered's items in the order they came. |
| fn:id and fn:idref | XQueryFO §15.5.2, §15.5.3 | An element's or attribute's ID and IDREF properties are those its document type declaration gave it. |

\[EXTENSION\] Part 14's 2006 draft gives no XML Name of a BINARY, a
BINARY VARYING or an interval type [§9.4], though §9.5 maps each [GR8b,
GR8l]. They are named as §9.4 names BLOB [GR3]: `BINARY_n`,
`VARBINARY_n`, and `INTERVAL_` with the fields, the leading precision
and, of SECOND, the fractional seconds precision, as
`INTERVAL_DAY_TO_SECOND_2_6`. An interval type's pattern [§9.5 GR8l],
XMLCAST's and XMLMAP's alike, is read so it describes what §9.8 GR9h
writes: of no fractional seconds, the seconds have no fraction, and
INTERVAL SECOND's are of its leading precision's digits. The `name`
§9.5 GR8r writes on `xs:any` is left out, as `xs:any` takes none
[Schema1 §3.10.2].

The mappings of tables, schemas and catalogs are read so the XML Schema
document each makes is one, and describes the data. XD's root [§9.11
GR12c, §9.14 GR13c, §9.17 GR13c] is the element XDROWS, XDSCHEMA or
XDCATALOG already is, declaring `xsi`'s namespace, not another of its
name around it, which no element the schema document declares holds; and
\[§9.11 GR12c ii\] of a target namespace its default namespace, as §9.10
GR7b writes it, not `xmlns:xs`. XSLA \[§9.11 GR11a\] is the zero-length
string, as XSR is no resource a URI identifies. A schema document of a
target namespace declares it its default namespace too, so the names of
the types it defines resolve where it writes them unprefixed [Schema1
§3.15.3]; of an interval type it declares and imports the namespace of
`xdt`, as of `sqlxml` it would [§9.11 GR10c, GR10d]. A schema's and the
catalog's schema documents define each table's row type and table type
[§9.9 GR10], and the catalog's each schema's type [§9.12], which their
types name [§9.12 GR5f, §9.15 GR5e]; not the elements §9.9 GR10b
declares, of which two schemas' tables of one name would be two. A
domain or a distinct type of a row, array or multiset type is a complex
type that extends it by nothing, as a simple type restricting a complex
one [§9.5 GR8m vi, GR8o vi] is none [Schema1 §3.14.6]; of an XML type, a
complex type of its mixed content. The data of a table AS FOREST is content of no root [§9.11 GR12c iii]; of
XML 1.1, it starts with a text declaration, which names its encoding
[XML 1.1 §4.3.1], so the parse reads its names. An XML value in the data
is its serialization as CONTENT [§9.8 GR9k], so an element of no
namespace in it is read in the target namespace.

The words of Part 14, XML and the operators' names among its reserved
words, are no key words of SQL-92, so each stays an identifier, read as a
word where SQL/XML reads it. A column named DOCUMENT or CONTENT is read as
the column where what follows it ends a value expression.

---

## SQL/MM Full-Text

ISO/IEC 13249-2:2002, cited by its committee draft (CD, SC32 N0640), is
built in (RULES.md D87). Its types FULLTEXT and FT_PATTERN, the FullText
methods and FullText_to_Character, Contains, Rank, the thesaurus tables
of clause 8 and FT_INFORMATION_SCHEMA's views FT_FEATURES and FT_SCHEMATA
are implemented, as is the whole pattern language of §5.3.1. The manual
is [doc/sql/08-fulltext.md](doc/sql/08-fulltext.md).

A method is written `doc.Contains(p)`, or as a call, `CONTAINS(doc, p)`,
and `NEW FullText(t)` is `FULLTEXT(t)`. LANGUAGE is a reserved word, so
its method is written `doc."LANGUAGE"`. Contains gives 1 or 0, so a
query compares it, `CONTAINS(doc, p) = 1`, and a full-text index of the
column answers that comparison.

### Implementation-defined values

| Value | turborel |
|---|---|
| FT_MaxTextLength [§5.1.1 DR2] | 32,767 characters. A value is held inline, as every extension type's is; past it, SQLSTATE 22001 |
| FT_MaxPatternLength [§5.3.1 DR1] | 32,767 characters |
| FT_MaxLanguageLength [§5.1.1 DR4] | 32 |
| FT_DefaultLanguage [§5.1.1 DR5] | ENGLISH |
| Languages [§5.1.9] | ENGLISH and NONE, in any case, trailing blanks ignored; another, an empty or a null one is SQLSTATE 2FF02 |
| FT_MinRankValue, FT_MaxRankValue [§5.1.3] | 0 and the greatest DOUBLE PRECISION |
| FT_RankDeterminism [§5.1.1 DR6] | NOT DETERMINISTIC. A rank moves with its index's statistics, which a statement reads once, so it is fixed within a statement |
| Thesauri [§8.3 D1] | Those whose names FT_THESAURUS's tables hold, made by the user with clause 8's definitions; at most 65,535 relations a statement, the greatest array |
| Comparison of FULLTEXT values | §5.1.1 defines none. Two compare by language, then by text in code point order, and are equal only of the same language and text |
| Casts | A character string casts to FULLTEXT in ENGLISH and FULLTEXT to a character string as its text [§5.1.10]. A string is no FULLTEXT by assignment; it is an FT_PATTERN by assignment |
| A null string [§5.1.9] | FULLTEXT(NULL) is null, not a value of null contents |
| A document of no characters [§5.1.2 D3c ii] | Contains gives 0 and Rank 0, of every pattern, NOT patterns too. A document of characters but no words matches a NOT pattern [§6.4.2] |
| Nulls | A null document gives null before its pattern is read, then a null pattern null |

### Reading patterns

| Choice | turborel |
|---|---|
| Words [§5.3.1 D3a, D3b] | Separated by white space; any character but an unescaped `"`, `""` being one `"` |
| Language specification [§5.3.1 D3c] | A name just before the quote, in any case; else the document's language |
| Normalization [§4.5, §6.4.5] | A word with no wild card is cut as a document is, so `"e-mail"` is the phrase e mail; a run between wild cards is normalized. Such a phrase inside a NEAR list is SQLSTATE 2FF01 |
| Escapes | An escape before anything but itself, `%` or `_` is 2FF01, as is an escape not one character |
| Optional words | A phrase part of unescaped `%` alone; a phrase of optional words alone is 2FF01 |
| Stop words [§4.4.2, §6.6.2] | Never 2FF01. A pattern of stop words alone is 2FF03. A lone stop word matches nothing, and inside a phrase matches any stop word of the document |
| A literal of another language [§6.4.2, §6.5.2] | LIKE over normalized text, its stop words and stems of its own language |
| Phrases | May cross a sentence's end; in a context condition a match lies wholly in one unit |
| IS ABOUT [§6.17.2] | Any word of the phrase that is not a stop word |
| SOUNDS LIKE [§6.9.5] | American Soundex of the word's ASCII letters; a word of none matches itself only |
| FUZZY FORM OF [§6.10.5] | Optimal string alignment distance at most 1, or 2 of a word of 8 or more characters |
| Thesaurus terms | Deduplicated, read in the start term's language and matched literally; names compared trimmed of blanks; synonyms transitive [§6.13.5] |
| Nesting | At most 100 levels, else 2FF01; a thesaurus expansion adds at most 64 |

### Tokenizing [§5.1.4–§5.1.8]

| Choice | turborel |
|---|---|
| Stop words | Left out of every result, as two of them need compare equal [§5.1.5 D4]; ordinals count them [D7–D9] |
| TokenizePosition units [§5.1.5 D2] | CHARACTERS, WORDS, SENTENCE(S), PARAGRAPH(S), in any case; another gives null [D5b] |
| Segmentize units [§5.1.6 D2] | SENTENCE and PARAGRAPH; another gives null, as the draft names no condition |
| CHARACTERS places | POSITION the word's first character from 1, CORRVAL its length as written; 0 of other units |
| Segments | From the previous segment's end to the white space after its last word, trimmed, in SELF's language; only sentences and paragraphs holding a word count |
| Arrays | At most 65,535 elements, else SQLSTATE 2202F; a document of no words gives an empty array |

### Readings of the draft

| Text | Reading |
|---|---|
| Proximity, §4.2.5.1 against §6.8.2 | §6.8.2. A distance is the difference of positions in the unit; IN ORDER means the second at or after the first, so two words of one sentence are in order both ways under SENTENCES; WITHIN 0 WORDS is one word, which may match both lists; under CHARACTERS the distance is the characters between the words |
| Bare lists beside NEAR | Accepted, as §4.2.5.1's example and §5.3.1 D5f write them, though the grammar asks for parentheses |
| Units [§5.3.1 D3f] | Singular and plural after WITHIN and IN SAME. CHARACTERS and WORDS after IN SAME are 2FF01 |
| FUZZY FORM OF | Of a `<word>`, as the grammar says, though D5j speaks of phrases |
| FOR n LEVELS [§6.11.5] | n steps. The draft's query recurses while LEVEL < n from 0, which gives n+1. Without FOR, every level [NOTE 14] |
| BROADER, NARROWER, SYNONYM, RELATED | Include the start term. PREFERRED and TOP include it only when nothing else is found |
| TokenizeAndStem, TokenizePositionAndStem | Their own clauses' types, §5.1.7 and §5.1.8, over §5.1.1's FullText ARRAY |
| FullText_Token, FT_TokenPosition [§7.1, §5.2.1] | CHARACTER VARYING(1024), and a ROW of TOKEN, POSITION and CORRVAL |

### Ranking [EXTENSION]

Rank's value is implementation-dependent [§5.1.3]. It is BM25
(Robertson and Walker, SIGIR 1994), the sum over the terms the pattern's
words match, outside NOT, of idf × tf(k1+1) / (tf + k1(1 − b + b·len/avg)),
with idf = ln(1 + (N − df + 0.5)/(df + 0.5)). k1 and b are the index's
FULLTEXT.K1 (1.2) and FULLTEXT.B (0.75). The statistics are those of the
full-text index of the document's column, the first by name of several;
of no index, every idf is 1.

`Database::register_ranking` adds a ranking, named by an upper-case
identifier, which an index names by FULLTEXT.RANKING (default BM25,
changed in place). An unknown name is SQLCODE -103; a ranking giving a
weight above its own bound is -910. `ORDER BY RANK(doc, p) DESC FETCH
FIRST n ROWS ONLY`, with a CONTAINS lookup of the same column, reads the
index best first and stops early. A character string or CLOB document
[EXTENSION] is read in its index's FULLTEXT.LANGUAGE, else ENGLISH.

### Not implemented

FT_DEFINITION_SCHEMA's base tables [§10] are a model only; the
information schema views are system tables of the same columns.

## SQL/MM Spatial

ISO/IEC 13249-3:2016, cited by its draft international standard (DIS,
SC32 N2593), is built in (RULES.md D119 to D132,
over D85). A bare `§` in this part is of that text. The code cites it
as `[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §N]`.

The manual is [doc/sql/10-spatial.md](doc/sql/10-spatial.md) for the
types and routines, [doc/sql/16-topology.md](doc/sql/16-topology.md),
[doc/sql/17-linear-referencing.md](doc/sql/17-linear-referencing.md)
and [doc/sql/18-spatial-networks.md](doc/sql/18-spatial-networks.md).
The guide is [doc/data/02-spatial.md](doc/data/02-spatial.md).

The standard has no feature IDs. §23.3 lists lettered claims instead,
and no claim of §23 is made yet. What is not built is said in each
section below. What turborel adds beyond the standard is under
[Geometry representations](#geometry-representations).

### Types and the model [§4–§10]

The geometry types are built in (`src/geometry.rs`, RULES.md D85,
D119, D120). They are ST_GEOMETRY, also named
GEOMETRY, and its 26 subtypes of §4.2. A value of any type and
coordinate dimension is read from and written to WKB [§5.1.68] and
stored canonically. No registration is needed.

`=` and `<>` are ST_OrderingEquals [§5.1.65]. ORDER BY, GROUP BY,
DISTINCT and the orderings are refused. No key is of a geometry value,
so PRIMARY KEY, UNIQUE and FOREIGN KEY of a geometry column are refused
when declared, with SQLCODE -103.

| Feature | Rules | What turborel does |
|---|---|---|
| Type hierarchy | §4.2, Figure 1 | All 27 types are SQL type names, each with an optional SRID parameter. The four that hold no values of their own (ST_Geometry, ST_Curve, ST_Surface, ST_Solid) refuse NEW. |
| Subtyping | 9075-2 §9.2 SR2, §6.17, §8.19 | A value can be assigned to a type above its own. Assigning it to a type below its own needs TREAT, which raises 0D000 at run time. IS OF and IS OF (ONLY) are supported. |
| Casts | §4.2.1.5 Table 4 | CAST within the hierarchy, else 2FF08. The ST_ToXxx cast methods of §5.1.55, each also a function. |
| Ordering | §5.1.65 | EQUALS ONLY BY RELATIVE with ST_OrderingEquals, the exact ST_Equals of x and y (RULES.md D126). Two empty values of one SRID are equal. PRIMARY KEY, UNIQUE and FOREIGN KEY of a geometry column are refused at declaration [9075-2 §11.7 SR1, §9.12 SR3], SQLSTATE 2A000. |
| Methods | §4.2, 9075-2 §9.18 | A method table per type, with overriding. The collection types override ST_Geometries. `v.m()` binds by the declared type and runs by the most specific type. `(v AS T).m()` runs T's. |
| SPECIFICTYPE | 9075-2 §6.15 | `v.SPECIFICTYPE` names the most specific type in INFORMATION_SCHEMA, the built-in types' schema. The function form SPECIFICTYPE(v) is not built. |
| Collections | §10.1.3 to §10.1.5 | ST_NumGeometries, ST_GeometryN (01F01 out of range) and ST_Geometries, accessor and mutator. |
| Relationships | §4.2.2, §5.1.43 to §5.1.54 | ST_Relate, ST_Equals, ST_Disjoint, ST_Intersects, ST_Touches, ST_Crosses, ST_Within, ST_Contains and ST_Overlaps, exact on binary64 (`src/spatial/planar`, RULES.md D126). An `rtree` index plans each but ST_Disjoint as it plans ST_Intersects, and ST_Relate when its pattern is a constant asking an interior or boundary of each to meet (RULES.md D135). |
| Set operations | §5.1.31 to §5.1.40, Tables 6 to 11 | ST_Intersection, ST_Union, ST_Difference and ST_SymDifference over mixed dimensions, typed by Tables 8 to 11 (`routines/results.rs`). |
| Buffer, hull and boundary | §5.1.17, §5.1.29, §5.1.30 | ST_Buffer with and without a unit, ST_ConvexHull and ST_Boundary. |
| Validity and simplicity | §5.1.8, §5.1.10, §7.1.8 | ST_IsValid, ST_IsSimple and ST_IsRing. |
| Measures | §5.1.41, §7.1.2, §8.1.2 to §8.1.8, §10.3.5, §10.5.3 to §10.5.9 | ST_Area, ST_Length, ST_Perimeter (each with a unit form), ST_Centroid, ST_PointOnSurface and ST_Distance with a unit. Values with arcs or other curves are measured exactly by the curve engine (RULES.md D124), linear ones by the planar engine. |
| Polygons from rings | §8.3.9, §10.6.7 | ST_BdPolyFromText, ST_BdPolyFromWKB, ST_BdMPolyFromText and ST_BdMPolyFromWKB. The standard has no GML forms of them. |
| Curves | §7 | The routines of the nine curve kinds (`src/geometry/observers.rs`, RULES.md D124), with the unit forms the clause gives, ST_DistanceToPoint's and ST_PointAtDistance's among them. ST_PerpPoints has none [§7.1.15]. ST_BulgeNormal and ST_Normal return ST_Vector values, ST_StartAngle and ST_EndAngle ST_Angle values, ST_RefLocation an ST_AffinePlacement whose reference directions are the axis vectors, and ST_ControlPoints and ST_Knots arrays of ST_NURBSPoint and ST_Knot (`src/geometry/observers/records.rs`). |
| Surfaces and TINs | §8, §17.1 | The routines of the six surface kinds, ST_TIN of elements and of triangles, ST_TINElements as an observer and a mutator, and the ST_TINElement type with its empty constructor, observers, mutators and ST_IsEmpty (`src/geometry/tins.rs`, `src/geometry/tin_element.rs`). |
| Constructors | §6.1.2, §7.2.2 to §7.10.2, §8.2.2 to §8.6.2, §10.1.2 to §10.6.2 | `NEW ST_X(...)` and the function ST_X of each instantiable type of §6 to §10, every form its clause defines: the text, binary and GML forms with an SRID, a point of x, y, z and m, a curve of its points or parameters, with measures, a unit and an SRID, a circular string of a center or of bulges, a circle of a center and a normal, a surface of its rings, a polyhedral surface of its patches and a collection of its elements (`src/geometry/mutators/constructors.rs`). |
| Mutators and observers | §5.1, §6.1.3 to §6.1.7, §7.2 to §7.10, §8.1 to §8.6, §10.3 | Each attribute's mutator, checked as its clause checks it: ST_X, ST_Y, ST_Z and ST_M; ST_Points, ST_Curves, ST_ExteriorRing, ST_InteriorRings and ST_Patches; ST_RefLocation, the axis lengths, angles, measures, control points, knots, scale factor, distances, length, curvatures and spiral type, with the unit forms. Also ST_Dimension, ST_CoordDim, ST_Is3D, ST_IsMeasured, ST_Envelope, ST_EnvelopeAsPts, ST_MinZ to ST_MaxM, ST_ExplicitPoint, ST_IsWorld, ST_IsClosed and ST_PerpPoints of a multicurve, and the ToSQL methods (`src/geometry/mutators.rs`). Every routine of §5 to §10 binds and runs. |
| Locating by measure | §5.1.13 to §5.1.16 | ST_LocateAlong and ST_LocateBetween, and their 3D forms. |
| Routines in space | §5.1, §8, §9 | The 3D relationships, set operations, distance, measures, boundary, simplicity and closure, the shells of a solid, ST_MaxSideLength, ST_Clip and ST_TINTable (`src/geometry/solids.rs`, RULES.md D127). ST_BRepSolid's constructors from surfaces [§9.2.2] and the routines of ST_CompoundSurface beyond measures over its patches are not built. |
| Table functions | §8.6, §11, §12, §13 | The mechanism of RULES.md D125. ST_TINTable, the topology and network checks and the shortest paths use it. |
| SQLSTATEs | §22 Table 16 | All 98 conditions are defined, and the model raises 2FF05, 2FF06, 2FF07, 2FF08, 2FF10, 2FF11, 2FF12, 2FF16, 2FF18, 2FF23, 2FF25, 2FF66, 2FF67, 2FF75, 2FF77, 2FF78, 2FF84, 2FF86, 2FF96 and 2FF98. Defect, 2FF01 to 2FF03 clash with 13249-2's. Each part raises its own (RULES.md D132). |

#### Readings of the standard

- Casts [§4.2.1.5]. A subtype to a supertype is the identity. An empty
  value becomes the target's empty value. A collection of one element
  becomes that element, except when the target is a collection type.
  Anything else raises 2FF08.
- ST_Equals of two empty values is 1. ST_Intersects, ST_Disjoint and
  the relationships give NULL of an empty operand.
- Relationships [§4.2.2.1, §5.1.43 to §5.1.54]. All are exact on the
  stored binary64 coordinates. There is one exception. When a crossing
  binary64 cannot hold lies within half a unit in the last place of
  another feature, snap rounding may make them meet. The standard
  leaves the computation implementation-defined (Annex A).
- ST_OrderingEquals and `=` [§5.1.65]. These are the exact ST_Equals of
  x and y. When both values have z, GR2b asks for x, y and z, but the
  engine is planar, so z is not compared.
- Equals [§5.1.43]. Defined through SymDifference being empty, it is
  evaluated as the equivalent pattern `**F**FFF*`. Two empty values are
  equal.
- Overlay operations [§5.1.31 to §5.1.37]. They work over mixed
  dimensions and return the most specific type for what remains, one of
  Tables 7 to 11's sets. Crossings binary64 cannot hold are rounded to
  the nearest position.
- Aggregates. The standard has no union or intersection aggregate, so
  the engine has none.
- Buffer [§5.1.30]. It takes only a distance. The result is the point
  set at most that distance away, empty below zero and the value itself
  at zero. Round joins and ends are 32-sided polygons inscribed in their
  circles, so a buffer lies inside the exact set and within about 0.5%
  of the distance of its edge. A distance that is not finite is an
  error.
- Scope. Nothing beyond the standard is offered. There is no covers or
  covered-by relationship. The validity reason (`validate`) is an
  engine diagnostic, not a routine.
- ST_IsValid [§5.1.10]. It follows the well-formedness conditions of
  §7.2.1, §8.2.1 Descriptions 11 to 14, §8.3.1 and §10.6.1. A polygon's
  interior ring may touch its exterior ring, or another interior ring,
  in one point, as long as the interior stays connected.
- ST_IsSimple [§5.1.8]. Surfaces are always simple. Repeated
  consecutive positions are not an anomaly.
- ST_BdPolyFromText and ST_BdMPolyFromText [§8.3.9, §10.6.7]. The
  implementation-dependent assembly takes faces inside an odd number of
  rings, so ring order and direction do not matter. A ring that is not
  a linear ring is "invalid argument". So are rings that do not make
  exactly one polygon for the ST_Polygon form.
- ST_Area, ST_Length, ST_Perimeter, ST_Centroid and ST_PointOnSurface
  [§7.1.2, §8.1.2 to §8.1.8, §10.3.5, §10.5.3 to §10.5.9]. They are
  Euclidean in coordinate units, summed with compensation. ST_Centroid
  is the null value for a value with no surface, as only surfaces define
  it. ST_PointOnSurface gives an interior point of the largest polygon,
  checked exactly.
- ST_Distance [§5.1.41]. It is exactly zero for operands that meet or
  nest. Otherwise it is the least point-to-segment distance, accurate to
  a few units in the last place.
- Curves are paths of pieces. Arcs and segments are measured exactly,
  splines, clothoids and spirals by adaptive Gauss-Legendre quadrature,
  and linearized to a relative deviation of 1e-3 of the curve's extent
  (RULES.md D124). ST_CurveToLine has no parameters in the
  standard, so it linearizes so. There is no ST_LineToCurve in the
  standard.
- Surface measures are planar. Geodesic areas are measured in the
  coordinate plane, and lengths through the geodesy of the value's
  system.
- ST_PointAtDistance gives z and m by linear interpolation along the
  piece.
- NURBS knots need not have end multiplicity degree+1. Any valid knot
  vector of points + degree + 1 is taken.
- A clothoid's t is arc length scaled by its scale factor (§7.8,
  A² = R·L).
- A 2D circle's normal orientation is the sign of z of its three points'
  turn.
- ST_MidPointRep, the center and the radius of a collinear arc return
  null.
- Point tolerance is 1e-12 relative for ends meeting, and ON_CURVE 1e-9
  for ST_DistanceToPoint's point on a curve.
- An invalid position gives the warning 01F01.
- ST_PerpPoints joins arcs meeting end to start into one curve.
- ST_LocateBetween interpolates M linearly along a curve's linear form.
  Of a surface, which the standard leaves to the implementation, it
  locates along its rings.
- TIN [§8.6]. Stop lines remove the triangles they pierce (the ISO 19107
  reading). A draped element takes the base surface's height.
- §5.1.31 to §5.1.49 in space. A pair whose systems differ raises
  2FF10, as the 2D routines do. Both values lacking z is checked first
  (2FF72).
- 3D set operation results are x, y and z, m dropped (§5.1.31
  Description 4), in SELF's SRID, coplanar faces merged, the most
  specific type. An empty result is an empty ST_GeomCollection.
- ST_3DDistance with a unit. SRID 0 and SRIDs the catalog does not hold
  measure in metres. A geographic system has no linear unit, so a named
  unit raises 2FF14. Supported units are ST_UNITS_OF_MEASURE's LINEAR
  rows. Area units are squared, volume cubed.
- ST_3DIsSimple. Polygons and curve polygons are simple, as validity
  covers them. Patch surfaces, multisurfaces and solids are simple when
  no two patches meet except along both rims. A multicurve's curves
  meet only at points on both their boundaries. A collection is simple
  when its elements are.
- ST_3DBoundary of a collection is the collection of its elements'
  boundaries, of a solid its shells, and of a patch surface the edges
  one patch uses, chained into line strings.
- ST_3DLocateAlong and ST_3DLocateBetween interpolate z and m linearly
  along each segment, and consecutive stretches join. Surfaces and
  solids (implementation-defined, §5.1.16 D2f) give an empty point.
- ST_3DSlope is rise over run, the tangent of the angle to the x-y
  plane. A vertical triangle raises 2FF02.
- ST_3DCentroid of a 2D surface returns an XY point, and with z an XYZ
  point. Points returned by 3D methods carry no m.
- ST_3DPointOnSolid is the centroid when inside, else a point just
  inside a face, else a boundary vertex.
- ST_3DIsClosed of a surface means no edge is used by exactly one patch
  ring. ST_Surface's definition (§8.1.1) declares ST_Is3DClosed while
  §8.1.11 is titled ST_3DIsClosed. Both spellings are registered, for
  ST_Curve, ST_Surface and ST_MultiCurve, and run the same routine.
- ST_IsShell is simple and closed (§8.1.13). A polygon is never a shell.
- ST_ExteriorShell(ST_Surface) and ST_InteriorShells(ARRAY). Every shell
  must be a closed simple surface with z and without m (2FF73, 2FF97
  otherwise). Shells meeting in more than points, an interior shell not
  within the exterior, or two interior shells overlapping raise 2FF02.
  A null array element raises 2FF03. Interior shells of a solid with no
  exterior raise 2FF98.
- ST_InteriorShellN out of range gives null and warning 01F01.
- ST_MaxSideLength(length, 1) with no elements retriangulates the
  triangle corners as random points. A null length raises 2FF03 even of
  a null SELF. A length not above 0 raises 2FF02.
- ST_Clip cuts each triangle by the polygon in plan and lifts the pieces
  to its plane. The clip polygon replaces the boundary elements, draped
  on the surface (z of the triangle under it, else the nearest
  corner's). Other elements are kept.
- ST_TINTable(). Points are the elements' points plus the triangles'
  corners, so each triangle can name its corners (D3c lists element
  points only), unique, sorted by x, y, then z. Visibility is -1 within
  a hole, else 0 within a void or crossed by a stop line, else 1 (the
  least that applies). A multipoint element is a row of its points'
  numbers (D3g covers only line strings and polygons, and D5c reads
  them back). Hole rows use item '<type> hole'.
- ST_TINTable() is the table function `TABLE(ST_TINTABLE(tin))`. A
  method returning a table cannot be a scalar method, so there is no
  `tin.ST_TINTable()` method form.
- ST_TINTable(name, columns..., maxsidelength). Names are identifiers
  (a table may be schema-qualified), regular ones folded to upper case
  and delimited ones kept. Anything else, or a table that does not
  exist, raises 2FF02. A column that does not exist raises 2FF02 too,
  since AppendedQuery names the columns its first table must have and
  the binder appends nothing without them. Rows alike count once. A
  hole row joins the element of its type, ID and tag. The result has
  SRID 0 with max side length set (D5's ST_MaxSideLength(.., 0)).
- ST_3DDistanceToPt takes an ST_Point argument. An ST_Geometry needs
  TREAT.
- SQLSTATE 2FF01 to 2FF03 are shared with Full-Text's class (RULES.md
  D132).
- A curve's start and end measures are both set or both removed. A
  mutator of one measure on a curve without measures sets both to the
  value, and the null value removes both [§7.6.2 Description 24c].
- The mutators of a curve given by parameters raise 2FF02 of an empty
  curve, which has no parameters to keep.
- ST_X and ST_Y of the empty point raise 2FF18, as no point has one
  coordinate alone. ST_Z and ST_M of a value raise 2FF03 there.
- The patches of a polyhedral surface are adjacent when they share an
  edge, each with at least one other and no edge with more than two.
- ST_IsWorld is 0. No surface stored covers the whole earth.
- ST_MinZ, ST_MaxZ, ST_MinM and ST_MaxM are of the linear form of a
  curve, its points and stroked arcs.
- ST_Point(1, 2, 3) is x, y and the SRID 3, the form of three integers
  [§6.1.2 Description 11]. NEW ST_Point(1E0, 2E0, 3E0) has z 3.
- ST_Envelope widens a flat extent by a small amount, so that the
  rectangle of a point or a line along an axis is a polygon.
- A solid in the routines that ignore z [§5.1.29 to §5.1.54] is the
  region of x and y its faces cover. Their results are values of x and
  y, so the ST_Solid cells of Tables 8 to 11 give surfaces.
- ST_IsValid of a solid is 1 when each shell is closed and the patches
  meet only along their boundaries [§9.2.1 Description 8 to 10].
  ST_IsSimple of a solid is that second test, in space, as z is what
  makes a solid simple.
- CAST of a character string to a geometry type reads it as
  ST_GeomFromText does, of any type and coordinate dimension.
- Negative zero is read as zero before a value is checked, so a ring
  closed by -0 and 0 is closed.

### Reference systems [§14, §20]

The reference systems are those of ST_SPATIAL_REFERENCE_SYSTEMS
[§20.3], seeded and defined, computed by one engine
(`src/spatial/srs`, RULES.md D122, D121).

| Feature | Rules | What turborel does |
|---|---|---|
| Spatial reference systems | §20.3, §14.1.9 | 125 seeded, EPSG 3857, 4258, 4269, 4326, 4978 and 32601 to 32660, 32701 to 32760, as well-known text. Others are defined and dropped by ST_DEFINESRS and ST_DROPSRS [EXTENSION], keeping §20.3's constraints. |
| Well-known text of a system | §14.1.9 | PROJCS, GEOGCS and GEOCCS are read and written. |
| ST_Transform | §5.1.6 | Moves points and the points that define curves between any two systems the statement's catalog holds (`routines/transform.rs`). Elliptical curves, clothoids, spirals and rational NURBS curves fail with 2FF15, as Description 3c allows. |
| Geodetic measures | §5.1.41 | Distance, length and area of a geographic system by Karney's geodesics. |
| Units | §19.4, §20.4 | 12 seeded units. A unit form converts through them. SRID 0 measures in metres. A geographic system has no linear unit and gives 2FF14. |
| Registered columns | §4.11, §20.2 | A column registered with a system holds only values of that system, checked as each row is written and when it is registered, SQLSTATE 23000. |
| The ST_SpatialRefSys type | §14.1 | Not built yet. ST_SpatialRefSys, ST_WKTSRSToSQL and the methods of §14.1 are not there. |

#### Readings of the standard

- §14.1.9 accepts the AUTHORITY, TOWGS84 and AXIS items of the OGC text
  as [EXTENSION]s, and a signed number wherever a `<number>` appears.
  Malformed text always raises 2FF22 (§14.1.2 Description 2 leaves it
  optional).
- §14.1.5 makes SRID 0 the unspecified Cartesian plane. It has no text,
  and no transform reaches or leaves it (2FF15). Text with an EPSG
  AUTHORITY takes that code as its SRID.
- §14.1.6 ST_Equals holds when two systems place every coordinate at the
  same point. The kind, the datum (normalized name, ellipsoid, TOWGS84),
  the prime meridian, the unit factors, the axis directions, and the
  projection method and parameters in metres and radians are compared.
  Names and authorities are ignored.
- §5.1.6 Description 3d, the transform pipeline, goes through the axes,
  unprojection, geocentric Helmert shifts through WGS 84 when the datums
  differ, then the target's steps reversed. Datums with no shift raise
  2FF15. Points outside a projection's domain raise 2FF15. M values pass
  through unchanged.
- §5.1.41 Description 2a iv makes geodetic distance the shortest
  ellipsoidal geodesic (Karney 2013).
- The seeded GRS 1980 systems 4258 and 4269 have a zero TOWGS84, so
  every seeded system transforms to every other.
- Values' SRIDs are labels (D85). The standard asks no SRID to name a
  defined system. SRIDs are implementation-dependent (§14.1.5
  Description 2), and ST_SRID(INTEGER) sets any value (§5.1.5
  Description 5). So a type naming an SRID that no system has still
  takes values. Only a lookup, by ST_Transform or a unit form, needs the
  system.
- §4.11 ties a column to a system only through ST_GEOMETRY_COLUMNS, and
  says such a column holds only values of that system, modelled as the
  CHECK `COL.ST_SRID() = SRS_ID`. A column of `GEOMETRY(srid)` already
  has its SRID checked on assignment.
- A system ST_DEFINESRS defines as LOCAL_CS is stored, but every lookup
  of it gives 2FF22. ST_DEFINESRS does not check its definition.
- A constant folded at planning knows only the seeded systems, so a
  call naming another is left for its statement. A seeded system cannot
  be dropped and a defined one cannot reuse a seeded SRID, so the two
  never disagree.
- The units are those of ST_UNITS_OF_MEASURE, and a metre is the base
  of the LINEAR ones.

The accuracy of each projection against its published worked example
is in RULES.md D122. The residuals are at most a few
millimetres against EPSG Guidance Note 7-2, and tens of millimetres
against Snyder's examples printed to 0.1 m.

### Formats, angles, directions and support types [§16–§18]

The well-known text, WKB and GML of the geometry types
[§5.1.56 to §5.1.68] are read and written by `src/spatial/formats` and
the model (`src/geometry/representations.rs`, RULES.md
D124). The types of §14, §16 and §17.2 to §17.5 are SQL
types with every routine the standard gives them, and the routines of
§18 are built (`src/geometry/support`, RULES.md D123). The
angle and direction engine of §16 computes them (`src/spatial/srs`).

| Feature | Rules | What turborel does |
|---|---|---|
| Well-known text | §5.1.67 | Every instantiable type, in Z and M too, by every *FromText routine, ST_WKTToSQL, ST_GeomFromText and ST_AsText. ST_AsText of the seven basic XY kinds keeps D85's writer. |
| WKB | §5.1.68 | Every type, both byte orders, the new Z/M/ZM codes and the deprecated codes on input. An empty value of each type is written in full. |
| GML | §5.1.60 to §5.1.64, Table 14 | Every instantiable type, by every *FromGML routine, ST_GMLToSQL, ST_GeomFromGML and ST_AsGML. srsName is the organization and its identifier, read back through the Spatial Information Schema's systems as the statement is bound. GML carries no M. |
| Large objects | §5.1.4, §5.1.5, §5.1.62, §5.1.63 | ST_GEOMFROMTEXT takes a CLOB and ST_GEOMFROMWKB a BLOB, each read whole up to 16 MiB. ST_AsText returns a CLOB and ST_AsBinary a BLOB, up to 16 MiB. |
| ST_SpatialRefSys | §14.1 | Built, with its constructors of a definition and of an SRID, ST_WKTSRSToSQL, ST_SRID, ST_AsWKTSRS and ST_Equals. EQUALS ONLY by ST_OrderingEquals. A definition takes the SRID of a system supplied or defined with the same text. |
| ST_Angle, ST_Direction | §16.1, §16.2 | Built, with every constructor, observer, mutator, method and function of both. ORDER FULL by ST_OrderingCompare, so `<`, ORDER BY, GROUP BY, MAX and an index work on them. |
| ST_TINElement | §17.1 | Built, with its constructors, the empty one among them, its observers, mutators and ST_IsEmpty. |
| ST_Vector | §17.2 | Built, with its constructors, observers, mutators and its text, WKB and GML routines. EQUALS ONLY by ST_OrderingEquals. |
| ST_AffinePlacement, ST_NURBSPoint, ST_Knot | §17.3 to §17.5 | Built, with their constructors, observers, mutators and ST_IsEmpty, and ST_Transform of a placement. They declare no ordering, so nothing compares them. They are also read and written as parts of a curve's text and binary forms. |
| Transform groups and casts | §14.1.8, §16.1.20, §16.2.22, §17.2.21 | Each group is a CAST: the well-known text to and from a character string, an angle's or a direction's radians to and from a number, a vector's WKB to and from a binary string. The GML groups are the types' GML routines. INFORMATION_SCHEMA.TRANSFORMS shows none, as a built-in type is no user-defined type. |
| ST_Geometry ARRAY support routines | §18 | Built: ST_MaxDimension, ST_CheckSRID, ST_GetCoordDim, ST_GetIs3D and ST_GetIsMeasured, of geometry, vector and NURBS point arrays as each clause gives them; the procedures ST_CheckNulls and ST_CheckConsecDups; and the nineteen functions ST_ToPointAry to ST_ToBRepSolidAry. |

#### Readings of the standard

- Typos in the DIS text are read as meant. The WKB NURBS productions all
  name `<wkbnurbszm>`. The WKT `<angle text>` cites "Subclause 15.1.21"
  for §16.1.21. ST_TIN's elements clause cites a stale number.
- A `<bit>` is one octet. A TIN element's id is an int32 in the
  element's byte order, and BRepSolid is code 1025.
- Defect. The representation of an empty parametric curve is not given,
  so it is written as a NaN placement with no direction.
- Bulges. GML ArcByBulge's bulge is the sagitta over half the chord. The
  2D normal is its sign on z.
- GML extensions where GML 3.2.1 has no element are EllipticalCurve (ISO
  19107 form), SpiralCurve, a weights element on BSpline, and a
  gml:element (type, id, tag) per TIN element. GML 3.2.1 Tin's
  stopLines, breakLines, controlPoint and maxLength are read. GML 3.3
  Tin is not read.
- An empty value in GML is its element with no content.
- WKT. A nested `<z m>` may be left out and is then the outermost one's.
  Given, it must match (rule ai). `<just z>` is strict (rule ak). An
  empty `<length text>` is NaN. A spiral type is any letters, and an
  unknown one is 2FF02. A NURBS point or knot that is EMPTY is 2FF02.
  Weights all 1 read as non-rational. Angles are written in radians. TIN
  elements are written with ID always.
- The well-known text and GML of the four non-instantiable kinds do not
  exist. The 23 instantiable kinds are covered.
- The standard has no worked WKT or GML examples to test against. Tests
  use values built here.
- §16.1.5 to §16.1.7 take the components by FLOOR, so −10.5° is
  −11° 30′ 0″. The misplaced parenthesis of §16.1.7's Definition is read
  as (degrees − D − M/60) × 3600.
- §16.1.8 rounds the seconds and carries 60″ upward. ST_String(dms) reads
  "D M S" as D + M/60 + S/3600, so it inverts ST_String.
  ST_MaxAngleString and ST_MaxDirectionString are both 64.
- §17.2.5's Definition and Description give ST_Z(DOUBLE PRECISION)'s two
  conditions the other way round. The Description is followed: 2FF03 of
  a value for an empty vector, 2FF16 of a null for one that is not.
- ST_Vector's well-known text is `VECTOR (x y)` or `VECTOR Z (x y z)`,
  a third coordinate needing the Z (2FF25). Its WKB is code 101, or 1101
  with z, little endian on output. Its GML is a gml:vector with srsName.
- ST_VectorFromGML(gml) is of SRID 0, as Description 2 says, whatever
  srsName the GML names.
- A §18 routine passes over a null element of the array where its
  clause compares dimensions, as the comparison is unknown.
- ST_CheckNulls and ST_CheckConsecDups are procedures, CALLed. They are
  built in, so ST_CheckNulls takes either array type.
- §16.1.2 Descriptions 17, 19 and 23 give the lesser clockwise angle in
  [0, π]. The Definition's CASE, which yields the negative difference,
  is not followed.
- §16.2.2 directions between points are planar azimuths from +y,
  clockwise. The constructor ST_Direction('S', a) follows Description
  7c.
- §16.2.17 and §16.2.18 reduce into [0, 2π), including the case where a
  tiny negative result would reduce to exactly 2π.
- §16.1.15 and §16.2.6 GML use a gml:Angle element and a
  gml:DirectionVector with horizontalAngle and verticalAngle. `uom`
  takes an EPSG URN or code (9101, 9102, 9103, 9104, 9105, 9122) or a
  unit name. With no `uom`, the value is in radians. A missing element
  is 2FF24 and a non-angular unit is 2FF14.
- ST_ApproximatePi is the binary64 nearest π.

### Topology [§11]

Every routine of §11.3, the views of §11.1 and the topology exceptions
of §22 Table 16 are built (`src/spatial/topology`, RULES.md
D128).

| Feature | Rules | What turborel does |
|---|---|---|
| A topology | §11.1, §11.2 | A schema of its name, with base tables ST_TOPO_NODE, ST_TOPO_EDGE and ST_TOPO_FACE under the views ST_NODE, ST_EDGE and ST_FACE. |
| ST_InitTopoGeo, ST_CreateTopoGeo | §11.3 | ST_InitTopoGeo makes a topology, and ST_CreateTopoGeo fills one from a geometry collection. |
| Editing routines | §11.3 | Every routine that adds, moves, removes, splits, heals or changes nodes, edges and faces. |
| Reading routines | §11.3 | ST_GetFaceEdges, ST_GetFaceGeometry and ST_ValidateTopoGeo are table functions. ST_GetFaceGeometry is also a function of any value expression. |
| Privileges | §11.3 | Reading the views is the SELECT check (2FF63). A view that is not there is 2FF62. |

#### Implementation-defined and implementation-dependent choices

- ST_MaxTopologyName is 128, the identifier length.
- §11.2's base tables are realised per topology schema without the
  TOPOLOGY column. §11.2.1 makes them a model only, and §11.1.1 allows
  objects of a topology beyond the clause.
- Identifiers generated are max + 1 of the table. A replaced edge's id
  is never reused by its replacements.
- ST_RemEdgeModFace keeps the left face (§11.3.15 2e iv,
  implementation-dependent).
- ST_CreateTopoGeo numbers nodes by position (x, then y) and edges in
  the order its noder finds them. The modification of the collection
  (§11.3.18 2i v 1) is noding. Every curve is cut where it crosses or
  touches another or passes through a point. Crossing points are
  rounded to binary64.
- The topology name of ST_GetFaceEdges, ST_GetFaceGeometry and
  ST_ValidateTopoGeo must be known when the statement is prepared.
- The functions that change a topology are invoked where a function
  that possibly modifies SQL-data may be (INSERT VALUES rows, SET, CALL
  arguments, program values), not in a query.

#### Readings of the standard

- ST_Crosses(curve, point) is undefined (§5.1.51). "Edge crosses node"
  is read as the curve passing through the point off its ends.
- A nonexistent node or edge in ST_MoveIsoNode, ST_RemIsoNode,
  ST_RemIsoEdge and ST_ChangeEdgeGeom raises 2FF46 or 2FF43, as the text
  updates no row.
- ST_MoveIsoNode literally makes no face check, and its coincidence
  check includes the node itself.
- Table 16 has no "not isolated edge". ST_RemIsoEdge of a connected edge
  raises 2FF50.
- A closed isolated edge (start = end) is added as ST_AddEdgeModFace
  adds it.
- A heal of an edge with itself raises 2FF40.
- ST_RemEdgeNewFace returns NULL unless a new face is made.
- New ids are max + 1, taken before the replaced rows go, so
  ST_NewEdgesSplit and ST_NewEdgeHeal never reuse the replaced edge's
  id.
- A split keeps the given point's z and m if it has the edge's
  dimensions, else interpolates.
- ST_ValidateTopoGeo lists each unordered pair once, lesser id first.
  Face 0 is never "face without edges".
- ST_GetFaceGeometry is XY. It gives an empty polygon if the rings make
  no valid polygon.
- ST_GetFaceEdges rotates rings to their least signed edge, sorted by
  it.
- ST_CreateTopoGeo nodes by an exact O(n²) noder, and orders nodes by
  position.
- A topology name is valid when it is not empty and at most 128
  characters (ST_MaxTopologyName).

### Networks [§12]

All 16 routines of §12.3 and the views of §12.1 are built
(`src/spatial/network`, RULES.md D129). The 7 that return
INTEGER (ST_AddIsoNetNode, ST_AddLink, the four splits and
ST_NewLinkHeal) are functions, the rest procedures. ST_ValidLogicalNet
and ST_ValidSpatialNet are table functions.

| Item | Reading |
|---|---|
| §12.1, §12.2 tables | A network is a schema with base tables ST_NET_NODE and ST_NET_LINK under views ST_NODE and ST_LINK (§12.2.1, the tables are a model) |
| §12.3 Definitional Rule 1 | ST_MaxNetworkName is 128. A longer name fails on assignment (22001) |
| §12.3 Description 2b, 2c | Views not there are 2FF64, not readable 2FF65 |
| §12.3.2, §12.3.3, §12.3.6 | Moving or removing a node or link that is not there does nothing |
| §12.3.5 | `anothernode` read as `alink`. A link not there is 2FF58 |
| §12.3.4, §12.3.5 | An empty curve's ends match no node (2FF57, 2FF33). Ends are compared as ST_Equals |
| §12.2.2 Description 5 | Mixed SRIDs are 2FF10, the replaced geometry left out |
| §12.3.10, §12.3.11 | Splits act on the linear form. Results are linestrings. A point at an end is 2FF55 |
| §12.3.8 to §12.3.11 | New link identifiers are found before the split link is removed |
| §12.3.12, §12.3.13 | A link healed with itself is 2FF41, mixed dimensions 2FF25. Curves are joined without a repeated point |
| §12.3.16 | Nodes merged by exact XY. Crossings not noded. Empty elements skipped. Surfaces and solids 2FF02 |
| §12.3.14, §12.3.15 | Topology not there 2FF47, view not there 2FF48, name 2FF62, privilege 2FF63 |
| §12.3.18 | One mixed-SRID row over all geometries |

#### Readings of the standard

- A network name is a character string of at most ST_MaxNetworkName
  (128) characters, the parameter typed CHARACTER VARYING(128). A longer
  one fails on assignment to the parameter (22001), before the routine.
- ST_InitTopoNet of an empty name gives 2FF64. A schema of that name
  already there gives 2FF56 (§12.3.7 Description 2b).
- A network's views that cannot be read give 2FF64 when they are not
  there and 2FF65 when the user lacks the privilege (§12.3 Description
  2b, 2c).
- Removing or moving a node, or removing a link, that is not there does
  nothing. The Descriptions check only that the node is isolated, and a
  node that is not there has no link.
- ST_ChangeLinkGeom of a link that is not there, with a non-null curve,
  gives 2FF58. Its Description checks the node geometries of the link's
  nodes, and there are none.
- The `anothernode` in §12.3.5 is a typo for `alink`.
- Updates and inserts happen once all checks pass, with a null curve
  too. A null curve makes a link logical.
- An empty curve's end never equals a node's point, so it gives 2FF57
  or 2FF33.
- A node is compared with a curve's end as ST_Equals does. A point on a
  link is decided by exact orientation, with no tolerance.
- Mixed SRIDs give 2FF10, and the geometry being replaced is left out of
  the check, so a network of one node can be moved to another system.
- Splits work on the curve's linear form, and the parts are linestrings.
  Z and M are interpolated along the segment. The point must lie within
  the curve, not at either end, and an end gives 2FF55. A closed curve's
  ends are within it, but a split there would make a part of no length,
  so it is 2FF55 too.
- A split's new link identifier is found before the split link is
  removed, so no new link takes the old one's identifier.
- A heal of a link with itself gives 2FF41.
- Heals test the four ways two links meet in the order the text gives
  them (end-start, end-end, start-start, start-end). The healed link
  runs in the first link's direction. The curves are joined without
  repeating the meeting point, and joined by a segment if their ends
  differ.
- A heal of curves of different coordinate dimensions gives 2FF25.
- A heal where one link has a curve and the other none gives a link of
  no curve.
- ST_SpatNetFromGeom makes points nodes and curves links. Nodes are
  merged where their XY bits are equal. Crossings are not noded. Ids are
  numbered from 1 in the order of first appearance, and empty elements
  are skipped. A surface or solid gives 2FF02.
- ST_LogiNetFromTGeo and ST_SpatNetFromTGeo read a topology's ST_NODE
  and ST_EDGE views. A topology that is not there gives 2FF47, a view
  that is not there 2FF48, a name that is no name 2FF62, and a read the
  user may not make 2FF63. A network not empty gives 2FF42.
- ST_ValidSpatialNet's mixed-SRID row is one row over all geometries,
  with both primitives null.
- Both checks of an empty network give 2FF31.

### Shortest paths [§13]

ST_ShortestUndPath and ST_ShortestDirPath are built-in table functions,
with and without the weight column (`src/spatial/paths`, RULES.md
D130).

| Item | Reading |
|---|---|
| §13.1 Description 6 | The unweighted cost is planar length in coordinate units |
| §13.1 Description 8 | Rows of null values or non-curve geometry are skipped |
| §13.1 Description 11 | Every tied path, as simple paths sorted by id list. Ties by exact equality. More than 65535 paths or 2^26 steps fail (-900) |
| §13.1 | Names must be known at bind time. A point to itself is an empty path of weight 0 |

#### Readings of the standard

- The table name is parsed as a `<table name>` after spaces are trimmed.
  Column names are matched exactly, as delimited identifiers.
- The table and column names must be known when the query is bound,
  that is literals or parameters known then. A column reference gives
  SyntaxRule.
- A table, view or column that is not there, or not readable, gives
  2FF02.
- Nodes are curve end points keyed by their XY bits, with -0 taken as 0.
- A row with a null id, geometry, start point or weight is skipped. A
  row whose geometry is not a curve is skipped (Description 8).
- Two rows of one id give 2FF05.
- A negative, non-finite or non-number weight gives 2FF02.
- With no weight column the weight is the planar length, in coordinate
  units (Description 6).
- In the directed form, a start point at neither end of its curve gives
  2FF02. A start point at the end reverses the curve.
- A `from` or `to` point that is no curve's end gives 2FF02. An empty
  table gives no rows, and so does a pair of points with no path.
- Every tied shortest path is returned. Paths are simple, sorted by
  their id lists. Ties are exact f64 equality of the sums.
- The path from a point to itself is an empty array of weight 0.
- Limits are MAX_PATHS = 65535 (the array's cardinality) and MAX_STEPS =
  2^26 walk steps. Past either the query fails with SQLCODE -900
  (Unsupported), never a partial answer. A path longer than the array
  gives 2202F.

### Linear referencing [§15, §21]

The thirteen types of §15 are built with their constructors, observers,
mutators and methods, the functions of their well-known text and GML,
and the §21 catalog tables and views ST_LRMS and ST_LR_COLUMNS
(`src/spatial/lr`, RULES.md D131).

| Feature | Rules | What turborel does |
|---|---|---|
| Types | §15.1 to §15.13 | All thirteen, with their routines, ST_VectorOffsetExp's constructor of an ST_Vector array and ST_Vectors() among them. |
| Text and GML | §15.14 | The functions of their well-known text and GML. A value's character form is its well-known text. |
| Catalog | §21.1, §21.2 | ST_LRMS and ST_LR_COLUMNS, changed by ST_DEFINELRM, ST_DROPLRM, ST_REGISTERLRCOLUMN and ST_UNREGISTERLRCOLUMN [EXTENSION]. |
| Lengths and cardinalities | Annex A | A routine checks each argument against its parameter's length and cardinality (22001, 2202F), as storing it would. |

#### Readings of the standard

- §15 defines no ST_AsText or ST_AsGML for the linear referencing types.
  None is added. A value's character form, from a cast or a display, is
  its well-known text.
- §15.8.1 lists the observer as ST_LRMeasure(), and §15.8.4 defines it
  as ST_Measure(). ST_Measure is what is built.
- §15.3.1 declares ST_FeatureID() RETURNS ST_LinearElement. That reads as
  a typo, since every other mention is a feature ID string. It returns
  CHARACTER VARYING(ST_MaxFeatureIDLength).
- §15.6 declares ST_Equals "INTEGER SELF AS RESULT". It is built as a
  method returning INTEGER.
- §21.1.3's view selects ORGANIZATION_COORDSYS_ID, a column the base
  table does not have. ORGANIZATION_LRM_ID is used.
- The §21.2.2 foreign key references ST_LINEAR_REFERENCING_METHODS, a
  table §21 never defines. ST_LRMS is used.
- §21 names its schema ST_DEFN_SCHEMA. The tables are in
  ST_DEFINITION_SCHEMA, beside those of §20.
- An offset vector has no spatial reference system. A vector given to
  ST_VectorOffsetExp or ST_Vectors(ST_Vector ARRAY) loses its SRID, and
  ST_Vectors() gives vectors of SRID 0. A null element is the empty
  vector.
- The standard gives no statement to change ST_LRMS or ST_LR_COLUMNS
  (§21.1.1 leaves it implementation-defined). The procedures
  ST_DEFINELRM, ST_DROPLRM, ST_REGISTERLRCOLUMN and ST_UNREGISTERLRCOLUMN
  do it. ST_DEFINELRM checks the definition's ST_LRMID against LRM_ID
  (§21.2.3 Description 8).
- A routine that finds an LRM or a linear element by identifier reads
  ST_LRMS and every registered column of a linear element type. The
  query is appended as an ARRAY as the statement is bound, through the
  `catalog_query` hook. Only registered columns are searched, and
  reading them needs SELECT on them.
- A geometry parameter accepts any geometry type at bind time. The
  routine checks the subtype at run time (2FF08).
- ST_Point and ST_Curve exist as geometry functions too. One dispatcher
  per name picks the linear referencing routine when an argument is of a
  linear referencing type, and otherwise the geometry function. Every
  other capability the geometry function declares (its spatial
  operation, index and batch promises, context, preparation) is the
  dispatcher's too, read of the geometry arguments no routine of §15
  takes, so its R-tree plans and unit forms are kept. A preparation of
  the geometry function is wrapped again with the routines of its name.
- The overload is chosen by declared types at bind time and by values at
  run time. A null value matches any parameter, so an overload choice
  can be ambiguous when every argument is null. The first routine in
  table order wins.
- The units are those of ST_UNITS_OF_MEASURE of type LINEAR, and a metre
  is the base, so metres_per_unit is 1.
- GML srsName resolves against the supplied reference systems only, not
  ones defined by ST_DEFINESRS.

### The Information Schema and sizings [§19, §20, Annex A]

The views of ST_INFORMTN_SCHEMA are system views over the system tables
of ST_DEFINITION_SCHEMA (RULES.md D121).

| Feature | Rules | What turborel does |
|---|---|---|
| Information Schema | §19.1 to §19.6 | ST_INFORMTN_SCHEMA with all seven views. SELECT is PUBLIC's. |
| Definition Schema | §20.2 to §20.5 | System tables, read only through the views. A statement naming one gets -103. |
| Geometry columns | §20.2 | ST_REGISTERGEOMETRYCOLUMN and ST_UNREGISTERGEOMETRYCOLUMN [EXTENSION] keep §20.2's constraints. A registration goes with its column. |
| ST_SIZINGS | §19.5, §20.5, Annex A.1 | A row for each A.1 meta-variable with an integer value, from the constants the code enforces. |

#### Readings of the standard

- Defect. §19.2 finds geometry columns through
  INFORMATION_SCHEMA.DIRECT_SUPERTYPES, which has no rows for built-in
  types. The view joins COLUMNS to the base table instead, which lists
  the columns of a geometry type.
- A column whose SRID is not a seeded system has SRS_NAME NULL, as of a
  column with no system [§20.2 Description 3].
- ST_SIZINGS leaves out ST_ApproximatePi, ST_TypeCatalogName and
  ST_TypeSchemaName, which are not integers. 0 means no limit and NULL
  means not supported.

| Sizing [Annex A.1] | turborel |
|---|---|
| ST_MaxGeometryAsText, ST_MaxGeometryAsBinary | 16 MiB |
| ST_MaxGeometryAsGML | 16777216 characters, the text limit; a longer GML representation, written or read, raises 22001 |
| ST_MaxTopologyName | 128, the identifier length |
| ST_MaxNetworkName | 128, the identifier length |
| ST_MaxTopologyOrNetworkName | 128, the lesser of the topology and network name limits |
| ST_MaxLRMNameLength, ST_MaxFeatureIDLength | 128 |
| ST_MaxConstraintLength | 256 |
| ST_MaxConstraintArrayElements, ST_MaxReferentArrayElements, ST_MaxStartValueArrayElements, ST_MaxPositionExpArrayElements | 1024 |
| ST_MaxLRAsText, ST_MaxLRAsGML | 16 MiB, the geometry text limit |
| ST_MaxArrayElements | 65535, the array cardinality limit, so the paths of one shortest path |
| ST_MaxLEMeasureArrayElements | Not supported, since no ST_LEMeasure type is defined by the text |
| ST_MaxAngleString, ST_MaxDirectionString | 64, the engine's longest strings |
| ST_MaxAngleAsText, ST_MaxDirectionAsText | 64 |
| ST_MaxAngleAsGML, ST_MaxDirectionAsGML | 4096 |
| ST_MaxSRSAsText | 2048, the length of a system's definition |
| ST_MaxVectorAsText, ST_MaxVectorAsGML | 8192 |
| ST_MaxVectorAsBinary | 29 |
| ST_MaxVectorArrayElements | 65535, the geometry array limit |

## Simple feature access, SQL option

OGC 06-104r4, Simple feature access part 2, SQL option (ISO 19125-2), is
built in its "SQL with Geometry Types" environment over SQL/MM Spatial
(RULES.md D136). A bare `§` in this part is of that text, which the code
cites as `[OGC 06-104r4 §N]`. Part 1, OGC 06-103r4, is cited for the
text and binary forms. The manual is the section "Simple feature access"
of [doc/sql/10-spatial.md](doc/sql/10-spatial.md).

The claim is the conformance class (gS, f, 3D, M, tS) of Annex A.4,
which is the geometry types environment, its features, values of z and
of m, and annotation text of SQL types [Annex A.2.6.2]. Its tests T1 to T52 of Annex C.3.3 run in
`test/integration/simple_features/blue_lake.rs`.

| Feature | Rules | What turborel does |
|---|---|---|
| SPATIAL_REF_SYS | §7.1.2 | A view of SF_INFORMTN_SCHEMA over the systems supplied and those ST_DEFINESRS defines, with §7.1.2.2's columns. SELECT is PUBLIC's. |
| GEOMETRY_COLUMNS | §7.1.3 | A view of SF_INFORMTN_SCHEMA of every column of a geometry type a user may see, of a table or view, typed or registered, with §7.1.3.2's columns. |
| Unqualified names | §7.1, Annex C.3.3.3 | `GEOMETRY_COLUMNS` and `SPATIAL_REF_SYS` name the views where the default schema has no table of the name. |
| Geometry types | §7.2.2 | GEOMETRY, POINT, CURVE, LINESTRING, SURFACE, POLYGON, POLYHEDSURFACE, GEOMCOLLECTION, MULTIPOINT, MULTICURVE, MULTILINESTRING, MULTISURFACE and MULTIPOLYGON name SQL/MM's ST_ types. |
| Feature tables | §7.2.3 | Tables of columns of those types. |
| Routines | §7.2.4 to §7.2.19 | SQL/MM's routines, called by their names without ST_ too, functions and methods. |
| GeometryType, AsText, AsBinary | §7.2.8, Annex C.3.3.3 | Functions of their own, giving the type's name without ST_ and Part 1's text and binary forms. |
| PolyhedSurface | §7.2.14 | Its six constructors through NEW, and NumSurfaces, SURFACE and ST_Geometries, ST_NumGeometries and ST_GeometryN of its patches. |
| Text | §7.2.20 | ANNOTATION_TEXT, ANNOTATION_TEXT_ELEMENT and ANNOTATION_TEXT_ELEMENT_ARRAY, each with its constructor and every method of the clause. |
| Text metadata | §6.2.9.2 | ANNOTATION_TEXT_METADATA, a view of SF_INFORMTN_SCHEMA of every column of type ANNOTATION_TEXT a user may see, with Table 3's columns. |
| Well-known text and binary | OGC 06-103r4 §7, §8 | Read in both standards' forms; Part 1's written by AsText and AsBinary. |

### Readings of the standard

- §7.1.1 lets the tables be views. Both live in SF_INFORMTN_SCHEMA, as
  SQL/MM's ST_INFORMTN_SCHEMA already holds views of those names with
  other columns. Procedures, ST_DEFINESRS and ST_REGISTERGEOMETRYCOLUMN,
  change what they show, and they are read only.
- An unqualified name of a view takes it as its correlation name, so
  `GEOMETRY_COLUMNS.SRID` is its column.
- A column's SRID is the one it is registered with, else its type's,
  else 0, SQL/MM's unspecified system. SPATIAL_REF_SYS has no row of 0,
  so §7.1.3.2's foreign key holds of every other SRID only.
- STORAGE_TYPE and MAX_PPR are NULL, as nothing is stored normalized
  [§7.1.3.3 d, h]. G_TABLE_* equal F_TABLE_* [§7.1.3.3 c].
- COORD_DIMENSION is 3 for a column in a geocentric system and 2
  otherwise, since a type does not say whether its values have z.
  GEOMETRY_TYPE then adds Table 4's 1000 of z.
- GEOMETRY_TYPE is Table 4's code, or the code of Part 1's Table 7 it
  is a subset of. Codes 8 to 12, 16 and 17 are those of the curves,
  surfaces and collections Table 4 leaves out. A type neither codes, a circle,
  NURBS curve, compound surface or solid, has NULL.
- SPATIAL_REF_SYS's SRTEXT is NOT NULL, as every system has a
  definition. AUTH_NAME and AUTH_SRID are its organization's, NULL when
  ST_DEFINESRS gave none.
- A routine without ST_ of a first argument of a geometry type is the
  method of its declared type, chosen before a registered function of
  the name, so CONTAINS of two geometries is spatial. Of a supertype's
  value it is SQL/MM's function of the name, or the method of the one
  subtype declaring it, of the value treated as that type. A value of
  another type raises the routine's exception.
- GeometryType gives `MULTILINESTRING`, as T7 expects, and
  POLYHEDSURFACE for a polyhedral surface. ST_GeometryType keeps
  SQL/MM's `ST_MultiLineString`.
- AsText and AsBinary write Part 1's forms. They differ from SQL/MM's
  only for a triangle (a polygon of one ring), a polyhedral surface
  (patches without PATCHES) and a TIN (triangles as polygons, nothing
  after them). ST_AsText and ST_AsBinary keep SQL/MM's. A TIN's elements
  and maximum side length have no Part 1 form and are left out.
- The readers take both forms. A Part 1 TIN has no elements and no
  maximum side length. Binary bytes are read as SQL/MM's first, then as
  Part 1's.
- The constructors are SQL/MM's without ST_. GeomCollFromText calls
  ST_GeomCollFromTxt.
- UNION and OVERLAPS are reserved words, and INTERSECTION is a set
  function. Each before `(` at the start of an operand is a call, so
  `Union(a, b)`, `Overlaps(a, b)` and `Intersection(a, b)` work.
  INTERSECTION of one argument is still the set function.
- POSITION, the column of Annex C.3.3.1's bridges and buildings, is a
  key word and is delimited, `"POSITION"`.
- T37 compares a value of SRID 101 with one of SRID 1, which SQL/MM's
  ST_Equals refuses. The test uses SRID 101 for both.
- A structured type a schema defines of a name of §7.2.2.2, POINT say,
  is the type that name names in that schema.
- The text types are built in as the geometry types are, with no
  ordering, so no comparison, key or index takes them. A value is shown,
  and CAST to a character string, as the constructors that make it. The
  clause gives the text no well-known text or binary [Annex A.2.6 Note].
- MaxArraySize, which the clause leaves open, is 4096. It bounds a
  text string, text attributes and an array's elements. A longer string
  raises 22001 and a longer array 2202E.
- §4.9.3.3 of ISO/IEC 9075-2 names a constructor method as its type, so
  the element's constructor is ANNOTATION_TEXT_ELEMENT. The clause's
  AnnotationTextElement is a function of the same four arguments.
- Each type has the constructor function of §4.9.3.3, its attributes
  null. An element is then given a value without a leader line through
  the methods of one argument. Every other routine returns NULL on NULL
  input, as the clause declares, so a null argument gives the null
  value.
- The clause gives ANNOTATION_TEXT_ELEMENT_ARRAY no constructor. It has
  one of an array of elements, none of them null (2FF09), and the
  constructor function, whose array is empty. ElementN(n) past either
  end is the null value. ElementN(element, n) replaces element n, or
  adds it after the last for n one past it, and raises 2202E for any
  other n.
- A text's envelope, which the clause names without defining, is the
  envelope ST_Envelope gives of its elements' locations and leader
  lines, or NULL if they have none. Geometries of two SRIDs raise 2FF10.
  CONCAT appends the other text's elements and so widens the envelope.
- ANNOTATION_TEXT_METADATA's F_TEXT_COLUMN is named as §6.2.9.2's table
  names it, where Table 3 writes F_TEXT_COLUMN_NAME. Its three defaults
  are NULL, since §6.2.9.2 says nothing of how one is set. Each text
  value then holds its own text and attributes. A text column is not a
  geometry column, so GEOMETRY_COLUMNS does not list it.

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

### UUID

UUID, of RFC 9562, is built in, so it needs no plug-in, and is otherwise
a type of the table above. See [Data Types](doc/sql/02-data-types.md#uuids)
and [UUID_V4 and UUID_V7](doc/sql/04-expressions.md#uuid_v4-and-uuid_v7).

| Facility | RULES.md | What turborel does |
|---|---|---|
| UUID | D180, D181, D184 | A type of 16 octets, `value/types/uuid.rs`. Its literal is `UUID '...'`. Text is read hyphenated, braced, in either case or as 32 bare digits, and written hyphenated in lower case. A binary string of 16 octets casts to it and from it. Compared, ordered, grouped and hashed by its octets, so it takes a primary key, a unique constraint and a btree or hash index. COLUMNS gives its DATA_TYPE as UUID with no length or precision. The native protocol carries it as tag 27. |
| UUID_V4(), UUID_V7() | D182, D183 | A random UUID of version 4, and a time-ordered one of version 7 from the database's clock, `value/types/uuid/generate.rs`. A version 7 UUID never sorts before the last one its database made. |

### XMLMAP

A value expression mapping a table, a schema or the catalog to XML and
to an XML Schema document, as Part 14 defines the mappings but gives them
no syntax [ISO/IEC 9075-14:2006 draft §4.10.9 NOTE 13] (RULES.md D106,
`parser/xml/map.rs`, `binder/xml/map.rs`, `functions/xml/map.rs`). Its
clauses are written in this order, each at most once.

```
XMLMAP ( { TABLE <table name> | SCHEMA <schema name> | CATALOG }
         [ NULLS { ABSENT | NIL } ]
         [ AS { FOREST | ELEMENT } ]
         [ TARGETNAMESPACE <character string literal> ]
         [ DATA | METADATA | DATA AND METADATA ]
         [ ENCODING { BASE64 | HEX } ] )
```

| Clause | Parameter | Default |
|---|---|---|
| NULLS ABSENT, NIL | NULLS absent, nil [§9.11 SR1] | ABSENT |
| AS FOREST, ELEMENT | TABLEFOREST True, False | ELEMENT |
| TARGETNAMESPACE | TARGETNS | '', no namespace |
| DATA, METADATA, DATA AND METADATA | DATA True, METADATA True, both | DATA |
| ENCODING BASE64, HEX | ENCODING | HEX |

Its value is the XML value the draft's XMLPARSE makes of XS and XD,
which XMLSERIALIZE writes as XSR and XDR: of METADATA the XML Schema
document, of XML(DOCUMENT(UNTYPED)); of DATA the data, of
XML(DOCUMENT(UNTYPED)), or of a table AS FOREST XML(CONTENT(UNTYPED));
of both, the schema document's document node then the data's, of
XML(SEQUENCE). A target namespace of `xml`'s or `xmlns`'s, or holding a
character no XML character, a table that is a query a `<with clause>`
names, and XMLMAP in a constraint are SQLCODE -103; a schema that is not
there SQLSTATE 3F000.

### Geometry representations

GEOMETRY is a second name of ST_GEOMETRY, the root type of
[SQL/MM Spatial](#sqlmm-spatial) (`src/geometry.rs`, RULES.md D85,
D120). This section lists what turborel adds beyond that
standard. The types and functions are built in, so every database has
them. Sessions call the functions by name while SQL.FUNCTION_CALLS is
ON, its default. They are deterministic, so a CHECK, assertion, domain
constraint or generated column may call them, and keeps its calls when
the setting is OFF later (RULES.md D74, D133).

The optional nonnegative SRID parameter of every geometry type, using
D53's existing type syntax, enforces the reference on assignment, CAST
and storage, including empty values. The unparameterized type permits
any signed 32-bit identifier. EWKB flags are refused. Empty points of
the seven XY kinds of D85 use the NaN-pair WKB convention, so their
stored payloads are unchanged.

The free functions ST_POINT(x, y), ST_X, ST_Y, ST_SETSRID,
ST_MAKEENVELOPE, ST_MINX, ST_MAXX, ST_MINY and ST_MAXY are extensions,
as are the function forms of the standard's methods (RULES.md D87).
Point ordinate observers return NULL for an empty point and refuse
other geometry kinds. ST_GEOMETRYTYPE returns the standard's names,
including ST_GeomCollection. ST_SETSRID changes only the identifier.
ST_MAKEENVELOPE constructs a rectangular polygon from four numeric
bounds and an optional SRID. It refuses non-finite, reversed and
zero-area bounds (`Geometry::rectangle`), and it does not implement
SQL/MM's ST_Envelope method. ST_MINX, ST_MAXX, ST_MINY and ST_MAXY
return DOUBLE PRECISION extrema of any kind, or NULL for an empty point
set.

The catalog procedures ST_DEFINESRS, ST_DROPSRS,
ST_REGISTERGEOMETRYCOLUMN, ST_UNREGISTERGEOMETRYCOLUMN, ST_DEFINELRM,
ST_DROPLRM, ST_REGISTERLRCOLUMN and ST_UNREGISTERLRCOLUMN are
extensions, since the standard defines the tables but no procedure
(RULES.md D121, D131).

ST_INTERSECTS and the other relationships are exact
(`src/spatial/planar`, RULES.md D126). A filter comparing
one of them to 1 can use an R-tree on a `GEOMETRY(srid)` column with a
constant or parameter query geometry (`physical/access/spatial.rs`,
`index/rtree/geometry.rs`). Envelope candidates are rechecked against
the visible geometry and full condition. Mixed-reference parameters
fall back to all entries, preserving runtime errors.
Inner joins between two columns constrained to the same SRID can probe
one geometry index for each outer row (`physical/indexed/spatial.rs`).
Both table orders and column projections are considered; the complete ON
condition rechecks each visible pair. Other raising conjuncts and raising
inner filters prevent this path. LEFT, RIGHT and FULL outer joins probe
it too, keeping the rows no probe matched with nulls
(`physical/indexed/spatial/outer.rs`, `exec/join/indexed/preserved.rs`).
ST_Relate of a constant pattern asking the values to meet, and
ST_Distance compared `<` or `<=` with a limit, in any unit that converts,
read a window, the distance's widened by the limit
(`index/rtree/geometry/within.rs`, RULES.md D135). A value of another
SRID, or a tree whose distances may overflow, reads every entry, so the
error is raised as a scan raises it. The ON condition or filter must be
one of these relationships or a distance limit, as no envelope test of
its own is in SQL/MM, and a distance compared `>` or `>=` reads the
table. A join of two columns of one SRID,
each with an R-tree, on ST_Intersects or a meeting relationship, may
descend both trees together, pairing the entries whose envelopes meet,
inner or outer, when both tables' statistics put the pairs at a quarter
an outer row or fewer, each pair rechecked against the visible rows, the outer
operand's filter and the full condition
(`physical/indexed/spatial/paired.rs`, `exec/join/indexed/paired.rs`,
RULES.md D138).
Under SERIALIZABLE, a filter's search and each join probe record the
envelope window they read, not the whole table, and a change conflicts
when its old or new envelope meets one (`exec/read.rs`,
`storage/region.rs`, RULES.md D134).
Choosing between a scan and a spatial probe is planning only, outside
the standards. A geometry column's statistics keep a grid of its
envelopes, its mean vertices and its clustering. A broad window's rows
are estimated from the grid, and every path is priced for the spatial
calls it evaluates and a probe for how closely its rows lie, as EXPLAIN
shows. The filter over the access estimates the same rows
(`stats/envelopes.rs`, `physical/access/geometric.rs`,
`physical/access/filter.rs`, RULES.md D137). No result depends on the choice.
An unordered inner join whose remaining outer rows fit its memory
budgets can choose a single visible table scan when index estimates price
it below repeated probes (`exec/join/indexed/adaptive.rs`). A temporary
spatial hierarchy prunes groups; four boundary masks per leaf select
envelope candidates; exact geometry relationships and the remaining ON condition
still decide every result. Memory refusal resumes the buffered input
through probes. Input past MEMORY.WORK_BYTES is spilled, and when a scan
still costs less, both inputs are partitioned by a grid of envelopes and
joined a cell at a time, each pair produced only in the cell of its
reference point (`exec/join/indexed/adaptive/spilled.rs`, RULES.md D138). Snapshot visibility and serializable reads use the
same storage contracts as ordinary scans.
Optional setup waits until completed probes have read at least a table's
worth of candidates, avoiding repeated estimates for a selective join.
Column-only projections can be built directly from the two source rows
after exact intersection refinement (`exec/join/indexed/refined.rs`).
Repeated columns and their order are preserved; additional ON conditions
still evaluate before projection. This changes row construction only.
For the proven same-SRID adaptive join, the relationship and its residuals
cannot raise a SQL expression error. Its cursor streams unless another
part of the plan can raise; such plans still evaluate eagerly before OPEN
returns (`physical/raising.rs`, D15). Stored-data errors are not suppressed.
Column-only spatial output gives its first row directly, then uses at most
64 rows of lookahead, reserving
its buffer within the memory budgets. Refusal preserves the current row
after its buffered prefix and resumes direct streaming. Storage errors
remain ordered after their preceding rows (`db/stream/chunks.rs`).
ST_DISTANCE returns the approximate minimum Cartesian distance in
coordinate units, zero for intersections and NULL for an empty set
(OGC 06-103r4 §6.1.2.4, `src/geometry/distance.rs`). The same SRID and
polygon validity requirements apply. Outward-rounded interval filters
fall back to exact dyadic arithmetic for uncertain endpoint classifications
and perpendicular determinants; the final magnitude is approximate.
Distance norms have explicit axis/L1 bounds and binary scale factors.
Finite geometry distances cannot fall below their conservative envelope
bound, which never increases when either envelope expands (`src/spatial.rs`).
Unrepresentable final distances raise 22003.
Relationship calls may retain a decoded constant operand with a database
memory reservation. Polygon and MultiPolygon constants can additionally
retain per-ring Y-interval trees for point location
(`src/geometry/prepared/locator.rs`). They use the same exact crossings,
closed boundaries and hole rules as ordinary evaluation; an insufficient
budget leaves the decoded operand without an index. A single, closed,
axis-aligned rectangular ring without holes can classify a point directly
against its closed bounds (`src/geometry/prepared/rectangle.rs`). Prepared
distance reuses that intersection decision and keeps the ordinary
separation kernel. Indexed joins can prepare their outer geometry once
per probe, preserving the full ON condition and runtime errors.
Reused physical plans prepare calls with each
execution's arguments when constant folding is enabled; failed preparation
preserves ordinary evaluation, including lazy branches and runtime errors.
Indexed geometry plans can learn despite different prepared function
objects: probe shapes compare the declared spatial semantics, retaining
ordinary function identity checks for other predicates.
The index extension API can return geometry distance candidates in monotone
envelope-bound order (`Index::nearest`, `src/index/nearest.rs`). Its finite
range certificate includes every root split sibling and may decline a
request before output. ST_DISTANCE with NULLS LAST and a known OFFSET
plus FETCH FIRST count, ONLY or WITH TIES, can select these candidates on
a fixed-SRID geometry column (`physical/nearest.rs`, `exec/nearest.rs`),
of a distance in a unit too, each bound converted as the measure is.
Ascending reads the nearest first, descending the farthest first by
upper bounds (RULES.md D135). PERCENT and NULLS FIRST read the table.
The full filter and visible geometry are evaluated before ranking; a
strict cutoff keeps distance ties for the final SQL sort. Exhausted
candidates are supplemented with qualifying NULL distances when needed.
Uncertified requests and an unaffordable threshold heap scan before output.
WITH TIES, PERCENT and other orders retain ordinary plans. Serializable
reads, cancellation, prepared values and runtime errors are preserved.
Simple feature access (ISO 19125-2) is claimed in its own part,
[below](#simple-feature-access-sql-option).

### Indexes

These are RULES.md D34, of the methods `btree`, `hash`, `rtree`,
`fulltext`, `json`, the vector methods [below](#vector-indexes), or a plug-in's. The statements run outside a transaction. The `rtree` method
indexes two approximate-numeric point coordinates or four rectangle
coordinates and answers independent SQL comparisons (`index/rtree.rs`,
`physical/access/columns.rs`, D72). Inner joins can bind these comparisons
from each outer row (`physical/indexed/coordinates.rs`). Single-column
geometry indexes use the projection contract described above (D85).
An `rtree` built over existing rows, by CREATE INDEX, a rebuild or
recovery, is packed by Sort-Tile-Recursive from the root down through
the sorted spill. Nodes that deletes empty or leave under two fifths
full, leaves and then the branches above them, are taken out or copied
into a merged node, their pages freed by epochs, so scans neither miss
nor repeat an entry. The ancestors' envelopes are then narrowed, and a
root left with one child gives way to it (`engine/rtree/pack.rs`,
`engine/rtree/reclaim.rs`, `engine/rtree/reclaim/narrow.rs`,
`Index::load_beside_writers`, RULES.md D139).

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

The methods `xml_path`, `xml_value`, `xml_property` and `xml_primary`
index one XML column (RULES.md D106, `src/index/xml.rs`), after the
PATH_VALUE, VALUE, PROPERTY and primary indexes of Pal et al. (VLDB
2004): each element and attribute of a value by its path, its value as a
string and as a number, and its ORDPATH, keyed in that method's order;
the primary's, the rows of the Infoset by row and ORDPATH, and by row,
path and value. The primary answers a query of a row from its entries
alone, a path, comparisons and filters of its steps
(`Index::xml_answers`, `index::xml::XmlQuery`), checking a row only where
a value is cut, a comparison raises or the row's versions differ. CREATE INDEX refuses them on
another column, or on more than one (SQLCODE -103). No SQL comparison
probes them, as XML values are not comparable; the Rust API asks them for
the rows that may hold a node of a path of child and descendant steps
with a value a general comparison selects (`Index::xml_rows`,
`xml_cost`, `index::xml::XmlLookup`), which the caller checks
(`XmlLookup::matches`, one pass over the stored form), and
`Database::xml_candidates` reads those rows. The planner reads them, by
cost against a full read, for a WHERE or ON conjunct of XMLEXISTS, of
XMLQUERY NULL ON EMPTY IS NOT NULL, and for XMLTABLE joined to its rows
not by LEFT, of a query of the column alone, of any XML type (of
XML(SEQUENCE), the row of every value not one document node given by each
lookup and checked, its query never answered by the index), that is one
path of child, descendant and attribute steps, `self` and `parent`
steps among them, and a last step of the ancestor, following or preceding
axes, a relation of an element to the node before, which `xml_primary`
answers by the nodes' ORDPATHs [Pal et al. §2.2], or of a kind test, read
by the lookup of the path before, not exactly; each step of predicates, each a relative path or its comparison with a
literal, or a conjunction of them [XQuery §3.6]; several such conjuncts
of the column are read together (`xquery/lookup.rs`,
`physical/access/xml.rs`). A probe gives the rows every lookup of the
query gives, its path's and each predicate's [Pal et al. §3.3], or, of a
predicate compared with a number or by a value comparison, the rows its
lookup gives too. `xml_primary` answers such a query, exact, of every
predicate's node fixed, from its entries (`Index::xml_answers`), a row it
may hold of checked by the query's own test; of the others each row
given is fetched and checked. Each is priced against a full read that
checks every row's value (`XmlCost::of_check`), and a table another
conjunct of which may raise an error is read whole (RULES.md D15). A
conjunct of ON, or of WHERE over a product, that may raise an error, of
one table's columns, is read by an index of that table below the guard
it is moved over, as any index method's conjuncts are
(`physical/access/guarded.rs`).
EXPLAIN shows the query and whether the index answers it. A value whose
nodes or keys pass their bounds is refused, SQLCODE -900.

### Full-text search

TEXT_MATCHES(document, words) is true of a document holding every word
of `words` that is not a stop word, true of a search of stop words
alone, and null of a null argument (`fulltext/matches.rs`). Both are
character strings or large objects, read a piece at a time. It reads
both in English and compares words normalized, not stemmed. ISO/IEC
13249-2:2002 (CD, SC32 N0640) searches by its FULLTEXT type's CONTAINS
method instead, [below](#sqlmm-full-text).

The index method `fulltext` answers TEXT_MATCHES of one character string
column (`index/fulltext.rs`, RULES.md D86). Its options are
FULLTEXT.LANGUAGE, ENGLISH or NONE, and FULLTEXT.STOP_WORDS, DEFAULT,
NONE or a list of words, each a rebuild; and FULLTEXT.K1 (0 to 1000,
1.2) and FULLTEXT.B (0 to 1, 0.75), BM25's weights, changed in place. A
column with a collation is refused (SQLCODE -900), as its key is its
sort key and not its text. A word the index takes for a stop word has no
entries, so a search of it reads the table. A document's places are
sorted in a work area of 4 MiB before its postings are written, spilling
beyond it (D38).

13249-2 leaves the tokenizer to the implementation [§4.1, §4.4, §4.5].
These are turborel's choices (`fulltext/text.rs`).

| Choice | turborel |
|---|---|
| Word | A run of Unicode 13.0 letters, numbers and marks, two joined by one connector (Pc) or, between letters, one apostrophe (U+0027 or U+2019). Each Han, Hiragana or Katakana character is a word |
| Sentence end | `.`, `!` or `?`, with closing brackets and quotes after it, then a blank, a line terminator or the end; U+3002, U+FF01, U+FF0E and U+FF1F alone |
| Paragraph end | Two line terminators with only blanks between, or U+2029 |
| Ordinals | Every word, stop words too, from 1; sentences and paragraphs from 1, counting those holding a word |
| Offset and length | In characters, as written |
| Normalization [§4.5] | Simple case folding, canonical decomposition, folding again, nonspacing marks (Mn) removed, U+2019 made U+0027 |
| Stop words [§4.4.2] | English, Snowball's list of 174; NONE, none |
| Stems [§4.2.3.4] | English, Porter2 (Snowball English, 2006); NONE, the word |
| Languages [§5.1.9] | ENGLISH, NONE; another is SQLSTATE 2FF02 |
| Word and term length | A word's first 256 characters; a term's first 255 bytes of UTF-8, cut at a character |

### JSON documents

CAST of a character string to JSON parses it as JSON ( ) does, and CAST
of JSON to a character string serializes it as JSON_SERIALIZE does,
where §4.8.1 and Table 6 cast JSON to JSON alone. A character string
assigned to a JSON site is parsed WITHOUT UNIQUE KEYS, so JSON text is
the input form of the type.

JSON_CONTAINS(document, candidate) is True if the candidate is
contained in the document (`functions/json/contains.rs`). A scalar is
contained if it equals the document, strings by code point and numbers
as SQL compares them. An object is contained if each member is in the
member of the same key, and an array if each element is in some element
of an array, or in the document itself if it is no array. It is null of
a null argument, and 22032 of an argument that is no JSON text.

The index method `json` answers JSON_EXISTS, JSON_VALUE compared equal
and JSON_CONTAINS of one column of JSON documents, a character string's
or the JSON type's (`index/json.rs`, RULES.md D91). A call is a lookup
if its other arguments are constants or parameters and it reads NULL ON
ERROR and NULL ON EMPTY, JSON_EXISTS's default FALSE ON ERROR among
them. Every row found is checked by the WHERE condition. Its options are
JSON.PATHS_ONLY, ON or OFF (OFF), and JSON.MAX_DEPTH, 0 to 128 (128),
each a rebuild. A column with a collation is refused, SQLCODE -900, as
its key is its sort key.

### Indexes of expressions

A key part of CREATE INDEX may be `( <value expression> )`, by `btree`
or `hash` (RULES.md D92).

| Use | Behaviour |
|---|---|
| Admitted | A column, a constant, a negation, a sum, difference or product of exact numbers fitting 18 digits, a CASE of such, UPPER, LOWER, and JSON_VALUE, JSON_QUERY and JSON_EXISTS that cannot raise and read no time zone. |
| Refused | A quotient, a sum or product that can overflow, a CAST, a subquery, CURRENT_DATE and other possibly non-deterministic functions, JSON ERROR ON ERROR, a path with datetime(), time_tz() or timestamp_tz(), a method other than btree or hash, each SQLCODE -103. |
| Planner | `=`, ranges, BETWEEN and IN of an expression of the same canonical form probe it; ORDER BY of a never-null one reads a btree index in order. |
| Maintenance | INSERT, UPDATE of an input column, DELETE, rollback, recovery, ALTER TABLE and temporary tables. Renaming an input column is refused, and dropping one drops the index. |
| Shown | INDEXES.COLUMN_NAMES gives the key part as written, in parentheses; EXPLAIN shows the probe. |

### Vectors

VECTOR is a built-in type beyond the standards (RULES.md D93), with its
distances, operators, arithmetic, aggregates and quantization, and
distance functions of SQL. No ISO standard defines it. It follows the
vector design document, which reads Microsoft SQL Server 2025's and
Oracle AI Database 26ai's vector features as one design and is cited
here as "vector §x" ([below](#the-vector-design-document-clause-by-clause)).
The SQL Reference Manual's [Vectors](doc/sql/14-vectors.md) and
[Hybrid Search](doc/sql/15-hybrid-search.md) and the
[Vector Guide](doc/data/04-vectors.md) describe them.

| Facility | Behaviour |
|---|---|
| VECTOR [vector §3] | `VECTOR [(dims, format, layout)]`, each `*` or left out for open. FLOAT32, FLOAT16, FLOAT64, INT8 and BINARY, DENSE or SPARSE. No order and no `=` (D93) |
| Text forms [vector §5] | TO_VECTOR, VECTOR(text), CAST of text, FROM_VECTOR RETURNING ... FORMAT, and a character string assigned to a VECTOR site |
| Distances [vector §6.1 to §6.3] | VECTOR_DISTANCE in both argument orders, seven metrics, the shorthand functions and the operators `<-> <=> <#> <+> <~> <%>` |
| Arithmetic and aggregates [vector §6.5] | `+ - * /`, SUM, AVG, VECTOR_MIN and VECTOR_MAX of each element |
| Quantization [vector §7.1] | VECTOR_QUANTIZE and VECTOR_DEQUANTIZE; PQ, SQ8 and BQ quantizers of an index |
| Distance functions [vector §8] | CREATE and DROP VECTOR DISTANCE FUNCTION of LANGUAGE SQL, with EXECUTE (D93) |
| Plug-ins | `Database::register_metric` and `register_quantizer`, a metric named `NAME` or `SCHEMA.NAME` |
| Catalog [vector §16] | INFORMATION_SCHEMA.VECTOR_COLUMNS and VECTOR_DISTANCE_FUNCTIONS |

### Vector indexes

The index methods `diskann`, `hnsw` and `ivf` index one dense VECTOR
column of fixed dimensions and format, FLOAT64 excepted, and `inverted`
one SPARSE column (RULES.md D95 to D99). A null is not indexed. Each
serves FETCH APPROX of its own metric only, EUCLIDEAN and
EUCLIDEAN_SQUARED serving each other, and an index of a NORMALIZED
column serves COSINE, DOT, EUCLIDEAN and EUCLIDEAN_SQUARED alike.

```
CREATE VECTOR INDEX name ON table (column) [ORGANIZATION ...] [DISTANCE metric]
    [WITH TARGET ACCURACY n] [PARAMETERS (...)] [QUANTIZATION ...] [RERANK {ON | OFF}]
CREATE VECTOR INDEX name ON table (column) [USING method] WITH (option = value, ...)
ALTER INDEX name {REBUILD [ONLINE] [PARAMETERS (...)] | MERGE DELTA | LOAD | UNLOAD}
```

Both forms desugar to CREATE INDEX ... USING with the method's options
(`parser/ddl`, [CREATE VECTOR INDEX](doc/sql/statements/create-vector-index.md)).
Each method's options, named `METHOD.OPTION`, are in INFORMATION_SCHEMA.INDEX_OPTIONS
and on that page.

| Method | Structure | Options group |
|---|---|---|
| `diskann` | A Vamana graph in pages, PQ codes and the first hops cached in memory (D96) | DISKANN. |
| `hnsw` | A layered graph in memory, an image and a log in pages (D97) | HNSW. |
| `ivf` | k-means centroids in memory, lists in pages written through (D98) | IVF. |
| `inverted` | Postings of a sparse vector's dimensions, searched by block-max WAND (D99) | INVERTED. |
| `hybrid` | A `fulltext` part and a vector part of two columns, or of a MODEL a `fulltext` index of the text with a table of chunks, their vector index and a pipeline (D100) | HYBRID. and each part's |

`Session::index_accuracy_report(index, samples, k)` gives an index's
recall against the exact answer (`db/indexes/accuracy.rs`), as
INDEX_ACCURACY_REPORT asks [vector §10.6], since a statement would give
no rows.

### Approximate nearest-neighbour search

FETCH {APPROX | APPROXIMATE | EXACT} {FIRST | NEXT} n {ROW | ROWS} ONLY
[vector §9.1, §9.2] asks for an approximate or an exact search
(RULES.md D94). APPROX takes WITH TARGET ACCURACY n (1 to 100, R9.4) or
WITH TARGET ACCURACY PARAMETERS (EFSEARCH n, NEIGHBOR PARTITION PROBES
n, SEARCH_LIST n, BEAM_WIDTH n). APPROX with WITH TIES or PERCENT is
refused, SQLSTATE 2A000, SQLCODE -103, as is an accuracy without APPROX
(`parser/query/ordered/approx.rs`, `binder/limit.rs`). Hints follow the
fetch clause, `HINT (VECTOR_INDEX(t i), NO_VECTOR_INDEX(t),
VECTOR_PREFILTER, VECTOR_INFILTER, VECTOR_POSTFILTER, VECTOR_WAIT_FRESH)`.

```
SELECT ... FROM VECTOR_SEARCH(TABLE = t [AS c], COLUMN = col, SIMILAR_TO = v,
    METRIC = 'm', TOP_N = n) [AS s]
```

VECTOR_SEARCH [vector §9.3] is read as a derived table of the table's
columns and DISTANCE, under the function's correlation name or the
table's (`parser/query/vector_search.rs`).

An approximate search returns only rows that satisfy the WHERE
condition, measured again by the query's own distance, and raises what
an exact search would (D15). Too few rows widen the search, at most 4
times, then the table is read exactly, unless VECTOR.APPROX_FALLBACK is
OFF, when the statement is refused with SQLSTATE 0A000, SQLCODE -900.
EXPLAIN shows the metric, the filter strategy, the target accuracy, the
search list, beam width, probes and rerank.

### Hybrid search

```
CREATE HYBRID VECTOR INDEX name ON table (text_column, vector_column)
    [PARAMETERS (VECTOR_IDX 'parameters', TEXT_IDX 'options')]
CREATE HYBRID VECTOR INDEX name ON table (text_column)
    PARAMETERS (MODEL m [, CHUNKER BY unit MAX n [OVERLAP k]] [, MODE {SYNC | ASYNC}]
    [, VECTOR_IDX 'parameters'] [, TEXT_IDX 'options'])
SELECT ... FROM HYBRID_SEARCH(INDEX = i, QUERY_TEXT = t [, QUERY_VECTOR = v], TOP_N = n
    [, CANDIDATES = c] [, FUSION = 'RRF' | 'WEIGHTED'] [, VECTOR_WEIGHT = w]
    [, TEXT_WEIGHT = w] [, RRF_K = k]) [AS h]
```

A hybrid index (RULES.md D100) is a `fulltext` index of the text column
and a vector index of the vector column, HNSW unless VECTOR_IDX's TYPE
or HYBRID.VECTOR_METHOD names IVF, DISKANN or INVERTED
([CREATE HYBRID VECTOR INDEX](doc/sql/statements/create-hybrid-vector-index.md)).
HYBRID_SEARCH reads each part's top CANDIDATES rows, TOP_N × 4 by
default, and fuses them by RRF, of K 60 by default, or by WEIGHTED
min-max normalized scores. Its columns are DOC_ID (the table's
one-column primary key), SCORE, VECTOR_SCORE, TEXT_SCORE, VECTOR_RANK,
TEXT_RANK and CHUNK_TEXT. EXPLAIN shows both parts' reads. The text
part serves CONTAINS of the text column too. An index of a MODEL is a
full-text index of its text, read by CONTAINS and RANK a document a
row, with a table of the text's chunks, a vector index of their
embeddings and a pipeline that keeps them. Its search gives each
document once, at its best chunk, and of no QUERY_VECTOR embeds
QUERY_TEXT by the model. ALTER INDEX of its MODEL, CHUNKER or MODE makes
its chunks again, and a database that stopped while one was made or
dropped drops it whole as it opens.

### Embedding models and pipelines

```
CREATE EMBEDDING MODEL name [FROM FILE 'path'] WITH (option = value, ...)
ALTER EMBEDDING MODEL name WITH (option = value, ...)
DROP EMBEDDING MODEL name [CASCADE | RESTRICT]
CREATE VECTOR PIPELINE name SOURCE table (columns) [CHUNK BY unit MAX n [OVERLAP m]
    [SPLIT_ON 'regex'] [NORMALIZE WHITESPACE]] EMBED USING model
    TARGET table (columns) [MODE {SYNC | ASYNC}]
DROP VECTOR PIPELINE name [CASCADE | RESTRICT]
GRANT EXECUTE ON EMBEDDING MODEL name TO ...
```

| Facility | Behaviour |
|---|---|
| Models [vector §12.1] | Schema objects of a registered `Embedder` plug-in, its TYPE (RULES.md D101). The database reads TYPE, VERSION, BATCH_SIZE, ON_ERROR, MAX_RETRIES, DIMENSIONS, FORMAT and LAYOUT, and gives the others to the plug-in. HASH_DENSE and HASH_SPARSE are built in |
| Calls [vector §12.2] | EMBED(model, text [, 'query' \| 'document']), VECTOR_EMBEDDING and EMBED_VERSION, of a model of the catalog with EXECUTE checked, or of a plug-in by name |
| Chunks [vector §12.3] | CHUNK_TEXT in FROM, `FROM CHUNK_TEXT(text [, chunking])`, with a correlation name and derived column list |
| Generated columns [vector §12.4] | `GENERATED ALWAYS AS (EMBED(...)) [STORED] [SYNC \| ASYNC]`, ASYNC filled after commit (D103) |
| Pipelines [vector §12.5] | Over existing tables, as four row triggers on the source, filled as made (D102) |
| Freshness [vector R12.6] | `/*+ VECTOR_WAIT_FRESH */` before a statement, outside a transaction, and `Database::wait_for_embeddings` |
| Views | INFORMATION_SCHEMA.EMBEDDING_MODELS, EMBEDDING_MODEL_PRIVILEGES, VECTOR_PIPELINES and EMBEDDING_QUEUE |

### Vector settings

These are RULES.md D36 settings (`settings/vector.rs`), listed in
[Appendix C](doc/appendix/c-settings.md).

| Setting | Scope | Default | Range |
|---|---|---|---|
| VECTOR.SEARCH_LIST | Session | 0, each index's own | 0 to 10,000 |
| VECTOR.BEAM_WIDTH | Session | 0, each index's own | 0 to 16 |
| VECTOR.BUILD_MEMORY_BYTES | Database | 256 MiB | At least 64 KiB |
| VECTOR.APPROX_FALLBACK | Session | ON | ON, OFF |
| VECTOR.APPROX_DEFAULT | Session | OFF | ON, OFF |

### The vector design document, clause by clause

Each clause of the vector design document (version 2.0) and turborel's
state. Yes means implemented as written; any other entry is how
turborel reads the clause. Networked and distributed search, §15.2, is
out of scope. A clause not yet done has no row.

| Clause | State |
|---|---|
| §1, §2, §20, §21 | Purpose, the vendors compared, the delivery plan and sources; no requirement of their own |
| §3.1 R3.1 | `VECTOR` alone leaves dimensions, format and layout open, so a sparse value is taken too |
| §3.1 R3.2, R3.3 | Yes; `float32` and `float16` in any case |
| §3.1 R3.4 | Yes; a type out of bounds is 2A000, SQLCODE -103, a value 22003 |
| §3.1 R3.5 to R3.8 | Yes; an index of an open column is -900, and so is a dense index of FLOAT64 (D95) |
| §3.2, R3.9, R3.10 | Yes; NaN and infinities are 22003 |
| §3.3 R3.11, R3.12 | Yes; each refused use is -103 (D93), a vector method's UNIQUE index -900 |
| §4.1, §4.2, R4.1, R4.2 | Yes; a repeated or descending index is 22018 |
| §4.3 R4.3 | A vector is a value as long as its image, kept in its row or out of line as any long value is |
| §4.3 R4.4 | Yes, a version byte and the elements of a fixed dense column |
| §4.3 R4.5 | The column store keeps a fixed vector as a fixed-width field, scanned by the same kernels |
| §4.3 R4.6 | The image is what is logged, checkpointed and replicated, as any value (D57) |
| §5.1 | Yes, the JSON object form of a sparse vector too |
| §5.2 | TO_VECTOR, VECTOR(text ...), CAST of a character string, FROM_VECTOR RETURNING VARCHAR or CLOB FORMAT DENSE or SPARSE, CAST to a character string |
| §5.2 R5.1 to R5.6 | Yes |
| §5.3 R5.7 | Every client is sent a vector's type name, its text and its image together, with no negotiation |
| §5.3 R5.8 | A column's type is sent as VECTOR with its parameters, the dimensions and the format and layout packed |
| §5.3 R5.10 | `turborel --import` reads the text form of a CSV or TSV field |
| §6.1 R6.1, R6.2, R6.4 to R6.6 | Yes; a distance function of SQL takes dense vectors only |
| §6.1 R6.3 | The metric of an index of v1's column, else COSINE (D93) |
| §6.2, R6.7, R6.8 | Yes, L2_SQUARED too |
| §6.3, §6.4, §6.5, R6.9, R6.10 | Yes; `/` of two vectors is -103 |
| §6.6 kernels | Portable lanes the compiler vectorizes for every CPU, with no `unsafe` (R-SAFE-1) |
| §6.6 overflow, determinism | Yes; overflow is 22003, and every function is deterministic |
| §7.1 | Yes; VECTOR_QUANTIZE to INT8 clamps to its range, and VECTOR_QUANTIZE_PARAMS gives an ARRAY of scale and zero point |
| §7.2 | Yes; QUANTIZER NONE, SQ8, PQ or BQ and RERANK of `hnsw`, `ivf` and `diskann`; INVERTED.QUANTIZATION of `inverted` |
| §7.2 R7.1 | HNSW and IVF give four times the rows asked, a constant; DiskANN its whole search list; the executor measures each again exactly |
| §7.2 R7.2 | Yes; samples of at most 100,000 vectors, 65,536 of DiskANN, retrained by REBUILD |
| §8 | CREATE and DROP VECTOR DISTANCE FUNCTION of LANGUAGE SQL; another language is -900 |
| §8 R8.5 | Yes, -104 |
| §8 R8.2 | `diskann` and `hnsw` refuse a metric not SYMMETRIC and NON_NEGATIVE, -900 (D95) |
| §9.1 | Yes, by ORDER BY and FETCH FIRST, and FETCH EXACT |
| §9.2, §9.3, §9.4 R9.1 | Yes (D94); VECTOR_SEARCH's columns are named by its correlation name, `s.ID` where §9.3 writes `t.id` |
| §9.5 | A correlated subquery with FETCH APPROX reads the index for each outer row; GROUP BY with MIN(distance) is ordinary SQL over it |
| R9.2 to R9.5 | Yes (D94) |
| R9.6 | The nearest read is planned only under a limit over one table, so no k-limit is pushed below a join |
| §10.1 | Yes, `hnsw`, `ivf`, `diskann`, `inverted` and `hybrid` |
| §10.2 | Yes, desugared to CREATE INDEX ... USING with options. `DISTANCE schema.name` names a metric registered as `SCHEMA.NAME`. LOCAL and GLOBAL choose nothing, as no table is partitioned. ONLINE changes nothing, as every build leaves the table open to writers, and PARALLEL n changes nothing, as builds run on one thread |
| §10.3 | Yes; MAXDOP and ONLINE change nothing |
| §10.4 | Each method's defaults are in [Implementation-defined choices](#vectors-and-their-indexes). HNSW's M is 16, DiskANN's PQ codes half the dimensions, and a search list follows TARGET ACCURACY by a fixed formula, not one calibrated at build |
| §10.5 R10.6 | Yes |
| §10.5 R10.7 | An index's memory is reserved of the database's budget (D38); past it CREATE fails with SQLCODE -918 naming the bytes |
| §10.5 R10.8 | ONLINE as above; writes go to the index itself, with no delta store (R14.2) |
| §10.6 | REBUILD [ONLINE] [PARAMETERS], MERGE DELTA, which runs the index's maintenance now, and LOAD and UNLOAD, which change nothing of a method whose structure is always resident or always paged. INDEX_ACCURACY_REPORT is the Rust call `Session::index_accuracy_report` |
| §10.6 R10.10 | Maintenance consolidates or retrains past 20% dead entries |
| §11.1 | CREATE HYBRID VECTOR INDEX of a text and a vector column, VECTOR_IDX and TEXT_IDX, or of a text column, MODEL, CHUNKER and MODE SYNC or ASYNC: a full-text index on the text, the chunks, their embeddings and their vector index, kept by a pipeline, made in steps no crash leaves half done, dropped with the index, and made again by ALTER INDEX of MODEL, CHUNKER or MODE (D100); CONTAINS and RANK of the text read the index a document a row |
| §11.2 | HYBRID_SEARCH, its vector list of QUERY_VECTOR, or of QUERY_TEXT embedded by the index's MODEL, and its text list of QUERY_TEXT; a dynamic parameter read twice is one; the plain SQL form is ordinary SQL, a radius in WHERE reading the index |
| §11.3, R11.1, R11.2 | Yes; CHUNK_TEXT is the row's text column, or of an index of a MODEL the document's best chunk |
| §12.1 | CREATE, ALTER and DROP EMBEDDING MODEL [FROM FILE] of a registered plug-in (D101); HASH_DENSE and HASH_SPARSE built in, others the host's |
| §12.2 | EMBED(model, text [, 'query' \| 'document']) and VECTOR_EMBEDDING of EMBED's arguments, and EMBED_VERSION |
| §12.2 R12.1, R12.3, R12.4 | Yes; an `Embedder` is called holding no row lock |
| §12.2 R12.5 | A model's VERSION is tagged in a pipeline's version column, by EMBED_VERSION, and the queue embeds a row of another version again |
| §12.3 | CHUNK_TEXT, VECTOR_CHUNKS and AI_GENERATE_CHUNKS in FROM, BY CHARS, WORDS, SENTENCES, PARAGRAPHS or TOKENS of a model, MAX, OVERLAP, SPLIT_ON of an XQuery regular expression and NORMALIZE WHITESPACE |
| §12.4 R12.6 | Yes (D103) |
| §12.5 | Yes, as four row triggers (D102) |
| §13 R13.1, R13.2, R13.5 | Yes (D94); no shards |
| §13 R13.3 | The approximate read is priced by the rows, the search list and the selectivity; the index's kind, the format and quantization are left out |
| §13 R13.4 | Every hint, written `HINT (...)` after the fetch clause; VECTOR_WAIT_FRESH also as `/*+ VECTOR_WAIT_FRESH */` before the statement (D105) |
| §14 R14.1, R14.3, R14.4, R14.6, R14.7 | Yes; candidates are read as the snapshot sees them and checked again |
| §14 R14.2, R14.5 | Each index is changed as its table is, HNSW and IVF in place and DiskANN by FreshDiskANN's inserts, with tombstones and no delta store, so a search needs no merge and every read is fresh |
| §14 R14.8 | A standby replays the log, and its indexes are kept by it as the primary's are (D57) |
| §15.1 | No table is partitioned, so LOCAL and GLOBAL have nothing to choose |
| §16 catalog | VECTOR_COLUMNS; INDEXES and INDEX_OPTIONS for vector indexes; EMBEDDING_MODELS, EMBEDDING_MODEL_PRIVILEGES, VECTOR_DISTANCE_FUNCTIONS, VECTOR_PIPELINES, EMBEDDING_QUEUE; MEMORY_RESERVATIONS for an index's memory |
| §16 security | Vectors follow table and column privileges. EXECUTE on models and distance functions; making a model, index or distance function needs the schema's ownership. No view shows a model's options. An index of a table of its own encoding is refused, and database encryption covers index files |
| §16 governance | The memory budget and VECTOR.BUILD_MEMORY_BYTES |
| §17 | Yes, each restriction, -103 or -900 |
| §18 | Each condition by SQLSTATE, as [below](#vector-errors) |
| §19 | Each item that names a behaviour above has a test |

#### Vector errors

| Design code | SQLSTATE | SQLCODE |
|---|---|---|
| VEC-0001, VEC-0002 of a type, VEC-0009 | 2A000 | -103 |
| VEC-0002 of a value, VEC-0006, VEC-0007, VEC-0012 | 22003 | -301 |
| VEC-0003, VEC-0004, VEC-0008 and VEC-0013 of a call, VEC-0031 of a value | 22000 | -301 |
| VEC-0005, VEC-0016 | 22018 | -301 |
| VEC-0008 of an index, VEC-0010, VEC-0015, VEC-0020 of an index | 0A000 | -900 |
| VEC-0013 of an index, VEC-0030, VEC-0031 of a model | 2A000 | -103 |
| VEC-0011 | TB918 | -918 |
| VEC-0032 | The plug-in's own, under ON_ERROR | |

### Property graphs

```
CREATE PROPERTY GRAPH g VERTEX TABLES (...) [EDGE TABLES (...)]
    [OPTIONS ({ENFORCED | TRUSTED} MODE)] [WITH INVOKER RIGHTS]
CREATE PROPERTY GRAPH g AS VERTICES FROM (query) [AS alias] KEY (...) ...
    [, ...] [EDGES FROM (query) [AS alias] KEY (...) SOURCE ... DESTINATION ...]
CREATE ADJACENCY INDEX [name] ON GRAPH g EDGE TABLE alias
    [DIRECTION {FORWARD | REVERSE | BOTH}] [INCLUDE (column, ...)]
CREATE LABEL INDEX [name] ON GRAPH g [{VERTEX | EDGE} TABLE alias]
CREATE REACHABILITY INDEX [name] ON GRAPH g EDGE TABLE alias
SELECT ... FROM GRAPH_TABLE ({g | LINE_GRAPH (g, label)} MATCH pattern [WHERE condition]
    [ONE ROW PER {MATCH | VERTEX (v) | STEP (v1, e, v2)}]
    COLUMNS (expression [AS name], ... | v.*)
    [OPTIONS (MAX_PATH_LENGTH n, MAX_PATHS_PER_PARTITION n,
    MAX_SEARCH_STATES n, ON LIMIT {ERROR | TRUNCATE}, DETERMINISTIC_PATHS,
    GRAPH_START (v), GRAPH_DIRECTION (e, REVERSE), NO_WCOJ,
    USE_ADJACENCY (alias), PATH_ALGO (BFS | BIBFS | DFS | DIJKSTRA | MSBFS))])
    [FOR SYSTEM_TIME AS OF t] [[AS] name] [AS OF {TIMESTAMP t | SCN n}]
```

A property graph (RULES.md D140) is defined over tables and views, and
the schema of node and edge tables has SYS_GRAPH, which the database
keeps ([CREATE PROPERTY GRAPH](doc/sql/statements/create-property-graph.md)).
GRAPH_TABLE (RULES.md D142) matches its pattern of the property graph
standard's GPML [GS §6] and gives rows to the query around it, which it
may read laterally ([Property graphs](doc/sql/20-property-graphs.md)).
It takes the selectors ANY, ANY k, ANY SHORTEST, ALL SHORTEST,
SHORTEST k [GROUPS] and ANY, ALL or k CHEAPEST [COST expression]; the
restrictors WALK, TRAIL, ACYCLIC and SIMPLE; the match modes REPEATABLE
ELEMENTS, DIFFERENT EDGES and DIFFERENT VERTICES; label expressions with
`&`, `|`, `!` and `%`; quantifiers; unions and alternations; element
and parenthesized WHERE clauses; and a postfilter. Its expressions take
ELEMENT_ID, VERTEX_ID, EDGE_ID, LABELS, PROPERTY_EXISTS, SAME,
ALL_DIFFERENT, IS [NOT] SOURCE OF, IS [NOT] DESTINATION OF, IS [NOT]
DIRECTED, IS [NOT] LABELED, PATH_LENGTH, VERTICES, EDGES, IS_ACYCLIC,
IS_TRAIL, IS_SIMPLE, PATH_TO_JSON and the set functions of a group
variable's elements, COUNT(e.*) and the inverse distribution functions
among them. A subpath variable `(q = ...)` names the part of the path
its parenthesized pattern matches, of which the path functions are. A
parenthesized WHERE takes set functions over the elements bound within
it, each iteration's own, and PREV and FIRST of an element variable,
its binding in the iteration before or the first [GS §11.1]. FOR EACH
SEGMENT (e1, e2) IN p WHERE condition keeps the matches whose every two
edges one after the other meet it. A prefilter may read the path so far
through the path functions, a variable bound later in the pattern, and
a variable of another path pattern [GS §6.8, §6.9]. [NOT] EXISTS IN
PATH p (pattern) tests a pattern along the path p binds [GS §11.2].
LINE_GRAPH (g, label) is the graph of g's edges of the label, joined by
FOLLOWS where one leaves the vertex the other ends at, and an element
table may hold a query's rows in place of a table's [GS §11.3].
AS OF SCN n reads the element tables
at SCN n, the microseconds of UTC from 0001-01-01 of a transaction
timestamp, which TIMESTAMP_TO_SCN and SCN_TO_TIMESTAMP convert; an SCN
no instant is 22008. EXPLAIN shows each path's
automaton, the algorithm chosen and how each edge table is followed.
A rigid pattern is planned as a join, its edges read by Expand,
Expand (reverse) or ExpandInto (RULES.md D147), a cyclic one by a
worst-case optimal join, and a small quantifier is unrolled (RULES.md
D148). OPTIONS also take the hints GRAPH_START, GRAPH_DIRECTION,
NO_WCOJ, USE_ADJACENCY and PATH_ALGO [GS §16 R16.8].
An edge table is followed through its adjacency index (RULES.md D141)
if it has one, and an ENFORCED graph's base edge tables are given one
as it commits. A label index (RULES.md D149) gives a GRAPH_TABLE its
label tests and starts as bitmaps, and a reachability index (RULES.md
D150) drops the starts of a quantified search that reach no end, with
the same rows. A search past a limit fails with SQLSTATE 54061, or ON
LIMIT TRUNCATE ends with warning 01W61. A negative COST is 22W34.

### Property graphs (Mode B)

```
CREATE TABLE t [(column, ...)] AS NODE
CREATE TABLE e [(column, ..., CONSTRAINT c CONNECTION (A TO B [, ...])
    [ON DELETE {CASCADE | NO ACTION}])] AS EDGE
```

The property graph standard's Mode B [GS §4.2] is native node and edge
tables (RULES.md D143). It covers CREATE TABLE ... AS NODE and AS EDGE,
their pseudo-columns, CONNECTION constraints, the implicit graph
SYS_GRAPH, and DML on them [GS §12.2]
([CREATE TABLE](doc/sql/statements/create-table.md)).

| Rule | Behaviour |
|---|---|
| §4.2 AS NODE, AS EDGE | Make a node or edge table, with or without columns of its own. Not temporary, typed, from a query or system-versioned (-103). |
| §4.2 Pseudo-columns | `$NODE_ID` of a node table; `$EDGE_ID`, `$FROM_ID`, `$TO_ID` of an edge table. Hidden from `*`, an INSERT of no column list and LIKE; named in any case; shown by COLUMNS with IS_HIDDEN YES. Never dropped or altered. |
| R5.1 Identity | `{"type":"node","schema":"S","table":"T","id":N}`, `"edge"` of an edge, N from 0 per table, stable for the row's life. Given on INSERT, never written by a statement (-103). |
| §4.2 CONNECTION | `CONSTRAINT c CONNECTION (A TO B [, ...]) [ON DELETE {CASCADE \| NO ACTION}]`, of an edge table, naming node tables of its schema (else GRF-0004). Added and dropped by ALTER TABLE. DROP TABLE of a node table it names needs CASCADE, which drops it. |
| R12.2 Edge inserts | An endpoint is the identity of a node of a node table of the schema, else GRF-0053 when written. A CONNECTION constraint checks, on INSERT and UPDATE of an endpoint, that the pair of tables is one of its clauses and each node a row, else GRF-0053. Several constraints each hold. |
| R12.1 Node delete | A node with edges in an edge table fails with GRF-0052, unless a CONNECTION constraint ON DELETE CASCADE of that table names the node's table. Then those edges are deleted in the same statement. TRUNCATE of a node table is refused while the schema has an edge table. |
| §4.2 SYS_GRAPH | One per schema with a node table, described from its node and edge tables, TRUSTED, each table labelled by its name with its own columns as properties. Kept as tables change, shown in the property graph views, never defined, altered or dropped by a statement (-103). |
| §4.2, §13 Adjacency | Each edge table has an `adjacency` index of ($FROM_ID, $TO_ID), `S.T.ADJACENCY`, and B-trees of its identities. |
| §12.2 DML | INSERT, UPDATE, DELETE, MERGE and SELECT as on any table, endpoints written by value or by subquery. |
| §16 Views | INFORMATION_SCHEMA.GRAPH_TABLES and CONNECTION_CONSTRAINTS. TABLE_CONSTRAINTS shows a CONNECTION constraint's type as CONNECTION. |
| §22 Conditions | GRF-0052 and GRF-0053 are SQLCODE -201, SQLSTATE 23000, the code in the message. GRF-0004 is -103. |

### Graph compatibility syntax

```
SELECT ... FROM t1, e, t2 WHERE MATCH (t1-(e)->t2) ...
SELECT ... FROM MATCH (a)-[e]->(b) [ON g] [WHERE ...] ...    -- PGQL
[USE g] MATCH (a)-[r]->(b) [WHERE ...] RETURN ...            -- openCypher
```

The compatibility syntax of the property graph standard [GS §10]
[EXTENSION] compiles to GRAPH_TABLE (RULES.md D146,
[Graph Compatibility Syntax](doc/sql/22-graph-compatibility.md)).

| Item | Behaviour |
|---|---|
| GS §10.1 MATCH in WHERE, SHORTEST_PATH, FOR PATH, WITHIN GROUP (GRAPH PATH), LAST_NODE | Implemented. MATCH is a predicate of a WHERE clause alone, elsewhere -103. Its graph is SYS_GRAPH of the tables' schema when they are all its element tables, else the one graph of them all, else -103. |
| GS §10.1 OR and NOT around MATCH, repeated edge aliases, quantifiers outside SHORTEST_PATH | Implemented. A MATCH not a conjunct of WHERE may have no FOR PATH tables or GRAPH PATH aggregates (-103). |
| GS §10.2 VERTEX_ID, EDGE_ID, VERTEX_EQUAL, EDGE_EQUAL, JSON dot notation, hints | Implemented. Hints in `/*+ ... */` after `GRAPH_TABLE (` are those of OPTIONS, an unknown word passed over, a malformed known hint -102. |
| GS §10.3 PGQL SELECT ... FROM MATCH, reachability, PATH macros | Implemented. No ON is SYS_GRAPH of the default schema. |
| GS §10.3 openCypher MATCH and OPTIONAL MATCH ... RETURN, abbreviated relationships | Implemented, the `--` forms written with no space. |
| R5.1 `$NODE_ID` and `$EDGE_ID` in GRAPH_TABLE | A variable's identity. An element table keyed by one alone has its text as ELEMENT_ID. |

### GQL

```
EXECUTE GQL [ON [PROPERTY] GRAPH g] 'request'
SELECT ... FROM GQL_TABLE(g, 'request') [AS] name [(column, ...)]
SET SESSION.LANGUAGE = 'GQL'      -- direct statements read as GQL, on GQL.GRAPH
CREATE GRAPH TYPE [IF NOT EXISTS] t [AS] {element type, ...}
CREATE GRAPH [IF NOT EXISTS] g [{TYPED | ::} t | ANY [PROPERTY GRAPH]]
DROP GRAPH [IF EXISTS] g [CASCADE | RESTRICT]
DROP GRAPH TYPE [IF EXISTS] t [CASCADE | RESTRICT]
```

GQL (ISO/IEC 39075:2024) is read beside SQL over the same tables and
transactions (RULES.md D144,
[EXECUTE GQL](doc/sql/statements/execute-gql.md)). It reads and writes
a property graph of Mode A, a schema's SYS_GRAPH of Mode B, and the
typed and open graphs of Mode C. No word of GQL is reserved in SQL.
A MATCH is a GRAPH_TABLE, so its privileges are GRAPH_TABLE's.

| Facility | Behaviour |
|---|---|
| Requests [GS §9] | By EXECUTE GQL, GQL_TABLE and a session whose SESSION.LANGUAGE is GQL, on the graph GQL.GRAPH names. GQL_TABLE of a writing or session request is -900. |
| Linear statements | MATCH, OPTIONAL MATCH, FILTER, WHERE, LET, FOR, ORDER BY, OFFSET or SKIP, LIMIT, RETURN, GROUP BY and NEXT. |
| MATCH | Through GRAPH_TABLE, with CHEAPEST, every path function, restrictors that hold of the whole path, and aggregates of a group variable's properties. |
| Set operators | UNION, EXCEPT, INTERSECT and OTHERWISE. |
| CALL | Inline and named, an inline one that writes run once per row of the binding table. |
| Writes [GS §12] | INSERT, SET, REMOVE, DELETE and DETACH DELETE on Modes A, B and C, under a savepoint. Of Mode A where R12.1 holds, else GRF-0051. SET and REMOVE of a label of Mode A are refused unless the label already is, or is not, the table's. |
| Session and transactions | GQL's session and transaction statements. |
| Mode C [GS §4.3] | CREATE and DROP GRAPH TYPE, CREATE and DROP GRAPH TYPED or ANY. A typed graph has a table per node and edge type, an open graph two, and a property graph over them. A typed graph refuses what its type lacks (GRF-0053). |
| Views | INFORMATION_SCHEMA.GQL_GRAPHS and GRAPH_TYPES. |
| Conditions | GRF-0012 (unknown label, property or graph type), GRF-0022 (a group variable read as one element, or a path function of no path variable) and GRF-0051 are -103. GRF-0052 and GRF-0053 are -201, SQLSTATE 23000. |

### Graph algorithms

```
SELECT ... FROM GRAPH_ALGO.name({TABLE (edges) | TABLE (query) | graph}
    [, argument, ...] [, parameter => value, ...]) [AS] name
GRAPH_ALGO.name(...) INTO PROPERTY column [OF table [(key)]]
```

The functions of schema GRAPH_ALGO [GS §18] (RULES.md D145) run
over a property graph or a table of edges, in parallel, the same rows
on any number of threads
([Graph Algorithms](doc/sql/21-graph-algorithms.md)).

| Clause | Behaviour |
|---|---|
| Graph algorithm library [GS §18] | PAGERANK, WCC, SCC, BFS_LEVELS, SSSP, TRIANGLE_COUNT, LOCAL_CLUSTERING, BETWEENNESS, LOUVAIN, LABEL_PROPAGATION, K_CORE and JACCARD_SIMILARITY, each `GRAPH_ALGO.<name>(...)` a table reference giving (vertex key, value) rows, or JACCARD's (key, key, similarity). BFS_LEVELS' column is DEPTH, as LEVEL is a reserved word. |
| The graph [GS §18] | A table argument of edges, its first two columns the endpoints' keys, and VERTICES adds vertices. Or the edges are a property graph's name, of CREATE PROPERTY GRAPH or SYS_GRAPH. Then its vertex tables' rows are the vertices, each its identity as ELEMENT_ID gives it (of SYS_GRAPH its `$NODE_ID`), and its edge tables' rows the edges, joined to the vertices their endpoints reference, undirected ones both ways. VERTICES with a graph is -103. |
| Arguments | Positional, then named as `name => value`. |
| Results [GS §18] | Joinable to any table of the keys. Components and communities are named by their least key. BFS_LEVELS and SSSP give the reached vertices alone. |
| INTO PROPERTY [GS §18] | Of a property graph's name, `... INTO PROPERTY c [OF t]` writes c of each vertex table, or of vertex table t, the row of the vertex each result identity names, adding c if missing, one MERGE per table (UPDATE of c needed on each), of one run of the algorithm. Of a table of edges, `GRAPH_ALGO.<name>(...) INTO PROPERTY c OF t [(key)]` writes each value to column c of t's row of the key, the primary key of one column unless named, adding c (DOUBLE PRECISION, BIGINT or the key's type) if missing, in the statement's transaction. JACCARD_SIMILARITY is -103. |
| R18.1, R18.2 | The edges are read in the statement's snapshot, with its privileges. |
| R18.3 | Parallel on EXECUTOR.GRAPH_THREADS threads, the same rows on any number. Not distributed. |
| GRAPH_ALGO_RUNS [GS §20] | INFORMATION_SCHEMA.GRAPH_ALGO_RUNS, the latest 1000 runs, each with its number, algorithm, arguments, user, start, time, vertices, edges, rows, threads, DONE/FAILED and message. |
| GRF-0034 [GS §22] | A COST naming no column, null or not a number, negative where the algorithm takes none (all but SSSP), or SSSP's negative cycle the start reaches. SQLSTATE 22W34, SQLCODE -301. |

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

### Backups and recovery to a point in time

Neither standard has these (RULES.md D160 to D166). They run outside a
transaction, by the database's owner.

```
BACKUP DATABASE TO 'target' [INCREMENTAL [FROM 'id']] [LABEL 'label']
CREATE RESTORE POINT name
RESCAN BACKUP TARGET 'target'
```

| What | Behaviour |
|---|---|
| Online backup (D160) | A checkpoint, then the files in 64 KiB chunks and the log's records written meanwhile, while readers and writers go on. The manifest is written last, so a backup cut short is none. Also `Database::backup`. |
| Incremental backup (D160) | The chunks whose digest differs from the parent's manifest. A restore follows the chain back to a full backup. |
| Objects (D161) | Each a block file of format version 1, checksummed and passed through the database's encodings and keys, so an encrypted database's backup is encrypted. BACKUP.COMPRESSION puts LZ4 first. |
| Verification (D161) | `Database::verify_backup` and `turboreld --verify-backup` read a chain, its chunks, image and records, restoring nothing. A failure is SQLSTATE TBB01. |
| Restore (D162) | `Database::restore` and `turboreld --restore`, offline, into a staging directory renamed once whole. |
| Archive of the log (D163) | ARCHIVE.MODE, TARGET, SEGMENT_BYTES, TIMEOUT_MS and RETENTION_SECONDS. A checkpoint keeps every record not archived. |
| Recovery to a point in time (D164) | To a time, a position, a restore point or the archive's end. A target not reached is SQLSTATE TBB02. |
| Timelines (D165) | A restore through the archive begins a timeline past every one the archive holds, and never writes over another's segments. |
| Targets (D166) | `BackupTarget` (and `ArchiveTarget`, the same trait) and `TargetProvider`, object-safe, of strings, integers and byte chunks. The `dir` and `memory` providers are built in. |
| History (D166) | Durable, in the database's directory. INFORMATION_SCHEMA.BACKUPS, ARCHIVED_SEGMENTS and RESTORE_POINTS show it to the owner, and a non-owner sees no rows. RESCAN BACKUP TARGET rebuilds it from a target. |

BACKUP, INCREMENTAL, LABEL, RESTORE, POINT, RESCAN and TARGET are no key
words. A backup is cancelled as a statement is.

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
ALTER TABLE ... RENAME [COLUMN] ... TO ...
```

The row store takes OUT_OF_LINE and the options beginning STATISTICS. and
TRANSACTIONS. The column store, `columnstore`, takes them too, and those
beginning COLUMNSTORE., which say how a table of it freezes its rows into
compressed groups and writes them again (RULES.md D31).

The database declares ENCODINGS for every table. These are the registered
encodings its rows pass through, such as LZ4, or AES-256-GCM or
ChaCha20-Poly1305 with its key (RULES.md D48). RESEAL copies a table's rows
through those it names now, and is a non-reserved word.

RENAME COLUMN gives a column a new name in place (RULES.md D83), which
SQL:2023 has no statement for [§11.10]. RENAME is a non-reserved word.
The rename changes the column's descriptor, the periods it bounds and
the table options that name it, and is refused while a view, CHECK,
assertion, trigger, routine or generation expression reading the table
names the column or its new name, or a view selects `*` of it, SQLCODE
-103, as is one of a typed or system-versioned table.

### Partitioned tables

PARTITION BY and PARTITION OF of CREATE TABLE, ATTACH PARTITION and
DETACH PARTITION of ALTER TABLE, and the views PARTITIONED_TABLES,
PARTITIONS and PARTITION_INDEXES are extensions
([Partitioned Tables](doc/sql/23-partitioning.md), RULES.md D153 to D159).

| Use | Behaviour |
|---|---|
| Strategies | RANGE, LIST and HASH built in, others registered by `Database::register_partition_strategy`. A HASH table's moduli divide each other, and it has no DEFAULT partition. |
| Rows | INSERT, UPDATE and MERGE route each row down the tree. A row no partition holds is SQLSTATE 23T01. A row whose key changes is moved, its row triggers fired as an update of the tables over both partitions. |
| Reads | Partitions pruned by comparisons, IS NULL, IN and OR of the key with constants, and with a prepared statement's parameters as it runs. Reads in an order merged, nearest reads taken from each partition's index. Index nested-loop joins into a partitioned table, each probe routed to the partitions its values admit, or read through a global index. |
| Keys | PRIMARY KEY and UNIQUE held across partitions, by each partition's index where the key holds the partition key, else by a global index over every partition's rows. Foreign keys to and from partitioned tables. CHECK constraints and defaults inherited. |
| Indexes | Every method a template, made on each partition and adopted or built on ATTACH. A hybrid index of a MODEL keeps one table of chunks of the tree. A full-text rank uses the statistics of every partition. |
| Tables | Temporary, system-versioned with a history per partition, and each partition of its own engine. Cursors over them updatable. Typed tables and node or edge tables are not partitioned. |

### Views of the running system

| View | Shows |
|---|---|
| CONNECTIONS, SERVER, REPLICAS | A server's connections, its counts and its replicas |
| FILES, FILE_FORMATS | A database's files and their formats |
| JOBS | Its background jobs |
| MEMORY_RESERVATIONS | What holds its memory |
| GRAPH_ALGO_RUNS | The latest 1000 runs of graph algorithms |
| EXTENSIONS, EXTENSION_OBJECTS, EXTENSION_PACKAGES | The extensions created, the objects their manifests declare, and the database's lock of their packages |
| BACKUPS, ARCHIVED_SEGMENTS, RESTORE_POINTS | A database's backup history (RULES.md D166) |

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

### External plug-ins

A plug-in may be a program of another party, run as processes of its own
and spoken to in plug-in protocol 1 over their pipes (RULES.md D167 to
D173 and D220 to D229, `src/plugin.rs`, [Plug-ins](doc/client/04-plugins.md),
[Kinds of Plug-in](doc/client/04-plugins-kinds.md)).

| Mechanism | Behaviour |
|---|---|
| CREATE EXTENSION | `CREATE EXTENSION n [VERSION r] [FROM 'f']` loads the newest version installed under PLUGINS.DIRECTORIES that meets r, or the package file or directory f, creating the extensions it REQUIRES first. Its objects are the database's: scalar functions, aggregates, table functions, procedures, vector metrics, embedding models, authentication mechanisms, backup targets, partitioning strategies, full-text rankings, vector quantizers, collations, system tables, XQuery functions, data types, storage engines, index methods, credentials and sinks of events. The statement is logged with the manifest read and the extension kept in the catalog, so it is recovered, replicated and made again as the database opens. |
| ALTER EXTENSION | `ALTER EXTENSION n UPDATE [TO r] [FROM 'f']` replaces it by another version, running the scripts of SQL its manifest names for the version it had, once each extension requiring it accepts the new version. |
| DROP EXTENSION | RESTRICT, the default, is refused while an extension requiring it, or a view, generated column, constraint or routine calling one of its objects remains; CASCADE drops them first. Both are refused while a table uses its storage engine, a column its type or collation, a collation is made from its collation, or an index uses its method or names its ranking or quantizer, as what is stored by them goes first. |
| Transactions | The three statements run in the session's transaction, and ROLLBACK undoes them, the processes of what is undone stopped. |
| Privileges | Only the database's owner, or any user of a database with none, creates, updates or drops one; any user calls its objects. PLUGINS.DIRECTORIES, PLUGINS.TRUSTED_KEYS and PLUGINS.REQUIRE_SIGNED are a node's settings the host gives, which SQL may not set. |
| Processes | At most PLUGINS.PROCESSES of an extension run at once. One that ends, breaks the protocol, runs past PLUGINS.CALL_TIMEOUT_MS or sends more than PLUGINS.MAX_MESSAGE_BYTES or PLUGINS.MAX_CALL_BYTES is stopped and its call fails, and the next call starts another. A statement cancelled sends CANCEL (D44). |
| Calls | A function's rows, a metric's candidates and a model's texts are sent BATCH at a time, so a vector index search over a plug-in's metric is one call for many candidates. A function is DETERMINISTIC only as its manifest says. |
| Errors | A program's SQLSTATE is kept if of class 22, 23, 38, 39, 42 or 2F, and is 38000 else; the database's own conditions of a plug-in are 38T01 to 38T11 ([SQLSTATE values](doc/appendix/a-sqlstate.md)). |
| Plans | EXPLAIN ends with the plug-in calls a plan makes. |
| Rankings, quantizers, collations | A ranking's weights and bounds are asked a batch a call; a quantizer trains a codebook in one call, which the database then searches by alone; a collation gives sort keys a batch a call, compared as bytes, and is refused (SQLCODE -103) by LIKE, TRIM, XQuery and index keys made character by character. |
| System tables, XQuery functions | A system table is read in INFORMATION_SCHEMA a page of rows a call, told the reader and whether it is the owner, at most 1,000,000 rows. An XQuery function is called by its expanded name, of atomic arguments and result. |
| Types | A type is made of its text by CAST from a character string and of its binary form by CAST from a binary string, a batch a call; each value is a key, a binary form and a text form the program made, compared, ordered and indexed by the key with no call. It has no arithmetic and no other cast. |
| Storage engines | A table USING an extension's engine keeps its rows in the program; the coordinator's table locks isolate its transactions, whose writes are applied at commit, BATCH rows a call, each call all or none, and read back a page or a row by id a call. A checkpoint's position is the program's, and recovery replays the log from it. A process stopped while a transaction writes fails that transaction at commit with 40001. |
| Index methods | An index USING an extension's method serves the equality, range and nearest searches its manifest declares; writes are given a batch a call, each entry is rechecked against the row the reader sees, and an index not clean is rebuilt as the database opens. |
| Credentials, sinks of events | Credentials make each verifier, of at most one extension. A sink is told each event, queued and sent a batch a call on a thread of its own, the events past a full queue counted. |
| Host plug-ins | Encodings, checksums, the clock, log and catalog storage, spill, the file system, served connections, listeners, package cryptography, keyrings and background jobs are served by host plug-ins, which the node names in PLUGINS.HOST and starts before the database opens. Each call either answers or stops the node, every later statement failing with 38T11 until the next start recovers. A commit's sync is one call for the records waiting, the clock is a copy pushed every EVERY milliseconds, a page is one call each way, and a listener relays bytes a call an exchange. Package cryptography is pinned by its program's digest (RULES.md D229, D246 to D255). |
| Packages | A package is one file. Each entry carries its SHA-256 digest, and an optional Ed25519 signature covers the manifest and the digests; it is checked as it is installed and as each of its processes starts. |
| Versions | Versions are semantic and ranges npm-style. Resolution picks the newest compatible set among the package files given and the packages installed, backtracking where needed, and refuses cycles and conflicts with 38T09, a requirement nothing meets naming the package file to add. |
| No registry | turborel has no registry and fetches nothing. A package is a file the operator distributes (RULES.md D260). |
| Lock | A lock file holds each package's digest and license, and `install --lock` reproduces a set exactly from the files given, refusing a file whose digest differs (38T07). |
| Licenses | A manifest's LICENSE is an SPDX license expression, checked against the grammar alone, and LICENSE FILE a file of the package its digests and signature cover. A license is told by install, `license`, `info`, `\dx+`, the lock and INFORMATION_SCHEMA.EXTENSION_PACKAGES, and never enforced. `turborel::plugin::package::package_license` reads one for a host's plug-in. |
| Starter and pack | `turborel extension new` writes a project on the plug-in SDK serving an object of each kind asked for, tested on its harness; `pack` builds a package of the manifest, the program cargo built, the files the manifest names and those under `doc/` and `data/` ([Writing a Plug-in](doc/client/04-plugins-sdk.md)). |
| Command line | `turborel extension` has install, remove, autoremove, list, info, license, verify, why, lock export, new, pack and keygen ([Packages](doc/client/04-plugins-packages.md)). |

### Function call syntax

Registered functions may be called by name in sessions whose setting
SQL.FUNCTION_CALLS is ON, as it is by default. A name has one scalar function, or several that
each declare the types of their parameters and that no call could invoke two
of alike, chosen by the arguments' types as an SQL-invoked function is
(RULES.md D53, D74). Any other second registration of a name is refused. A
function of the default schema that takes the arguments is invoked before a
registered one. A call of a registered aggregate's name is the
aggregate's unless a function of the default schema of the name has as
many parameters as the call has arguments, as that is told before the
arguments are bound. The setting is checked as a definition is made. A
view, constraint, assertion, domain, trigger, generated column or SQL
routine body that calls a registered function keeps the call, whatever
the setting is when it is used, reopened, recovered or replayed on a
replica (RULES.md D74). CREATE and ALTER TABLE check the setting of the
session writing a new CHECK or generation expression. A registered
function whose `deterministic` is false is possibly non-deterministic,
so a CHECK, assertion, domain constraint, generation expression or
index key part calling it is SQLCODE -103 [ISO/IEC 9075-2:2023 §11.9
SR5, CR2, §11.4 SR10d, §9.16 SR1am] (RULES.md D133).

---

## Features claimed

Each feature and subfeature claimed, as the feature taxonomies of
ISO/IEC 9075-2:2023 (Annexes D and H) and of the 2006 drafts of
ISO/IEC 9075-4, 9075-11 and 9075-14 name it, with the section saying how
it is done. A feature claimed claims each of its subfeatures
[ISO/IEC 9075-11:2006 draft §6.43 Description 7]. Every other feature
and subfeature of those taxonomies is not claimed, nor is any part of
ISO/IEC 9075 whole, and none of those texts defines a package.

INFORMATION_SCHEMA.SQL_FEATURES shows this table, and SQL_PARTS the
parts: `cargo xtask features` writes their rows from it, and
`cargo xtask verify` fails if they differ (RULES.md D37).

| Feature | Name | Claimed in |
|---|---|---|
| T191 | Referential action RESTRICT | [Schemas and constraints](#schemas-and-constraints) |
| F231 | Privilege tables | [Transactions, sessions and the Information Schema](#transactions-sessions-and-the-information-schema) |
| F341 | Usage tables | [Transactions, sessions and the Information Schema](#transactions-sessions-and-the-information-schema) |
| F501 | Features and conformance views | [Transactions, sessions and the Information Schema](#transactions-sessions-and-the-information-schema) |
| F502 | Enhanced documentation tables | [Transactions, sessions and the Information Schema](#transactions-sessions-and-the-information-schema) |
| T011 | Timestamp in Information Schema | [Transactions, sessions and the Information Schema](#transactions-sessions-and-the-information-schema) |
| F411 | Time zone specification | [Data types](#data-types) |
| F555 | Enhanced seconds precision | [Data types](#data-types) |
| F690 | Collation support | [Collations](#collations) |
| F692 | Extended collation support | [Collations](#collations) |
| F693 | SQL-session and client module collations | [Collations](#collations) |
| T285 | Enhanced derived column names | [SQL:2023](#sql2023) |
| T031 | BOOLEAN data type | [Truth values](#truth-values) |
| T662 | Underscores in numeric literals | [Numeric literals](#numeric-literals) |
| T021 | BINARY and VARBINARY data types | [Binary strings](#binary-strings) |
| T022 | Advanced support for BINARY and VARBINARY data types | [Binary strings](#binary-strings) |
| T023 | Compound binary literals | [Binary strings](#binary-strings) |
| T024 | Spaces in binary literals | [Binary strings](#binary-strings) |
| T039 | CLOB locator: non-holdable | [Large objects](#large-objects) |
| T040 | Concatenation of CLOBs | [Large objects](#large-objects) |
| T041 | Basic LOB data type support | [Large objects](#large-objects) |
| T042 | Extended LOB data type support | [Large objects](#large-objects) |
| T045 | BLOB data type | [Large objects](#large-objects) |
| T046 | CLOB data type | [Large objects](#large-objects) |
| T047 | POSITION, OCTET_LENGTH, TRIM, and SUBSTRING for BLOBs | [Large objects](#large-objects) |
| T048 | Concatenation of BLOBs | [Large objects](#large-objects) |
| T049 | BLOB locator: non-holdable | [Large objects](#large-objects) |
| T050 | POSITION, CHAR_LENGTH, OCTET_LENGTH, LOWER, TRIM, UPPER, and SUBSTRING for CLOBs | [Large objects](#large-objects) |
| T312 | OVERLAY function | [Large objects](#large-objects) |
| T561 | Holdable locators | [Large objects](#large-objects) |
| F262 | Extended CASE expression | [Simple CASE](#simple-case) |
| F263 | Comma-separated predicates in simple CASE expression | [Simple CASE](#simple-case) |
| T151 | DISTINCT predicate | [DISTINCT predicate](#distinct-predicate) |
| T152 | DISTINCT predicate with negation | [DISTINCT predicate](#distinct-predicate) |
| T054 | GREATEST and LEAST | [GREATEST and LEAST](#greatest-and-least) |
| T055 | String padding functions | [Trims and pads](#trims-and-pads) |
| T056 | Multi-character TRIM function | [Trims and pads](#trims-and-pads) |
| T081 | Optional string types maximum length | [Optional maximum lengths](#optional-maximum-lengths) |
| T071 | BIGINT data type | [BIGINT](#bigint) |
| T076 | DECFLOAT data type | [Decimal floating point](#decimal-floating-point) |
| F841 | LIKE_REGEX predicate | [Regular expressions](#regular-expressions) |
| F842 | OCCURRENCES_REGEX function | [Regular expressions](#regular-expressions) |
| F843 | POSITION_REGEX function | [Regular expressions](#regular-expressions) |
| F844 | SUBSTRING_REGEX | [Regular expressions](#regular-expressions) |
| F845 | TRANSLATE_REGEX | [Regular expressions](#regular-expressions) |
| F846 | Octet support in regular expression operators | [Regular expressions](#regular-expressions) |
| F847 | Non-constant regular expression | [Regular expressions](#regular-expressions) |
| T062 | Character length units | [Character length units](#character-length-units) |
| T839 | Formatted cast of datetimes to/from character strings | [Formatted CAST](#formatted-cast) |
| T176 | Sequence generator support | [Sequence generators](#sequence-generators) |
| T177 | Sequence generator support: simple restart option | [Sequence generators](#sequence-generators) |
| T174 | Identity columns | [Identity columns and generated columns](#identity-columns-and-generated-columns) |
| T175 | Generated columns | [Identity columns and generated columns](#identity-columns-and-generated-columns) |
| T178 | Identity columns: simple restart option | [Identity columns and generated columns](#identity-columns-and-generated-columns) |
| F384 | Drop identity property clause | [Identity columns and generated columns](#identity-columns-and-generated-columns) |
| F385 | Drop column generation expression clause | [Identity columns and generated columns](#identity-columns-and-generated-columns) |
| F386 | Set identity column generation clause | [Identity columns and generated columns](#identity-columns-and-generated-columns) |
| F200 | TRUNCATE TABLE statement | [TRUNCATE TABLE](#truncate-table) |
| F202 | TRUNCATE TABLE: identity column restart option | [TRUNCATE TABLE](#truncate-table) |
| F312 | MERGE statement | [MERGE](#merge) |
| F313 | Enhanced MERGE statement | [MERGE](#merge) |
| F314 | MERGE statement with DELETE branch | [MERGE](#merge) |
| T495 | Combined data change and retrieval | [Data change delta tables](#data-change-delta-tables) |
| F122 | Enhanced diagnostics management | [Diagnostics](#diagnostics) |
| F123 | All diagnostics | [Diagnostics](#diagnostics) |
| T511 | Transaction counts | [Diagnostics](#diagnostics) |
| T171 | LIKE clause in table definition | [LIKE and AS in CREATE TABLE](#like-and-as-in-create-table) |
| T172 | AS subquery clause in table definition | [LIKE and AS in CREATE TABLE](#like-and-as-in-create-table) |
| T173 | Extended LIKE clause in table definition | [LIKE and AS in CREATE TABLE](#like-and-as-in-create-table) |
| F292 | UNIQUE null treatment | [UNIQUE null treatment](#unique-null-treatment) |
| T121 | WITH (excluding RECURSIVE) in query expression | [WITH and WITH RECURSIVE](#with-and-with-recursive) |
| T122 | WITH (excluding RECURSIVE) in subquery | [WITH and WITH RECURSIVE](#with-and-with-recursive) |
| T131 | Recursive query | [WITH and WITH RECURSIVE](#with-and-with-recursive) |
| T132 | Recursive query in subquery | [WITH and WITH RECURSIVE](#with-and-with-recursive) |
| T133 | Enhanced cycle mark values | [WITH and WITH RECURSIVE](#with-and-with-recursive) |
| F403 | Partitioned join tables | [Partitioned joins](#partitioned-joins) |
| T431 | Extended grouping capabilities | [Grouping sets](#grouping-sets) |
| T432 | Nested and concatenated GROUPING SETS | [Grouping sets](#grouping-sets) |
| T433 | Multi-argument GROUPING function | [Grouping sets](#grouping-sets) |
| T434 | GROUP BY DISTINCT | [Grouping sets](#grouping-sets) |
| T625 | LISTAGG | [LISTAGG and ANY_VALUE](#listagg-and-any_value) |
| T626 | ANY_VALUE | [LISTAGG and ANY_VALUE](#listagg-and-any_value) |
| T621 | Enhanced numeric functions | [Enhanced numeric functions](#enhanced-numeric-functions) |
| T301 | Functional dependencies | [Functional dependencies](#functional-dependencies) |
| E121-02 | ORDER BY columns need not be in select list | [Sort keys](#sort-keys) |
| E121-03 | Value expressions in ORDER BY clause | [Sort keys](#sort-keys) |
| F868 | ORDER BY in grouped table | [Sort keys](#sort-keys) |
| F850 | Top-level ORDER BY in query expression | [ORDER BY, OFFSET and FETCH FIRST](#order-by-offset-and-fetch-first) |
| F851 | ORDER BY in subqueries | [ORDER BY, OFFSET and FETCH FIRST](#order-by-offset-and-fetch-first) |
| F852 | Top-level ORDER BY in views | [ORDER BY, OFFSET and FETCH FIRST](#order-by-offset-and-fetch-first) |
| F855 | Nested ORDER BY in query expression | [ORDER BY, OFFSET and FETCH FIRST](#order-by-offset-and-fetch-first) |
| F856 | Nested FETCH FIRST in query expression | [ORDER BY, OFFSET and FETCH FIRST](#order-by-offset-and-fetch-first) |
| F857 | Top-level FETCH FIRST in query expression | [ORDER BY, OFFSET and FETCH FIRST](#order-by-offset-and-fetch-first) |
| F858 | FETCH FIRST in subqueries | [ORDER BY, OFFSET and FETCH FIRST](#order-by-offset-and-fetch-first) |
| F859 | Top-level FETCH FIRST in views | [ORDER BY, OFFSET and FETCH FIRST](#order-by-offset-and-fetch-first) |
| F860 | Dynamic FETCH FIRST row count | [ORDER BY, OFFSET and FETCH FIRST](#order-by-offset-and-fetch-first) |
| F861 | Top-level OFFSET in query expression | [ORDER BY, OFFSET and FETCH FIRST](#order-by-offset-and-fetch-first) |
| F862 | OFFSET in subqueries | [ORDER BY, OFFSET and FETCH FIRST](#order-by-offset-and-fetch-first) |
| F863 | Nested OFFSET in query expression | [ORDER BY, OFFSET and FETCH FIRST](#order-by-offset-and-fetch-first) |
| F864 | Top-level OFFSET in views | [ORDER BY, OFFSET and FETCH FIRST](#order-by-offset-and-fetch-first) |
| F865 | Dynamic offset row count in OFFSET | [ORDER BY, OFFSET and FETCH FIRST](#order-by-offset-and-fetch-first) |
| F866 | FETCH FIRST clause: PERCENT option | [ORDER BY, OFFSET and FETCH FIRST](#order-by-offset-and-fetch-first) |
| F867 | FETCH FIRST clause: WITH TIES option | [ORDER BY, OFFSET and FETCH FIRST](#order-by-offset-and-fetch-first) |
| T611 | Elementary OLAP operations | [Window functions](#window-functions) |
| T612 | Advanced OLAP operations | [Window functions](#window-functions) |
| T614 | NTILE function | [Window functions](#window-functions) |
| T615 | LEAD and LAG functions | [Window functions](#window-functions) |
| T616 | Null treatment option for LEAD and LAG functions | [Window functions](#window-functions) |
| T617 | FIRST_VALUE and LAST_VALUE functions | [Window functions](#window-functions) |
| T618 | NTH_VALUE function | [Window functions](#window-functions) |
| T619 | Nested window functions | [Window functions](#window-functions) |
| T620 | WINDOW clause: GROUPS option | [Window functions](#window-functions) |
| T627 | Window framed COUNT DISTINCT | [Window functions](#window-functions) |
| R010 | Row pattern recognition: FROM clause | [Row pattern recognition](#row-pattern-recognition) |
| R020 | Row pattern recognition: WINDOW clause | [Row pattern recognition](#row-pattern-recognition) |
| R030 | Row pattern recognition: full aggregate support | [Row pattern recognition](#row-pattern-recognition) |
| S090 | Minimal array support | [Arrays and row types](#arrays-and-row-types) |
| S091 | Basic array support | [Arrays and row types](#arrays-and-row-types) |
| S093 | Arrays of distinct types | [Arrays and row types](#arrays-and-row-types) |
| S095 | Array constructors by query | [Arrays and row types](#arrays-and-row-types) |
| S096 | Optional array bounds | [Arrays and row types](#arrays-and-row-types) |
| S097 | Array element assignment | [Arrays and row types](#arrays-and-row-types) |
| S098 | ARRAY_AGG | [Arrays and row types](#arrays-and-row-types) |
| S099 | Array expressions | [Arrays and row types](#arrays-and-row-types) |
| S301 | Enhanced UNNEST | [Arrays and row types](#arrays-and-row-types) |
| S403 | ARRAY_MAX_CARDINALITY | [Arrays and row types](#arrays-and-row-types) |
| S404 | TRIM_ARRAY | [Arrays and row types](#arrays-and-row-types) |
| T051 | Row types | [Arrays and row types](#arrays-and-row-types) |
| T053 | Explicit aliases for all-fields reference | [Arrays and row types](#arrays-and-row-types) |
| S202 | SQL-invoked routines on multisets | [Multisets](#multisets) |
| S271 | Basic multiset support | [Multisets](#multisets) |
| S272 | Multisets of user-defined types | [Multisets](#multisets) |
| S275 | Advanced multiset support | [Multisets](#multisets) |
| S281 | Nested collection types | [Multisets](#multisets) |
| S402 | Distinct types based on multiset types | [Multisets](#multisets) |
| S011 | Distinct data types | [Distinct types](#distinct-types) |
| S028 | Permutable UDT options list | [Distinct types](#distinct-types) |
| S401 | Distinct types based on array types | [Distinct types](#distinct-types) |
| S023 | Basic structured types | [Structured types](#structured-types) |
| S024 | Enhanced structured types | [Structured types](#structured-types) |
| S025 | Final structured types | [Structured types](#structured-types) |
| S026 | Self-referencing structured types | [Structured types](#structured-types) |
| S027 | Create method by specific method name | [Structured types](#structured-types) |
| S151 | Type predicate | [Structured types](#structured-types) |
| S161 | Subtype treatment | [Structured types](#structured-types) |
| S211 | User-defined cast functions | [Structured types](#structured-types) |
| S241 | Transform functions | [Structured types](#structured-types) |
| S261 | Specific type method | [Structured types](#structured-types) |
| S051 | Create table of type | [Typed tables](#typed-tables) |
| S081 | Subtables | [Typed tables](#typed-tables) |
| S111 | ONLY in query expressions | [Typed tables](#typed-tables) |
| S041 | Basic reference types | [Reference types](#reference-types) |
| S043 | Enhanced reference types | [Reference types](#reference-types) |
| S094 | Arrays of reference types | [Reference types](#reference-types) |
| S162 | Subtype treatment for references | [Reference types](#reference-types) |
| S274 | Multisets of reference types | [Reference types](#reference-types) |
| T180 | System-versioned tables | [Periods](#periods) |
| T181 | Application-time period tables | [Periods](#periods) |
| T502 | Period predicates | [Periods](#periods) |
| T200 | Trigger DDL | [Triggers](#triggers) |
| T211 | Basic trigger capability | [Triggers](#triggers) |
| T212 | Enhanced trigger capability | [Triggers](#triggers) |
| T213 | INSTEAD OF triggers | [Triggers](#triggers) |
| T214 | BEFORE triggers | [Triggers](#triggers) |
| T215 | AFTER triggers | [Triggers](#triggers) |
| T216 | Ability to require true search condition before trigger is invoked | [Triggers](#triggers) |
| T217 | TRIGGER privilege | [Triggers](#triggers) |
| T218 | Multiple triggers for the same event executed in the order created | [Triggers](#triggers) |
| T321 | Basic SQL-invoked routines | [SQL-invoked routines](#sql-invoked-routines) |
| T341 | Overloading of SQL-invoked functions and SQL-invoked procedures | [SQL-invoked routines](#sql-invoked-routines) |
| B033 | Untyped SQL-invoked function arguments | [SQL-invoked routines](#sql-invoked-routines) |
| B128 | Routine language SQL | [SQL-invoked routines](#sql-invoked-routines) |
| S203 | Array parameters | [SQL-invoked routines](#sql-invoked-routines) |
| S204 | Array as result type of functions | [SQL-invoked routines](#sql-invoked-routines) |
| B200 | Polymorphic table functions | [Polymorphic table functions](#polymorphic-table-functions) |
| B201 | More than one PTF generic table parameter | [Polymorphic table functions](#polymorphic-table-functions) |
| B202 | PTF copartitioning | [Polymorphic table functions](#polymorphic-table-functions) |
| B203 | More than one copartition specification | [Polymorphic table functions](#polymorphic-table-functions) |
| B204 | PRUNE WHEN EMPTY | [Polymorphic table functions](#polymorphic-table-functions) |
| B205 | Pass-through columns | [Polymorphic table functions](#polymorphic-table-functions) |
| B206 | PTF descriptor parameters | [Polymorphic table functions](#polymorphic-table-functions) |
| B207 | Cross products of partitionings | [Polymorphic table functions](#polymorphic-table-functions) |
| B208 | PTF component procedure interface | [Polymorphic table functions](#polymorphic-table-functions) |
| B209 | PTF extended names | [Polymorphic table functions](#polymorphic-table-functions) |
| T331 | Basic roles | [Roles](#roles) |
| T332 | Extended roles | [Roles](#roles) |
| T801 | JSON data type | [SQL/JSON](#sqljson) |
| T802 | Enhanced JSON data type | [SQL/JSON](#sqljson) |
| T811 | Basic SQL/JSON constructor functions | [SQL/JSON](#sqljson) |
| T812 | SQL/JSON: JSON_OBJECTAGG | [SQL/JSON](#sqljson) |
| T813 | SQL/JSON: JSON_ARRAYAGG with ORDER BY | [SQL/JSON](#sqljson) |
| T814 | Colon in JSON_OBJECT or JSON_OBJECTAGG | [SQL/JSON](#sqljson) |
| T821 | Basic SQL/JSON query operators | [SQL/JSON](#sqljson) |
| T822 | SQL/JSON: IS JSON WITH UNIQUE KEYS predicate | [SQL/JSON](#sqljson) |
| T823 | SQL/JSON: PASSING clause | [SQL/JSON](#sqljson) |
| T824 | JSON_TABLE: specific PLAN clause | [SQL/JSON](#sqljson) |
| T825 | SQL/JSON: ON EMPTY and ON ERROR clauses | [SQL/JSON](#sqljson) |
| T826 | General value expression in ON ERROR or ON EMPTY clauses | [SQL/JSON](#sqljson) |
| T827 | JSON_TABLE: sibling NESTED COLUMNS clauses | [SQL/JSON](#sqljson) |
| T828 | JSON_QUERY | [SQL/JSON](#sqljson) |
| T829 | JSON_QUERY: array wrapper options | [SQL/JSON](#sqljson) |
| T830 | Enforcing unique keys in SQL/JSON constructor functions | [SQL/JSON](#sqljson) |
| T831 | SQL/JSON path language: strict mode | [SQL/JSON](#sqljson) |
| T832 | SQL/JSON path language: item method | [SQL/JSON](#sqljson) |
| T833 | SQL/JSON path language: multiple subscripts | [SQL/JSON](#sqljson) |
| T834 | SQL/JSON path language: wildcard member accessor | [SQL/JSON](#sqljson) |
| T835 | SQL/JSON path language: filter expressions | [SQL/JSON](#sqljson) |
| T836 | SQL/JSON path language: starts with predicate | [SQL/JSON](#sqljson) |
| T837 | SQL/JSON path language: regex_like predicate | [SQL/JSON](#sqljson) |
| T838 | JSON_TABLE: PLAN DEFAULT clause | [SQL/JSON](#sqljson) |
| T840 | Hex integer literals in SQL/JSON path language | [SQL/JSON](#sqljson) |
| T851 | SQL/JSON: optional keywords for default syntax | [SQL/JSON](#sqljson) |
| T860 | SQL/JSON simplified accessor: column reference only | [SQL/JSON](#sqljson) |
| T862 | SQL/JSON simplified accessor: wildcard member accessor | [SQL/JSON](#sqljson) |
| T863 | SQL/JSON simplified accessor: single-quoted string literal as member accessor | [SQL/JSON](#sqljson) |
| T864 | SQL/JSON simplified accessor | [SQL/JSON](#sqljson) |
| T865 | SQL/JSON item method: bigint() | [SQL/JSON](#sqljson) |
| T866 | SQL/JSON item method: boolean() | [SQL/JSON](#sqljson) |
| T867 | SQL/JSON item method: date() | [SQL/JSON](#sqljson) |
| T868 | SQL/JSON item method: decimal() | [SQL/JSON](#sqljson) |
| T869 | SQL/JSON item method: decimal() with precision and scale | [SQL/JSON](#sqljson) |
| T870 | SQL/JSON item method: integer() | [SQL/JSON](#sqljson) |
| T871 | SQL/JSON item method: number() | [SQL/JSON](#sqljson) |
| T872 | SQL/JSON item method: string() | [SQL/JSON](#sqljson) |
| T873 | SQL/JSON item method: time() | [SQL/JSON](#sqljson) |
| T874 | SQL/JSON item method: time_tz() | [SQL/JSON](#sqljson) |
| T875 | SQL/JSON item method: time precision | [SQL/JSON](#sqljson) |
| T876 | SQL/JSON item method: timestamp() | [SQL/JSON](#sqljson) |
| T877 | SQL/JSON item method: timestamp_tz() | [SQL/JSON](#sqljson) |
| T878 | SQL/JSON item method: timestamp precision | [SQL/JSON](#sqljson) |
| T879 | JSON in equality operations | [SQL/JSON](#sqljson) |
| T880 | JSON in grouping operations | [SQL/JSON](#sqljson) |
| T881 | JSON in ordering operations | [SQL/JSON](#sqljson) |
| T882 | JSON in multiset element grouping operations | [SQL/JSON](#sqljson) |
| T271 | Savepoints | [Savepoints](#savepoints) |
| T272 | Enhanced savepoint management | [Savepoints](#savepoints) |
| T351 | Bracketed comments | [Bracketed comments](#bracketed-comments) |
| P002-01 | <compound statement> | [SQL/PSM](#sqlpsm) |
| P002-02 | <handler declaration> | [SQL/PSM](#sqlpsm) |
| P002-03 | <condition declaration> | [SQL/PSM](#sqlpsm) |
| P002-04 | <SQL variable declaration> | [SQL/PSM](#sqlpsm) |
| P002-05 | <assignment statement> | [SQL/PSM](#sqlpsm) |
| P002-06 | <case statement> | [SQL/PSM](#sqlpsm) |
| P002-07 | <if statement> | [SQL/PSM](#sqlpsm) |
| P002-08 | <iterate statement> | [SQL/PSM](#sqlpsm) |
| P002-09 | <leave statement> | [SQL/PSM](#sqlpsm) |
| P002-10 | <loop statement> | [SQL/PSM](#sqlpsm) |
| P002-11 | <repeat statement> | [SQL/PSM](#sqlpsm) |
| P002-12 | <while statement> | [SQL/PSM](#sqlpsm) |
| P002-13 | <for statement> | [SQL/PSM](#sqlpsm) |
| P002-14 | <signal statement> | [SQL/PSM](#sqlpsm) |
| P002-15 | <resignal statement> | [SQL/PSM](#sqlpsm) |
| P008 | Comma-separated predicates in a CASE statement | [SQL/PSM](#sqlpsm) |
| X010 | XML type | [SQL/XML](#sqlxml) |
| X011 | Arrays of XML type | [SQL/XML](#sqlxml) |
| X012 | Multisets of XML type | [SQL/XML](#sqlxml) |
| X013 | Distinct types of XML type | [SQL/XML](#sqlxml) |
| X014 | Attributes of XML type | [SQL/XML](#sqlxml) |
| X015 | Fields of XML type | [SQL/XML](#sqlxml) |
| X016 | Persistent XML values | [SQL/XML](#sqlxml) |
| X020 | XMLConcat | [SQL/XML](#sqlxml) |
| X025 | XMLCast | [SQL/XML](#sqlxml) |
| X030 | XMLDocument | [SQL/XML](#sqlxml) |
| X031 | XMLElement | [SQL/XML](#sqlxml) |
| X032 | XMLForest | [SQL/XML](#sqlxml) |
| X034 | XMLAgg | [SQL/XML](#sqlxml) |
| X035 | XMLAgg: ORDER BY option | [SQL/XML](#sqlxml) |
| X036 | XMLComment | [SQL/XML](#sqlxml) |
| X037 | XMLPI | [SQL/XML](#sqlxml) |
| X038 | XMLText | [SQL/XML](#sqlxml) |
| X040 | Basic table mapping | [SQL/XML](#sqlxml) |
| X041 | Basic table mapping: null absent | [SQL/XML](#sqlxml) |
| X042 | Basic table mapping: null as nil | [SQL/XML](#sqlxml) |
| X043 | Basic table mapping: table as forest | [SQL/XML](#sqlxml) |
| X044 | Basic table mapping: table as element | [SQL/XML](#sqlxml) |
| X045 | Basic table mapping: with target namespace | [SQL/XML](#sqlxml) |
| X046 | Basic table mapping: data mapping | [SQL/XML](#sqlxml) |
| X047 | Basic table mapping: metadata mapping | [SQL/XML](#sqlxml) |
| X048 | Basic table mapping: base64 encoding of binary strings | [SQL/XML](#sqlxml) |
| X049 | Basic table mapping: hex encoding of binary strings | [SQL/XML](#sqlxml) |
| X050 | Advanced table mapping | [SQL/XML](#sqlxml) |
| X051 | Advanced table mapping: null absent | [SQL/XML](#sqlxml) |
| X052 | Advanced table mapping: null as nil | [SQL/XML](#sqlxml) |
| X053 | Advanced table mapping: table as forest | [SQL/XML](#sqlxml) |
| X054 | Advanced table mapping: table as element | [SQL/XML](#sqlxml) |
| X055 | Advanced table mapping: with target namespace | [SQL/XML](#sqlxml) |
| X056 | Advanced table mapping: data mapping | [SQL/XML](#sqlxml) |
| X057 | Advanced table mapping: metadata mapping | [SQL/XML](#sqlxml) |
| X058 | Advanced table mapping: base64 encoding of binary strings | [SQL/XML](#sqlxml) |
| X059 | Advanced table mapping: hex encoding of binary strings | [SQL/XML](#sqlxml) |
| X060 | XMLParse: Character string input and CONTENT option | [SQL/XML](#sqlxml) |
| X061 | XMLParse: Character string input and DOCUMENT option | [SQL/XML](#sqlxml) |
| X065 | XMLParse: BLOB input and CONTENT option | [SQL/XML](#sqlxml) |
| X066 | XMLParse: BLOB input and DOCUMENT option | [SQL/XML](#sqlxml) |
| X070 | XMLSerialize: Character string serialization and CONTENT option | [SQL/XML](#sqlxml) |
| X071 | XMLSerialize: Character string serialization and DOCUMENT option | [SQL/XML](#sqlxml) |
| X072 | XMLSerialize: Character string serialization | [SQL/XML](#sqlxml) |
| X073 | XMLSerialize: BLOB serialization and CONTENT option | [SQL/XML](#sqlxml) |
| X074 | XMLSerialize: BLOB serialization and DOCUMENT option | [SQL/XML](#sqlxml) |
| X075 | XMLSerialize: BLOB serialization | [SQL/XML](#sqlxml) |
| X076 | XMLSerialize: VERSION | [SQL/XML](#sqlxml) |
| X077 | XMLSerialize: explicit ENCODING option | [SQL/XML](#sqlxml) |
| X078 | XMLSerialize: explicit XML declaration | [SQL/XML](#sqlxml) |
| X080 | Namespaces in XML publishing | [SQL/XML](#sqlxml) |
| X081 | Query-level XML namespace declarations | [SQL/XML](#sqlxml) |
| X082 | XML namespace declarations in DML | [SQL/XML](#sqlxml) |
| X083 | XML namespace declarations in DDL | [SQL/XML](#sqlxml) |
| X084 | XML namespace declarations in compound statements | [SQL/XML](#sqlxml) |
| X085 | Predefined namespace prefixes | [SQL/XML](#sqlxml) |
| X086 | XML namespace declarations in XMLTable | [SQL/XML](#sqlxml) |
| X090 | XML document predicate | [SQL/XML](#sqlxml) |
| X091 | XML content predicate | [SQL/XML](#sqlxml) |
| X096 | XMLExists | [SQL/XML](#sqlxml) |
| X120 | XML parameters in SQL routines | [SQL/XML](#sqlxml) |
| X131 | Query-level XMLBINARY clause | [SQL/XML](#sqlxml) |
| X132 | XMLBINARY clause in DML | [SQL/XML](#sqlxml) |
| X133 | XMLBINARY clause in DDL | [SQL/XML](#sqlxml) |
| X134 | XMLBINARY clause in compound statements | [SQL/XML](#sqlxml) |
| X135 | XMLBINARY clause in subqueries | [SQL/XML](#sqlxml) |
| X141 | IS VALID predicate: data-driven case | [SQL/XML](#sqlxml) |
| X142 | IS VALID predicate: ACCORDING TO clause | [SQL/XML](#sqlxml) |
| X143 | IS VALID predicate: ELEMENT clause | [SQL/XML](#sqlxml) |
| X144 | IS VALID predicate: schema location | [SQL/XML](#sqlxml) |
| X145 | IS VALID predicate outside check constraints | [SQL/XML](#sqlxml) |
| X151 | IS VALID predicate with DOCUMENT option | [SQL/XML](#sqlxml) |
| X152 | IS VALID predicate with CONTENT option | [SQL/XML](#sqlxml) |
| X153 | IS VALID predicate with SEQUENCE option | [SQL/XML](#sqlxml) |
| X155 | IS VALID predicate: NAMESPACE without ELEMENT clause | [SQL/XML](#sqlxml) |
| X157 | IS VALID predicate: NO NAMESPACE with ELEMENT clause | [SQL/XML](#sqlxml) |
| X160 | Basic Information Schema for registered XML Schemas | [SQL/XML](#sqlxml) |
| X161 | Advanced Information Schema for registered XML Schemas | [SQL/XML](#sqlxml) |
| X170 | XML null handling options | [SQL/XML](#sqlxml) |
| X171 | NIL ON NO CONTENT option | [SQL/XML](#sqlxml) |
| X181 | XML(DOCUMENT(UNTYPED)) type | [SQL/XML](#sqlxml) |
| X182 | XML(DOCUMENT(ANY)) type | [SQL/XML](#sqlxml) |
| X190 | XML(SEQUENCE) type | [SQL/XML](#sqlxml) |
| X191 | XML(DOCUMENT(XMLSCHEMA)) type | [SQL/XML](#sqlxml) |
| X192 | XML(CONTENT(XMLSCHEMA)) type | [SQL/XML](#sqlxml) |
| X200 | XMLQuery | [SQL/XML](#sqlxml) |
| X201 | XMLQuery: RETURNING CONTENT | [SQL/XML](#sqlxml) |
| X202 | XMLQuery: RETURNING SEQUENCE | [SQL/XML](#sqlxml) |
| X203 | XMLQuery: passing a context item | [SQL/XML](#sqlxml) |
| X204 | XMLQuery: initializing an XQuery variable | [SQL/XML](#sqlxml) |
| X205 | XMLQuery: EMPTY ON EMPTY option | [SQL/XML](#sqlxml) |
| X206 | XMLQuery: NULL ON EMPTY option | [SQL/XML](#sqlxml) |
| X211 | XML 1.1 support | [SQL/XML](#sqlxml) |
| X221 | XML passing mechanism BY VALUE | [SQL/XML](#sqlxml) |
| X222 | XML passing mechanism BY REF | [SQL/XML](#sqlxml) |
| X231 | XML(CONTENT(UNTYPED)) type | [SQL/XML](#sqlxml) |
| X232 | XML(CONTENT(ANY)) type | [SQL/XML](#sqlxml) |
| X241 | RETURNING CONTENT in XML publishing | [SQL/XML](#sqlxml) |
| X242 | RETURNING SEQUENCE in XML publishing | [SQL/XML](#sqlxml) |
| X251 | Persistent XML values of XML(DOCUMENT(UNTYPED)) type | [SQL/XML](#sqlxml) |
| X252 | Persistent XML values of XML(DOCUMENT(ANY)) type | [SQL/XML](#sqlxml) |
| X253 | Persistent XML values of XML(CONTENT(UNTYPED)) type | [SQL/XML](#sqlxml) |
| X254 | Persistent XML values of XML(CONTENT(ANY)) type | [SQL/XML](#sqlxml) |
| X255 | Persistent XML values of XML(SEQUENCE) type | [SQL/XML](#sqlxml) |
| X256 | Persistent XML values of XML(DOCUMENT(XMLSCHEMA)) type | [SQL/XML](#sqlxml) |
| X257 | Persistent XML values of XML(CONTENT(XMLSCHEMA)) type | [SQL/XML](#sqlxml) |
| X260 | XML type: ELEMENT clause | [SQL/XML](#sqlxml) |
| X261 | XML type: NAMESPACE without ELEMENT clause | [SQL/XML](#sqlxml) |
| X263 | XML type: NO NAMESPACE with ELEMENT clause | [SQL/XML](#sqlxml) |
| X264 | XML type: schema location | [SQL/XML](#sqlxml) |
| X271 | XMLValidate: data-driven case | [SQL/XML](#sqlxml) |
| X272 | XMLValidate: ACCORDING TO clause | [SQL/XML](#sqlxml) |
| X273 | XMLValidate: ELEMENT clause | [SQL/XML](#sqlxml) |
| X274 | XMLValidate: schema location | [SQL/XML](#sqlxml) |
| X281 | XMLValidate: with DOCUMENT option | [SQL/XML](#sqlxml) |
| X282 | XMLValidate with CONTENT option | [SQL/XML](#sqlxml) |
| X283 | XMLValidate with SEQUENCE option | [SQL/XML](#sqlxml) |
| X284 | XMLValidate NAMESPACE without ELEMENT clause | [SQL/XML](#sqlxml) |
| X286 | XMLValidate: NO NAMESPACE with ELEMENT clause | [SQL/XML](#sqlxml) |
| X300 | XMLTable | [SQL/XML](#sqlxml) |
| X301 | XMLTable: derived column list option | [SQL/XML](#sqlxml) |
| X302 | XMLTable: ordinality column option | [SQL/XML](#sqlxml) |
| X303 | XMLTable: column default option | [SQL/XML](#sqlxml) |
| X304 | XMLTable: passing a context item | [SQL/XML](#sqlxml) |
| X305 | XMLTable: initializing an XQuery variable | [SQL/XML](#sqlxml) |
| X400 | Name and identifier mapping | [SQL/XML](#sqlxml) |

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
| 2023 §6.31 SR16b, SR17b, SR18b, IV146, IV148, IV150, the type of LN, EXP, POWER and SQRT | DOUBLE PRECISION | `functions/numeric.rs` |
| 2023 §6.31 SR16a, SR17a, SR18a, IV145, IV147, IV149, of a DECFLOAT operand | DECFLOAT(34); LN, EXP and POWER to a non-whole power found in double precision through the decimal exponent, kept to 16 digits; SQRT and whole powers to 34 digits | `functions/numeric/decimal.rs` |
| 2023 §6.31 SR20a, IV151, the type of FLOOR and CEILING of NUMERIC(p, s) | NUMERIC(p − s + 1, 0); of scale 0 the operand's type | `functions/numeric.rs` |
| 2023 §10.9 SR7h ii, SR9c ii 2, IV173, IV176, the type of the statistical set functions | DOUBLE PRECISION | `functions/statistics.rs` |
| 2023 §10.9 SR7h i, SR9c ii 1, IV172, IV175, of a DECFLOAT operand | DECFLOAT(34) | `functions/statistics.rs` |
| 2023 §10.9 SR9c i, IV174, the type of REGR_COUNT | NUMERIC(18), COUNT's | `functions/statistics.rs` |
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
| 2023 §4.7.2, §6.1 Table 13, IL061 and IL062, the least and greatest time zone displacements | -12:59 and +14:00 | `value/datetime.rs` |
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

### SQL/JSON

| Rule | Choice | Where |
|---|---|---|
| 2023 §6.28 SR1, ID074, and §6.35 SR1b, ID101, the type JSON_VALUE and JSON_QUERY return without RETURNING | CHARACTER VARYING(32767) | `json/query.rs`, `binder/json.rs` |
| 2023 §9.46 GR11b i 2 B, IV165, the character string type a PASSING argument of another type is cast to | CHARACTER VARYING(32767) | `json/query.rs` |
| 2023 §9.46 GR4, GR5, IA050, IA054, IV185, whether and how the context item and the FORMAT JSON arguments are parsed before the path is evaluated | They are, WITHOUT UNIQUE KEYS, so a text that is no JSON text is 22032 whether the path reads it or not | `json/query.rs` |
| 2023 §9.42 GR3a iii 3 A VIII 2, UA057, whether members of one key are removed as JSON text is parsed | None is. Every member is kept, so WITH UNIQUE KEYS can refuse them. | `json/text.rs` |
| 2023 §9.42 GR3a iii 3 A VIII 1, US039, the order the members of a JSON object are enumerated in | The order written | `json/text.rs` |
| 2023 §9.43 GR2a ii, UV097, the JSON text an SQL/JSON item is serialized as | No insignificant white space. Members in the order kept, those of one key all. Of a string, `"` and `\` escaped as `\"` and `\\`, and U+0000 to U+001F as `\b`, `\f`, `\n`, `\r` or `\t`, else `\u` and four lower case hexadecimal digits; no other character. A number as CAST writes it, a DECFLOAT without an exponent. | `json/serialize.rs` |
| 2023 §9.46 SR9, IA223, whether a syntactic analysis of a path may find an exception its evaluation would raise | None is made. Each exception is raised as the path is evaluated. | `json/path/parse.rs` |
| 2023 §9.46 GR11g ii 1 C, GR11g ii 2 B, IA071, whether a member or wildcard member accessor of an object of two members of one key raises 22037 | It does not | `json/path/eval/accessors.rs` |
| 2023 §9.46 GR11g ii 1 D II 2 a, UA075, which member of one key a member accessor takes | The first kept, the first written | `json/path/eval/accessors.rs` |
| 2023 §9.46 GR11g ii 2 C II 2 a iii, GR11g ii 6 C VIII 1 c iv, UA058, how many members of one key the wildcard member accessor and keyvalue() delete | None | `json/path/eval/accessors.rs`, `json/path/eval/methods/keyvalue.rs` |
| 2023 §9.46 GR11g ii 2 C II 2 a ii, iv, GR11g ii 6 C VIII 1 c iii, US040, the order the members of an SQL/JSON object are enumerated in | The order kept | `json/path/eval/accessors.rs`, `json/path/eval/methods/keyvalue.rs` |
| 2023 §9.46 GR11g ii 3 F II 1, IA073, whether a subscript is truncated or rounded to scale 0 | Truncated toward zero | `json/path/eval/accessors.rs` |
| 2023 §9.46 GR11g ii 6 C VII 2 a, UA070, which exception datetime() raises of several items in error | The first item's | `json/path/eval/methods/datetime.rs` |
| 2023 §9.46 GR11g ii 6 C VIII 1 b, IA072, whether keyvalue() of an object of two members of one key raises 22037 | It does not | `json/path/eval/methods/keyvalue.rs` |
| 2023 §9.46 GR11g ii 6 C VIII 2 b, IA224, the value of `id` that uniquely identifies an object of keyvalue() | Each item of the context item, then of the PASSING variables in order, is numbered in preorder from 0, and an object the path reaches there has its number however it is reached. An object the path makes is numbered past them, in the order reached. | `json/path/eval/methods/keyvalue.rs` |
| 2023 §8.2 GR10, IA224, IA228, IA229, the order of SQL/JSON values | The null, numbers by exact value, strings by code point, false, true, datetimes (times, then dates and timestamps by UTC instant), arrays element by element, objects by their members sorted by key | `json/binary/key.rs` |
| 2023 §7.11 SR1c ii, IV158, the type of an ordinality column of JSON_TABLE | INTEGER | `binder/json/table.rs` |
| 2023 §9.46 GR11g ii 6 C XIV 1 b, IV186, the numeric type NT of number() | A number's own, so a number is kept as it is. Of a character string, the type of the `<signed numeric literal>` it is. | `json/path/eval/methods/casts.rs` |
| 2023 §9.46 GR11g ii 6 C XV 1 b, IL006, ML, the maximum length of string()'s CHARACTER VARYING | 32,767 characters | `json/path/eval/methods/casts.rs` |
| 2023 §9.46 GR12c iii 2 E II, GR12d ii 2 F II, GR12e ii 3 D II, UA073, a comparison, like_regex or starts with of which one test is in error and one True | True. A test stops at its first success in lax mode, and at its first error in strict mode, where the rule before makes it Unknown. | `json/path/eval/predicate.rs` |
| 2023 §6.34 SR2b, ID098, and §10.11 SR1b, ID134, the type a constructor returns without RETURNING | CHARACTER VARYING(32767), FORMAT JSON | `json/construct/object.rs`, `json/construct/array.rs`, `functions/json/operands.rs` |
| 2023 §6.34 GR2d ii 1 F IV 2, GR3c ii 1 B IV 2, GR4b ii 1 B IV 2, ID100, and §10.11 GR4b ii 2 D IV 2, GR5f ii 2 D II, IV179, the character string type SDT a constructor's input of another type is cast to | CHARACTER VARYING(32767), of a datetime too | `json/construct.rs` |
| 2023 §6.34 GR3c ii 1 B III, GR4b ii 1 B III, IV156, and §10.11 GR5f ii 2 C, IV178, the `<JSON key uniqueness constraint>` an array constructor's input FORMAT JSON is parsed under | WITHOUT UNIQUE KEYS, as the query operators' (IV185) | `json/construct/array.rs` |
| 2023 §6.34 GR2a, IL049, the maximum length of a JSON name, and GR2b, GR3a, IL050, of a string input | That of its type, which no value of it exceeds | `json/construct.rs` |
| 2023 §6.34 GR2d ii 3, §10.11 GR4b ii 4, IL051, and §6.34 GR3c ii 2, GR4b ii 2, §10.11 GR5g, IL052, the most members of a JSON object and elements of a JSON array | 1,048,576 each, one more 2203E or 2203D | `json/construct.rs` |
| 2023 §10.11 GR5c, US044, the order of JSON_ARRAYAGG's elements without ORDER BY | The order its rows are read | `json/construct/aggregate.rs`, `functions/json/aggregate.rs` |

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
| §14.1 GR5, §4.28 what a SERIALIZABLE read through a spatial index reads | The rows whose geometries' envelopes, as the `rtree` index keys them, meet the window its search read, each join probe's own. A NULL is in no window and an empty geometry is at the origin. Past TRANSACTIONS.TRACKED_ROWS windows of a table, they merge into one rectangle of each column (RULES.md D134). | `storage/region.rs`, `exec/read.rs`, `engine/rows/mvcc/reads.rs` |
| §14.1 GR5, §4.28 what a SERIALIZABLE distance read or outer join through a spatial index reads | A distance window's search reads its value's envelope widened by the limit, or the whole table where the tree cannot certify its distances finite. A FULL outer join's probes read their windows, and then the whole table, for the rows no probe matched (RULES.md D135). | `index/rtree/geometry/within.rs`, `exec/join/indexed/preserved.rs` |
| §14.1 GR5, §4.28 what a SERIALIZABLE join reads that descends two spatial indexes together | The whole outer table, and of the other the rows whose envelopes meet the hull of the outer index's envelopes, and of a FULL join then the whole of it (RULES.md D138). | `exec/join/indexed/paired.rs` |
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
| IS003, ISO/IEC 9075-2:2023 §4.46.1 the order of triggers of one timestamp of creation | The order they were created in: each keeps an order of creation besides, one more than the greatest of the catalog's as it is defined | `db/triggers/define.rs` |
| US048, ISO/IEC 9075-2:2023 §15.21 GR4a the order a row trigger takes its transitions in | The order the statement changed the rows | `db/triggers/changes.rs`, `db/triggers/fire.rs` |
| IV223, ISO/IEC 9075-2:2023 §11.51 SR7a the source type of a distinct type of an exact or an approximate type | NUMERIC of its precision and scale, or FLOAT of its precision | `db/user_types.rs` |
| ID047, ISO/IEC 9075-2:2023 §9.18 SR10b the SQL-path | The schema of the statement's names, the session's default schema or a schema statement's own | `binder/routines.rs` |
| ID048, ISO/IEC 9075-2:2023 §4.9.6 a session's transform group of a distinct type | The one its definition makes, named FNUDT, in every session | `db/dynamic/using.rs`, `protocol/src/fields.rs` |

### Savepoints

| Rule | Choice | Where |
|---|---|---|
| IL208, ISO/IEC 9075-2:2023 §17.5 GR3 the maximum number of savepoints per SQL-transaction | 1,024, `MAX_SAVEPOINTS`. The 1,025th is 3B002. | `db/savepoints.rs` |
| IA167, ISO/IEC 9075-2:2023 §17.8 GR3i, GR3j whether a rollback to a savepoint deallocates a prepared statement | None is deallocated, whether prepared before the savepoint or after | `db/savepoints.rs` |
| IA168, ISO/IEC 9075-2:2023 §17.8 GR3h the status of a cursor opened before the savepoint | Closed, as every open cursor is [GR3g] | `db/savepoints.rs` |
| IA211, ISO/IEC 9075-2:2023 §4.43.2 how a rollback to a savepoint affects the diagnostics area, SQL descriptor areas and prepared statements | As any statement completing does. The diagnostics area tells of the ROLLBACK, the descriptor areas are unchanged, and no prepared statement is deallocated | `db/savepoints.rs` |

### Vectors and their indexes

The vector design document leaves these to the implementation or names
defaults that turborel tunes (RULES.md D93 to D104).

| Rule | Choice | Where |
|---|---|---|
| vector R6.3 the metric of VECTOR_DISTANCE written without one | The metric of an index of the first vector's column, else COSINE | `binder/vectors.rs` |
| vector R5.6 a vector's text | Shortest round-trip numbers, `[a,b,c]` with no spaces | `vector/text.rs` |
| vector R9.3 widening | The search asked again twice as wide, at most 4 times (`MOST_WIDENINGS`), then the table read exactly | `exec/nearest/approx.rs` |
| vector R13.2 filter strategy thresholds | Pre-filter below 1% estimated selectivity, post-filter above 50%, in-filter between | `physical/nearest/approx.rs` |
| vector R9.4, R10.2 TARGET ACCURACY | 90 by default. Accuracy a asks a list of k × 100 / (101 − a) and probes of lists × 100 / ((101 − a) × 80), capped | `index/vector/ask.rs`, `index/vector/options.rs` |
| vector §10.4 HNSW | M 16, EF_CONSTRUCTION 200, EF_SEARCH 64, codes trained at 10,000 nodes | `index/hnsw/options.rs` |
| vector §10.4 IVF | LISTS the square root of the rows, at least 16; PROBES an eighth of the lists; 256 vectors sampled per list | `index/ivf/options.rs` |
| vector §10.4 DiskANN | MAX_DEGREE 64, BUILD_LIST 100, ALPHA 1.2, 2 passes, SEARCH_LIST 100, BEAM_WIDTH 4, the nodes within 3 hops of the medoid cached, at most 1% of them, past BUILD_MEMORY (256 MiB) a build of 40 clusters, each point in its nearest 2, consolidation at 20% dead, codes trained at 4,096 nodes, PQ codes of half the dimensions | `engine/diskann.rs`, `index/diskann.rs` |
| vector §10.4 INVERTED | Blocks of 128 postings, weights FLOAT32 | `index/sparse/options.rs` |
| vector R7.1 RERANK_FACTOR | 4 of HNSW and IVF; DiskANN's whole search list | `engine/hnsw/query.rs`, `index/ivf/options.rs`, `index/diskann.rs` |
| vector R11.1, §11.3 hybrid candidates and K | TOP_N × 4, and RRF's K 60 | `binder/hybrid_search.rs`, `parser/query/hybrid_search.rs` |
| vector R12.3 embedding batch | 64 texts by default, a model's BATCH_SIZE; 4,096 texts embedded ahead by the queue | `vector/embed.rs`, `vector/embed/model.rs` |
| vector R12.6 when the queue runs | After each commit that wrote, and as the database opens, by the job `turborel-embedding` | `db/embedding/queue.rs` |
| A metric's or a quantizer's name | A regular identifier in upper case, or two joined by a period | `db/vectors.rs` |

### Property graphs

The property graph standard leaves these to the implementation (RULES.md
D140 to D145, D147 to D152).

| Rule | Choice | Where |
|---|---|---|
| GS §5 R5.1 an element's identity | ELEMENT_ID is the canonical JSON text `{"table":alias,"key":[...]}` of its element table's alias and key, VERTEX_ID and EDGE_ID its JSON value | `logical/graph/identity.rs` |
| GS §7.3 the default match mode | REPEATABLE ELEMENTS | `binder/graph/pattern.rs` |
| GS §20 limits | MAX_PATH_LENGTH and MAX_PATHS_PER_PARTITION none, MAX_SEARCH_STATES 10,000,000, ON LIMIT ERROR, of GRAPH.* | `settings/graph.rs` |
| GS §17 parallel search | EXECUTOR.GRAPH_THREADS workers, 1 by default | `exec/graph_table/limits.rs` |
| GS §17 when a search is parallel | on its workers only when there are more than one and its states, estimated from the graph's size, reach GRAPH.PARALLEL_MIN_STATES (16,384 by default); the starts in morsels, or one start's levels | `graph/search/choice.rs` |
| GS §15 the algorithm of ANY SHORTEST from many starts | MS-BFS, 64 starts at once, when a batch's starts times each one's estimated reach cover the graph's vertices; else BFS from each start. A step's degree is of the label transitions its edge's labels and the labels of the node patterns beside it match, once counted, else of every edge | `graph/search/choice.rs` |
| GS §16 R16.8 PATH_ALGO | chooses within the family the selector and restrictors call for: BFS and MSBFS of ANY SHORTEST and ANY, BFS of ALL SHORTEST and of a restricted shortest selector, BIBFS of one start and one end, DFS of every path, DIJKSTRA of the CHEAPEST selectors; else refused, -103 (GRF-0070) | `graph/search/choice.rs` |
| GS §13.2 an adjacency index's name | `GRAPH_ALIAS_ADJACENCY` if none is written | `db/graphs/adjacency.rs` |
| GS §13.2 DIRECTION | FORWARD keeps the OUT entries, REVERSE the IN ones, BOTH both, the default | `db/graphs/adjacency.rs` |
| GS §5 R5.1 a native element's identity | `$NODE_ID` and `$EDGE_ID` are the canonical JSON text `{"type":"node","schema":"S","table":"T","id":N}`, `"edge"` of an edge, N from 0 per table, numbers skipped after reopening as an identity column's are | `db/graphs/native/rows.rs` |
| GS §4.2 SYS_GRAPH's edge table aliases | The table's name when it joins one pair of node tables, else the name and the pair's, as `KNOWS PERSON TO CITY` | `db/graphs/native/graph.rs` |
| GQL's session | SESSION.LANGUAGE SQL, and GQL.GRAPH empty, naming no graph | `settings/builtin.rs` |
| GS §18 R18.3 parallel algorithms | EXECUTOR.GRAPH_THREADS threads, 1 by default, over morsels of 1024 vertices | `graph/algo/parallel.rs` |
| GS §20 GRAPH_ALGO_RUNS | The latest 1000 runs, kept in memory | `system/graph_runs.rs` |
| GS §16 R16.2 the joins of a rigid pattern | At most 64, of each choice of tables and ways, as a UNION ALL, and at most 4 whole scans of edge tables of no adjacency index | `binder/graph/joins.rs` |
| GS §16 R16.2 the worst-case optimal join | Generic join of the vertex variables, over every edge table of the cycle's adjacency index | `graph/wcoj.rs` |
| GS §16 R16.4 unrolling | Quantifiers of at most 3 iterations within no other, at most 16 rigid patterns | `binder/graph/joins/rigid.rs` |
| A label index's name | `GRAPH_ALIAS_LABELS`, or `NAME_ALIAS` of each table if no table is written | `db/graphs/indexes.rs` |
| A reachability index's name | `GRAPH_ALIAS_REACHABILITY` if none is written | `db/graphs/indexes.rs` |
| GS §13.6 statistics | INFORMATION_SCHEMA.GRAPH_STATISTICS: each element table's rows, an edge table's out-degree and in-degree average, median, 90th and 99th percentiles and maximum, a column property's distinct values and share NULL, and each label transition's edges, made of the tables' statistics (RULES.md D151); of an endpoint of several columns, its column of most distinct values stands for it | `stats/graph.rs` |
| GS §16 R16.8 where a search starts | GRAPH_START, GRAPH_DIRECTION (e, REVERSE), else the end of fewer candidates times fan-out once statistics are known, of a pattern of no selector, ALL SHORTEST or SHORTEST k GROUP, no COST, no limit cutting it and no DETERMINISTIC_PATHS; any other is searched from its first vertex | `exec/graph_table/start.rs` |

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
| XMLMAP's text | 64 MiB of a mapping's data, of all its tables, held as a set function writes it, row by row, and of its XML Schema document | §9.10 to §9.17 set no limit, and a table holds rows without end. The data is held as text until it is parsed, bounded as it grows. | `functions/xml/map/rows.rs`, `functions/xml/map/data.rs` |
| XML text and values | Elements and content models nest 512 deep; a tree holds 16 MiB of text and 1,048,576 nodes and namespace bindings; XMLPARSE reads 16 MiB more of entities' replacement texts and 64 MiB of input; a value's stored form is 64 MiB | XML 1.0 and Part 14 set no limit, and nested entities expand without end. A walk of a tree keeps a stack of its ancestors. | `xml.rs` |
| XQuery queries | Expressions nest 64 deep; an evaluation nests 256 deep in operations and function calls; a static typing 256 deep in expressions, its types 256 deep, an axis's closure of 4,096 item types; a sequence holds 1,048,576 items; one evaluation constructs nodes of 64 MiB | XQuery sets none. The parser, the analysis, the static typing and the evaluation recurse, so each is bounded within half of a thread's stack. | `xquery.rs`, `xquery/eval.rs`, `xquery/statics.rs` |
| Savepoints | 1,024 of a transaction, past which SAVEPOINT fails with SQLSTATE 3B002, SQLCODE -917, as §17.5 GR3 names | IL208 leaves the number to the implementation. Each savepoint holds a savepoint of each engine that undoes its own changes, so the number is bounded. | `db/savepoints.rs` |
| Nesting depth | 64 levels of the syntax tree, about 30 nested parentheses, 60 chained `AND`, `OR`, `+` or `UNION`, or 60 tables in a FROM clause | Without a limit, deep nesting overflowed the stack and aborted the process. 64 levels run in half of Rust's default 2 MiB thread stack. | `parser.rs` |
| SQL/JSON item nesting | 128 levels of arrays and objects, of JSON text parsed and of an item compared or serialized | §9.42 sets no limit, and RFC 8259 §9 lets a parser limit the depth of nesting. Every walk of an item recurses, and the limit bounds it (RULES.md R-CF-6). | `json.rs`, `json/text.rs`, `json/item.rs`, `json/serialize.rs` |
| SQL/JSON path nesting | 64 levels of a path's syntax tree, `$` one of them, so at most 63 accessors or operators in a row, and parentheses, filters, subscripts and signs nested at most 64 deep | §9.46 sets no limit. The parser and the evaluator recurse over the tree, which has no counter of its own, and the limit bounds both (RULES.md R-CF-6). | `json/path.rs`, `json/path/parse.rs` |
| SQL/JSON numbers | An exact number within DECFLOAT's range, a HexIntegerLiteral's too, and an approximate one within a double's, of JSON text and of a path | RFC 8259 §6 and §9 let an implementation limit the range and precision of numbers. An exact number of more than 18 digits is a DECFLOAT, rounded to 34 (RULES.md D84), and no type holds one past DECFLOAT's range. | `json/text.rs`, `json/path/lex.rs` |
| SQL/JSON item method precisions | 18 digits of decimal(); 6 digits of a second's fraction of time(), time_tz(), timestamp() and timestamp_tz(), of a string datetime() reads, and of a template's FFn | §9.46 sets none. The methods cast to DECIMAL and to the datetime types [GR11g ii 6 C XII, XVI to XIX], and datetime() makes a datetime [VII], which hold no more digits here (Exact precision above). | `json/path/parse/method.rs`, `json/path/eval/methods/datetime.rs` |
| A large object read as SQL/JSON | 262,144 octets, a chunk, of a character large object read whole as JSON text, as a PASSING argument or as a constructor's input or name | The text is held in memory whole to be parsed, as a LIKE pattern of a large object is, and to the same bound. | `json/query.rs`, `json/construct.rs`, `json/predicate.rs`, `functions/lobs/like.rs` |
| JSON object members and array elements | 1,048,576 of an object or an array constructed, one more SQLSTATE 2203E or 2203D | §6.34 GR2d ii 3, GR3c ii 2, GR4b ii 2 and §10.11 GR4b ii 4, GR5g leave the number to the implementation (IL051, IL052) and name these exceptions past it. The limit bounds what an aggregate holds. | `json/construct.rs` |

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
| Subscripts tested before they are defined [2023 §9.46 GR11g ii 3 E III] | The rule tests TJSFROMi and TJSTOi, which F II 1 defines after it. RJSFROMi and RJSTOi, the results of E II, are meant. An exception of a subscript's own evaluation, of which the rule says nothing, is the path's, as a base's is [GR11g i]. |
| The elements of a range in lax mode [2023 §9.46 GR11g ii 3 F II 2 b] | The rule takes the subscripts "between 0 (zero) and Nj, inclusive", and Nj is the subscript of no element. The elements there are are taken. |
| size() of "any Vj" [2023 §9.46 GR11g ii 6 C II 1] | Vj is what the rule then defines. The item Ij is meant, so in strict mode an item that is no array is 22039. |
| datetime() of a string without a time zone [2023 §9.46 GR11g ii 6 C VII 1 c ii to v] | c ii tries a string as a TIME WITH TIME ZONE before c iii tries it as a TIME WITHOUT TIME ZONE. A literal without a `<time zone interval>` is of a type without time zone [§5.3 SR34, SR35], so it determines no value of the type with one, and the type without is taken, a timestamp's likewise. |
| OBJh of keyvalue() [2023 §9.46 GR11g ii 6 C VIII 1 c vi] | The rule defines OBJh, 1 ≤ h ≤ q, of Ki and BVi. The key and bound value of the h-th member left are meant. |
| time_tz() and timestamp_tz() of a number [2023 §9.46 GR11g ii 6 C XVII 1 a, XIX 1 a] | Each takes "a number or character string", where time() and timestamp() take a character string alone. That is followed, and CAST of a number to a datetime is 22005. |
| The value of a HexIntegerLiteral [2023 §9.45 GR1a] | GR1a gives a numeric literal the value of the `<signed numeric literal>` of its characters, which a HexIntegerLiteral is not. Its value is ECMAScript's [ECMAScript 5.1 §7.8.3], exact. |
| starts with of a sequence [2023 §9.46 GR12e ii 2] | The rule asks whether B "is not a character string", B being an SQL/JSON sequence. It is to be one item, a character string, else the predicate is Unknown. |
| `+` and `-` in lax mode [2023 §9.46 GR11j] | GR11j has no step that unwraps its operands in lax mode, as GR11i ii has for `*`, `/` and `%`, and none is made. So `lax $ + 1` of `[1]` is 22038, where `lax $ * 2` of `[4]` is 8. |
| ST2 of JSON_VALUE [2023 §6.28 GR6] | GR6 raises ST2, and GR5 has named SR2 the status §9.48 returns. That status is meant. |
| The cast of DEFAULT ON EMPTY [2023 §9.48 GR4c iii] | The rule writes `CAST (VE AS DT>)`. `CAST (VE AS DT)` is meant. |
| When a DEFAULT is evaluated [2023 §9.48 GR4c iii, GR5d] | The rules evaluate `CAST (VE AS DT)` only when the behavior is taken, and a call evaluates its arguments first. A DEFAULT that can raise an exception is bound as a CASE of which behavior the row takes, and one that draws NEXT VALUE FOR keeps every DEFAULT an argument, as §6.14 GR1a draws of each row and SR2 puts it in no CASE. "This `<cast specification>`", whose exception ON ERROR replaces, is the cast's own, so an exception VE raises before the cast is raised. |
| Serialization's 22001 [2023 §9.43 GR2a iii] | GR2a ii lets a failed parse be the status ON ERROR handles, and GR2a iii says its exception "is raised". It is raised directly, so neither NULL nor EMPTY ARRAY ON ERROR replaces it. |
| `%` of a number with a fraction [2023 §6.31 SR12, §9.46 GR11i] | MOD's operands are exact of scale 0 or DECFLOAT, a Syntax Rule the path can check only as it runs. It is checked then, data exception 22000. A whole exact number of nonzero scale, `4.0`, is taken. |
| Numbers past 34 digits [2023 §9.42 GR3a iii 3 A IV, §5.3 SR29] | A number of which no number of the implementation is the value is out of range, 22003, never rounded. In a path literal it is SQLCODE -900, as other limits of the path's parse are. IS JSON raises it too, the text being of RFC 8259's grammar [§8.22 GR3a]. |
| The types of `number()`, `size()`, `string()` and `type()` in the simplified accessor [2023 §6.36 SR3a i 7, 8] | As printed `number()` and `size()` give CHARACTER VARYING and `string()` and `type()` BIGINT, which swaps `size()` and `string()` against §9.46 GR11g ii 6 C. `size()` is BIGINT, and `number()`, `string()` and `type()` CHARACTER VARYING(32767). |
| PASSING variables of a NESTED path [2023 §7.11 SR1g v] | The rule names "the PASSING clause of the JSON table". The row pattern and every NESTED path may name its variables, and a column's own path, which has no PASSING clause [SR1e iii, SR1f vi], may not, SQLCODE -103. |
| A JSON column of JSON_TABLE [2023 §7.11 SR1e ii] | A regular column is of no JSON type, so a column of the JSON type written without a formatted column's clause is formatted, and gives JSON_QUERY's value. |
| EMPTY BEHAVIOR of §9.48 [2023 §9.48 Subclause Signature] | The signature says it is ERROR, EMPTY ARRAY or EMPTY OBJECT, which are JSON_QUERY's. Its General Rules read JSON_VALUE's ERROR, NULL and DEFAULT, and they are followed. |
| A string input of IS JSON [2023 §8.22 CR2] | CR2 forbids a `<string value expression>` as the input without Feature T801, the JSON type, which a string is not. T803, String-based JSON, is meant, as of every string input. |
| JVNi of JSON_OBJECT [2023 §6.34 GR2d ii 1 G] | The rule makes JVNi a member's key. VJNi, the value of its name, is meant. |
| VBVi, CJOA and NJVE of the aggregates [2023 §10.11 GR4b ii 2 D II, GR4c, GR5g] | GR4b ii 2 D II lets VBVi be the value, where the other cases let JBVi be; GR4c lets JACF be CJOA, where GR4b made CJO; GR5g counts NJVE, which only §6.34 defines. JBVi, CJO and NTXA are meant. |
| FORMAT after a query of JSON_ARRAY [2023 §6.34 F, §7.6] | In JSON_ARRAY of a query whose FROM clause ends in a table of no correlation name, FORMAT is read as that table's correlation name, as the grammar lets it be, FORMAT being no reserved word. AS before a name, or the query in parentheses, keeps it the `<JSON input clause>`. |

### The property graph standard

| Text | Reading |
|---|---|
| T7's expected path [GS §23.2] | The fixture's transfer t6 from a6 to a5 makes the shortest path from Scott's account to Jay's and back to a blocked account 5 edges long, t1 t2 t3 t4 t6. The 6 edges the test lists are not the shortest of that fixture, so the 5-edge path is returned. |
| One variable of several labels [GS §6.2] | An element variable written twice with different label expressions may bind the tables of either, each occurrence then testing its own. T5 needs Zembla, a country only, bound by `(c:City) \| (c:Country)`. |
| Anonymous variables of a union [GS §6.4] | An element written without a variable is bound by no variable, so the branches of a union that match one path give one binding (T9). |
| A prefilter of a later variable [GS §6.8] | The standard names no point where it is tested, and [P1] tests a WHERE as the rigid pattern is matched, before the selector. So it is tested on the walk where the last variable it reads is bound. Inside a quantifier it waits in the thread for that binding. A variable left unbound at the end of the scope it is declared in is null, so a filter read before it in an iteration with no later binding sees null. |
| A prefilter of another path pattern's variable [GS §6.8] | The path pattern it reads is searched first, and this one once for each of its bindings, the prefilter reading that binding. A match joins the bindings each was searched with. Two path patterns reading each other's variables is SQLCODE -103, as neither can be searched first. |
| A path function in a prefilter [GS §6.9] | It is of the path so far, the element tested its last. A path kept across an unbounded quantifier outside every restrictor is GRF-0032, as the group of R7.4 is. |
| EXISTS IN PATH [GS §11.2] | The pattern is matched along the path in its order, each edge as the path follows it, from each of its vertices. Its variables are its own, so one of the graph pattern's declared in it is SQLCODE -103. Its prefilters may read the match's other variables. It is a postfilter of the path. |
| LINE_GRAPH [GS §11.3] | The text does not name the line graph's edge label. It is FOLLOWS, of no properties, from each edge of the label to each that leaves the vertex the first ends at. An undirected edge may be followed either way. The vertices keep the label's properties, the edge tables' keys their keys. No edge of the label is GRF-0012. |
| Graphs of query results [GS §11.3] | Each element table of VERTICES FROM and EDGES FROM is of its query's rows, read as the graph is queried. A query's rows declare no key, so its KEY is written and the graph is TRUSTED, and ENFORCED is SQLCODE -103. The graph depends on the tables its queries read, as on its element tables, and INFORMATION_SCHEMA.PG_ELEMENT_TABLES shows the query in ELEMENT_TABLE_QUERY, the table's names null. A GRAPH_TABLE read FOR SYSTEM_TIME refuses such a table, whose query says when it reads. |
| A set function in a group's filter [GS §5 R5.6, §7.4 R7.4] | It is over the elements its variable is bound to within the parenthesized pattern, so of a quantified one, within each iteration. A variable unbounded there is GRF-0032 (T12), one a restrictor inside bounds is not. |
| A subpath variable [GS §6.4, §6.6] | It names the part of the path from where its parenthesized pattern begins to where it ends, of which the path functions are. Within a quantifier it would be a list of subpaths, of which no path function is, so it is SQLCODE -103. One of an alternative of a union is null of a match through another. A path or subpath variable declared twice is -103, as no join is on a path. |
| A selector in a parenthesized path pattern [GS §7.2] | GRF-0033, SQLCODE -103. |
| AS OF SCN [GS §8, §19] | No SCN is named elsewhere in the standard. A system-versioned table keeps the transaction timestamp of each change, and the logical timestamps commits are ordered by are kept with no row (RULES.md D29, D82). So the SCN of a change is its transaction timestamp's microseconds from 0001-01-01 00:00:00 UTC, AS OF SCN n is AS OF TIMESTAMP SCN_TO_TIMESTAMP(n), and TIMESTAMP_TO_SCN(CURRENT_TIMESTAMP) is the current SCN. An SCN below 0 or past 9999-12-31 is SQLSTATE 22008. |
| PREV and FIRST [GS §11.1] | Each is of a prefilter, elsewhere SQLCODE -103. Outside a quantifier PREV and FIRST are null. In a quantifier's first iteration PREV is null and FIRST the binding of that iteration. Without a property each gives the element's identity, as ELEMENT_ID does. |
| FOR EACH SEGMENT [GS §11.1] | It is a postfilter, tested with the WHERE after the pattern. Its two variables are edges of every edge table, and a path of fewer than two edges meets it. |
| An undirected self-loop [GS §4.1] | It is followed once, its source and destination the same vertex. |
| ONE ROW PER STEP of a path of no edge [GS §8 R8.3] | It gives one row, of its vertex, the edge and the next vertex null. |
| FOR SYSTEM_TIME of a GRAPH_TABLE [GS §8] | It reads every element table as of that time. A table not system-versioned is refused as in FROM. |
| A TRUSTED key that repeats [GS §4.1 R4.3] | Each row of the key is a vertex, and an end that names it binds each. An end that names no row is no edge. |
| DETACH of a node delete [GS §12 R12.1] | R12.1 names DETACH, which is GQL's. SQL DML has none, so ON DELETE CASCADE of a CONNECTION constraint is the way a node's edges go with it. Without one, any edge forbids the delete. |
| SYS_GRAPH over a schema's tables [GS §4.2] | "SYS_GRAPH over all node and edge tables in a schema" is read as one graph per schema, named SYS_GRAPH in that schema. |
| Several CONNECTION constraints [GS §4.2] | An edge table of several CONNECTION constraints holds edges that each allows, so SYS_GRAPH joins it by the pairs all of them allow. |
| An edge table of no CONNECTION constraint [GS §12 R12.2] | It joins any node tables of its schema. R12.2 checks CONNECTION constraints, so without one only the endpoint's form and node table are checked, not its row. |
| A native identity's form [GS §5 R5.1] | §5.1's "JSON value" form is taken, with schema and table, so an identity stays one text however it is written. |
| The class of GRF-0052 and GRF-0053 [GS §22] | They are constraint violations, so their SQLSTATE is of class 23. |
| A GQL CALL of a binding table [GQL's call procedure statement] | The procedure runs for each record of the working table, a multiset whose duplicates are kept, and each record is joined with that run's result. A procedure that returns no table keeps the record as it is. Francis et al. §5.1 model CALL as a join, which agrees for queries, where a run per distinct binding gives the same rows. So an inline CALL that writes runs once per row, duplicates each their own. |
| The automatic adjacency index [GS §13.2] | An index is built outside a transaction (RULES.md D34), so an ENFORCED graph's indexes are made as the transaction that created it commits, each in a transaction of its own. |
| Rigid sub-patterns as joins [GS §16 R16.2] | A GRAPH_TABLE whose path patterns are rigid whole, and which reads no table before it, is one join. A rigid part of a pattern with a quantifier, a selector or a union is searched with the rest, so a match never depends on where the search would split. |
| Limits of a joined pattern [GS §16 R16.2, §20] | The limits bound a search. A pattern planned as a join, or matched by the worst-case optimal join, is not searched, so GRAPH.* and 54061 do not apply to it. OPTIONS of a limit keep it searched. |
| Hints [GS §16 R16.8] | GRAPH_START, GRAPH_DIRECTION, USE_ADJACENCY and PATH_ALGO direct a search, so a pattern written with one is searched. NO_WCOJ joins a cyclic pattern pairwise. A hint naming no variable of the pattern, a direction of a vertex, or an edge table with no adjacency index is GRF-0070. |
| Quantifiers unrolled [GS §16 R16.4] | "When cheaper" is read as a bound of three iterations, past which the joins of every count outgrow the search. Zero iterations make the node patterns around the quantifier one vertex, which two named ones cannot be, so such a pattern is searched. |
| Common sub-patterns [GS §16 R16.6] | A path pattern is common to another when it is written alike, anonymous elements aside. Common parts of different path patterns share their variables' operands in a join already. |

### The vector design document

| Text | Reading |
|---|---|
| The default metric [vector R6.3] | "The vector index on v1's column" is any index of the column whose method has a metric, the first in the catalog's order if there are several. |
| `DISTANCE schema.udf_name` [vector §10.2, §8] | It names a metric a program registers as `SCHEMA.NAME`. A distance function of SQL serves VECTOR_DISTANCE alone, as an index's entries must not depend on a body a user may drop (RULES.md D93). |
| The k-limit pushed below a join [vector R9.6] | The rule allows it only where the join cannot remove rows. A nearest read is planned only under a limit over one table, so it is never pushed, which the rule permits. |
| The delta store [vector R14.2, R14.5] | Its purpose is DML at all times with fresh reads (G5, R14.1). Every built-in index is changed as its table is, with tombstones, which meets that with no store to merge, so MERGE DELTA runs the index's maintenance. |
| LOAD and UNLOAD [vector §10.6] | They move an index's structure in or out of memory. HNSW's is always in memory and the others' always paged, so each is taken and changes nothing. |
| The columns of VECTOR_SEARCH [vector §9.3] | The example qualifies the table's columns by the table's correlation name and the distance by the function's. The function is one derived table, so both are qualified by the function's name, or the table's if it has none. |
| VECTOR with no arguments [vector R3.1] | R3.1 makes it DENSE, but its purpose is a flexible column. Its layout is left open as its dimensions and format are, so a sparse value is taken too. |
| Quantizing out of range [vector §7.1, R5.3] | R5.3 makes a CAST out of range an error. VECTOR_QUANTIZE is no CAST: it maps by a scale chosen to fit, and an element past it is clamped to INT8's range. |
