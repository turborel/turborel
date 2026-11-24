# SAVEPOINT

[Statements](README.md)

## NAME

SAVEPOINT: mark a place in a transaction to roll back to

## SYNOPSIS

```
<savepoint statement> ::= SAVEPOINT <savepoint name>
```

## DESCRIPTION

SAVEPOINT establishes a savepoint, a named place in the session's
transaction. [ROLLBACK TO SAVEPOINT](rollback.md) later cancels every
change made since that place, and the transaction goes on.
[RELEASE SAVEPOINT](release-savepoint.md) destroys it, keeping the
changes.

SAVEPOINT with no transaction active begins one.

## RULES

A savepoint is established at the session's current savepoint level. A
session has a level of its own, and a new one is established, current
until it ends, as each of these runs:

1. a function;
2. a procedure defined NEW SAVEPOINT LEVEL (see
   [CREATE PROCEDURE](create-procedure.md));
3. an ATOMIC compound statement (see [BEGIN](begin.md));
4. a trigger's action.

As a level ends, its savepoints are destroyed, and what was changed
since each stays. A procedure of OLD SAVEPOINT LEVEL, the default, runs
at its caller's level, so a savepoint it establishes outlives the CALL.
A statement names only the savepoints of the current level.

If a savepoint of the same name exists at the current level, it is
destroyed first, and the name marks the new place.

A transaction has at most 1,024 savepoints. COMMIT and ROLLBACK destroy
them all.

SAVEPOINT may be a statement of a compound statement or of the body of
a procedure or function. It may not be the action of a trigger, and a
procedure that a trigger's action calls fails if it runs one.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 3B002 | -917 | The transaction has 1,024 savepoints already |
| 42000 | -103 | It is the action of a trigger |
| 09000 | -917 | A procedure a trigger's action calls runs it (SQLSTATE 0W000, prohibited statement encountered during trigger execution) |

## EXAMPLES

Going back to a savepoint cancels the second row and the update, and the
first row stays:

```sql
CREATE TABLE ORDERS (ID INTEGER, STATUS VARCHAR(10));
COMMIT;
INSERT INTO ORDERS VALUES (1, 'new');
SAVEPOINT FIRST_ORDER;
INSERT INTO ORDERS VALUES (2, 'new');
UPDATE ORDERS SET STATUS = 'paid' WHERE ID = 1;
ROLLBACK TO SAVEPOINT FIRST_ORDER;
SELECT ID, STATUS FROM ORDERS;
```

```
OK
OK
OK
OK
OK
OK
OK
           ID | STATUS
--------------+------------
            1 | new
(1 row)
```

A savepoint of a name in use replaces it:

```sql
SAVEPOINT STEP;
INSERT INTO ORDERS VALUES (3, 'new');
SAVEPOINT STEP;
INSERT INTO ORDERS VALUES (4, 'new');
ROLLBACK TO SAVEPOINT STEP;
SELECT ID FROM ORDERS;
COMMIT;
```

```
OK
OK
OK
OK
OK
           ID
--------------
            1
            3
(2 rows)
OK
```

## STANDARD

ISO/IEC 9075-2:2023 §17.5, Feature T271, and the savepoint levels of
§4.43.2, Feature T272. A savepoint of the same name at the current level
is destroyed [§17.5 GR2]. The maximum number of savepoints per
SQL-transaction is implementation-defined (IL208), 1,024 here, and
SQLSTATE 3B002 past it [GR3]. SAVEPOINT initiates a transaction
[§4.41.2.4]. A function, a procedure of NEW SAVEPOINT LEVEL [§9.18 GR7,
GR15], a triggered action [§15.21 GR6c, GR6j] and an ATOMIC compound
statement [ISO/IEC 9075-4:2006 draft §14.1 GR1, GR6] each establish a
savepoint level. A trigger's action that is a savepoint statement is
refused as the trigger is defined (IA046), and one run while a trigger's
action runs raises 0W000 [§15.21 GR6e iii], which fails the action.

## SEE ALSO

[RELEASE SAVEPOINT](release-savepoint.md), [ROLLBACK](rollback.md),
[COMMIT](commit.md), [Transactions](../../user/06-transactions.md)
