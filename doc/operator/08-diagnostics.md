# Diagnostics

[Contents](README.md)

## The event log

The server tells what an operator must know as events, a line of JSON
each, to standard error or the file `--log` names. On Unix, a file
`--log` makes is the server's user's alone (0600); one that exists is
appended to as it is:

```
{"time":"2026-10-28T19:41:24.918Z","level":"info","event":"opened","how":"reopened"}
{"time":"2026-10-28T17:12:28.772Z","level":"warning","event":"failed","sqlcode":-913,"sqlstate":"40002","message":"..."}
```

Each has its time, in UTC, its level, `info`, `warning` or `error`, its
event, and what the event is of:

| Event | When |
|---|---|
| `registered` | a plug-in is registered, as the server starts: its kind and name |
| `opened` | the database is opened: `how`, created, reopened or recovered, and of a recovery what it did, `cut` the bytes cut from the end of its log ([Recovery after a crash](03-databases.md#recovery-after-a-crash)) |
| `closed` | the database is closed cleanly |
| `checkpoint_taken` | a checkpoint is taken |
| `checkpoint_overdue` | a warning: the log keeps `changes` row changes, or the data file's side file its bytes, four times what makes a checkpoint due, for want of one; `cause` says why: `refused`, each checkpoint failing, with its `sqlcode` and `message` (a damaged table, a failed sync of the log, an I/O error); `transaction`, checkpoints taken but an open transaction keeping the log from its first change, made `seconds` ago; or `not_taken`, under the JOBS.POLICY `policy` named, MANUAL or PAUSED, or AUTOMATIC with its thread behind; told at most once a minute ([Checkpoints](03-databases.md#checkpoints)) |
| `thread` | a thread of background work, `label` `turborel-checkpoint` or `turborel-maintenance`, has `change`d: started, stopped, failed, with its error, or restarted after a fault |
| `setting_changed` | the database's value of a setting is changed |
| `user_changed`, `owner_changed` | a user is made, given a password or dropped; the database is given an owner |
| `failed` | a failure an operator must know of: a commit rolled back, a file not read, stored data corrupt, a log segment that could not be begun (told once until one is), a log segment that could not be synced (the database then takes no change until it is opened again), files left as the database closed, a file whose sync failed as it was to be closed ([Open files](03-databases.md#open-files)), a file nothing needs, as a spill file done with, that could not be removed |
| `slow_statement` | a statement worked longer than DIAGNOSTICS.SLOW_STATEMENT |
| `files_reopened` | a warning: more of the database's files were opened again in a minute, `reopened`, than FILES.MAX_OPEN, `limit`, lets be open; told at most once a minute ([Open files](03-databases.md#open-files)) |

## Slow statements

DIAGNOSTICS.SLOW_STATEMENT, in milliseconds, tells each statement whose
work takes as long or longer, 0 for none, the default:

```sql
SET DATABASE DIAGNOSTICS.SLOW_STATEMENT = 50;
```

```
{"time":"2026-10-28T19:26:31.014Z","level":"warning","event":"slow_statement","user":"ANN","millis":2771,"statement":"SELECT COUNT(*) FROM N A, N B, N C"}
```

A statement's text is told as its session gave it, values written in it
included; but while anything of the database is encrypted, its files or
a table ([Encryption](07-encryption.md)), a slow statement is told by
who ran it and its time alone, `"statement":null`, so no value meant for
an encrypted table reaches the log in the clear.
