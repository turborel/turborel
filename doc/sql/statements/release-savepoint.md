# RELEASE SAVEPOINT

[Statements](README.md)

## NAME

RELEASE SAVEPOINT: destroy a savepoint, keeping the changes since

## SYNOPSIS

```
<release savepoint statement> ::= RELEASE SAVEPOINT <savepoint name>
```

## DESCRIPTION

RELEASE SAVEPOINT destroys the savepoint of that name at the session's
current savepoint level, and every savepoint established at that level
after it. Nothing is undone: the changes made since stay in the
transaction, and commit or roll back with it.

## RULES

The savepoint must have been established, by [SAVEPOINT](savepoint.md),
at the current savepoint level and in the current transaction. With no
transaction active there is none.

RELEASE SAVEPOINT may be a statement of a compound statement or of the
body of a procedure or function, but not of a trigger's action.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 3B001 | -917 | No savepoint of that name is established at the current savepoint level |
| 2A000 | -103 | It is the action of a trigger |
| 09000 | -917 | A procedure a trigger's action calls runs it (SQLSTATE 0W000) |

## EXAMPLES

Releasing A destroys B too, and both rows stay:

```sql
CREATE TABLE STEPS (N INTEGER);
COMMIT;
SAVEPOINT A;
INSERT INTO STEPS VALUES (1);
SAVEPOINT B;
INSERT INTO STEPS VALUES (2);
RELEASE SAVEPOINT A;
ROLLBACK TO SAVEPOINT B;
SELECT N FROM STEPS;
```

```
OK
OK
OK
OK
OK
OK
OK
turborel: SQLCODE -917 (SQLSTATE 3B001): no savepoint B is established at the current savepoint level [ISO/IEC 9075-2:2023 §17.8 GR3b]
            N
--------------
            1
            2
(2 rows)
```

## STANDARD

ISO/IEC 9075-2:2023 §17.6, Feature T271. A name that identifies no
savepoint of the current level raises 3B001 [GR2], and the savepoint and
those established after it at its level are destroyed [GR3].

## SEE ALSO

[SAVEPOINT](savepoint.md), [ROLLBACK](rollback.md),
[Transactions](../../user/06-transactions.md)
