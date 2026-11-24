# DROP TRIGGER

[Statements](README.md)

## NAME

DROP TRIGGER: drop a trigger

## SYNOPSIS

```
<drop trigger statement> ::= DROP TRIGGER <trigger name>
```

No `<drop behavior>` is written: nothing depends on a trigger.

## DESCRIPTION

DROP TRIGGER drops a trigger; no row changes. Dropping an INSTEAD OF
trigger takes back the privileges it gave the schema's owner on its
view, and those granted of them, as REVOKE with CASCADE does.

A trigger is dropped too with its subject table or view, and, with
CASCADE, with a table, view, column or constraint its action depends
on, or a privilege its owner needs for it. Without CASCADE such a drop
or revoke is refused.

DROP TRIGGER runs within the session's transaction: ROLLBACK undoes it.

## RULES

1. The trigger exists, and its user owns the trigger's schema.
2. The transaction is not READ ONLY.

## PRIVILEGES

The trigger's schema is its user's.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 42000 | -102 | Anything is written after the name |
| 42000 | -103 | There is no such trigger |
| 42000 | -104 | Its user does not own the trigger's schema |
| 25000 | -917 | The transaction is READ ONLY |

## EXAMPLES

```sql
CREATE TABLE ITEMS (ID INTEGER);
CREATE TABLE LOG (ID INTEGER);
CREATE TRIGGER LOGGED AFTER INSERT ON ITEMS REFERENCING NEW AS N
  FOR EACH ROW INSERT INTO LOG VALUES (N.ID);
DROP TABLE LOG RESTRICT;
DROP TRIGGER LOGGED;
DROP TRIGGER LOGGED;
DROP TABLE LOG RESTRICT;
```

```
OK
OK
OK
turborel: SQLCODE -103 (SQLSTATE 42000): the triggered action of trigger ANN.LOGGED references ANN.LOG, so it is dropped only with CASCADE [ISO/IEC 9075-2:2023 §11.31 SR7g] (at character 11)
OK
turborel: SQLCODE -103 (SQLSTATE 42000): there is no trigger ANN.LOGGED [ISO/IEC 9075-2:2023 §11.50 SR1] (at character 13)
OK
```

## STANDARD

ISO/IEC 9075-2:2023 §11.50, Feature T211. SQL-86 and SQL-92 have no
triggers.

## SEE ALSO

[CREATE TRIGGER](create-trigger.md)
