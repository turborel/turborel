# CREATE TRIGGER

[Statements](README.md)

## NAME

CREATE TRIGGER: define a trigger

## SYNOPSIS

```
<trigger definition> ::=
    CREATE TRIGGER <trigger name> <trigger action time> <trigger event>
    ON <table name> [ REFERENCING <transition table or variable> ... ]
    <triggered action>

<trigger action time> ::= BEFORE | AFTER | INSTEAD OF

<trigger event> ::= INSERT | DELETE | UPDATE [ OF <column name> [ , <column name> ]... ]

<transition table or variable> ::=
      OLD [ ROW ] [ AS ] <old transition variable name>
    | NEW [ ROW ] [ AS ] <new transition variable name>
    | OLD TABLE [ AS ] <old transition table name>
    | NEW TABLE [ AS ] <new transition table name>

<triggered action> ::=
    [ FOR EACH { ROW | STATEMENT } ]
    [ WHEN ( <search condition> ) ]
    <triggered SQL statement>

<triggered SQL statement> ::=
      <SQL procedure statement>
    | BEGIN ATOMIC { <SQL procedure statement> ; }... END
```

TRIGGER, BEFORE, AFTER, INSTEAD, REFERENCING, OLD, NEW, ROW, EACH,
STATEMENT and ATOMIC are read as words, not key words, so a table or
column may still be named by one.

## DESCRIPTION

CREATE TRIGGER defines a trigger: an action its schema runs each time a
statement inserts into, deletes from or updates its subject table.

A trigger's action time says when the action runs. A BEFORE trigger
runs once the statement has chosen its rows and before it writes any of
them, and a row trigger's action can change the new row by assigning a
column of its new transition variable with SELECT INTO. An AFTER
trigger runs at the end of the statement, after its referential actions
and once its immediate constraints hold. An INSTEAD OF trigger is on a
view and runs in place of the change. Nothing is written through the
view, which need not be updatable. A change of an updatable view over
it reaches it as a change of its own rows, and so does an UPDATE or
DELETE through a cursor over it. Its action may change the view in
turn, through the view's own triggers.

FOR EACH ROW runs the action once for each row changed, in the order
the statement changed them, and its old and new transition variables
are that row as it was and as it is to be. FOR EACH STATEMENT, the
default, runs it once for each statement, even one that changes no row,
and its old and new transition tables are all the rows changed. A WHEN
condition decides, for each run, whether the action runs. A trigger of
UPDATE OF its columns runs only for an UPDATE that sets one of them.

Triggers of the same table, time and event run in the order they were
made. An action's statements run as statements of their own: each runs
the referential actions, checks the immediate constraints and runs the
triggers its own changes call for. Every change a statement causes is
done under the statement's own instant, CURRENT_TIMESTAMP among them,
and undone together if any part of it fails.

An action runs as the owner of the trigger's schema: CURRENT_USER in it
is that owner, and it uses the owner's privileges, whoever ran the
statement. An INSTEAD OF trigger gives the schema's owner the privilege
of its event on the view, with grant option, so the owner may grant it
to others.

An action sees the tables and views it names, and its subject table, as
they were when the trigger was made: a column added later is not among
the columns of its `SELECT *` or of an INSERT without a column list.

CREATE TRIGGER runs within the session's transaction: ROLLBACK undoes
it.

## RULES

1. The trigger's schema exists, its user owns it, and the schema has no
   trigger of the name. A trigger named without a schema is of the
   session's default schema. In CREATE SCHEMA, the trigger is of the
   schema made.
2. A BEFORE or AFTER trigger is on a persistent base table. An INSTEAD
   OF trigger is on a view with no CHECK OPTION that no view WITH CHECK
   OPTION reads, and that has no other INSTEAD OF trigger of the same
   event.
3. An INSTEAD OF trigger has no WHEN condition and no column list.
4. The columns of UPDATE OF are columns of the table, each listed once.
5. A REFERENCING name is given once each, and no two are the same. A
   trigger of INSERT has no old transition variable or table, and one of
   DELETE no new one. A transition variable needs FOR EACH ROW. A BEFORE
   trigger has no transition tables.
6. The action holds only INSERT, a searched UPDATE or DELETE, MERGE,
   SELECT INTO, GET DIAGNOSTICS and CALL: no schema, transaction, connection,
   session, cursor or dynamic statement, and no parameter.
7. A BEFORE trigger's action changes no table, calls no procedure that
   possibly changes one, and references no generated column of its new
   transition variable.
8. A target of SELECT INTO, GET DIAGNOSTICS or an output parameter of
   CALL is a column of the new transition variable, and only of a BEFORE
   row trigger. One of SELECT INTO or CALL may be an element of the
   column's array, `N.C[i]`, at an integer literal position.
9. The transaction is not READ ONLY.

## PRIVILEGES

The schema's owner holds TRIGGER on the subject table, and, if a
transition variable or table is named, SELECT on it. The action needs
the privileges its statements need, held by the schema's owner.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 3F000 | -103 | The schema does not exist |
| 2A000 | -102 | The definition is not well formed |
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the schema, or the owner lacks a privilege it needs |
| 25000 | -917 | The transaction is READ ONLY |

Once made, a statement whose trigger's action fails fails with SQLSTATE
09000, the SQLCODE of the action's failure, and a message naming the
trigger and that failure. Nothing the statement did is kept.
GET DIAGNOSTICS shows the trigger as TRIGGER_SCHEMA and TRIGGER_NAME,
and its table as SCHEMA_NAME and TABLE_NAME. A trigger that runs itself
again stops at 16 levels of triggers and procedures within each other:
the statement fails with SQLSTATE 09000, SQLCODE -900.

## EXAMPLES

A row trigger that keeps a history of balances, and a BEFORE trigger
that gives a balance not written:

```sql
CREATE TABLE ACCOUNTS (ID INTEGER NOT NULL PRIMARY KEY, BALANCE DECIMAL(9,2));
CREATE TABLE HISTORY (ID INTEGER, OLD_BALANCE DECIMAL(9,2), NEW_BALANCE DECIMAL(9,2));
CREATE TRIGGER AUDITED AFTER UPDATE OF BALANCE ON ACCOUNTS
  REFERENCING OLD AS O NEW AS N
  FOR EACH ROW WHEN (O.BALANCE <> N.BALANCE)
  INSERT INTO HISTORY VALUES (O.ID, O.BALANCE, N.BALANCE);
CREATE TRIGGER OPENED BEFORE INSERT ON ACCOUNTS
  REFERENCING NEW AS N FOR EACH ROW
  SELECT COALESCE(N.BALANCE, 0) INTO N.BALANCE FROM (VALUES (1)) AS ONE (X);
INSERT INTO ACCOUNTS (ID) VALUES (1);
INSERT INTO ACCOUNTS VALUES (2, 50);
UPDATE ACCOUNTS SET BALANCE = BALANCE + 10 WHERE ID = 1;
UPDATE ACCOUNTS SET BALANCE = BALANCE WHERE ID = 2;
SELECT * FROM ACCOUNTS ORDER BY ID;
SELECT * FROM HISTORY;
```

```
OK
OK
OK
OK
OK
OK
OK
OK
           ID |     BALANCE
--------------+-------------
            1 |       10.00
            2 |       50.00
(2 rows)
           ID | OLD_BALANCE | NEW_BALANCE
--------------+-------------+-------------
            1 |        0.00 |       10.00
(1 row)
```

A statement trigger of several statements, reading its old transition
table:

```sql
CREATE TRIGGER CLOSED AFTER DELETE ON ACCOUNTS
  REFERENCING OLD TABLE AS GONE
  BEGIN ATOMIC
    INSERT INTO HISTORY SELECT ID, BALANCE, CAST(NULL AS DECIMAL(9,2)) FROM GONE;
    DELETE FROM HISTORY WHERE ID IN (SELECT ID FROM GONE) AND NEW_BALANCE IS NOT NULL;
  END;
DELETE FROM ACCOUNTS WHERE ID = 1;
SELECT * FROM HISTORY;
```

```
OK
OK
           ID | OLD_BALANCE | NEW_BALANCE
--------------+-------------+-------------
            1 |       10.00 |        NULL
(1 row)
```

A BEFORE trigger that assigns one element of an array column. The new
row of a null array fails:

```sql
CREATE TABLE POSTS (ID INTEGER, TAGS VARCHAR(10) ARRAY[3]);
CREATE TRIGGER TAGGED BEFORE INSERT ON POSTS
  REFERENCING NEW AS N FOR EACH ROW
  SELECT 'new' INTO N.TAGS[1] FROM (VALUES (1)) AS ONE (X);
INSERT INTO POSTS VALUES (1, ARRAY['draft', 'today']);
INSERT INTO POSTS VALUES (2, NULL);
SELECT * FROM POSTS;
```

```
OK
OK
OK
turborel: SQLCODE -301 (SQLSTATE 09000): the triggered action of trigger ANN.TAGGED failed: SQLSTATE 2200E: an element of a null array is assigned [ISO/IEC 9075-2:2023 §14.5 GR5b i 1 A, §14.7 GR4b ii 1 A, §10.4 GR10b ii 1 A] [ISO/IEC 9075-2:2023 §15.21 GR6m]
           ID | TAGS
--------------+---------------------------------------------
            1 | ARRAY['new', 'today']
(1 row)
```

An INSTEAD OF trigger makes a grouped view take a DELETE:

```sql
CREATE VIEW TOTAL (BALANCE) AS SELECT SUM(BALANCE) FROM ACCOUNTS;
DELETE FROM TOTAL;
CREATE TRIGGER EMPTIED INSTEAD OF DELETE ON TOTAL DELETE FROM ACCOUNTS;
DELETE FROM TOTAL;
SELECT COUNT(*) FROM ACCOUNTS;
```

```
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): ANN.TOTAL is a read-only table [X3.135-1986 §8.5 SR2; X3.135-1986 §4.4] (at character 12)
OK
OK

----------------------
                    0
(1 row)
```

Rules refused, and an action that fails:

```sql
CREATE TRIGGER EARLY BEFORE DELETE ON ACCOUNTS DELETE FROM HISTORY;
CREATE TRIGGER LIMITED AFTER INSERT ON ACCOUNTS REFERENCING NEW AS N
  FOR EACH ROW INSERT INTO ACCOUNTS VALUES (N.ID, 0);
INSERT INTO ACCOUNTS VALUES (3, 30);
SELECT COUNT(*) FROM ACCOUNTS;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): the <triggered action> of a BEFORE trigger shall not contain an SQL-statement that possibly modifies SQL-data, which turborel refuses (implementation-defined, IA047) [ISO/IEC 9075-2:2023 §11.49 SR14a] (at character 47)
OK
turborel: SQLCODE -201 (SQLSTATE 09000): the triggered action of trigger ANN.LIMITED failed: SQLSTATE 23000: PRIMARY KEY ANN.ACCOUNTS_PRIMARY1 (ID) of ANN.ACCOUNTS would be violated: a row of these values is there [X3.135-1986 §6.4 GR2] [ISO/IEC 9075-2:2023 §15.21 GR6m]

----------------------
                    0
(1 row)
```

## STANDARD

ISO/IEC 9075-2:2023 §11.49, §4.46, §15.19 to §15.21, Features T200,
T211 to T218. An INSTEAD OF trigger takes INSERT, UPDATE, DELETE and
MERGE of its view, an UPDATE or DELETE WHERE CURRENT OF a cursor over
it, and a change of an updatable view over it. A target `N.C[i]` is
the `<target array element specification>` of §6.4, Feature S097.
SQL-86 and SQL-92 have no triggers.

## SEE ALSO

[DROP TRIGGER](drop-trigger.md), [CALL](call.md), [GRANT](grant.md),
[GET DIAGNOSTICS](get-diagnostics.md), [CREATE VIEW](create-view.md),
[Constraints](../../user/07-constraints.md)
