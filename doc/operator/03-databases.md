# Databases

[Contents](README.md)

## A database's directory

A database kept in a directory holds there:

| File | What it holds |
|---|---|
| `catalog.0`, `catalog.1` | the catalog: the definitions, settings and users |
| `data` | the tables and indexes, in pages |
| `data.shadow0`, `data.shadow1` | what was written to `data` since the last checkpoint |
| `log/` | the write-ahead log, in segments |
| `undo/` | older versions of rows, for readers of earlier snapshots |
| `tmp/` | the work areas of large sorts, joins and groupings |
| `plugins/` | the files of plug-in engines and index methods, each in a directory of its name, made as one first keeps a file |
| `lock` | held while a server, or a program, has the database open |

`undo/` and `tmp/` are made anew each time the database opens. The
files are opened only by a release that knows their format versions
([Versions](../appendix/f-versions.md)).

On Unix, the directories and files a database makes are the user's who
runs it alone: directories 0700, files 0600, less what the umask takes
away. A directory that already exists keeps its mode. On Windows they
take the permissions of the directory they are made in.

The log begins a new segment as the current one reaches
LOG.SEGMENT_SIZE. If one cannot be begun, as when the disk is full,
the current segment grows past that size, commits going on, until one
can be; the failure is told once as a `failed` event
([Diagnostics](08-diagnostics.md)).

A database made with `--memory` has no directory: it is gone when the
server stops. Its pages, its temporary tables' rows and what its queries
spill are all kept in memory, of its MEMORY_BUDGET, never in the
system's temporary directory: a statement that needs a page past the
budget is refused with SQLCODE -918, and what was written before it
stays.

The data file's pages are read through a buffer pool of
STORAGE.POOL_BYTES, and what a query spills through one of WORK_MEMORY.
A page wanted while every page of its pool is held at once, as by a
pool set too small for the work, is refused with SQLCODE -918 too, at
once, never waited for.

## Opening and closing

As it opens, a database is **created**, if its directory holds no
catalog; **reopened**, if it was closed cleanly; or **recovered**, if it
stopped without closing. The diagnostics tell which:

```
{"time":"2026-10-28T19:41:24.918Z","level":"info","event":"opened","how":"reopened"}
```

Closing, as the server stops, takes a checkpoint and marks the catalog
closed cleanly. Then, before it lets go of `lock`, it removes the spill
files left in `tmp/` and closes every file of the database; a spill file
it cannot remove, or a file still in use, is told as a `failed` event.

## Recovery after a crash

A database that stopped without closing is recovered as it opens: from
the last checkpoint, the log is read, the changes of committed
transactions redone and those of transactions never committed undone,
and the indexes mended. No committed transaction is lost; none not
committed survives.

```
{"time":"2026-10-28T19:41:11.802Z","level":"warning","event":"opened","how":"recovered","records":6,"definitions":3,"committed":1,"not_committed":0,"redone":2,"undone":0,"cut":0}
```

A record the crash interrupted, written in part at the end of the log,
was never synced, so no transaction it belonged to committed: it is cut
off as the database opens, and `cut` tells how many bytes were. Valid
records after invalid ones are another matter: the log is damaged, as by
a failing disk, and cutting it there would lose committed transactions.
The database is then not opened, and nothing of its log is cut: opening
fails with SQLCODE -916, naming the log's file and the offset of the
damage, and the database is to be restored from a copy of its
directory. Invalid bytes that show a write the crash interrupted, part
of them never written, are cut with what follows them, as the system
may write a file's last pages in any order before the database syncs
it.

If a sync of the log fails as the database runs, it may have lost what
the log held, and a sync tried again may succeed without it; so the log
syncs nothing more, the commit fails with SQLCODE -915, and the database
takes no change until it is opened again, which recovers it from what
its log holds. The failure is told as a `failed` event.

## Checkpoints

A checkpoint writes the changes since the last to the data file, so a
recovery reads the log only from there. One is due once
CHECKPOINT.AFTER_CHANGES row changes have been logged, 100,000 by
default, or CHECKPOINT.AFTER_BYTES bytes written to the data file,
64 MiB; the database's checkpoint thread takes it, so no transaction
waits for it. Each is told to the diagnostics, `checkpoint_taken`. One
that fails leaves the changes since the last counted, so the next is due
as soon as a transaction ends.

The log is kept from the last checkpoint, or from the first change of
the oldest transaction still open, whichever is earlier. While
checkpoints are refused (a table damaged, a failed sync of the log),
not taken, or taken while one transaction stays open, the log only
grows. Once it keeps four times CHECKPOINT.AFTER_CHANGES, or the side
file four times CHECKPOINT.AFTER_BYTES, a `checkpoint_overdue` warning
says so and why, at most once a minute
([Diagnostics](08-diagnostics.md)):

```
{"time":"2026-10-30T09:12:44.310Z","level":"warning","event":"checkpoint_overdue","changes":400000,"cause":"transaction","seconds":5412}
```

## Open files

A database holds at most FILES.MAX_OPEN of its files open at once, 1024
by default, 0 for no limit, so a server stays within what the system
allows a process. Past it, the file used least recently is closed, and
opened again as it is next used; while every open file is being read or
written, the next waits for one. A file with writes not yet on disk is
closed only after those without, and its writes are synced first: if
that sync fails, the file is kept open, the failure is told as a
`failed` event, and the file's next sync fails with it, so the
statement, commit or checkpoint that needs the file on disk fails
rather than succeed over writes lost. If more files are opened again in a
minute than the limit, it is too low for the work, and a
`files_reopened` warning says so, at most once a minute
([Diagnostics](08-diagnostics.md)). The database's owner sees each file in
INFORMATION_SCHEMA.FILES, its kind, whether it is open, and its reads,
writes and syncs. The server's own files are there too, of KIND SERVER,
though not under the limit: the `--log` file and the socket while they
are open, the TLS certificate chain and key, read as the server starts,
and the `--keys` directory, a read for each key taken from it:

```sql
SELECT KIND, COUNT(*) FROM INFORMATION_SCHEMA.FILES GROUP BY KIND;
```

## Background work

The database does some of its work in the background: its checkpoints,
and its maintenance, which gathers tables' statistics (a table's, after
the database opens, once a query first plans with it, which plans
without them meanwhile) and freezes the rows of the column store.
JOBS.POLICY says how: AUTOMATIC, the
default, on threads of the database's own as the work comes due;
PAUSED, not at all until it changes, the work due kept; MANUAL, only
when the program embedding the database asks. It takes effect at once:

```sql
SET DATABASE JOBS.POLICY = 'PAUSED'
```

Each thread started or stopped is told to the diagnostics, `thread`, as
is each failure of its work.
