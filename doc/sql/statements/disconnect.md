# DISCONNECT

[Statements](README.md)

## NAME

DISCONNECT: end connections *(in the client)*

## SYNOPSIS

```
<disconnect statement> ::= DISCONNECT { <connection name> | DEFAULT | CURRENT | ALL }
```

The `<connection name>` is a character string literal.

## DESCRIPTION

DISCONNECT ends a connection, and its session: the one named, the
default connection, the current one, or ALL of them. If the current
connection is ended, none is current, and every statement but CONNECT
and SET CONNECTION is refused until one is. The client runs it, as it
does [CONNECT](connect.md).

A connection whose ending fails is ended all the same, and the
statement completes with the warning SQLSTATE 01002.

## RULES

1. The connection is established; of CURRENT, one is current.
2. No connection ended has a transaction active: COMMIT or ROLLBACK
   first.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 08003 | -919 | There is no such connection |
| 25000 | -917 | A connection ended has a transaction active |
| 2A000 | -102 | The name is not a character string literal |
| 0A000 | -900 | Run by a session of an embedded database |
| 01002 | 0 | Not an error: a connection's ending failed; it is ended all the same |

## EXAMPLES

Of the connections of [CONNECT](connect.md), the default one current:

```sql
DISCONNECT 'OWNERDB';
DISCONNECT 'OWNERDB';
DISCONNECT CURRENT;
VALUES 1;
CONNECT TO DEFAULT;
VALUES CURRENT_USER;
DISCONNECT ALL;
```

```
OK
turborel: SQLCODE -919 (SQLSTATE 08003): there is no connection OWNERDB [ISO/IEC 9075:1992 §15.3 GR1-GR3]
OK
turborel: SQLCODE -919 (SQLSTATE 08003): there is no current connection [ISO/IEC 9075:1992 §15.3 GR9]
OK

-----
 ANN
(1 row)
turborel: SQLCODE -917 (SQLSTATE 25000): a connection's transaction is active [ISO/IEC 9075:1992 §15.3 GR6]
```

## STANDARD

ISO/IEC 9075:1992 §15.3. SQL-86 has no connections.

## SEE ALSO

[CONNECT](connect.md), [SET CONNECTION](set-connection.md)
