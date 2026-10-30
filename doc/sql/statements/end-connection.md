# END CONNECTION

[Statements](README.md)

## NAME

END CONNECTION: end a connection of the server *(extension)*

## SYNOPSIS

```
<end connection statement> ::= END CONNECTION <connection identifier>

<connection identifier> ::= <unsigned integer>
```

## DESCRIPTION

END CONNECTION ends the connection of the `<connection identifier>`,
its ID in INFORMATION_SCHEMA.CONNECTIONS: the statement it runs, if any,
is cancelled, then the connection is closed and its session rolled
back, as one lost is. Its client is told:

```
turborel: SQLCODE -919 (SQLSTATE 08006): the server's operator ended the connection
```

It returns at once: the connection ends as soon as what it runs stops.
It changes no definition and is not logged: it may run in a
transaction, which goes on. A session that ends its own connection is
ended so too.

## RULES

1. Its user is the database's owner.
2. A server serving the database serves a connection of that ID.

## PRIVILEGES

The database's owner alone.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -104 | Its user is not the database's owner |
| 08003 | -919 | No connection of that ID is served |

The connection running END CONNECTION is not lost by an 08003: the
connection it names is another.

## EXAMPLES

Run by the database's owner, ANN, connected as connection 1, beside
BOB's, connection 2:

```sql
END CONNECTION 2;
END CONNECTION 9;
```

```
OK
turborel: SQLCODE -919 (SQLSTATE 08003): no connection 9 is served (at character 15)
```

## STANDARD

An extension.

## SEE ALSO

[CANCEL CONNECTION](cancel-connection.md), [DISCONNECT](disconnect.md),
[Connections](../../operator/06-network.md#ending-a-connection)
