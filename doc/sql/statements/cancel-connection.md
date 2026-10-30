# CANCEL CONNECTION

[Statements](README.md)

## NAME

CANCEL CONNECTION: cancel what a connection of the server runs *(extension)*

## SYNOPSIS

```
<cancel connection statement> ::= CANCEL CONNECTION <connection identifier>

<connection identifier> ::= <unsigned integer>
```

## DESCRIPTION

CANCEL CONNECTION cancels the statement that the connection of the
`<connection identifier>` runs, as its client's own cancel does: that
statement fails (SQLCODE -920, SQLSTATE TB920) and is undone, and its
session, its transaction among it, goes on. A connection running
nothing is not changed. The identifier is the connection's ID in
INFORMATION_SCHEMA.CONNECTIONS.

It changes no definition and is not logged: it may run in a
transaction, which goes on.

CANCEL is not a key word: it stays a name everywhere else.

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

## EXAMPLES

Run by the database's owner, ANN, connected as connection 1, beside
BOB's, connection 2:

```sql
SELECT ID, USER_NAME FROM INFORMATION_SCHEMA.CONNECTIONS ORDER BY ID;
CANCEL CONNECTION 2;
```

```
                   ID | USER_NAME
----------------------+--------------------------------------------------------------
                    1 | ANN
                    2 | BOB
(2 rows)
OK
```

## STANDARD

An extension.

## SEE ALSO

[END CONNECTION](end-connection.md),
[Connections](../../operator/06-network.md#ending-a-connection)
