# ALTER DATABASE

[Statements](README.md)

## NAME

ALTER DATABASE: hand the database on to another owner, or change the
members of the group of replicas keeping it *(extension)*

## SYNOPSIS

```
<alter database statement> ::=
    ALTER DATABASE OWNER TO <authorization identifier>
  | ALTER DATABASE REPLICATION MEMBERS <character string literal>
```

## DESCRIPTION

ALTER DATABASE OWNER TO makes a user the database's owner, in place of
the session's own: the owner hands the database on, so a running
server's owner is changed without stopping it. The new owner manages the
users and sets the database's settings from its next statement on.

Sessions go on as they are: none is ended. A session of the owner
before goes on as the owner no more, from its next statement.

It is made at once, apart from any transaction: no ROLLBACK undoes it.
It is kept with the definitions, so it holds when the database is
opened again, and told to the diagnostics, `owner_changed`.

ALTER DATABASE REPLICATION MEMBERS changes the members of the group of
replicas keeping the database ([Replication](../../operator/09-replication.md)).
The `<character string literal>` names every member of the group from
then on, each its number, `=`, and the host and port its replica
listens at, by commas, as the node setting REPLICATION.MEMBERS does:
`'1=db1.example.com:7423,2=db2.example.com:7423,4=db4.example.com:7423'`.
A member not named leaves the group; one named for the first time
joins it, once its server is started with settings naming the new
members and itself. A member's number names it for the life of the
group: a number is never given again to another node.

It is run on the primary, and returns once the group has committed it:
the change is a record of the database's log, the last of the group's
epoch, and the next epoch's primary, the first of the new members by
number, serves from then on. The statement is logged for the group's
replicas, told to the diagnostics, `reconfigured`, and made at once,
apart from any transaction: no ROLLBACK undoes it. Sessions of the
primary before go on only if it is the next epoch's primary too; else
they are ended as the service ends, as a view change ends them.

## RULES

1. The session has no transaction open: COMMIT or ROLLBACK first.
2. Its user is the database's owner.
3. OWNER TO: the `<authorization identifier>` is a user.
4. REPLICATION MEMBERS: the literal names each member once, by a number
   above 0 and a host and port; the database is kept by a group of
   replicas, and this node is its primary.

## PRIVILEGES

The database's owner's alone.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 25000 | -917 | The session has a transaction open; REPLICATION MEMBERS, this node is a backup, or was removed from the group |
| 2A000 | -102 | OWNER, TO or the identifier is not written; REPLICATION, MEMBERS or the literal is not written |
| 2A000 | -103 | There is no such user; a member is named by no number, by 0, twice, or at no host and port |
| 2A000 | -104 | Its user is not the database's owner |
| TB910 | -910 | REPLICATION MEMBERS of a database kept by no group of replicas |

## EXAMPLES

Run by ANN, the database's owner, beside the user BOB:

```sql
ALTER DATABASE OWNER TO nope;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): there is no user NOPE to own the database (at character 24)
```

The members of the group, asked of a database kept by no group, here,
and malformed; on the primary of a group of three, with a fourth node
started with settings naming the four and the member 3 to leave, the
first prints OK once the group has committed it:

```sql
ALTER DATABASE REPLICATION MEMBERS '1=db1.example.com:7423,2=db2.example.com:7423,4=db4.example.com:7423';
```

```
turborel: SQLCODE -910 (SQLSTATE TB910): RULES.md D57: a database with no log is kept by no group (at character 35)
```

```sql
ALTER DATABASE REPLICATION MEMBERS '1=db1.example.com,2=db2.example.com:7423';
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): RULES.md D57: in REPLICATION MEMBERS, '1=db1.example.com' has no host:port (at character 35)
```

Handed on to BOB, ANN owns it no more:

```sql
ALTER DATABASE OWNER TO bob;
```

```
OK
```

ANN owns the database no more:

```sql
ALTER DATABASE OWNER TO ann;
```

```
turborel: SQLCODE -104 (SQLSTATE 2A000): only the database's owner runs ALTER DATABASE, and ANN is not it
```

## STANDARD

An extension. ISO/IEC 9075:1992 has no statement of a database's owner:
which identifiers are authorization identifiers is
implementation-defined [§5.4; Annex B, item 41]; nor of replication.

## SEE ALSO

[CREATE USER](create-user.md), [DROP USER](drop-user.md),
[SET DATABASE](set-database.md),
[Users](../../operator/05-users.md),
[Replication](../../operator/09-replication.md)
