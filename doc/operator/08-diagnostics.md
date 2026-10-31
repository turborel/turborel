# Diagnostics

[Contents](README.md)

## The event log

The server tells what an operator must know as events, a line of JSON
each, to standard error or the file `--log` names. On Unix, a file
`--log` makes is the server's user's alone (0600); one that exists is
appended to as it is:

```
{"time":"2026-10-28T19:41:24.918Z","level":"info","event":"opened","how":"reopened"}
{"time":"2026-10-28T19:41:24.921Z","level":"info","event":"listening","addresses":["127.0.0.1:7422"]}
{"time":"2026-10-28T17:12:28.772Z","level":"warning","event":"failed","sqlcode":-913,"sqlstate":"40002","message":"..."}
```

Each has its `time`, in UTC to the millisecond, its `level`, `info`,
`warning` or `error`, its `event`, and the fields of its event:

| Event | Level | Fields | When |
|---|---|---|---|
| `registered` | info | `kind`, `name` | a plug-in is registered, as the server starts |
| `opened` | info; warning if recovered | `how`: `created`, `reopened` or `recovered`; recovered, `records`, `definitions`, `committed`, `not_committed`, `redone`, `undone`, and `cut`, the bytes cut from the end of its log | the database is opened ([Recovery after a crash](03-databases.md#recovery-after-a-crash)) |
| `closed` | info | | the database is closed cleanly |
| `checkpoint_taken` | info | `taken`, the checkpoints taken since it opened | a checkpoint is taken |
| `checkpoint_overdue` | warning | `changes`, `cause`; of `refused`, `sqlcode` and `message`; of `transaction`, `seconds`; of `not_taken`, `policy` | the log keeps `changes` row changes, or the data file's side file its bytes, four times what makes a checkpoint due, for want of one; `cause` says why: `refused`, each checkpoint failing (a damaged table, a failed sync of the log, an I/O error); `transaction`, checkpoints taken but an open transaction keeping the log from its first change, made `seconds` ago; or `not_taken`, under the JOBS.POLICY `policy` named, MANUAL or PAUSED, or AUTOMATIC with its thread behind; told at most once a minute ([Checkpoints](03-databases.md#checkpoints)) |
| `thread` | info; warning if failed; error if restarted | `label`, `change`: `started`, `stopped`, `failed` (with `sqlcode` and `message`) or `restarted` | a thread of background work, `turborel-checkpoint` or `turborel-maintenance`, has changed |
| `setting_changed` | info | `name`, `value`, `null` when set back to its default | the database's value of a setting is changed |
| `user_changed` | info | `name`, `dropped`, `true` or `false` | a user is made, given a password or dropped |
| `owner_changed` | info | `name` | the database is given an owner |
| `failed` | error for SQLCODE -914, -915, -916 and -922; warning for -913 and -918 | `sqlcode`, `sqlstate`, `message` | a failure an operator must know of: a table damaged (-914), an I/O error (-915), as a file not read, a log segment that could not be begun (told once until one is), a log segment that could not be synced (the database then takes no change until it is opened again), files left as the database closed, a file whose sync failed as it was to be closed ([Open files](03-databases.md#open-files)), a file nothing needs, as a spill file done with, that could not be removed; stored data corrupt (-916); a commit rolled back (-913); a commit in doubt, its commit record not synced (-922); memory or another resource refused (-918) |
| `slow_statement` | warning | `user`, `millis`, `statement` | a statement worked longer than DIAGNOSTICS.SLOW_STATEMENT |
| `files_reopened` | warning | `reopened`, `limit` | more of the database's files were opened again in a minute, `reopened`, than FILES.MAX_OPEN, `limit`, lets be open; told at most once a minute ([Open files](03-databases.md#open-files)) |
| `listening` | info | `addresses`, an array of each address and socket | the server takes connections |
| `stopping` | info | `connections`, those it has | the server is told to stop |
| `stop_hurried` | warning | `sessions`, those whose statements were cancelled | the server, stopping, went past SERVER.SHUTDOWN_TIMEOUT or was told to stop again ([Signals](02-turboreld.md#signals)) |
| `accept_failed` | error | `failures`, since last told, `message` | the server could not take a connection, as when it has no file descriptor left; told at most once a minute |
| `tls_failed` | warning | `peer`, `message` | a client's TLS handshake failed, or did not end within SERVER.LOGIN_TIMEOUT |
| `tls_reloaded` | info | `certificate`, the chain's file | the server took its TLS files again, on SIGHUP or a change of their settings ([TLS](06-network.md#tls)) |
| `tls_reload_failed` | warning | `message` | the server could not take its TLS files again, and keeps those it had |
| `login_failed` | warning | `user`, `peer`, `mechanism`, `message` | a client was refused as the user it named, by the mechanism it asked for: never with its password |
| `connections_refused` | warning | `refused`, since last told, `limit` | connections were refused past SERVER.MAX_CONNECTIONS, `limit`; told at most once a minute ([Limits on connections](06-network.md#limits-on-connections)) |
| `other` | as the event's | `debug`, the event as the server's program prints it | an event the program writing the log does not know, of a later database |

A `peer` is the client's address and port, or `local` for a Unix
socket.

The log itself says two things:

| Event | Level | Fields | When |
|---|---|---|---|
| `lines_dropped` | warning | `lines` | the lines waiting to be written were so many, the disk or the pipe slow, that `lines` more were dropped rather than hold up the statements that told them; written with the next line |
| `log_not_reopened` | error | `path`, `message` | the log could not be opened again at `path`, after a rotation; its lines go on to the file it had |

## Levels

DIAGNOSTICS.LEVEL says the least level written: INFO, the default,
every event; WARNING, warnings and errors; ERROR, errors alone. The
database's owner sets it on a running server:

```sql
SET DATABASE DIAGNOSTICS.LEVEL = 'WARNING';
```

A change is told as `setting_changed` only if its level, info, is still
written.

## Rotating the log

On Unix, SIGHUP opens the `--log` file again at its path, so a log
rotation moves the file away and then signals the server. The lines
told before the signal stay in the file moved; those after go to a new
file, the server's user's alone. With logrotate:

```
/var/log/turborel/main.log {
    weekly
    rotate 8
    compress
    delaycompress
    postrotate
        kill -HUP $(pidof turboreld)
    endscript
}
```

Windows has no SIGHUP: there, a log is rotated with the server stopped.
A log to standard error is rotated by whatever reads it.

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
