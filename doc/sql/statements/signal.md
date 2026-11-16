# SIGNAL

[Statements](README.md)

## NAME

SIGNAL: raise a condition

## SYNOPSIS

```
<signal statement> ::=
    SIGNAL { SQLSTATE [ VALUE ] <character string literal> | <condition name> }
    [ SET <signal item> [ , <signal item> ]... ]

<signal item> ::= <condition information item name> = <simple value specification>

<condition information item name> ::=
      MESSAGE_TEXT | CLASS_ORIGIN | SUBCLASS_ORIGIN
    | CONSTRAINT_CATALOG | CONSTRAINT_SCHEMA | CONSTRAINT_NAME
    | CATALOG_NAME | SCHEMA_NAME | TABLE_NAME | COLUMN_NAME | CURSOR_NAME
```

SQLSTATE, VALUE and SET are key words. SIGNAL and the item names are
read as words, not key words.

## DESCRIPTION

SIGNAL raises a condition of an SQLSTATE value, or of a condition
declared in a [compound statement](begin.md) it is in. The items set
are those of the condition, as GET DIAGNOSTICS reads them. MESSAGE_TEXT
is its message. An item not set is empty.

A [handler](declare-handler.md) in scope for the condition handles it.
Else, of an SQLSTATE value that is an exception, it ends the statement
with that exception. A condition declared with no SQLSTATE value is
handled by its name alone, and ends the statement, unhandled, with
SQLSTATE 45000.

Of class 01, a warning, or 02, no data, SIGNAL raises a completion
condition. If no handler handles it, the statements after SIGNAL run,
and the statement completes with that condition.

SIGNAL stands in a routine's body, a trigger's action or a compound
statement, or alone as a statement of its own. It may be prepared, and
raises its condition each time it is executed. A function whose body
signals raises its condition from the query that invokes it.

## RULES

1. An SQLSTATE value is five digits or simple upper case letters, not
   of class 00.
2. A condition name is of a condition declared in a compound statement
   SIGNAL is in.
3. Each item is set at most once, to a character string.
4. SIGNAL takes no label.

## PRIVILEGES

Those of the item values.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -102 | It is not well formed, as an item value that is no simple value |
| 2A000 | -103 | A rule above is broken |
| *the value* | -401 | Not handled, the exception SIGNAL raises |
| 45000 | -401 | Not handled, a condition declared with no SQLSTATE value |
| *01xxx* | 0 | Not an error. An unhandled warning, after which the statement goes on |
| *02xxx* | 100 | Not an error. An unhandled no data condition, after which the statement goes on |

## EXAMPLES

A procedure refuses a withdrawal larger than the balance:

```sql
CREATE TABLE ACCOUNTS (ID INTEGER, BALANCE INTEGER);
INSERT INTO ACCOUNTS VALUES (1, 50);
CREATE PROCEDURE WITHDRAW (IN WHO INTEGER, IN AMOUNT INTEGER) MODIFIES SQL DATA
BEGIN
  IF (SELECT BALANCE FROM ACCOUNTS WHERE ID = WHO) < AMOUNT THEN
    SIGNAL SQLSTATE '75001' SET MESSAGE_TEXT = 'insufficient funds';
  END IF;
  UPDATE ACCOUNTS SET BALANCE = BALANCE - AMOUNT WHERE ID = WHO;
END;
CALL WITHDRAW(1, 20);
CALL WITHDRAW(1, 100);
SELECT ID, BALANCE FROM ACCOUNTS;
```

```
OK
OK
OK
OK
turborel: SQLCODE -401 (SQLSTATE 75001): insufficient funds (at character 155)
           ID |      BALANCE
--------------+--------------
            1 |           30
(1 row)
```

Conditions by name, a warning, and SIGNAL alone:

```sql
BEGIN DECLARE OVERDRAWN CONDITION; SIGNAL OVERDRAWN; END;
BEGIN DECLARE OVERDRAWN CONDITION FOR SQLSTATE '75002'; SIGNAL OVERDRAWN SET MESSAGE_TEXT = 'over'; END;
BEGIN SIGNAL SQLSTATE '01H01'; INSERT INTO ACCOUNTS VALUES (2, 0); END;
SIGNAL SQLSTATE '75003';
SELECT ID FROM ACCOUNTS ORDER BY ID;
```

```
turborel: SQLCODE -401 (SQLSTATE 45000): condition OVERDRAWN signalled [ISO/IEC 9075-4:2006 draft §17.2] (at character 35)
turborel: SQLCODE -401 (SQLSTATE 75002): over (at character 56)
OK, with a warning: SQLSTATE 01H01
turborel: SQLCODE -401 (SQLSTATE 75003): SQLSTATE '75003' signalled [ISO/IEC 9075-4:2006 draft §17.2] (at character 0)
           ID
--------------
            1
            2
(2 rows)
```

Rules refused:

```sql
SIGNAL SQLSTATE '00000';
SIGNAL SQLSTATE '7500';
SIGNAL OVERDRAWN;
SIGNAL SQLSTATE '75001' SET MESSAGE_TEXT = 'a', MESSAGE_TEXT = 'b';
SIGNAL SQLSTATE '75001' SET MESSAGE_TEXT = 5;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): '00000' is no SQLSTATE value: five digits or upper case letters, not of the class 00 of successful completion [ISO/IEC 9075-4:2006 draft §8.2 SR4, SR5] (at character 16)
turborel: SQLCODE -103 (SQLSTATE 2A000): '7500' is no SQLSTATE value: five digits or upper case letters, not of the class 00 of successful completion [ISO/IEC 9075-4:2006 draft §8.2 SR4, SR5] (at character 16)
turborel: SQLCODE -103 (SQLSTATE 2A000): no compound statement this is in declares the condition OVERDRAWN [ISO/IEC 9075-4:2006 draft §17.2 SR1a ii] (at character 7)
turborel: SQLCODE -103 (SQLSTATE 2A000): MESSAGE_TEXT is set twice [ISO/IEC 9075-4:2006 draft §17.2 SR2] (at character 48)
turborel: SQLCODE -103 (SQLSTATE 2A000): MESSAGE_TEXT is set to a value of NUMERIC(1,0): it takes a character string [ISO/IEC 9075-4:2006 draft §17.2 SR3] (at character 0)
```

## STANDARD

ISO/IEC 9075-4:2006 draft §17.2, with §8.2 for the SQLSTATE value and
§17.3 GR6 for how the condition is handled, Feature P002. SQL-86 and
SQL-92 have no such statement.

## SEE ALSO

[RESIGNAL](resignal.md), [DECLARE HANDLER](declare-handler.md),
[BEGIN](begin.md), [GET DIAGNOSTICS](get-diagnostics.md),
[PREPARE](prepare.md)
