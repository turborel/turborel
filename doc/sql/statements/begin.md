# BEGIN

[Statements](README.md)

## NAME

BEGIN: group statements with their own variables, conditions, cursors and handlers

## SYNOPSIS

```
<compound statement> ::=
    [ <beginning label> : ] BEGIN [ [ NOT ] ATOMIC ]
    [ DECLARE <XML lexically scoped options> ; ]
    [ <local declaration> ; ]...
    [ <local cursor declaration> ; ]...
    [ <handler declaration> ; ]...
    [ <SQL procedure statement> ; ]...
    END [ <ending label> ]

<local declaration> ::=
      <SQL variable declaration>
    | <condition declaration>

<SQL variable declaration> ::=
    DECLARE <variable name> [ , <variable name> ]... <data type> [ DEFAULT <default option> ]

<condition declaration> ::=
    DECLARE <condition name> CONDITION [ FOR SQLSTATE [ VALUE ] <character string literal> ]

<local cursor declaration> ::=
    DECLARE <cursor name> [ SENSITIVE | INSENSITIVE | ASENSITIVE ] [ SCROLL ]
    CURSOR FOR <cursor specification>

<default option> ::=
    <literal> | NULL | ARRAY[] | <datetime value function>
    | USER | CURRENT_USER | SESSION_USER | SYSTEM_USER | CURRENT_ROLE
```

BEGIN, END, DECLARE, DEFAULT, CURSOR, INSENSITIVE and SCROLL are key
words. ATOMIC, CONDITION, SENSITIVE and ASENSITIVE are read as words,
not key words. A label is an identifier.

## DESCRIPTION

A compound statement runs its statements in turn. It may be the body of
a procedure or a function, a trigger's action, or a statement of its
own, run directly. A compound statement may hold another.

Its declarations come first, its variables and conditions, then its
cursors, then its [handlers](declare-handler.md). Each lasts as long
as the compound statement runs, and is known only within it. A variable
starts as its default, or null without one. A variable of a domain with
no DEFAULT of its own starts as the domain's default. The default is a
`<default option>`, as a column's is, not an expression over the
parameters or other variables.

DECLARE's XML options, XMLNAMESPACES and XMLBINARY, first of all, are
in scope of all it holds, a compound statement within it too, whose
own are more inner, and a statement's own WITH more inner still: see
[XML Values](../16-xml.md#xml-options-of-a-scope).

A variable is named in any value, condition or query within, and
assigned by [SET](set-variable.md), SELECT INTO, FETCH INTO and GET
DIAGNOSTICS. A variable of an inner compound statement hides one of
its name outside it. `L.V` names the variable V of the compound
statement labelled L, hidden or not. In a query, a column of a table
the query reads hides a variable of its name.

A condition gives a name to an SQLSTATE value, which a handler or
[SIGNAL](signal.md) may use. One declared with no SQLSTATE is told
apart by its name alone.

A local cursor is opened, fetched from and closed by
[OPEN](open.md), [FETCH](fetch.md) and [CLOSE](close.md) within the
compound statement, and a positioned UPDATE or DELETE may name it. Its
query may read the variables. A cursor that is not SCROLL fetches NEXT
alone. The
cursors are destroyed as the compound statement ends, so a compound
statement in a loop declares and opens its cursor anew each time.

A compound statement is NOT ATOMIC unless ATOMIC is written. Each
statement within it that changes data is atomic on its own, so a
statement that fails is undone alone. Of a compound statement NOT
ATOMIC, the changes of the statements before an exception no handler
handles stay. Of one ATOMIC, they are all undone. A trigger's compound
statement is ATOMIC.

A compound statement NOT ATOMIC run directly may hold COMMIT and
ROLLBACK, which end the transaction it runs in. The statements after
run in a new transaction, begun as the first of them needs one. COMMIT
closes every cursor open, the compound statement's own among them, so
one must be opened again before it is fetched from. A COMMIT last leaves
no transaction active. A statement sees the tables and other objects
that the statements before it defined.

A handler's action may hold COMMIT too. It runs while the statement
that raised the condition runs, so if that is an ATOMIC compound
statement within the handler's own, COMMIT or ROLLBACK raises invalid
transaction termination, and the ATOMIC statement is undone.

A compound statement run directly is read as the body of a procedure
of no parameters. Its statements run with the privileges of the
session's user. In the body of a routine they run with those of the
owner of the routine's schema. A function whose body is a compound
statement is evaluated where the query that invokes it evaluates it.

The client ends a statement at a semicolon, but not at one between BEGIN
and its END, nor at one within an IF, CASE, LOOP, REPEAT, WHILE or FOR
statement run directly.

A statement that runs without end, such as a WHILE whose condition stays
true, is cancelled as any statement is, with SQLCODE -920.

## RULES

1. An ending label is the compound statement's beginning label.
2. A beginning label is not the label of a statement it is in.
3. The declarations come before the statements, variables and
   conditions first, then cursors, then handlers.
4. No two variables, no two conditions and no two cursors of one
   compound statement have one name.
5. A default is a value of the variable's type. ARRAY[] is the default
   of an array type alone.
6. An ATOMIC compound statement holds no COMMIT or ROLLBACK, nor does
   a FOR within it, nor the body of a routine.
7. A compound statement is not preparable.

## PRIVILEGES

Those of each statement within, as above.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | It is not well formed, or a declaration follows a statement |
| 2A000 | -103 | A rule above is broken |
| 2D000 | -917 | COMMIT or ROLLBACK runs while an ATOMIC compound statement does |
| 24000 | -501 | A cursor COMMIT or ROLLBACK closed is fetched from |
| TB920 | -920 | It is cancelled |

An exception a statement within raises, which no handler handles, ends
the compound statement with that exception.

## EXAMPLES

A loop that sums into a variable:

```sql
CREATE TABLE TALLY (N INTEGER);
BEGIN
  DECLARE I INTEGER DEFAULT 1;
  DECLARE TOTAL INTEGER DEFAULT 0;
  WHILE I <= 4 DO
    SET TOTAL = TOTAL + I;
    SET I = I + 1;
  END WHILE;
  INSERT INTO TALLY VALUES (TOTAL);
END;
SELECT N FROM TALLY;
```

```
OK
OK
            N
--------------
           10
(1 row)
```

NOT ATOMIC keeps the first insert, ATOMIC undoes it:

```sql
CREATE TABLE STEPS (K INTEGER);
BEGIN
  INSERT INTO STEPS VALUES (1);
  SIGNAL SQLSTATE '75001' SET MESSAGE_TEXT = 'stopped';
END;
BEGIN ATOMIC
  INSERT INTO STEPS VALUES (2);
  SIGNAL SQLSTATE '75001' SET MESSAGE_TEXT = 'stopped';
END;
SELECT K FROM STEPS;
```

```
OK
turborel: SQLCODE -401 (SQLSTATE 75001): stopped (at character 40)
turborel: SQLCODE -401 (SQLSTATE 75001): stopped (at character 47)
            K
--------------
            1
(1 row)
```

COMMIT keeps what came before it, and ROLLBACK undoes only what came
after:

```sql
DELETE FROM STEPS;
COMMIT;
BEGIN
  INSERT INTO STEPS VALUES (5);
  COMMIT;
  INSERT INTO STEPS VALUES (6);
  ROLLBACK;
  INSERT INTO STEPS VALUES (7);
  COMMIT;
END;
SELECT K FROM STEPS ORDER BY K;
```

```
OK
OK
OK
            K
--------------
            5
            7
(2 rows)
```

Rules refused:

```sql
B1: BEGIN END B2;
BEGIN INSERT INTO STEPS VALUES (3); DECLARE Y INTEGER; END;
BEGIN DECLARE Y INTEGER; DECLARE Y INTEGER; END;
BEGIN ATOMIC INSERT INTO STEPS VALUES (4); COMMIT; END;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): the <ending label> B2 is not B1 [ISO/IEC 9075-4:2006 draft §14.1 SR3] (at character 14)
turborel: SQLCODE -102 (SQLSTATE 2A000): a compound statement declares its variables and conditions, then its cursors, then its handlers, before its statements, found DECLARE [ISO/IEC 9075-4:2006 draft §14.1] (at character 36)
turborel: SQLCODE -103 (SQLSTATE 2A000): the compound statement declares the variable Y twice [ISO/IEC 9075-4:2006 draft §14.1 SR10] (at character 33)
turborel: SQLCODE -103 (SQLSTATE 2A000): an ATOMIC compound statement or a FOR holds no COMMIT or ROLLBACK [ISO/IEC 9075-4:2006 draft §14.1 SR6, §14.13 SR9] (at character 43)
```

## STANDARD

ISO/IEC 9075-4:2006 draft §14.1, with §14.3 for conditions, §14.4 for
variables and §12.1 for local cursors, Feature P002. `L.V` is Feature
P005. DECLARE's XML options are ISO/IEC 9075-14:2006 draft §15.1,
Features X084 and X134. Atomicity is that of §4.10.4, and a compound statement is not
preparable by §4.10.6. COMMIT and ROLLBACK are those of ISO/IEC
9075-2:2023 §17.7 and §17.8, invalid in an atomic execution context by
their GR2. SQL-86 and SQL-92 have no such statement.

## SEE ALSO

[DECLARE HANDLER](declare-handler.md), [SET](set-variable.md),
[IF](if.md), [CASE](case-statement.md), [LOOP](loop.md),
[WHILE](while.md), [REPEAT](repeat.md), [FOR](for.md),
[LEAVE](leave.md), [ITERATE](iterate.md), [SIGNAL](signal.md),
[CREATE PROCEDURE](create-procedure.md),
[CREATE FUNCTION](create-function.md),
[CREATE TRIGGER](create-trigger.md), [DECLARE CURSOR](declare-cursor.md)
