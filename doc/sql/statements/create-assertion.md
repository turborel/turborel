# CREATE ASSERTION

[Statements](README.md)

## NAME

CREATE ASSERTION: define an assertion

## SYNOPSIS

```
<assertion definition> ::=
    CREATE ASSERTION <constraint name> CHECK ( <search condition> )
    [ <constraint attributes> ]
```

The `<constraint attributes>` are those of [CREATE TABLE](create-table.md).

## DESCRIPTION

CREATE ASSERTION defines an assertion: a constraint of its schema, on no
one table, which holds while its condition is not false. It is checked
as it is made, and so made only if it holds; then at the end of each
statement that changes a table it reads, or, if it is deferred, at
commit.

CREATE ASSERTION runs within the session's transaction: ROLLBACK undoes
it.

## RULES

1. The assertion's schema exists, and its user owns it. An assertion
   named without a schema is of the session's default schema.
2. No constraint of the schema has the name.
3. Its condition holds as it is made.
4. Its condition contains no USER, CURRENT_USER, SESSION_USER or
   SYSTEM_USER, no CURRENT_DATE, CURRENT_TIME or CURRENT_TIMESTAMP, is
   not possibly non-deterministic, and reads no temporary table.
5. The transaction is not READ ONLY.

## PRIVILEGES

REFERENCES on each column the condition names of a table it reads, or,
if it names none, on one column of each table it reads.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 3F000 | -103 | The schema does not exist |
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the schema, or lacks the REFERENCES it needs |
| 23000 | -201 | The condition is false as it is made |
| 25000 | -917 | The transaction is READ ONLY |

Once made, a statement that leaves the assertion false fails with
SQLSTATE 23000, SQLCODE -201; a COMMIT at which a deferred one is false
fails with SQLSTATE 40002, SQLCODE -913, the transaction rolled back.

## EXAMPLES

Of the table LEDGER of [CREATE DOMAIN](create-domain.md):

```sql
CREATE ASSERTION BALANCED CHECK (NOT EXISTS (SELECT * FROM LEDGER WHERE AMOUNT > 1000));
INSERT INTO LEDGER VALUES (3, 5000);
CREATE ASSERTION LATER CHECK (NOT EXISTS (SELECT * FROM LEDGER WHERE ENTRY < 0)) INITIALLY DEFERRED;
INSERT INTO LEDGER VALUES (-1, 1);
COMMIT;
```

```
OK
turborel: SQLCODE -201 (SQLSTATE 23000): assertion ANN.BALANCED is false [ISO/IEC 9075:1992 §11.34 GR2]
OK
OK
turborel: SQLCODE -913 (SQLSTATE 40002): the transaction is rolled back: assertion ANN.LATER is false [ISO/IEC 9075:1992 §11.34 GR2] [ISO/IEC 9075:1992 §14.3 GR5a]
```

## STANDARD

ISO/IEC 9075:1992 §11.34. After a statement that changes a table it
reads, an assertion whose every conjunct has one of these forms is
checked from the rows the statement changed, the others whole.
`NOT EXISTS (SELECT ... FROM T WHERE p)` is checked from the rows the
statement inserted or updated; so are `(SELECT MAX(x) FROM T WHERE p)`
less than a constant, or not greater, and MIN greater, or not less
("bounded").
`(SELECT COUNT(*) FROM T WHERE p)` compared with a constant, and COUNT(x),
is checked from how many rows the statement brought into the count or
took out of it, whole only if that moves the count toward breaking the
comparison, or the transaction reads at a snapshot of each statement.
COUNT(DISTINCT x) alike: a row keeping its value changes nothing, and a
row bringing a value in, or taking one out, may move the count.
MAX greater than a constant, or not less, and MIN less, or not greater,
holds if a row the statement inserted or updated reaches the constant,
or if it changed none of the rows; else it is checked whole. MAX equal
to a constant is both MAX not greater and MAX not less, MIN alike. MAX
unequal to a constant holds if a row the statement inserted or updated
is greater, or if it only brought in rows less than it; MIN alike; else
it is checked whole. Of each form, `p` and `x` are of columns,
constants, arithmetic, comparisons, IS NULL, LIKE and CASE alone, of the
one table's row, through the filters and columns of views of it. Where
`x` may be null there (it is not a column NOT NULL or of the PRIMARY
KEY, not DEFERRABLE, nor one `p` is not true of when null), the warning
of a null eliminated depends on every row: the assertion is checked from
the change only if a row of it has `x` null, else whole. SUM and AVG
are checked whole. A deferred assertion of one `NOT EXISTS`, or of
conjuncts each `NOT EXISTS` or MAX or MIN bounded whose conditions raise
no error and whose `x` is never null, is checked from each row the
transaction wrote while it was deferred; any other, whole, once, however
many statements of the transaction changed a table it reads. Whichever way, a statement fails
exactly when the condition is false after it, with the same error and
warning. SQL-86 has no assertions.

## SEE ALSO

[DROP ASSERTION](drop-assertion.md), [SET CONSTRAINTS](set-constraints.md),
[COMMIT](commit.md), [Constraints](../../user/07-constraints.md)
