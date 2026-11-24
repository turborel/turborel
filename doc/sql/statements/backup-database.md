# BACKUP DATABASE

[Statements](README.md)

## NAME

BACKUP DATABASE: take a backup of the database while it serves *(extension)*

## SYNOPSIS

```
<backup statement> ::=
    BACKUP DATABASE TO <character string literal>
    [ INCREMENTAL [ FROM <character string literal> ] ]
    [ LABEL <character string literal> ]
```

## DESCRIPTION

BACKUP DATABASE copies the database to the backup target its first
literal names, while the database goes on serving. Readers and writers
are not held up, and checkpoints wait until the copy is done. The
backup holds every transaction committed before it ends, and no other,
as the database would hold them recovered after a crash at that moment.

A target is a directory's path, or `scheme:rest` of a target provider's
scheme. The scheme `dir` names a directory, `memory` a store in memory
kept as long as the database runs, and any other a plug-in's target
([Backups and Recovery](../../operator/10-backups.md#adding-a-target)).
A backup is in the target once its manifest is, written last, so a
backup that fails, is cancelled, or is cut short by a crash leaves no
backup behind.

A full backup copies every file. INCREMENTAL copies only the chunks of
64 KiB that changed since the backup FROM names, by its id, or since the
newest backup in the target. Restored, an incremental backup takes its
other chunks from the backups it follows, back to a full one.

LABEL gives the backup a label, shown with it.

The backup's id is in INFORMATION_SCHEMA.BACKUPS once it is done, with
its kind, its state and where it stops in the log. A backup of an
encrypted database is encrypted as its files are, through the same
encodings and keys, and compressed first if BACKUP.COMPRESSION is ON.

BACKUP DATABASE is run at once, apart from any transaction. It may be
cancelled as any statement is. What it wrote is then removed, and it is
recorded CANCELLED. Two backups may run at once.

## RULES

1. The session has no transaction open.
2. Its user is the database's owner.
3. The database is kept in a directory.
4. A target provider is registered for the location's scheme.
5. Of INCREMENTAL, the target holds the backup it follows.

## PRIVILEGES

The database's owner's alone.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 25000 | -917 | The session has a transaction open |
| 42000 | -104 | Its user is not the database's owner |
| TB910 | -910 | The database is in memory, no provider has the scheme, or the backup it follows is not in the target |
| TBB01 | -916 | The backup it follows is damaged |
| TB915 | -915 | The target cannot be written |
| TB920 | -920 | It was cancelled |

## EXAMPLES

Run by ANN, the database's owner:

```sql
CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE ACCOUNTS (ID INTEGER PRIMARY KEY, BALANCE DECIMAL(10,2));
INSERT INTO ACCOUNTS VALUES (1, 100.00), (2, 250.00);
COMMIT;
BACKUP DATABASE TO 'memory:nightly' LABEL 'first';
UPDATE ACCOUNTS SET BALANCE = 75.00 WHERE ID = 1;
COMMIT;
BACKUP DATABASE TO 'memory:nightly' INCREMENTAL LABEL 'second';
SELECT BACKUP_ID, KIND, PARENT_ID, STATE, LABEL FROM INFORMATION_SCHEMA.BACKUPS ORDER BY BACKUP_ID;
BACKUP DATABASE TO 'memory:nightly';
COMMIT;
BACKUP DATABASE TO 'memory:empty' INCREMENTAL;
```

```
OK
OK
OK
OK
OK
OK
OK
 BACKUP_ID                                                    | KIND        | PARENT_ID                                                    | STATE       | LABEL
--------------------------------------------------------------+-------------+--------------------------------------------------------------+-------------+--------------------------------------------------------------
 B00001795057200000000-000000                                 | FULL        | NULL                                                         | COMPLETED   | first
 B00001795057200000000-000001                                 | INCREMENTAL | B00001795057200000000-000000                                 | COMPLETED   | second
(2 rows)
turborel: SQLCODE -917 (SQLSTATE 25000): BACKUP DATABASE runs outside a transaction: commit or roll back first
OK
turborel: SQLCODE -910 (SQLSTATE TB910): RULES.md D161: memory:empty holds no backup
```

## STANDARD

An extension. Neither SQL-86 nor SQL-92 has a statement that backs up a
database.

## SEE ALSO

[CREATE RESTORE POINT](create-restore-point.md),
[RESCAN BACKUP TARGET](rescan-backup-target.md),
[Backups and Recovery](../../operator/10-backups.md)
