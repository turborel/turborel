# The Helm Charts

[Contents](README.md)

Two Helm charts run Turborel on Kubernetes: one installs the operator,
and one runs a single server as a StatefulSet. They are kept in
[turborel/charts](https://github.com/turborel/charts) and published as
a Helm repository at [charts.turborel.com](https://charts.turborel.com/):

```
helm repo add turborel https://charts.turborel.com
helm repo update
```

| Chart | What it installs |
|---|---|
| `turborel-operator` | the operator, its CustomResourceDefinitions and the permissions it needs ([The Kubernetes Operator](01-kubernetes-operator.md)) |
| `turborel` | `turboreld`, the server, as one StatefulSet, its database on a volume of its own, and a Service clients connect to |

The charts are in alpha, as Turborel is: a chart may change in any
version until Turborel's first release
([Versions](../appendix/f-versions.md)). Each chart parameterizes what
is commonly set and no more: the image, the server's settings, the
volume, the Service, resources and scheduling. The rest is a kustomize
patch or a values file over the chart, so that the charts stay small
and this manual stays the reference. Each chart's README in the
repository lists its values.

## The operator

```
helm install turborel-operator turborel/turborel-operator \
  --namespace turborel-system --create-namespace
```

The chart needs Kubernetes 1.33 or later, and installs the operator
alone: databases are made by Cluster resources
([A database](01-kubernetes-operator.md#a-database)). Its image is
`ghcr.io/turborel/turborel-operator:nightly` by default, and the
database image is the Cluster's `spec.image`, set apart from it. With
`watchNamespace` empty the operator watches every namespace, by a
ClusterRole; named, it watches that namespace alone, by a Role there.
One operator serves a watch scope: two whose scopes overlap are not
installed together.

### Upgrading the operator

Helm installs CustomResourceDefinitions the first time, and neither
updates them on upgrade nor removes them on uninstall. Before the
chart is upgraded, the CRDs of the version upgraded to are applied,
the version named for both commands:

```
helm repo update
CHART_VERSION=0.1.2
helm show crds turborel/turborel-operator --version "$CHART_VERSION" | \
  kubectl apply --server-side --force-conflicts -f -
helm upgrade turborel-operator turborel/turborel-operator \
  --namespace turborel-system --version "$CHART_VERSION" --reuse-values
```

Uninstalling the operator leaves its Clusters running, no longer
reconciled. Deleting the CRDs deletes every Cluster, Backup and
ScheduledBackup, and may collect what depends on them: it is no step
of an ordinary uninstall.

## One server

```
helm install turborel turborel/turborel --namespace turborel --create-namespace
```

The chart needs Kubernetes 1.25 or later, and runs one `turboreld`
([Running the Server](../operator/02-turboreld.md)) from
`ghcr.io/turborel/turborel:nightly`, the image that holds the server
and the client
([The container image](../operator/01-installing.md#the-container-image));
`image.tag` names a commit's tag for a fixed build. Its database is
kept on a volume of `persistence.size`, 10 GiB by default, and its
settings, under `settings`, are each passed as `--set NAME=VALUE` and
kept with the database. Clients connect to its Service on port 7422.

Replication (REPLICATION.MEMBERS, port 7423), TLS (SERVER.TLS and its
PEM files), encryption keys (`--keys`) and backups are this manual's
([Replication](../operator/09-replication.md),
[Connections](../operator/06-network.md),
[Encryption](../operator/07-encryption.md),
[Backups and Recovery](../operator/10-backups.md)), and are set up
with a kustomize patch or a values file over the chart rather than as
values of it. A replica group is run by the operator
([The Kubernetes Operator](01-kubernetes-operator.md)).
