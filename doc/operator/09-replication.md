# Replication

[Contents](README.md)

## A replica group

A database may be kept by a group of servers, its replicas, each a
node holding the whole database: one, the primary, takes the changes,
and the others, the backups, hold what it commits and serve reads. A
change is committed once a majority of the group holds it durably, so
the group bears the loss of fewer than half its nodes: three nodes bear
one lost, five bear two. When the primary is lost, the backups choose
the next primary among themselves within seconds, and go on; the node
lost, run again, rejoins and catches up. The protocol is Viewstamped
Replication, each term of a primary a *view*.

Replication is set up on each node by its settings, given as the
server starts (`--set`, or a [configuration file](02-turboreld.md#configuration-files)),
the node's own, kept with no database:

| Setting | Meaning |
|---|---|
| REPLICATION.REPLICA | this node's number among the members; 0, not replicated |
| REPLICATION.MEMBERS | every member: its number, `=`, and the host and port its replica listens at, by commas |
| REPLICATION.LISTEN | where this node listens for the others; its own member's host and port by default |
| REPLICATION.CA | the PEM file of the authorities the members' certificates are verified against |
| REPLICATION.TLS | ON: the members secure their frames by mutual TLS; OFF only on one machine |
| REPLICATION.TICK_MS | the pace of the protocol's clock: heartbeats, and how soon a lost primary is given up |
| REPLICATION.RETAIN_BYTES | how much of the group's log the node keeps before its last checkpoint, 256 MiB by default: a backup behind by less catches up from the log |

Each node presents SERVER.TLS_CERTIFICATE and SERVER.TLS_KEY to the
others, and takes only a member whose certificate the authorities of
REPLICATION.CA verify, its name the host REPLICATION.MEMBERS names for
it. Three nodes:

```
$ turboreld --dir /var/db --keys keys --set REPLICATION.REPLICA=1 \
    --set REPLICATION.MEMBERS=1=db1.example.com:7423,2=db2.example.com:7423,3=db3.example.com:7423 \
    --set REPLICATION.CA=/etc/turborel/replicas-ca.pem \
    --set SERVER.TLS_CERTIFICATE=/etc/turborel/db1.pem --set SERVER.TLS_KEY=/etc/turborel/db1.key
```

and the same on `db2` and `db3`, each with its own number. A new
database is made by the group as its nodes first start, once a majority
of them are up: give `--owner NAME` to each, and the owner is made once
a node serves. The settings fixed when the database is made, and every
other setting, are the database's, replicated with it; the REPLICATION
settings alone are each node's.

A node starting over an empty `log/`, new to the group, or its log lost,
learns the group's state from the others before it takes part, told as
a `recovering` event: it joins once a majority of the members answer
it, the primary among them, and fetches the primary's database if its
own is behind. A node whose `log/` directory is lost keeps the rows its
database holds and catches up the same way: nothing need be copied by
hand.

## Serving

Clients connect to any node. The primary takes changes; a backup
serves reads of what it holds, and refuses a change (SQLCODE -917,
SQLSTATE 25000: the transaction's state). A read left open on a backup
holds its snapshot, and the backup's checkpoints wait for it. A client
whose statement is refused so connects to the primary, which
INFORMATION_SCHEMA.REPLICAS names:

```sql
SELECT MEMBER, ADDRESS, IS_SELF, ROLE, VIEW_NUMBER, IS_CONNECTED, LAST_HEARD_MS
    FROM INFORMATION_SCHEMA.REPLICAS;
```

```
 MEMBER | ADDRESS              | IS_SELF | ROLE    | VIEW_NUMBER | IS_CONNECTED | LAST_HEARD_MS
--------+----------------------+---------+---------+-------------+--------------+---------------
      1 | db1.example.com:7423 | YES     | PRIMARY |           0 | YES          |
      2 | db2.example.com:7423 | NO      | BACKUP  |           0 | YES          |            18
      3 | db3.example.com:7423 | NO      | BACKUP  |           0 | YES          |            22
(3 rows)
```

It is the owner's to read. A commit on the primary returns once a
majority holds the change: while fewer nodes are up, commits wait.

The replica's life is told to the diagnostics as `replica` events
([Diagnostics](08-diagnostics.md)): the view entered and its primary,
this node serving or stopped, links to the other members up and down,
and frames or connections refused. The job that runs the replica is
`turborel-replica` ([Background work](03-databases.md#background-work)).

## Commands on a replicated database

A replicated database is managed through its primary, in SQL: users and
the owner by CREATE USER, ALTER USER, DROP USER and ALTER DATABASE OWNER
TO ([Users](05-users.md)), its files' formats by SET DATABASE
([Encryption](07-encryption.md)). The commands on a database's directory,
`user`, `reseal` and `rekey`, refuse one that is replicated.

## The log kept

Each node keeps the group's log only as far back as its own database
needs it: a checkpoint lets the records it holds go, but for
REPLICATION.RETAIN_BYTES of them, so a backup down for a while catches
up from the log of the others as it comes back. The log's segments in
`log/` are dropped as records go. Each node syncs a record to `log/`
before it acknowledges it to the group, whatever LOG.DURABILITY says:
the setting bears on a database alone.

A node so far behind that the primary's log no longer reaches back to
what it holds fetches the primary's database instead: its files as the
primary's last checkpoint left them, in pieces, over the same link, into
`transfer/` in its directory; then it installs them in place of its own,
told as a `state_installed` event, and follows the log from there. The
primary serves the transfer out of its files, which no checkpoint
changes meanwhile; a checkpoint due waits until the transfer is done.
Nothing stops on either node: the primary serves clients, and the node
fetching serves reads of what it held until the install. Started again
with a database far behind, a node fetches the state the same way.

## What is not yet done

The members are fixed by their settings: to add or remove a member,
stop the group and start it with new settings.
