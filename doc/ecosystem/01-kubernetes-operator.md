# The Kubernetes Operator

[Contents](README.md)

The [Turborel operator](https://github.com/turborel/turborel-operator)
runs Turborel on Kubernetes. Given a `Cluster`, it makes the volumes,
certificates, Pods and services, starts the database, and keeps them
running. It works through three resources:

| Resource | What it is |
|---|---|
| `Cluster` | A database: its storage, settings, certificates and services |
| `Backup` | One full or incremental backup, verified unless told not to |
| `ScheduledBackup` | Full backups at a fixed interval |

Their API is `database.turborel.com/v1alpha1`. The operator is in alpha,
as Turborel is: the database image and the operator's are pinned to
builds tested together. What the operator's own tests cover, and what
they do not, is in its
[testing guide](https://github.com/turborel/turborel-operator/blob/main/docs/testing.md).

## Installing

The operator needs Kubernetes 1.33 or later and a default storage
class. It is installed from the Turborel Helm repository
([The Helm Charts](02-helm-charts.md)):

```
helm repo add turborel https://charts.turborel.com
helm repo update
helm install turborel-operator turborel/turborel-operator \
  --namespace turborel-system --create-namespace
```

The chart installs the operator, its CustomResourceDefinitions and the
permissions it needs. The operator's
[quick start](https://github.com/turborel/turborel-operator#quick-start)
makes an owner Secret and starts a database named `example` in the
namespace `databases`, which the examples here use.

## A database

A Cluster names the database image, an owner Secret and the size of
its storage. The owner Secret holds `username` and `password`: the name
an unquoted SQL identifier, the password a line, not empty, without its
newline. The operator makes the owner as it makes the database, and
never again: a password changed in the Secret alone is not changed in
the database, and the two apart stop readiness and the operator's
administration of the database. A password is changed by SQL
([Users and Authentication](../operator/05-users.md)) and in the
Secret together.

A Cluster has one member, or three, or five, chosen as it is made and
not changed after. Each member has a number, a name in DNS and a volume
of its own, kept when its Pod is replaced. Turborel's
[replication](../operator/09-replication.md) chooses the primary among
the members; the operator follows that choice, and holds no election of
its own. Three replicas are placed on three nodes unless the Cluster's
affinity says otherwise.

Settings are given in `spec.settings`, by the names of
[Appendix C](../appendix/c-settings.md) with string values. The
SERVER, REPLICATION, ARCHIVE, JOBS and PLUGINS settings are the
operator's to set, and refused. Encryption keys are mounted from the
Secret `spec.keysSecret` names
([Encryption](../operator/07-encryption.md)). Memory budgets,
compression and encryption stay Turborel's settings, and tables and
their storage engines stay SQL's.

## Connections

Each member runs the client beside the server, so a SQL session is
opened in its Pod over the server's Unix socket; the owner's password
is asked for:

```
kubectl -n databases exec -it example-1 -c database -- \
  turborel --socket /run/turborel/database.sock --user admin --password
```

Programs connect by the Cluster's services, each on port 7422:

| Service | Where a connection goes |
|---|---|
| `example-rw.databases.svc` | the primary |
| `example-ro.databases.svc` | a ready replica; none, for a database of one member |
| `example-r.databases.svc` | any ready member |

A TCP connection is over TLS ([TLS](../operator/06-network.md#tls)),
the server's certificate verified against the public CA in the Secret
`example-ca`, and proves its user as usual
([Users and Authentication](../operator/05-users.md)). Replicas serve
reads and refuse writes, and a program reconnects after a change of
primary. The members secure their own frames by mutual TLS. Reaching a
database from outside the Kubernetes cluster is in the operator's
[connection guide](https://github.com/turborel/turborel-operator/blob/main/docs/operations.md#connections).

## Backups and recovery

`spec.backup.claimName` names a volume that is already there, of
access mode `ReadWriteMany`, which every member and every backup Job
mounts and writes. A Backup has the primary take a
[backup](../operator/10-backups.md) into it, records the backup's id,
and verifies that id before it reports complete, unless `spec.verify`
is false. An incremental Backup names, as its `parent`, a completed
Backup of the same Cluster and repository. A ScheduledBackup takes a
full backup every `everySeconds`. The operator removes no old backup,
and has no plug-in for an object store.

A restore is a new Cluster of one member with `spec.restore`: the
completed Backup's claim, repository path and backup id. The database
restored keeps its users and needs its encryption keys. The archive of
the log, and recovery through it to a point in time, are for a Cluster
of one member in this version of the operator; a recovery through the
archive writes a new timeline's history to the volume it recovers from.

## Updates and deletion

A change of image, settings or Secret has each Pod replaced in turn,
the replicas first and the primary last, once every member is healthy
and no backup is running. `spec.maintenance` true holds the automatic
updates. An update changes no member of the group, and a session may
be disconnected by it.

A Pod deleted is made again over its volume. A Cluster deleted keeps
its volumes and its CA Secret, though deleting the namespace removes
them too. A volume missing, or replaced by another of the same name,
is reported as an error; it is never made into an empty database.

Renewing certificates, credentials, growing a volume and the limits of
recovery are in the operator's
[operations guide](https://github.com/turborel/turborel-operator/blob/main/docs/operations.md).
Helm does not update the CustomResourceDefinitions on upgrade: they
are applied before the operator's chart is upgraded
([Upgrading the operator](02-helm-charts.md#upgrading-the-operator)).
