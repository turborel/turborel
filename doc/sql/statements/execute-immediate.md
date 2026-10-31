# EXECUTE IMMEDIATE

[Statements](README.md)

## NAME

EXECUTE IMMEDIATE: prepare a statement and run it at once

## SYNOPSIS

```
<execute immediate statement> ::= EXECUTE IMMEDIATE <SQL statement variable>

<SQL statement variable> ::= <simple value specification>
```

## DESCRIPTION

EXECUTE IMMEDIATE prepares the statement its operand holds, as text, as
[PREPARE](prepare.md) does, runs it, as [EXECUTE](execute.md) does, and
forgets it. The statement is one a program may prepare, but no query,
and has no dynamic parameter: nothing gives it values, nor takes its
rows.

The operand is a character string: a parameter's value, in a program,
or a literal, so EXECUTE IMMEDIATE runs in direct SQL, and the client,
as well. It is a statement of its own: the diagnostics area names
EXECUTE IMMEDIATE, and the statement prepared as its DYNAMIC_FUNCTION;
it begins a transaction if the statement prepared does.

## RULES

1. The operand is a character string.
2. Its statement is one a program may prepare, of no comment, no query,
   with no dynamic parameter, and follows the rules of its kind.

## PRIVILEGES

Those the statement prepared needs.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | The operand is not a character string (rule 1) |
| 37000 | -102, -103, -104 | A rule of its statement is broken, or a privilege it needs is not held (rule 2) |

The statement prepared raises its own errors as it runs.

## EXAMPLES

Of a table STAFF:

```sql
CREATE TABLE STAFF (ID INTEGER PRIMARY KEY, NAME VARCHAR(30), SALARY DECIMAL(9,2));
INSERT INTO STAFF VALUES (1, 'Ada', 7200), (2, 'Grace', 8100), (3, NULL, 6900);
EXECUTE IMMEDIATE 'UPDATE STAFF SET SALARY = SALARY + 100 WHERE ID = 3';
SELECT ID, SALARY FROM STAFF ORDER BY ID;
COMMIT;
```

```
OK
OK
OK
           ID |      SALARY
--------------+-------------
            1 |     7200.00
            2 |     8100.00
            3 |     7000.00
(3 rows)
OK
```

Refused:

```sql
EXECUTE IMMEDIATE 'SELECT * FROM STAFF';
EXECUTE IMMEDIATE 'DELETE FROM STAFF WHERE ID = ?';
EXECUTE IMMEDIATE 'DELETE FROM STAF';
EXECUTE IMMEDIATE 42;
```

```
turborel: SQLCODE -103 (SQLSTATE 37000): a statement executed immediately is no query, and has no <dynamic parameter specification> [ISO/IEC 9075:1992 §17.11 GR3a, GR3c]
turborel: SQLCODE -103 (SQLSTATE 37000): a statement executed immediately is no query, and has no <dynamic parameter specification> [ISO/IEC 9075:1992 §17.11 GR3a, GR3c]
turborel: SQLCODE -103 (SQLSTATE 37000): table ANN.STAF is not defined [X3.135-1986 §5.4 SR5]
turborel: SQLCODE -103 (SQLSTATE 2A000): an <SQL statement variable> is a character string [ISO/IEC 9075:1992 §17.11 SR1]
```

## STANDARD

ISO/IEC 9075:1992 §17.11. SQL-92 makes it no `<directly executable
statement>` [§20.1]: direct SQL takes it as a statement of this
implementation's. SQL-86 has no dynamic SQL.

## SEE ALSO

[PREPARE](prepare.md), [EXECUTE](execute.md)
