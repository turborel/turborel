# Running the Server

[Contents](README.md)

## The turboreld command line

```
turboreld (--dir PATH | --memory) [options]
turboreld --dir PATH [--set NAME=VALUE]... user COMMAND
```

| Option | Meaning |
|---|---|
| `--dir PATH` | serve the database kept in the directory PATH, made if there is none |
| `--memory` | serve a database kept nowhere, lost when the server stops |
| `--set NAME=VALUE` | set the database's setting NAME as the server starts, kept with the database; more than once for more |
| `--log FILE` | append the diagnostics, a line of JSON each, to FILE; standard error by default |
| `--keys DIR` | the encryption keys, a file in DIR each ([Encryption](07-encryption.md)) |
| `--help` | print the usage |
| `--version` | print the release |

`user COMMAND` manages the users of a database no server holds
([Users](05-users.md)). The settings `--set` sets are those of
[Settings](../appendix/c-settings.md); a setting fixed when the database
is made is set so only as it is made.

## Starting and stopping

```
$ turboreld --dir /var/lib/turborel/main --set SERVER.HOST=0.0.0.0
turboreld: listening at 0.0.0.0:7422
```

The server opens the database, recovering it if it stopped without
closing ([Databases](03-databases.md)), and says where it listens once it
takes connections. By default it listens at 127.0.0.1, port 7422: only
clients of its own machine reach it, and each proves it is a user.

One server holds a database at a time: another started on the same
directory, or a user command, is refused while one does:

```
turboreld: cannot open /var/lib/turborel/main: SQLCODE -915 (SQLSTATE TB915): /var/lib/turborel/main/lock is locked: another process, or another database of this one, holds it
```

## Signals

Ctrl-C, or SIGTERM on Unix, stops the server: it takes no new
connections, each connection finishes the statement it runs, and the
database is closed cleanly, which the diagnostics tell as `closed`. A
server killed without closing, as by SIGKILL or a crash, loses no
committed transaction: the database is recovered as it is next opened.

The server's threads are named `turborel-server-<n>`: its I/O threads,
one for each processor, and the threads statements run on, made as they
are needed, at most SERVER.WORKERS running at once. The database's own
are `turborel-checkpoint` and `turborel-maintenance`
([Background work](03-databases.md#background-work)).
