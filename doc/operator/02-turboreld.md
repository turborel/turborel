# Running the Server

[Contents](README.md)

## The turboreld command line

```
turboreld (--dir PATH | --memory) [options]
turboreld --dir PATH [--config FILE] [--set NAME=VALUE]... user COMMAND
turboreld --dir PATH [--keys DIR] reseal AUTH.TABLE
```

| Option | Meaning |
|---|---|
| `--dir PATH` | serve the database kept in the directory PATH, made if there is none |
| `--memory` | serve a database kept nowhere, lost when the server stops |
| `--owner NAME` | with `--memory`: make NAME the database's first user and owner as the server starts, its password typed or read from standard input ([Users](05-users.md#the-owner-of-a-database-kept-nowhere)) |
| `--set NAME=VALUE` | set the database's setting NAME as the server starts, kept with the database; more than once for more |
| `--config FILE` | set the database's settings FILE gives, as `--set` does; a setting `--set` gives too is set as `--set` gives it ([Configuration files](#configuration-files)) |
| `--log FILE` | append the diagnostics, a line of JSON each, to FILE; standard error by default |
| `--keys DIR` | the encryption keys, a file in DIR each ([Encryption](07-encryption.md)) |
| `--checksum-key NAME` | the key of `--keys` that HMAC-SHA-256 checksums are keyed by ([Encryption](07-encryption.md#checksums)) |
| `--help` | print the usage |
| `--version` | print the release |

`user COMMAND` manages the users of a database no server holds
([Users](05-users.md)); `reseal AUTH.TABLE` seals a table's rows again
through the encodings it names now, so a key it named before may be
retired ([Encryption](07-encryption.md#retiring-a-key)). The settings `--set` sets, and
those of a `--config` file ([Configuration files](#configuration-files)), are those of
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
$ turboreld --dir /var/lib/turborel/main --set STORAGE.PAGE_BYTES=4096
turboreld: cannot open /var/lib/turborel/main: SQLCODE -910 (SQLSTATE TB910): the database was created with STORAGE.PAGE_BYTES = 8192, fixed then: it cannot be 4096
```

With `--memory`, no directory makes the database, so a setting fixed
when it is made is refused, and the server does not start:

```
$ turboreld --memory --set STORAGE.PAGE_BYTES=4096
turboreld: cannot set STORAGE.PAGE_BYTES: it is fixed when a database is created in its directory, and --memory keeps it in memory
```

## Configuration files

`--config FILE` names a file of settings, each set as the server starts
as `--set` sets it: those read as the database opens before it opens,
the others after, each kept with the database. It is a source of the
database's settings, not a configuration of its own: each key names a
setting of [Settings](../appendix/c-settings.md), each value is the
setting's. A setting given by the file and by `--set` too is set as
`--set` gives it: the command line wins. The options of the command
line (`--dir`, `--log`, `--keys` and the rest) say where the database,
its diagnostics and its keys are, not how the database behaves; they
are not in the file.

```toml
# The settings of the main database's server, set as it starts.
[SERVER]
HOST = ["127.0.0.1"]
PORT = 7422
WORKERS = 2
SHUTDOWN_TIMEOUT_MS = 5_000
SCRAM_WITHOUT_TLS = false

[STORAGE]
PAGE_BYTES = 16_384        # fixed as the database is made
POOL_BYTES = 67_108_864
```

```
$ turboreld --dir /var/lib/turborel/main --config /etc/turborel/main.toml
turboreld: listening at 127.0.0.1:7422
```

The file is in a subset of TOML 1.0: what it reads means what TOML
means. A setting's name is the key of the last table header and the
pair's own key, joined by periods: `PORT = 7422` under `[SERVER]` sets
SERVER.PORT, as `SERVER.PORT = 7422` does before the first header. A
name is in any case, as setting names are. A value is

| Written | Is |
|---|---|
| `"text"`, `'text'` | a character string |
| `7422`, `-1`, `16_384` | a whole number, of 64 bits |
| `0.5`, `1e3`, `-2.5E-2` | an approximate number, finite |
| `true`, `false` | ON, OFF |
| `["a", 'b']` | the strings joined by commas, `a,b`, for a setting that takes a list, as SERVER.HOST and STORAGE.ENCODINGS do; `[]` for none |

In the notation of SQL's standard, the subset is:

```
<configuration file> ::= [ <line> [ { <newline> <line> }... ] ]
<line> ::= [ <blank> ] [ <table header> | <key value pair> ] [ <blank> ] [ <comment> ]
<table header> ::= <left bracket> [ <blank> ] <key> [ <blank> ] <right bracket>
<key value pair> ::= <key> [ <blank> ] <equals operator> [ <blank> ] <value>
<key> ::= <simple key> [ { [ <blank> ] <period> [ <blank> ] <simple key> }... ]
<simple key> ::= <bare key> | <basic string> | <literal string>
<bare key> ::= { <ASCII letter> | <digit> | <underscore> | <minus sign> }...
<value> ::= <basic string> | <literal string> | <integer> | <float> | <boolean> | <array>
<basic string> ::= <double quote> [ { <basic character> | <escape> }... ] <double quote>
<escape> ::= <reverse solidus>
      { b | t | n | f | r | <double quote> | <reverse solidus>
      | u <hex digit>4 | U <hex digit>8 }
<literal string> ::= <quote> [ <literal character>... ] <quote>
<integer> ::= [ <sign> ] <whole digits>
<whole digits> ::= 0 | <nonzero digit> [ { [ <underscore> ] <digit> }... ]
<float> ::= <integer> { <fraction> [ <exponent> ] | <exponent> }
<fraction> ::= <period> <digits>
<exponent> ::= { e | E } [ <sign> ] <digits>
<digits> ::= <digit> [ { [ <underscore> ] <digit> }... ]
<boolean> ::= true | false
<array> ::= <left bracket> <array blank>
      [ <string> [ { <array blank> <comma> <array blank> <string> }... ]
      <array blank> [ <comma> <array blank> ] ] <right bracket>
<string> ::= <basic string> | <literal string>
<array blank> ::= [ { <space> | <tab> | <newline> | <comment> }... ]
<blank> ::= { <space> | <tab> }...
<comment> ::= <number sign> [ <comment character>... ]
<newline> ::= <line feed> | <carriage return> <line feed>
```

A `<basic character>` is any character but a double quote, a reverse
solidus, and a control character other than tab; a `<literal
character>` any but a quote and such a control character; a `<comment
character>` any but such a control character and a newline. A string is
on one line; a string of an array holds no comma. A `<hex digit>4` is
four hexadecimal digits, of a Unicode scalar value. A leading byte order
mark is passed over.

Refused, with the rest of TOML: multi-line strings, integers other than
decimal, infinite and not-a-number floats, dates and times, arrays of
anything but strings, inline tables, and arrays of tables. A setting
given twice, or a table header, is refused too. The file is read, and
each of its keys checked to name a setting, before any setting is set;
what is wrong is told with the file's path, line and column, and the
server does not start:

```
$ turboreld --dir /var/lib/turborel/main --config /etc/turborel/main.toml
turboreld: /etc/turborel/main.toml:3:1: no setting is named STORAGE.NO_SUCH
```

A value its setting does not take is told the same way, as `--set`'s
is: `/etc/turborel/main.toml:1:1: cannot set SERVER.WORKERS: ...`.

## Starting and stopping

```
$ turboreld --dir /var/lib/turborel/main --set SERVER.HOST=0.0.0.0
turboreld: listening at 0.0.0.0:7422
```

The server opens the database, recovering it if it stopped without
closing ([Databases](03-databases.md)), and says where it listens once it
takes connections, which the diagnostics tell too, as `listening`, before
it says so. By then it hears SIGHUP, SIGTERM and Ctrl-C too
([Signals](#signals)), so a supervisor may rotate the log, or stop the
server cleanly, as soon as it is told. By
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

A statement that runs on is given SERVER.SHUTDOWN_TIMEOUT_MS, 30,000
milliseconds by default, 0 for no limit. Past it, or at a second Ctrl-C
or SIGTERM, the server hurries, told as `stop_hurried`: the statement
each session runs is cancelled, as a client's cancel does, every
connection is closed, its transaction rolled back, and the database is
then closed cleanly as before:

```
$ turboreld --dir /var/lib/turborel/main --set SERVER.SHUTDOWN_TIMEOUT_MS=5000
turboreld: listening at 127.0.0.1:7422
```

A server killed without closing, as by SIGKILL or a crash, loses no
committed transaction: the database is recovered as it is next opened.

SIGHUP, on Unix, opens the `--log` file again: a log rotation moves the
file away, then sends SIGHUP, and the lines after go to a new file at
the same path, made the server's user's alone
([Diagnostics](08-diagnostics.md#rotating-the-log)). It takes TLS's
certificate, key and client authorities again from their files too, so
a certificate renewed in place secures the connections after
([TLS](06-network.md#tls)). It does not stop the server. Windows has no
such signal: there, a log is rotated with the server stopped, and TLS's
files are taken again as their settings change.

The server's threads are named `turborel-server-<n>`: its I/O threads,
one for each processor, and the threads statements run on, made as they
are needed, at most SERVER.WORKERS running at once. The database's own
are `turborel-checkpoint`, `turborel-maintenance` and
`turborel-log-sync` ([Background work](03-databases.md#background-work)).
