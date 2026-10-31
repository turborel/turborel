# CONNECT

[Statements](README.md)

## NAME

CONNECT: connect to a server *(in the client)*

## SYNOPSIS

```
<connect statement> ::= CONNECT TO <connection target>

<connection target> ::=
      <SQL-server name> [ AS <connection name> ] [ USER <user name> ]
    | DEFAULT
```

Each name is a character string literal.

## DESCRIPTION

CONNECT makes a connection to a server, and makes it current: the
statements that follow run there, in a session of its own, until SET
CONNECTION or DISCONNECT. The connection current before stays, dormant,
its session and what it holds kept.

CONNECT, [SET CONNECTION](set-connection.md) and
[DISCONNECT](disconnect.md) are the SQL-agent's: the client runs them
itself, as SQL-92 has the SQL-agent do. A program embedding turborel
runs them through the client library's agent; a session of an embedded
database refuses them.

The `<SQL-server name>` is the server's address: `'host:port'`,
`'host'` for the default port 7422, an IPv6 address in brackets, or, on
Unix, the path of a socket beginning `/`, or, on Windows, the name of
a named pipe beginning `\\`, as `'\\.\pipe\turborel'`. A TCP
connection is secured by TLS as the client's first is. DEFAULT is the
server the client's command line names.

The `<connection name>` names the connection, for SET CONNECTION and
DISCONNECT, an identifier; without AS, the connection is named as its
address, which must then be an identifier: `'localhost'` may be,
`'localhost:7492'` may not. DEFAULT makes the default connection, of no
name, which the client makes as it starts.

The `<user name>` is who connects; without USER, the user the command
line names. Each connection proves its user as the command line says:
a password typed is asked for again.

## RULES

1. The current connection has no transaction active: COMMIT or
   ROLLBACK first. No transaction spans two servers.
2. The user name is an authorization identifier; the connection name an
   identifier.
3. No connection of the name is established.
4. The server can be reached, and accepts the connection.

## PRIVILEGES

The server's: the user proves who it is, as the server asks.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 0A001 | -900 | The current connection's transaction is active |
| 28000 | -104 | The user name is not an authorization identifier |
| 2E000 | -919 | The connection name is not an identifier |
| 08002 | -919 | A connection of the name is established |
| 08001 | -919 | The server cannot be reached |
| 08004 | -919 | The server refuses the connection |
| 2A000 | -102 | A name is not a character string literal |
| 0A000 | -900 | Run by a session of an embedded database |

## EXAMPLES

In the client, connected as ANN to one server, and BOB a user of another
at port 7492:

```sql
VALUES CURRENT_USER;
COMMIT;
CONNECT TO 'localhost:7492' AS 'OWNERDB' USER 'BOB';
VALUES CURRENT_USER;
SET CONNECTION DEFAULT;
```

```

-----
 ANN
(1 row)
OK
OK

-----
 BOB
(1 row)
turborel: SQLCODE -900 (SQLSTATE 0A001): the current connection's transaction is active, and none spans servers [ISO/IEC 9075:1992 §15.1 GR1, §15.2 GR1]
```

The VALUES began a transaction on OWNERDB, so it must end before
another connection is made current. Connections refused, none current
in a transaction:

```sql
CONNECT TO 'localhost:7492' AS 'OWNERDB';
CONNECT TO 'localhost:7492';
CONNECT TO 'localhost:1' AS 'NOWHERE';
CONNECT TO 'localhost:7492' AS 'bad name';
CONNECT TO 'localhost:7492' AS 'X2' USER '1x';
CONNECT TO 'localhost:7492' AS OWNERDB;
```

```
turborel: SQLCODE -919 (SQLSTATE 08002): the connection OWNERDB is established [ISO/IEC 9075:1992 §15.1 GR7]
turborel: SQLCODE -919 (SQLSTATE 2E000): 'localhost:7492' is not an <identifier> [ISO/IEC 9075:1992 §15.1 GR6]
turborel: SQLCODE -919 (SQLSTATE 08001): cannot connect: Connection refused (os error 61) [ISO/IEC 9075:1992 §15.1 GR10, GR11]
turborel: SQLCODE -919 (SQLSTATE 2E000): 'bad name' is not an <identifier> [ISO/IEC 9075:1992 §15.1 GR6]
turborel: SQLCODE -104 (SQLSTATE 28000): '1x' is not an <authorization identifier> [ISO/IEC 9075:1992 §15.1 GR3]
turborel: SQLCODE -102 (SQLSTATE 2A000): expected a <literal>, found OWNERDB (at character 31)
```

The text of an error the system gives, as "Connection refused (os error
61)", is the operating system's.

## STANDARD

ISO/IEC 9075:1992 §15.1. What a `<SQL-server name>` names is
implementation-defined: here, an address. SQL-86 has no connections.

## SEE ALSO

[SET CONNECTION](set-connection.md), [DISCONNECT](disconnect.md),
[The Command-Line Client](../../client/01-turborel.md)
