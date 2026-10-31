# Running the Server

[Contents](README.md)

## The turboreld command line

```
turboreld (--dir PATH | --memory) [options]
turboreld --dir PATH [--set NAME=VALUE]... user COMMAND
turboreld --dir PATH [--keys DIR] reseal AUTH.TABLE
```

| Option | Meaning |
|---|---|
| `--dir PATH` | serve the database kept in the directory PATH, made if there is none |
| `--memory` | serve a database kept nowhere, lost when the server stops |
| `--owner NAME` | with `--memory`: make NAME the database's first user and owner as the server starts, its password typed or read from standard input ([Users](05-users.md#the-owner-of-a-database-kept-nowhere)) |
| `--set NAME=VALUE` | set the database's setting NAME as the server starts, kept with the database; more than once for more |
| `--log FILE` | append the diagnostics, a line of JSON each, to FILE; standard error by default |
| `--keys DIR` | the encryption keys, a file in DIR each ([Encryption](07-encryption.md)) |
| `--checksum-key NAME` | the key of `--keys` that HMAC-SHA-256 checksums are keyed by ([Encryption](07-encryption.md#checksums)) |
| `--help` | print the usage |
| `--version` | print the release |

`user COMMAND` manages the users of a database no server holds
([Users](05-users.md)); `reseal AUTH.TABLE` seals a table's rows again
through the encodings it names now, so a key it named before may be
retired ([Encryption](07-encryption.md#retiring-a-key)). The settings `--set` sets are those of
[Settings](../appendix/c-settings.md), each set before the database is
opened if it is read as the database opens, after if not, and kept with
the database. A setting fixed when the database is made (TAKES_EFFECT
CREATION, [Settings](04-settings.md#when-a-change-takes-effect)) takes
the value given as `--dir` names a directory of no database, which it
then makes. Of a directory that holds a database already, it may be
given only as the database was made: so given, it changes nothing;
given otherwise, the database is not opened, and the server does not
start:

```
$ turboreld --dir /var/lib/turborel/main --set STORAGE.PAGE_SIZE=4096
turboreld: cannot open /var/lib/turborel/main: SQLCODE -910 (SQLSTATE TB910): the database was created with STORAGE.PAGE_SIZE = 8192, fixed then: it cannot be 4096
```

## Starting and stopping

```
$ turboreld --dir /var/lib/turborel/main --set SERVER.HOST=0.0.0.0
turboreld: listening at 0.0.0.0:7422
```

The server opens the database, recovering it if it stopped without
closing ([Databases](03-databases.md)), and says where it listens once it
takes connections, which the diagnostics tell too, as `listening`, before
it says so. By then it hears SIGHUP too ([Signals](#signals)), so a
supervisor may rotate the log as soon as it is told. By
default it listens at 127.0.0.1, port 7422: only clients of its own
machine reach it, and each proves it is a user.

One server holds a database at a time: another started on the same
directory, or a user command, is refused while one does:

```
turboreld: cannot open /var/lib/turborel/main: SQLCODE -915 (SQLSTATE TB915): /var/lib/turborel/main/lock is locked: another process, or another database of this one, holds it
```

## Signals

Ctrl-C, or SIGTERM on Unix, stops the server: it takes no new
connections, each connection finishes the statement it runs, its
transaction rolled back, and the database is closed cleanly, which the
diagnostics tell as `stopping`, then `closed`.

A statement that runs on is given SERVER.SHUTDOWN_TIMEOUT, 30,000
milliseconds by default, 0 for no limit. Past it, or at a second Ctrl-C
or SIGTERM, the server hurries, told as `stop_hurried`: the statement
each session runs is cancelled, as a client's cancel does, every
connection is closed, its transaction rolled back, and the database is
then closed cleanly as before:

```
$ turboreld --dir /var/lib/turborel/main --set SERVER.SHUTDOWN_TIMEOUT=5000
turboreld: listening at 127.0.0.1:7422
```

A server killed without closing, as by SIGKILL or a crash, loses no
committed transaction: the database is recovered as it is next opened.

SIGHUP, on Unix, opens the `--log` file again: a log rotation moves the
file away, then sends SIGHUP, and the lines after go to a new file at
the same path, made the server's user's alone
([Diagnostics](08-diagnostics.md#rotating-the-log)). It does not stop
the server. Windows has no such signal: there, a log
is rotated with the server stopped.

The server's threads are named `turborel-server-<n>`: its I/O threads,
one for each processor, and the threads statements run on, made as they
are needed, at most SERVER.WORKERS running at once. The database's own
are `turborel-checkpoint` and `turborel-maintenance`
([Background work](03-databases.md#background-work)).
