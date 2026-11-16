# RESIGNAL

[Statements](README.md)

## NAME

RESIGNAL: raise again the condition a handler handles *(in a handler)*

## SYNOPSIS

```
<resignal statement> ::=
    RESIGNAL [ SQLSTATE [ VALUE ] <character string literal> | <condition name> ]
    [ SET <signal item> [ , <signal item> ]... ]

<signal item> ::= <condition information item name> = <simple value specification>
```

SQLSTATE, VALUE and SET are key words. RESIGNAL and the item names are
read as words, not key words. The item names are those of
[SIGNAL](signal.md).

## DESCRIPTION

RESIGNAL, in the action of a [handler](declare-handler.md), raises
again the condition the handler handles. The items it sets replace the
condition's, so `RESIGNAL SET MESSAGE_TEXT = 'text'` passes the
condition on with a new message. With an SQLSTATE value or a condition
name, RESIGNAL raises that condition instead, as SIGNAL would, ahead
of the one handled. The diagnostics area holds one condition, so it
leaves the one handled out, and its MORE reads Y.

The condition goes to the handlers of the compound statements outside
the handler's, as any condition raised in a handler's action does. If
none handles it, it ends the statement.

Where no handler is executing, RESIGNAL raises SQLSTATE 0K000.

RESIGNAL may stand where SIGNAL may, alone too, and may be prepared.
Only in a handler's action does it raise its condition.

## RULES

1. An SQLSTATE value is five digits or simple upper case letters, not
   of class 00.
2. A condition name is of a condition declared in a compound statement
   RESIGNAL is in.
3. Each item is set at most once, to a character string.
4. RESIGNAL takes no label.

## PRIVILEGES

Those of the item values.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | It is not well formed |
| 2A000 | -103 | A rule above is broken |
| 0K000 | -301 | No handler is executing |
| *the condition's* | *its own* | Not handled outside, the condition raised again |

## EXAMPLES

An inner handler gives the condition a new message, and an outer one
records it:

```sql
CREATE TABLE ERRORS_SEEN (STATE CHAR(5), MSG VARCHAR(40));
CREATE PROCEDURE DIVIDE_NOTED () MODIFIES SQL DATA
BEGIN
  DECLARE M VARCHAR(40);
  DECLARE S CHAR(5);
  DECLARE EXIT HANDLER FOR SQLEXCEPTION
    BEGIN
      GET STACKED DIAGNOSTICS CONDITION 1 M = MESSAGE_TEXT, S = RETURNED_SQLSTATE;
      INSERT INTO ERRORS_SEEN VALUES (S, M);
    END;
  BEGIN
    DECLARE Z INTEGER DEFAULT 0;
    DECLARE EXIT HANDLER FOR SQLSTATE '22012'
      RESIGNAL SET MESSAGE_TEXT = 'division failed';
    SET Z = 1 / Z;
  END;
END;
CALL DIVIDE_NOTED();
SELECT STATE, MSG FROM ERRORS_SEEN;
```

```
OK
OK
OK
 STATE | MSG
-------+------------------------------------------
 22012 | division failed
(1 row)
```

A new SQLSTATE that no handler handles, and RESIGNAL where no handler
runs:

```sql
BEGIN
  DECLARE Z INTEGER DEFAULT 0;
  DECLARE EXIT HANDLER FOR SQLSTATE '22012'
    RESIGNAL SQLSTATE '75009' SET MESSAGE_TEXT = 'bad input';
  SET Z = 1 / Z;
END;
RESIGNAL;
BEGIN RESIGNAL SET MESSAGE_TEXT = 'nothing'; END;
```

```
turborel: SQLCODE -401 (SQLSTATE 75009): bad input (at character 85)
turborel: SQLCODE -301 (SQLSTATE 0K000): RESIGNAL where no handler is executing [ISO/IEC 9075-4:2006 draft §17.3 GR1] (at character 0)
turborel: SQLCODE -301 (SQLSTATE 0K000): RESIGNAL where no handler is executing [ISO/IEC 9075-4:2006 draft §17.3 GR1] (at character 6)
```

## STANDARD

ISO/IEC 9075-4:2006 draft §17.3, Feature P002. GET STACKED DIAGNOSTICS,
which reads the condition in the handler, is §17.1, Feature P007.
SQL-86 and SQL-92 have no such statement.

## SEE ALSO

[SIGNAL](signal.md), [DECLARE HANDLER](declare-handler.md),
[GET DIAGNOSTICS](get-diagnostics.md), [BEGIN](begin.md)
