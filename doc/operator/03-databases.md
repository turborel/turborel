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
| `lock` | held while a server, or a program, has the database open |

`undo/` and `tmp/` are made anew each time the database opens. The
files are opened only by a release that knows their format versions
([Versions](../appendix/f-versions.md)).

On Unix, the directories and files a database makes are the user's who
runs it alone: directories 0700, files 0600, less what the umask takes
away. A directory that already exists keeps its mode. On Windows they
take the permissions of the directory they are made in.

A database made with `--memory` has no directory: it is gone when the
server stops. Its pages, its temporary tables' rows and what its queries
spill are all kept in memory, of its MEMORY_BUDGET, never in the
system's temporary directory: a statement that needs a page past the
budget is refused with SQLCODE -918, and what was written before it
stays.

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
{"time":"2026-10-28T19:41:11.802Z","level":"warning","event":"opened","how":"recovered","records":6,"definitions":3,"committed":1,"not_committed":0,"redone":2,"undone":0}
```

## Checkpoints

A checkpoint writes the changes since the last to the data file, so a
recovery reads the log only from there. One is due once
CHECKPOINT.AFTER_CHANGES row changes have been logged, 100,000 by
default, or CHECKPOINT.AFTER_BYTES bytes written to the data file,
64 MiB; the database's checkpoint thread takes it, so no transaction
waits for it. Each is told to the diagnostics, `checkpoint_taken`.

## Open files

A database holds at most FILES.MAX_OPEN of its files open at once, 1024
by default, 0 for no limit, so a server stays within what the system
allows a process. Past it, the file used least recently is closed, and
opened again as it is next used; while every open file is being read or
written, the next waits for one. The database's owner sees each file in
INFORMATION_SCHEMA.FILES, its kind, whether it is open, and its reads,
writes and syncs:

```sql
SELECT KIND, COUNT(*) FROM INFORMATION_SCHEMA.FILES GROUP BY KIND;
```

## Background work

The database does some of its work in the background: its checkpoints,
and its maintenance, which gathers tables' statistics and freezes the
rows of the column store. JOBS.POLICY says how: AUTOMATIC, the
default, on threads of the database's own as the work comes due;
PAUSED, not at all until it changes, the work due kept; MANUAL, only
when the program embedding the database asks. It takes effect at once:

```sql
SET DATABASE JOBS.POLICY = 'PAUSED'
```

Each thread started or stopped is told to the diagnostics, `thread`, as
is each failure of its work.
