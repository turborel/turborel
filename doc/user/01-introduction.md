# Introduction

[Contents](README.md)

## What Turborel is

Turborel (TUR-boh-rel) is a relational database system. It keeps data in
tables, of rows and columns, and answers questions of them asked in SQL.
It keeps the data safe: a change committed survives a crash, and many
users may read and change it at once, each seeing it as if alone.

It runs as a server, `turboreld`, that many clients connect to at once,
or as a library a program embeds. This guide is of SQL, and the same
for both; the [Operator's Guide](../operator/README.md) is of running
the server, and the [Client Reference](../client/README.md) of the
programs that connect to it.

Turborel is in alpha: its SQL, its programs and its files may change in
any release until 1.0.0 ([Versions](../appendix/f-versions.md)).

## The SQL it speaks

Turborel's SQL is the language of the standards, ANSI X3.135-1986
(SQL-86), ISO/IEC 9075:1992 (SQL-92) and ISO/IEC 9075-2:2023 (SQL:2023),
read as one language, the newer prevailing where they differ. SQL-86 is
implemented in full and SQL-92 nearly so, and SQL:2023's features are
added as they are implemented ([CONFORMANCE.md](../../CONFORMANCE.md)).
Where Turborel adds to the standards, this manual marks it
*(extension)*, as for storage engines and indexes, users and passwords,
and settings. What it does not process of the standards, as character
sets other than SQL_TEXT, it refuses, saying so.

## How this guide is organized

[Getting Started](02-getting-started.md) connects, makes a table and
queries it. [Concepts](03-concepts.md) gives the ideas the rest rests
on: schemas, tables, privileges. Then a chapter each of querying,
changing data, transactions, constraints, dates and times, temporary
tables, and errors. Each chapter shows SQL as typed to the client, and
what it gives back; the [SQL Reference](../sql/README.md) gives each
statement whole.
