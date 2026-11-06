# CREATE COLLATION

[Statements](README.md)

## NAME

CREATE COLLATION: define a collation

## SYNOPSIS

```
<collation definition> ::=
    CREATE COLLATION <collation name> FOR SQL_TEXT
    FROM <existing collation name> [ <pad characteristic> ]

<pad characteristic> ::= NO PAD | PAD SPACE

<collate clause> ::= COLLATE <collation name>
```

## DESCRIPTION

A collation says how character strings compare. A column takes one by its
`<collate clause>`, a domain likewise, and an expression, ORDER BY or GROUP
BY names one for itself. There are four built in.

| Collation | Order | Pad |
|---|---|---|
| SQL_TEXT, UCS_BASIC | Unicode scalar value, so `Z` before `a` | PAD SPACE |
| UNICODE | Language-neutral, by base letter, then accent, then case | NO PAD |
| CASE_INSENSITIVE | Scalar value of each character's case folding | PAD SPACE |

CREATE COLLATION gives one of them another name and pad characteristic. Under
PAD SPACE a shorter string compares as if extended with spaces. Under NO PAD
it compares as a prefix, so `'a'` is less than `'a '`.

A literal, parameter or CAST takes the collation of the column it is compared
with. Two columns of different collations need COLLATE. A foreign key's
columns have the collation of those they reference.

UNIQUE and indexes on a column hold it by its collation, so `Bob` and `bob`
are duplicates under CASE_INSENSITIVE. An index of a collation serves
comparisons by that collation. Its keys are longer than the text, about 16
bytes for each character under UNICODE, which limits an indexed column's
length ([Appendix D](../../appendix/d-limits.md)).

## RULES

1. No collation of its name exists in its schema.
2. The collation it comes from exists. A name of no schema is a built-in.
3. It is for SQL_TEXT, the one character set.

## PRIVILEGES

Its schema's owner, holding USAGE on the collation it comes from. Built-ins
are everyone's. COLLATE needs USAGE, which GRANT gives.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | One of its name exists, or no collation of the name it comes from |
| 2A000 | -104 | Its user does not own the schema, or lacks USAGE |
| 0A000 | -900 | It is for another character set, or comes from SQL-92's EXTERNAL, DESC, DEFAULT or TRANSLATION |

## EXAMPLES

```sql
CREATE TABLE PEOPLE (NAME VARCHAR(20) COLLATE CASE_INSENSITIVE UNIQUE);
INSERT INTO PEOPLE VALUES ('Bob'), ('Ann');
INSERT INTO PEOPLE VALUES ('bob');
SELECT NAME FROM PEOPLE WHERE NAME = 'BOB';
CREATE COLLATION WITH_PAD FOR SQL_TEXT FROM UNICODE PAD SPACE;
CREATE TABLE WORDS (W VARCHAR(20) COLLATE ANN.WITH_PAD);
INSERT INTO WORDS VALUES ('résumé  '), ('Zebra'), ('apple');
SELECT W FROM WORDS ORDER BY W;
SELECT COUNT(*) FROM WORDS WHERE W = 'apple   ';
CREATE COLLATION WITH_PAD FOR SQL_TEXT FROM UNICODE;
CREATE COLLATION OTHER FOR SQL_TEXT FROM NOPE;
```

```
OK
OK
turborel: SQLCODE -201 (SQLSTATE 23000): UNIQUE ANN.PEOPLE_UNIQUE1 (NAME) of ANN.PEOPLE would be violated: a row of these values is there [X3.135-1986 §6.4 GR2]
 NAME
----------------------
 Bob
(1 row)
OK
OK
OK
 W
----------------------
 apple
 résumé
 Zebra
(3 rows)

----------------------
                    1
(1 row)
turborel: SQLCODE -103 (SQLSTATE 2A000): collation ANN.WITH_PAD exists [ISO/IEC 9075-2:2023 §11.43 SR3] (at character 17)
turborel: SQLCODE -103 (SQLSTATE 2A000): no collation INFORMATION_SCHEMA.NOPE [ISO/IEC 9075-2:2023 §10.7 SR1] (at character 41)
```

## STANDARD

ISO/IEC 9075-2:2023 §4.3.6, §10.7, §11.43, Features F690 and F692. See
[CONFORMANCE.md](../../../CONFORMANCE.md#collations).

## SEE ALSO

[DROP COLLATION](drop-collation.md), [SET COLLATION](set-collation.md),
[CREATE TABLE](create-table.md), [GRANT](grant.md)
