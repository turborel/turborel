# CREATE SCHEMA

[Statements](README.md)

## NAME

CREATE SCHEMA: define a schema

## SYNOPSIS

```
<schema definition> ::=
    CREATE SCHEMA <schema name clause>
    [ DEFAULT CHARACTER SET SQL_TEXT ]
    [ <schema element>... ]

<schema name clause> ::=
      <schema name>
    | AUTHORIZATION <authorization identifier>
    | <schema name> AUTHORIZATION <authorization identifier>

<schema name> ::= [ <catalog name> . ] <identifier>

<schema element> ::=
      <table definition>
    | <view definition>
    | <domain definition>
    | <assertion definition>
    | <grant statement>
```

Each element is the statement of its own page: [CREATE TABLE](create-table.md),
[CREATE VIEW](create-view.md), [CREATE DOMAIN](create-domain.md),
[CREATE ASSERTION](create-assertion.md) and [GRANT](grant.md).

## DESCRIPTION

CREATE SCHEMA defines a schema, and the tables, views, domains,
assertions and privileges written in it. Without a name, the schema is
named as its authorization identifier. Its owner is its AUTHORIZATION,
or, written without one, the session's authorization identifier; a
session creates only its own schemas.

A name written without a schema in an element is of this schema. The
elements are made together: a view may read a table written after it, a
CHECK a view of the schema, and the domains are made before the tables
whose columns are of them. A schema definition is made whole, or not at
all: if an element breaks a rule, none is made.

Run by a session, CREATE SCHEMA is made at once, apart from the session's
transaction: it commits nothing of that transaction, and ROLLBACK does
not undo it. It is refused in a READ ONLY transaction, or when SET
TRANSACTION has made the next one READ ONLY.

## RULES

1. No schema of the name exists, and the name is not MODULE, which names
   a session's declared temporary tables. A key word, MODULE is a name
   only delimited, `"MODULE"`.
2. A catalog named is the one catalog.
3. Run by a session, its AUTHORIZATION, if written, is the session's
   authorization identifier.
4. Each element names its objects in this schema, if it names their
   schema.
5. Each element follows its own statement's rules.
6. The transaction, active or next, is not READ ONLY.

A GRANT of a privilege the owner does not hold with grant option grants
nothing; the schema is made, with the warning SQLSTATE 01007.

## PRIVILEGES

None, beyond rule 3; each element's own.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | A schema of the name exists, or the name is MODULE; an element names an object in another schema; an element breaks a rule of its own |
| 3D000 | -103 | It names a catalog other than the one there is |
| 2A000 | -104 | Run by a session, its AUTHORIZATION is another's |
| 25000 | -917 | The transaction, active or next, is READ ONLY |
| 0A000 | -900 | It names a character set other than SQL_TEXT |
| 01007 | 0 | Not an error: a privilege granted was not held with grant option |

## EXAMPLES

```sql
CREATE SCHEMA SHOP
    CREATE DOMAIN MONEY AS DECIMAL(9,2) CHECK (VALUE >= 0)
    CREATE TABLE ITEMS (ID INTEGER PRIMARY KEY, PRICE MONEY)
    CREATE VIEW CHEAP AS SELECT ID FROM ITEMS WHERE PRICE < 10
    GRANT SELECT ON CHEAP TO PUBLIC;
ROLLBACK;
SELECT SCHEMA_NAME, SCHEMA_OWNER FROM INFORMATION_SCHEMA.SCHEMATA WHERE SCHEMA_NAME = 'SHOP';
```

The ROLLBACK does not undo it:

```
OK
OK
 SCHEMA_NAME                                                  | SCHEMA_OWNER
--------------------------------------------------------------+--------------------------------------------------------------
 SHOP                                                         | ANN
(1 row)
```

Schemas refused, run by ANN:

```sql
CREATE SCHEMA SHOP;
CREATE SCHEMA OTHER AUTHORIZATION BOB;
CREATE SCHEMA Y CREATE TABLE Z.T (K INTEGER);
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): a schema SHOP already exists [X3.135-1986 §6.1 SR1; ISO/IEC 9075:1992 §11.1 SR3, §21.2.1, §21.3.2]
turborel: SQLCODE -104 (SQLSTATE 2A000): ANN may create its own schema only, not BOB's [ISO/IEC 9075:1992 §11.1 AR1]
turborel: SQLCODE -103 (SQLSTATE 2A000): table Z.T is not in schema Y [X3.135-1986 §6.2 SR1] (at character 29)
```

## STANDARD

ISO/IEC 9075:1992 §11.1, and X3.135-1986 §6.1, whose `<schema>` is
CREATE SCHEMA AUTHORIZATION with tables, views and privileges. Which
privileges a schema definition needs is implementation-defined [§11.1
AR1]: here, a session's is its own. SQL-92 makes a `<schema definition>`
run directly part of the transaction [§20.1 GR6b iii], so ROLLBACK would
undo it; this implementation makes it apart from the transaction.

## SEE ALSO

[DROP SCHEMA](drop-schema.md), [SET SCHEMA](set-schema.md),
[CREATE TABLE](create-table.md), [Concepts](../../user/03-concepts.md)
