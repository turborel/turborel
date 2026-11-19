# Working in the turborel Command

[Contents](README.md)

The `turborel` command ([The turborel Command](01-turborel.md)) reads
lines of SQL and lines of its own commands. Each command begins with a
backslash and is done by the client, not sent to the server. The
commands set how results are written, describe what the database holds,
run files, keep variables and decide which lines run. This chapter
describes them, and the line editor that reads typed lines.

## Backslash commands

A backslash outside a literal, a delimited identifier or a comment
begins a command, which runs to the end of its line. A command may
follow the text of a statement on the same line, so `SELECT 1 \g` runs
the statement begun. Arguments are split at spaces. One in single
quotes may hold spaces, with a quote inside written twice. One in
double quotes keeps its quotes. `\?` lists every command.

```sql
\echo 'Hello, world' again
```

```
Hello, world again
```

A typed session reads each statement and command after a prompt. A
run of a file or of `-c` reads the same lines without a prompt.

The examples of this chapter are of this table:

```sql
CREATE TABLE STAFF (ID INTEGER PRIMARY KEY, NAME VARCHAR(20) NOT NULL, SALARY DECIMAL(9,2) DEFAULT 0);
INSERT INTO STAFF VALUES (1, 'Ada', 52000), (2, 'Grace', 61000), (3, 'Edsger', NULL);
COMMIT
```

```
OK
OK
OK
```

## Typing

At a terminal, lines are read by the client's own line editor. Left and
Right move the cursor. Home and End, or Ctrl-A and Ctrl-E, move it to
the ends of the line. Ctrl-Left and Ctrl-Right, or Alt-B and Alt-F, move
it a word at a time. Backspace and Delete delete a character. Ctrl-U
cuts what is before the cursor, Ctrl-K what is after it, and Ctrl-W the
word before it. Ctrl-L redraws the line, Ctrl-C throws it away, and
Ctrl-D on an empty line ends the session.

Up and Down walk the history of statements and commands typed. A
statement typed over several lines is one entry, its lines shown
together. Ctrl-R searches the history backwards for the text typed
after it, and Ctrl-R again finds an older entry. Enter runs the entry
found, another key takes it to edit, and Escape leaves the line as it
was. An entry that begins with a space is not kept, nor one the same as
the entry before. The history is kept in the file HISTORY_FILE names.
By default that is the environment's TURBOREL_HISTORY, else
`.turborel_history` in the home directory. At most HISTORY_SIZE entries
are kept, 500 by default.

Tab completes the word before the cursor. What it offers depends on
the word before:

| After | Tab offers |
|---|---|
| a backslash | a command's name |
| FROM, JOIN, INTO, UPDATE, TABLE or a describe command | a table's name |
| SCHEMA | a schema's name |
| USER, ROLE, AUTHORIZATION or TO | a user's or role's name |
| SELECT, WHERE, BY, SET or a comparison | a column's name or a key word |
| anything else | a key word |

A word begun in lower case is completed in lower case. A single
candidate replaces the word. With several, Tab adds what they share, or
lists them if that adds nothing. The names are read from the
Information Schema the first time they are needed, and again after a
statement that may change them, as CREATE, ALTER or DROP does.

The line editor puts the terminal in raw mode with the `stty` program.
It restores the terminal after each line and when the client ends.
Where standard input or output is not a terminal, or `stty` cannot set
its modes, lines are read whole and edited by the terminal alone.

## Variables

`\set NAME VALUE` sets a variable, joining the words of VALUE.
`\set NAME` sets it empty, and `\set` alone lists every variable.
`\unset NAME` unsets one. `-v NAME=VALUE` on the command line sets one
before the first statement. A name is letters, digits and underscores.

Within a statement, outside its literals, delimited identifiers and
comments, `:NAME` is the variable's value as written. `:'NAME'` is the
value as a character string literal, and `:"NAME"` is the value as a
delimited identifier. `:{?NAME}` is `true` if the variable is set and
`false` if not. A variable not set is left as written, as is the second
colon of `::`. The arguments of commands read variables the same way.

```sql
\set low 55000
\set name NAME
SELECT :"name" FROM STAFF WHERE SALARY < :low ORDER BY ID;
\echo :ROW_COUNT rows, :{?low} :{?high}
```

```
 NAME
----------------------
 Ada
(1 row)
1 rows, true false
```

These variables are the client's own:

| Variable | Means |
|---|---|
| AUTOCOMMIT | `on` makes each statement a transaction of its own, committed as it ends; `off` by default |
| STOP_ON_ERROR | `on` ends a run that is not typed at the first statement that fails; `--stop-on-error` sets it |
| ECHO | `queries` writes each statement before it runs; `errors` writes a failed statement with its failure; `-e` sets `queries` |
| QUIET | `on` writes no `OK` after a statement that is not a query; `-q` sets it |
| SAFE_UPDATES | `on` refuses an UPDATE or DELETE without WHERE before it is sent; `--safe-updates` sets it |
| FETCH_COUNT | a number of rows; a query's rows are fetched that many at a time by a cursor, rather than all at once |
| VERBOSITY | `terse` shows a failure's codes and message; `default` adds its rule and character; `verbose` also shows the statement's line, with the place marked |
| ROW_COUNT | the rows the last query gave |
| ERROR | `true` if the last statement failed, else `false` |
| SQLSTATE | the SQLSTATE of the last statement |
| LAST_ERROR_MESSAGE, LAST_ERROR_SQLSTATE | the message and SQLSTATE of the last statement that failed |
| PROMPT, PROMPT_MORE | the prompts ([Prompts](#prompts)) |
| HISTORY_FILE, HISTORY_SIZE | where the history is kept, and how many entries |
| USER, SERVER | the session's user, and the server's product and release |

## Conditionals

`\if VALUE` begins a block, `\elif VALUE` and `\else` begin a branch
of it, and `\endif` ends it. The lines of the first branch whose value
is true run. The other branches are read only for the commands that
begin and end blocks. A value is `true`, `on`, `yes` or `1`, or `false`,
`off`, `no` or `0`, in any case, and is usually a variable. Blocks may
be nested.

```sql
SELECT COUNT(*) AS N FROM STAFF \gset
\if :ERROR
  \echo cannot count
\elif :{?N}
  \echo :N people
\else
  \echo nobody
\endif
```

```
3 people
```

## Output formats

`\format OPTION VALUE` sets how rows are written. `\format OPTION`
shows one option, and `\format` alone shows every one.

| Option | Values |
|---|---|
| style | `aligned`, the default, makes columns as wide as their types; `wrapped` is aligned with the widest columns narrowed to fit the screen and their values wrapped; `unaligned` puts values between the separator; `csv` is RFC 4180's; `html` is a table; `json` is an array with one object a row; `tabs` is tab-separated, after a line of the columns' names |
| expanded | `on` writes a heading line for each row, then a line for each column with its name and value; `auto` does so only where a row is wider than the screen; `off` |
| border | 0 draws no lines between aligned columns, 1 draws them, and 2 adds a frame; 1 by default |
| null | what a null is shown as |
| separator | what is between unaligned values; `\|` by default |
| tuples | `on` writes the rows alone, without the names of the columns or the count of rows |
| footer | `off` writes no count of rows after them |
| pager | `on`, `off` or `always` ([The pager](#the-pager)) |

`\x` toggles expanded rows, and `\x on`, `\x off` or `\x auto` sets
them. `\a` toggles aligned and unaligned rows. `\t` toggles the rows
alone. A statement ended by `\G` rather than a semicolon has its rows
expanded, for that statement only. The options `-A`, `--csv`, `--html`,
`--json`, `-B`, `--style`, `--border`, `-F`, `-t`, `-x` and
`--auto-expanded` set these on the command line.

```sql
\format border 2
SELECT ID, NAME FROM STAFF WHERE ID < 3 ORDER BY ID;
```

```
+--------------+----------------------+
|           ID | NAME                 |
+--------------+----------------------+
|            1 | Ada                  |
|            2 | Grace                |
+--------------+----------------------+
(2 rows)
```

```sql
SELECT ID, NAME, SALARY FROM STAFF WHERE ID = 3\G
```

```
-[ ROW 1 ]-----
ID     | 3
NAME   | Edsger
SALARY | NULL
(1 row)
```

```sql
\format style json
SELECT ID, NAME, SALARY FROM STAFF ORDER BY ID;
```

```
[
{"ID":1,"NAME":"Ada","SALARY":52000.00},
{"ID":2,"NAME":"Grace","SALARY":61000.00},
{"ID":3,"NAME":"Edsger","SALARY":null}
]
```

```sql
\format style csv
SELECT ID, NAME FROM STAFF ORDER BY ID;
```

```
ID,NAME
1,Ada
2,Grace
3,Edsger
```

A value's width is the number of columns a terminal gives it. A
combining mark, a format character or a control character takes none.
A character of the wide scripts of East Asia takes two, and any other
takes one.

## The pager

At a terminal, rows taller than the screen are shown through a pager.
It is the program TURBOREL_PAGER names, else PAGER, else `less -S` if a
`less` is found in the directories of PATH. An empty value names no
pager. Output is held until it fills the screen, so a short result is
written as it is, and a long one is paged from its first screen while
it streams. `\format pager off` writes every result directly, and
`\format pager always` pages every one. Output sent elsewhere by `\o`
is never paged.

## Describing the database

The describe commands read the Information Schema, so they show what
any session of the user may see.

| Command | Shows |
|---|---|
| `\d` | the tables, views and sequences |
| `\d PATTERN` | each table or view it matches, with its columns and their types, nullability and defaults, then its indexes, constraints, foreign keys, the tables that reference it, and its triggers, or a view's definition |
| `\dt`, `\dv` | tables, views |
| `\di`, `\ds` | indexes, sequences |
| `\df` | routines |
| `\dn` | schemas |
| `\du` | users, and the roles granted |
| `\dT`, `\dD` | user-defined types, domains |
| `\dx` | the extension modules the server runs |
| `\dG` | property graphs |
| `\dp` | privileges on tables |
| `\l` | the catalog |

A pattern is `NAME` or `SCHEMA.NAME`, where `*` matches any text and
`?` one character. A part not in double quotes is taken in upper case,
as SQL takes a name. `+` after a command shows more columns, and `S`
adds the system's own schemas, as in `\dtS+ INFORMATION_SCHEMA.T*`. A
section with no rows is left out.

```sql
\d staff
```

```
Table ANN.STAFF
 Column | Type              | Nullable | Default
--------+-------------------+----------+---------
 ID     | INTEGER           | NO       | NULL
 NAME   | CHARACTER VARYING | NO       | NULL
 SALARY | DECIMAL           | YES      | 0.00
(3 rows)
Indexes
 Index             | Method | Unique | Columns
-------------------+--------+--------+---------
 ANN.STAFF.UNIQUE1 | btree  | YES    | ID
(1 row)
Constraints
 Constraint     | Type        | Check
----------------+-------------+------------------
 STAFF_NOTNULL1 | CHECK       | NAME IS NOT NULL
 STAFF_PRIMARY1 | PRIMARY KEY | NULL
(2 rows)
```

```sql
\dt
```

```
 Schema | Name  | Type
--------+-------+------------
 ANN    | STAFF | BASE TABLE
(1 row)
```

## Scripts

A run of `-f`, `-c` or piped input runs its statements and commands in
turn. Its exit status tells how it ended. It is 0 if every statement
succeeded, 1 if one failed and the run went on or the run could not
begin, 2 if the connection was lost, and 3 if STOP_ON_ERROR ended it at
a statement that failed.

```
$ cat check.sql
\set STOP_ON_ERROR on
SELECT COUNT(*) AS N FROM STAFF;
SELECT NOPE FROM STAFF;
\echo never
$ turborel -U ann -f check.sql
                    N
----------------------
                    3
(1 row)
turborel: SQLCODE -103 (SQLSTATE 2A000): no table in the FROM clause has a column NOPE [X3.135-1986 §5.7 SR3b] (at character 7)
```

`-1`, `--single-transaction`, runs every statement in one transaction.
It is committed as the run ends if all succeeded, and rolled back if
any failed. Without it, a statement that fails leaves no effect and the
transaction goes on, so nothing is needed to undo a failed statement
alone.

`\i FILE` runs a file's lines where it stands. `\ir FILE` finds the
file from the directory of the file being run, so files that include
one another may be moved together. Files are included at most 16 deep.

```
$ cat main.sql
\ir part.sql
\echo main done
$ cat part.sql
\echo part done
$ turborel -U ann -f main.sql
part done
main done
```

`\o FILE` sends results to a file from then on, `\o |PROGRAM` sends
them to a program's input, and `\o` alone sends them back to the
output. `\g FILE` and `\g |PROGRAM` do the same for one statement's
results. `\tee FILE` copies all that is written, failures too, to a
file until `\notee`.

`\g` runs the statement begun, or runs the last statement again if
none is begun. `\gset` runs it and stores the columns of its one row as
variables, each named by the prefix given and the column's name. A null
unsets its variable. `\gexec` runs it, then runs each value of its
result as a statement, row by row.

```sql
SELECT 'DELETE FROM STAFF WHERE ID = ' || CAST(ID AS VARCHAR(9)) FROM STAFF WHERE SALARY IS NULL \gexec
SELECT MAX(SALARY) AS TOP, MIN(SALARY) AS BOTTOM FROM STAFF \gset pay_
\echo from :pay_BOTTOM to :pay_TOP
ROLLBACK
```

```
OK
from 52000.00 to 61000.00
OK
```

`\watch SECONDS COUNT` runs the statement again every SECONDS, 2 by
default, COUNT times or until Ctrl-C or a failure. It writes a line
before each run.
`\e` edits the statement begun, or the last, in the editor
TURBOREL_EDITOR or EDITOR names, `vi` by default. What is saved is read
as if typed. `\e FILE` edits a file the same way. `\p` writes the
statement begun and `\r` throws it away.

`\delimiter TEXT` ends statements with TEXT rather than a semicolon, so
a statement may hold semicolons outside literals, as a routine's body
does; `\delimiter ;` goes back to the semicolon.

```sql
\delimiter //
VALUES 1; //
\delimiter ;
```

```

-----
   1
(1 row)
```

## Importing and exporting

`\import TABLE FILE` inserts a file's rows into a table, as `--import`
does ([Importing](01-turborel.md#importing)). Its options are words
after the file, which are `csv` or `tsv`, `header`, `delimiter C`,
`null TEXT`, `batch N` and `skip`.

`\export TABLE FILE`, or `\export (QUERY) FILE`, writes a table's or a
query's rows to a file as they come. It writes CSV, or tab-separated
text with `text`, and takes the options `header`, `delimiter C` and
`null TEXT`. In CSV a value that would read back as a null, as an empty
string does, is quoted, so `\import` reads back what `\export` wrote.

```
turborel> \export (SELECT ID, NAME FROM STAFF WHERE ID < 3) two.csv header
OK, 2 rows exported
turborel> \import STAFF_COPY two.csv header
OK, 2 rows imported
```

## Prompts

PROMPT is written before a statement, `turborel> ` by default.
PROMPT_MORE is written before a line that continues one, `turborel- `
by default. They may hold these escapes:

| Escape | Shows |
|---|---|
| `%n` | the session's user |
| `%c` | the catalog |
| `%s` | the default schema |
| `%#` | `#` for the database's owner, `>` for anyone else |
| `%x` | `*` while a transaction is active |
| `%R` | `>` before a statement, `-` within one |
| `%S` | the server's product and release |
| `%%` | a percent sign |

```
turborel> \set PROMPT '%n@%c.%s%x%# '
ANN@TURBOREL.ANN> INSERT INTO STAFF VALUES (4, 'Barbara', 70000);
OK
ANN@TURBOREL.ANN*> COMMIT;
OK
ANN@TURBOREL.ANN>
```

## The connection

`\conninfo` names the server, the user, and where they are connected.
`\c SERVER USER` connects anew. SERVER is `HOST:PORT`, or `-` for the
one the command line named. USER defaults to the command line's user.
The old connection ends once the new one is made.

`\password USER` sets a user's password, typed twice. The client makes
the SCRAM-SHA-256 verifier and sends that, never the password, to a
server that takes one. Another server is sent the password only over
TLS or a local socket, and never over plain TCP.

```
turborel> \conninfo
Connected to turborel 0.0.0 (alpha) as ANN, at 127.0.0.1:7422.
turborel> \c - BOB
Connected to turborel 0.0.0 (alpha) as BOB, by the connection TURBOREL_1.
```

## Statement help

`\h` lists the statements, and `\h NAME` writes one statement's
syntax. Both read the pages of the
[SQL Reference Manual](../sql/statements/README.md) the client was
built with.

```sql
\h release savepoint
```

```
RELEASE SAVEPOINT: destroy a savepoint, keeping the changes since

<release savepoint statement> ::= RELEASE SAVEPOINT <savepoint name>

See doc/sql/statements/release-savepoint.md.
```

## Administration

`turborel admin COMMAND` administers a server from the shell, over a
session the connection options open. Each command is one or two
statements the server runs. What it shows comes from the Information
Schema, and what it changes goes through the operator's statements.

| Command | Does |
|---|---|
| `ping` | whether the server answers, and how soon |
| `status` | the server's state, with its workers, the statements waiting and its connections |
| `processlist` | the connections the server holds |
| `kill ID` | ends the connection ID, END CONNECTION |
| `cancel ID` | cancels the statement the connection ID runs, CANCEL CONNECTION |
| `variables PATTERN` | the settings whose names match PATTERN, or every one |
| `password USER` | sets a user's password, as `\password` does |
| `create-schema NAME` | creates a schema |
| `drop-schema NAME` | drops a schema, after asking; `--yes` drops it unasked, and `--cascade` with all it holds |
| `version` | the server's product and release |
| `tls CERTIFICATE KEY` | names the server's certificate chain and key, SET DATABASE, taken for the connections after |

```
$ turborel admin -U ann variables server.tls*
 SETTING_NAME           | SETTING_VALUE
------------------------+---------------
 SERVER.TLS             | OFF
 SERVER.TLS_CERTIFICATE |
 SERVER.TLS_CLIENT_CA   |
 SERVER.TLS_KEY         |
(4 rows)
```

The files `tls` names are taken for later connections. Files changed
in place are taken again when `turboreld` is sent SIGHUP. The program
exits with 0 if the command succeeded, 1 if it failed, and 2 if the
connection was lost.
