# DROP SEQUENCE

[Statements](README.md)

## NAME

DROP SEQUENCE: drop a sequence generator

## SYNOPSIS

```
<drop sequence generator statement> ::=
    DROP SEQUENCE <sequence generator name> { RESTRICT | CASCADE }
```

## DESCRIPTION

DROP SEQUENCE drops a generator, and every USAGE granted on it. No view,
constraint or default can name a generator, but a trigger's action and a
procedure's body can draw from one with NEXT VALUE FOR. With RESTRICT, the
generator is not dropped while one of them does. With CASCADE, each trigger
and routine that draws from it is dropped with it, and what depends on
those goes too, as DROP ROUTINE ... CASCADE takes it. DROP SCHEMA with
CASCADE drops the schema's generators too, as CASCADE does.

A generator made again with the same name starts afresh.

DROP SEQUENCE runs within the session's transaction: ROLLBACK undoes it.

## RULES

1. The generator exists.
2. With RESTRICT, no trigger's action and no routine's body draws from
   it.

## PRIVILEGES

The owner of the generator's schema.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 42000 | -103 | No such generator; or, with RESTRICT, a trigger or routine draws from it |
| 42000 | -104 | Its user does not own the generator's schema |

## EXAMPLES

```sql
CREATE SEQUENCE S;
DROP SEQUENCE S RESTRICT;
SELECT NEXT VALUE FOR S FROM (VALUES (0)) AS X (Z);
DROP SEQUENCE S CASCADE;
```

```
OK
OK
turborel: SQLCODE -103 (SQLSTATE 42000): there is no sequence generator ANN.S [ISO/IEC 9075-2:2023 §6.14 SR3] (at character 7)
turborel: SQLCODE -103 (SQLSTATE 42000): there is no sequence generator ANN.S [ISO/IEC 9075-2:2023 §11.74 SR2] (at character 14)
```

Of a generator a trigger draws from:

```sql
CREATE TABLE ORDERS (ID INTEGER, AMOUNT INTEGER);
CREATE TABLE AUDIT (N INTEGER, ID INTEGER);
CREATE SEQUENCE S;
CREATE TRIGGER LOGGED AFTER INSERT ON ORDERS REFERENCING NEW ROW AS R FOR EACH ROW WHEN (R.AMOUNT > 100) INSERT INTO AUDIT VALUES (NEXT VALUE FOR S, R.ID);
CREATE TRIGGER CHANGED AFTER UPDATE OF AMOUNT ON ORDERS FOR EACH STATEMENT DELETE FROM AUDIT;
COMMIT;
DROP SEQUENCE S RESTRICT;
ROLLBACK;
DROP SEQUENCE S CASCADE;
SELECT TRIGGER_NAME FROM INFORMATION_SCHEMA.TRIGGERS;
COMMIT;
```

```
OK
OK
OK
OK
OK
OK
turborel: SQLCODE -103 (SQLSTATE 42000): the triggered action of trigger ANN.LOGGED draws from sequence generator ANN.S, so it is dropped only with CASCADE [ISO/IEC 9075-2:2023 §11.74 SR3] (at character 14)
OK
OK
 TRIGGER_NAME
--------------------------------------------------------------
 CHANGED
(1 row)
OK
```

## STANDARD

ISO/IEC 9075-2:2023 §11.74, Feature T176. See
[CONFORMANCE.md](../../../CONFORMANCE.md#sequence-generators).

## SEE ALSO

[CREATE SEQUENCE](create-sequence.md), [ALTER SEQUENCE](alter-sequence.md),
[DROP SCHEMA](drop-schema.md), [CREATE TRIGGER](create-trigger.md),
[CREATE PROCEDURE](create-procedure.md)
