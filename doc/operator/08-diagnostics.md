# Diagnostics

[Contents](README.md)

## The event log

The server tells what an operator must know as events, a line of JSON
each, to standard error or the file `--log` names:

```
{"time":"2026-10-28T19:41:24.918Z","level":"info","event":"opened","how":"reopened"}
{"time":"2026-10-28T17:12:28.772Z","level":"warning","event":"failed","sqlcode":-913,"sqlstate":"40002","message":"..."}
```

Each has its time, in UTC, its level, `info`, `warning` or `error`, its
event, and what the event is of:

| Event | When |
|---|---|
| `registered` | a plug-in is registered, as the server starts: its kind and name |
| `opened` | the database is opened: `how`, created, reopened or recovered, and of a recovery what it did |
| `closed` | the database is closed cleanly |
| `checkpoint_taken` | a checkpoint is taken |
| `thread` | a thread of background work, `label` `turborel-checkpoint` or `turborel-maintenance`, has `change`d: started, stopped, failed, with its error, or restarted after a fault |
| `setting_changed` | the database's value of a setting is changed |
| `user_changed`, `owner_changed` | a user is made, given a password or dropped; the database is given an owner |
| `failed` | a failure an operator must know of: a commit rolled back, a file not read, stored data corrupt |
| `slow_statement` | a statement worked longer than DIAGNOSTICS.SLOW_STATEMENT |

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
included.
