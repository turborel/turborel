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

A database made with `--memory` has no directory: it is gone when the
server stops.

## Opening and closing

As it opens, a database is **created**, if its directory holds no
catalog; **reopened**, if it was closed cleanly; or **recovered**, if it
stopped without closing. The diagnostics tell which:

```
{"time":"2026-10-28T19:41:24.918Z","level":"info","event":"opened","how":"reopened"}
```

Closing, as the server stops, takes a checkpoint and marks the catalog
closed cleanly.

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
recovery reads the log only from there. The end of a transaction takes
one once CHECKPOINT.AFTER_CHANGES row changes have been logged, 100,000
by default, or CHECKPOINT.AFTER_BYTES bytes written to the data file,
64 MiB. Each is told to the diagnostics, `checkpoint_taken`.
