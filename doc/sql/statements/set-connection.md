# SET CONNECTION

[Statements](README.md)

## NAME

SET CONNECTION: make another connection current *(in the client)*

## SYNOPSIS

```
<set connection statement> ::= SET CONNECTION { DEFAULT | <connection name> }
```

The `<connection name>` is a character string literal.

## DESCRIPTION

SET CONNECTION makes the connection named, or the default connection,
current: the statements that follow run in its session. The connection
current before stays, dormant, its session and what it holds kept: its
temporary tables' rows, its settings, its prepared statements. The
client runs it, as it does [CONNECT](connect.md).

## RULES

1. The current connection has no transaction active: COMMIT or
   ROLLBACK first.
2. The connection is established.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 0A001 | -900 | The current connection's transaction is active |
| 08003 | -919 | There is no such connection |
| 2A000 | -102 | The name is not a character string literal |
| 0A000 | -900 | Run by a session of an embedded database |

## EXAMPLES

Of the connections of [CONNECT](connect.md), OWNERDB current:

```sql
COMMIT;
SET CONNECTION DEFAULT;
VALUES CURRENT_USER;
COMMIT;
SET CONNECTION 'NOPE';
```

```
OK
OK

-----
 ANN
(1 row)
OK
turborel: SQLCODE -919 (SQLSTATE 08003): there is no connection NOPE [ISO/IEC 9075:1992 §15.2 GR2]
```

## STANDARD

ISO/IEC 9075:1992 §15.2. SQL-86 has no connections.

## SEE ALSO

[CONNECT](connect.md), [DISCONNECT](disconnect.md)
