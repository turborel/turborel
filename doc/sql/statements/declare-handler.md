# DECLARE HANDLER

[Statements](README.md)

## NAME

DECLARE HANDLER: say what runs when a condition is raised *(in a compound statement)*

## SYNOPSIS

```
<handler declaration> ::=
    DECLARE { CONTINUE | EXIT | UNDO } HANDLER FOR
    <condition value> [ , <condition value> ]...
    <SQL procedure statement>

<condition value> ::=
      SQLSTATE [ VALUE ] <character string literal>
    | <condition name>
    | SQLEXCEPTION
    | SQLWARNING
    | NOT FOUND
```

DECLARE, CONTINUE, FOR, SQLSTATE, VALUE, NOT and FOUND are key words.
EXIT, UNDO, HANDLER, SQLEXCEPTION and SQLWARNING are read as words, not
key words.

## DESCRIPTION

A handler is declared in a [compound statement](begin.md), after its
variables, conditions and cursors and before its statements. When a
statement within raises a condition the handler is for, the handler's
action runs in place of the condition ending the compound statement.

A condition value says which conditions the handler is for. An SQLSTATE
value is that one value alone. A condition name is the SQLSTATE value
it was declared for, or, declared for none, the condition SIGNAL raises
of that name. SQLEXCEPTION is every exception, the SQLSTATE classes
other than 00, 01 and 02. SQLWARNING is class 01, and NOT FOUND class
02, such as a FETCH past the last row.

The handler chosen is that of the innermost compound statement with
one for the condition. In one compound statement, a handler of the
condition's SQLSTATE value or name is chosen before SQLEXCEPTION,
SQLWARNING or NOT FOUND.

The handler type says where control goes once the action has run.

| Type | After the action |
|---|---|
| CONTINUE | The statement after the one that raised the condition runs |
| EXIT | The handler's compound statement ends |
| UNDO | The handler's compound statement ends, its changes undone before the action ran |

The statement that raised the condition is undone alone, as a
statement that fails is. UNDO undoes all the compound statement has
done.

The action is one statement, often a compound statement. It is in
scope of the handlers of the compound statements outside the handler's
alone. So a condition the action raises, or RESIGNAL in it, goes to an
outer handler, and if none handles it, it ends the statement. In the
action, GET STACKED DIAGNOSTICS reads the condition handled, and GET
CURRENT DIAGNOSTICS what the action's own last statement did.

A completion condition, a warning or no data, that no handler handles
does not end anything. The statement goes on, and completes with it.

No handler handles a condition after which the transaction cannot go
on, such as a serialization failure (class 40) or a cancellation.

## RULES

1. A condition name is of a condition declared in a compound statement
   the handler is in.
2. SQLEXCEPTION, SQLWARNING or NOT FOUND is the one condition value of
   its handler.
3. A handler names no condition twice, nor a condition by its name
   and by the SQLSTATE value it was declared for.
4. No two handlers of one compound statement are for one condition.
5. An UNDO handler is of an ATOMIC compound statement.
6. The action does not name the label of the handler's compound
   statement.

## PRIVILEGES

Those of the action.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | It is not well formed, or follows a statement |
| 2A000 | -103 | A rule above is broken, or an SQLSTATE value is not five digits or upper case letters, or is of class 00 |
| 0Z002 | -301 | GET STACKED DIAGNOSTICS where no handler is active |

## EXAMPLES

CONTINUE goes on after the failed insert, EXIT leaves its block:

```sql
CREATE TABLE LEDGER (K INTEGER, MEMO VARCHAR(40));
BEGIN
  DECLARE CONTINUE HANDLER FOR SQLSTATE '22012'
    INSERT INTO LEDGER VALUES (0, 'divided by zero');
  INSERT INTO LEDGER VALUES (1, 'first');
  INSERT INTO LEDGER VALUES (2 / 0, 'never');
  INSERT INTO LEDGER VALUES (3, 'after');
  B: BEGIN
    DECLARE EXIT HANDLER FOR SQLEXCEPTION
      INSERT INTO LEDGER VALUES (4, 'left B');
    SIGNAL SQLSTATE '75001';
    INSERT INTO LEDGER VALUES (5, 'skipped');
  END B;
END;
SELECT K, MEMO FROM LEDGER ORDER BY K;
```

```
OK
OK
            K | MEMO
--------------+------------------------------------------
            0 | divided by zero
            1 | first
            3 | after
            4 | left B
(4 rows)
```

UNDO undoes its block, and an outer handler reads the condition:

```sql
CREATE TABLE JOURNAL (STATE CHAR(5), MSG VARCHAR(40));
BEGIN
  DECLARE S CHAR(5);
  DECLARE M VARCHAR(40);
  DECLARE CONTINUE HANDLER FOR SQLEXCEPTION
    BEGIN
      GET STACKED DIAGNOSTICS CONDITION 1 S = RETURNED_SQLSTATE, M = MESSAGE_TEXT;
      INSERT INTO JOURNAL VALUES (S, M);
    END;
  W: BEGIN ATOMIC
    DECLARE UNDO HANDLER FOR SQLSTATE '75002'
      INSERT INTO JOURNAL VALUES ('75002', 'undone');
    INSERT INTO JOURNAL VALUES ('00000', 'not kept');
    SIGNAL SQLSTATE '75002';
  END W;
  SIGNAL SQLSTATE '75003' SET MESSAGE_TEXT = 'read by GET STACKED';
END;
SELECT STATE, MSG FROM JOURNAL ORDER BY STATE;
```

```
OK
OK
 STATE | MSG
-------+------------------------------------------
 75002 | undone
 75003 | read by GET STACKED
(2 rows)
```

Rules refused:

```sql
BEGIN DECLARE UNDO HANDLER FOR SQLEXCEPTION BEGIN END; END;
BEGIN
  DECLARE CONTINUE HANDLER FOR SQLSTATE '22012' BEGIN END;
  DECLARE EXIT HANDLER FOR SQLSTATE '22012' BEGIN END;
END;
BEGIN DECLARE CONTINUE HANDLER FOR SQLEXCEPTION, SQLSTATE '22012' BEGIN END; END;
BEGIN DECLARE EXIT HANDLER FOR MISSING BEGIN END; END;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): an UNDO handler is of an ATOMIC compound statement [ISO/IEC 9075-4:2006 draft §14.1 SR17] (at character 6)
turborel: SQLCODE -103 (SQLSTATE 2A000): two handlers of one compound statement handle one condition [ISO/IEC 9075-4:2006 draft §14.2 SR4] (at character 92)
turborel: SQLCODE -103 (SQLSTATE 2A000): SQLEXCEPTION, SQLWARNING or NOT FOUND is the one condition value of its handler [ISO/IEC 9075-4:2006 draft §14.2 SR3] (at character 6)
turborel: SQLCODE -103 (SQLSTATE 2A000): no condition MISSING is declared here [ISO/IEC 9075-4:2006 draft §14.2 SR2] (at character 31)
```

## STANDARD

ISO/IEC 9075-4:2006 draft §14.2, with §14.3 for condition names and
§8.2 for the SQLSTATE value, Feature P002. GET STACKED DIAGNOSTICS is
§17.1, Feature P007. SQL-86 and SQL-92 have no such statement.

## SEE ALSO

[BEGIN](begin.md), [SIGNAL](signal.md), [RESIGNAL](resignal.md),
[GET DIAGNOSTICS](get-diagnostics.md)
