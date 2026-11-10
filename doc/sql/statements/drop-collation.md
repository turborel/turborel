# DROP COLLATION

[Statements](README.md)

## NAME

DROP COLLATION: drop a collation

## SYNOPSIS

```
<drop collation statement> ::=
    DROP COLLATION <collation name> { RESTRICT | CASCADE }
```

## DESCRIPTION

DROP COLLATION drops a collation CREATE COLLATION made. Its columns and
domains go back to the default collation, each table's rows copied and its
indexes built again. That can take as long as ALTER TABLE, and it fails if a
constraint no longer holds, as when strings become duplicates. If it fails on
one table after another is done, the transaction can only roll back. A
table whose foreign key references one reverted after it is checked again,
so a key holds by the collation its referenced columns end with. The tables
reverted may be anyone's.

With RESTRICT it is refused while a view or constraint names the collation.
With CASCADE they go too.

## RULES

1. It names a collation a definition made, not a built-in.

## PRIVILEGES

Its schema's owner.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | No such collation, or, with RESTRICT, a view or constraint names it |
| 2A000 | -104 | Its user does not own the collation's schema |
| 23000 | -201 | A constraint no longer holds of a table's rows |

## EXAMPLES

```sql
CREATE COLLATION TIGHT FOR SQL_TEXT FROM CASE_INSENSITIVE NO PAD;
CREATE TABLE T (C VARCHAR(9) COLLATE ANN.TIGHT);
CREATE VIEW V AS SELECT C FROM T WHERE C COLLATE ANN.TIGHT = 'x';
INSERT INTO T VALUES ('Bob');
DROP COLLATION ANN.TIGHT RESTRICT;
DROP COLLATION ANN.TIGHT CASCADE;
SELECT COUNT(*) FROM T WHERE C = 'BOB';
SELECT COUNT(*) FROM T WHERE C = 'Bob';
DROP COLLATION ANN.TIGHT CASCADE;
```

```
OK
OK
OK
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): a view or constraint names collation ANN.TIGHT, so it is dropped only with CASCADE [ISO/IEC 9075-2:2023 §11.44 SR4] (at character 15)
OK

----------------------
                    0
(1 row)

----------------------
                    1
(1 row)
turborel: SQLCODE -103 (SQLSTATE 2A000): no collation ANN.TIGHT that a definition made [ISO/IEC 9075-2:2023 §10.7 SR1] (at character 15)
```

## STANDARD

ISO/IEC 9075-2:2023 §11.44, Feature F690.

## SEE ALSO

[CREATE COLLATION](create-collation.md), [ALTER TABLE](alter-table.md), [REVOKE](revoke.md), [DROP SCHEMA](drop-schema.md)
