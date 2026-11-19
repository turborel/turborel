# CREATE RESTORE POINT

[Statements](README.md)

## NAME

CREATE RESTORE POINT: name a point of the log a restore may stop at *(extension)*

## SYNOPSIS

```
<restore point statement> ::=
    CREATE RESTORE POINT <identifier>
```

## DESCRIPTION

CREATE RESTORE POINT writes a record of the log naming the point, with
the time by the database's clock, and makes it durable. A restore from a
backup through the archive of the log may then stop just after it,
`--until-point NAME`, holding every transaction committed before it and
none after ([Backups and Recovery](../../operator/10-backups.md#recovery-to-a-point-in-time)).
Recovery after a crash passes over it.

The name is an identifier, folded to upper case unless delimited. Each
restore point the database made is in INFORMATION_SCHEMA.RESTORE_POINTS,
with its position in the log and the time it was made, and no two have
one name.

CREATE RESTORE POINT is made at once, apart from any transaction.

## RULES

1. The session has no transaction open.
2. Its user is the database's owner.
3. The database has a log.
4. No restore point of the database has the name.

## PRIVILEGES

The database's owner's alone.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 25000 | -917 | The session has a transaction open |
| 2A000 | -104 | Its user is not the database's owner |
| TB910 | -910 | The database has no log, or a restore point has the name |
| TB915 | -915 | The record cannot be written or synced |

## EXAMPLES

Run by ANN, the database's owner:

```sql
CREATE RESTORE POINT before_migration;
SELECT POINT_NAME, CREATED FROM INFORMATION_SCHEMA.RESTORE_POINTS;
COMMIT;
CREATE RESTORE POINT BEFORE_MIGRATION;
```

```
OK
 POINT_NAME                                                   | CREATED
--------------------------------------------------------------+----------------------------------
 BEFORE_MIGRATION                                             | 2026-11-19 03:00:00.000000+00:00
(1 row)
OK
turborel: SQLCODE -910 (SQLSTATE TB910): RULES.md D164: a restore point BEFORE_MIGRATION was made already (at character 21)
```

## STANDARD

An extension. Neither SQL-86 nor SQL-92 has a statement that names a
point to restore a database to.

## SEE ALSO

[BACKUP DATABASE](backup-database.md),
[Backups and Recovery](../../operator/10-backups.md)
