# RESCAN BACKUP TARGET

[Statements](README.md)

## NAME

RESCAN BACKUP TARGET: find again the backups and archived segments a target holds *(extension)*

## SYNOPSIS

```
<rescan statement> ::=
    RESCAN BACKUP TARGET <character string literal>
```

## DESCRIPTION

The database keeps a history of its backups, archived segments, restore
points and restores, durably, which INFORMATION_SCHEMA.BACKUPS,
ARCHIVED_SEGMENTS and RESTORE_POINTS show. A target's manifests and
segments are the truth of what it holds. RESCAN BACKUP TARGET reads
them, and records in the history each backup and segment the target
holds that it lacks, as of a database restored from a backup taken
before them, or of another database. Each backup the history shows as
completed and the target no longer holds, removed by hand, it marks
GONE, and each such segment removed.

The literal is the target's location, as BACKUP DATABASE names it.

RESCAN BACKUP TARGET is made at once, apart from any transaction.

## RULES

1. The session has no transaction open.
2. Its user is the database's owner.
3. A target provider is registered for the location's scheme.

## PRIVILEGES

The database's owner's alone.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 25000 | -917 | The session has a transaction open |
| 2A000 | -104 | Its user is not the database's owner |
| TB910 | -910 | No provider has the scheme |
| TBB01 | -916 | A manifest is damaged |
| TB915 | -915 | The target cannot be read |

## EXAMPLES

Run by ANN, the database's owner, of a database that backed up to
`memory:nightly` twice:

```sql
RESCAN BACKUP TARGET 'memory:nightly';
SELECT KIND, STATE FROM INFORMATION_SCHEMA.BACKUPS ORDER BY BACKUP_ID;
```

```
OK
 KIND        | STATE
-------------+-------------
 FULL        | COMPLETED
 INCREMENTAL | COMPLETED
(2 rows)
```

## STANDARD

An extension.

## SEE ALSO

[BACKUP DATABASE](backup-database.md),
[Backups and Recovery](../../operator/10-backups.md)
