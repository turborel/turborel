# Backups and Recovery *(extension)*

[Contents](README.md)

## Backups

A backup is a copy of a database kept in a directory, taken while it
serves. Readers and writers go on as it is taken, and checkpoints wait
until it is done. Restored, it holds every transaction committed before
it ended and no other, as the database would after a crash at that
moment. A backup is taken three ways, all alike:

- by SQL, [BACKUP DATABASE](../sql/statements/backup-database.md), run
  by the database's owner;
- by a program that embeds the database, `Database::backup`
  ([Embedding](../client/03-embedding.md#backups));
- by a client of the server, which runs the statement.

A backup goes to a target, named by its location. A location of no
scheme, or `dir:PATH`, is a directory, made if it is not there. A
location `memory:NAME` is a store in memory, kept while the database
runs, for programs and tests. Any other scheme is a plug-in's
([Adding a target](#adding-a-target)). One target holds many backups,
each by its id, which begins with the time it was taken.

A full backup copies every file of the database, in chunks of 64 KiB,
with the checkpoint it began from and the log's records written while
it ran. INCREMENTAL copies only the chunks that differ from those of the
backup it follows, the newest in the target or the one FROM names.
Restored, it takes the others from the backups before it, back to a
full one. A backup is in its target once its manifest is, written last,
so one that fails, is cancelled or is cut short by a crash leaves
nothing a restore would take for a backup. What it wrote is removed as
it fails, and after a crash its objects stay until removed by hand.

Run by ANN, the database's owner:

```sql
CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE ORDERS (ID INTEGER PRIMARY KEY, ITEM VARCHAR(20));
INSERT INTO ORDERS VALUES (1, 'lamp'), (2, 'desk');
COMMIT;
BACKUP DATABASE TO 'memory:nightly' LABEL 'monday';
INSERT INTO ORDERS VALUES (3, 'chair');
COMMIT;
BACKUP DATABASE TO 'memory:nightly' INCREMENTAL LABEL 'tuesday';
SELECT KIND, PARENT_ID, STATE, LABEL FROM INFORMATION_SCHEMA.BACKUPS ORDER BY BACKUP_ID;
COMMIT;
```

```
OK
OK
OK
OK
OK
OK
OK
 KIND        | PARENT_ID                                                    | STATE       | LABEL
-------------+--------------------------------------------------------------+-------------+--------------------------------------------------------------
 FULL        | NULL                                                         | COMPLETED   | monday
 INCREMENTAL | B00001795057200000000-000000                                 | COMPLETED   | tuesday
(2 rows)
OK
```

Two backups may run at once, to one target or two. A backup is
cancelled as any statement is, by the client or by CANCEL CONNECTION,
and recorded CANCELLED.

A backup of an encrypted database is encrypted as its files are,
through the encodings STORAGE.ENCODINGS names and the same keys, and
checked by the same checksum, so a backup is no easier to read than the
database ([Encryption](07-encryption.md)). With BACKUP.COMPRESSION ON,
its objects are compressed with LZ4 first, which the server registers.

## Verifying a backup

`turboreld --verify-backup TARGET` reads a backup and every backup it
follows, the newest or the one `--backup ID` names, and checks each
chunk against its manifest, each block against its checksum, and that
the log's records run unbroken from where it began to where it stops.
It restores nothing. `--keys` gives the keys of an encrypted backup.

```
$ turboreld --keys /etc/turborel/keys --verify-backup /var/backups/main
the backup is sound: 2 backups, 418 chunks, 1093 records checked
```

A backup that fails is reported with SQLSTATE TBB01, naming the object
that failed, and the program exits with a failure.

## Restoring

A backup is restored into a directory not there yet, with no server
running, by `turboreld --dir PATH --restore TARGET`, or by
`Database::restore`. The files are put together in `PATH.restoring`,
the database recovered there, and the directory renamed to PATH once it
is whole, so a restore cut short leaves no database at PATH, and the
next restore removes what it left.

```
$ turboreld --dir /var/lib/turborel/main --keys /etc/turborel/keys --restore /var/backups/main
restored B00001795057200000000-000001 into /var/lib/turborel/main, 0 records replayed through 18734, as timeline 1
```

The restored database is then served as any other.

## The archive of the log

A backup alone restores the database as it was when the backup ended.
With the log archived, a restore goes on past it, to any later moment
the archive holds. The settings of the archive:

| Setting | Meaning |
|---|---|
| ARCHIVE.MODE | ON, the log is archived; OFF, its default, it is not |
| ARCHIVE.TARGET | the target the log is archived to |
| ARCHIVE.SEGMENT_BYTES | the bytes of records gathered into one segment, 16 MiB by default |
| ARCHIVE.TIMEOUT_MS | how long records wait for a segment to fill before they are archived anyway, a minute by default |
| ARCHIVE.RETENTION_SECONDS | how long a segment is kept, for ever by default |
| ARCHIVE.TIMELINE | the timeline archived to, which a restore sets |

The job `turborel-archive` archives the records as they are made
durable, a segment an object. While ARCHIVE.MODE is ON, a checkpoint
keeps every record the archive lacks, so a target out of reach holds
the log back rather than losing it. Set ARCHIVE.MODE OFF to let it go.
A segment older than ARCHIVE.RETENTION_SECONDS is removed once the
newest backup in the archive's target no longer needs its records.

```sql
SET DATABASE ARCHIVE.TARGET = 'memory:archive';
SET DATABASE ARCHIVE.MODE = 'ON';
SELECT SETTING_NAME, SETTING_VALUE FROM INFORMATION_SCHEMA.SETTINGS WHERE SETTING_NAME LIKE 'ARCHIVE.%' ORDER BY SETTING_NAME;
COMMIT;
```

```
OK
OK
 SETTING_NAME                                                 | SETTING_VALUE
--------------------------------------------------------------+--------------------------------------------------------------
 ARCHIVE.MODE                                                 | ON
 ARCHIVE.RETENTION_SECONDS                                    | 0
 ARCHIVE.SEGMENT_BYTES                                        | 16777216
 ARCHIVE.TARGET                                               | memory:archive
 ARCHIVE.TIMELINE                                             | 1
 ARCHIVE.TIMEOUT_MS                                           | 60000
(6 rows)
OK
```

## Recovery to a point in time

`--archive TARGET` restores the backup and then recovers it through the
archive's records, to their end, or to the point one option names:

| Option | Stops |
|---|---|
| `--until-time 'YYYY-MM-DD HH:MM:SS'` | before the first commit after that time, UTC |
| `--until-lsn N` | after the record at position N |
| `--until-point NAME` | at the restore point NAME, made by [CREATE RESTORE POINT](../sql/statements/create-restore-point.md) |

```sql
CREATE RESTORE POINT before_cleanup;
SELECT POINT_NAME FROM INFORMATION_SCHEMA.RESTORE_POINTS;
```

```
OK
 POINT_NAME
--------------------------------------------------------------
 BEFORE_CLEANUP
(1 row)
```

```
$ turboreld --dir /var/lib/turborel/before --restore /var/backups/main --archive /var/backups/archive --until-point before_cleanup
restored B00001795057200000000-000001 into /var/lib/turborel/before, 212 records replayed through 20118, as timeline 2
```

A point the backup and the archive do not reach is refused with SQLSTATE
TBB02, and nothing is restored.

Each restore through the archive begins a new timeline, numbered past
every timeline the archive holds, and writes its history there. The
restored database archives to its timeline and never over the old one,
which the first database, still running, may go on writing. A later
restore follows the newest timeline, or the one `Database::restore` is
given, through the timelines it branched from.

## The backup history

The database keeps the history of its backups, archived segments,
restore points and restores, durably, beside its files. These views of the Information Schema show
it to the database's owner, and a non-owner sees no rows.

| View | Rows |
|---|---|
| BACKUPS | each backup, its kind, the backup it follows, its timeline, where it starts and stops in the log, its bytes, state, label and times |
| ARCHIVED_SEGMENTS | each segment archived and not removed, its timeline, first and last positions, bytes and time |
| RESTORE_POINTS | each restore point, its position and time |

A backup's STATE is RUNNING while it is taken, then COMPLETED, FAILED or
CANCELLED. One the history shows running that no longer is, cut short by
a crash, is INTERRUPTED. A database restored from a backup has the
history the backup took with it.
[RESCAN BACKUP TARGET](../sql/statements/rescan-backup-target.md) adds
what a target holds that the history lacks, and marks GONE each backup
the target no longer holds.

## Adding a target

A target is a store of named objects, and a target provider opens the
targets of one scheme from their locations. A program that embeds the
database registers a provider with `Database::register_target_provider`
before it takes a backup, and the scheme then names its targets in SQL
and settings alike ([Embedding](../client/03-embedding.md#backups)). A
provider of object storage, a tape library or a remote service is
written so. An extension whose manifest declares a TARGET serves the
scheme it names the same way, from its own process, when no registered
provider has that scheme ([Plug-ins](../client/04-plugins.md)).

The target's methods are few, each of strings, integers and chunks of
bytes, so a plug-in may serve them from another process. An object is
written by an upload begun, its chunks sent, and the upload finished,
when the object exists, durably, or abandoned. An object is read by
ranges, listed by prefix, and removed. An object is never replaced. The
backup writes nothing a target must understand, and every object is
checked as it is read back, so a target that loses or damages one is
found out by verifying.

In a target, a backup's objects are under `backups/<id>/`, and the
archive's under `archive/<timeline>/`, each timeline's history beside
them as `archive/<timeline>.history`. A directory target writes each
object as a file of that name, written first under a name of its own
and renamed once synced.
