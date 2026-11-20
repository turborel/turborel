# Ecosystem

For whoever runs Turborel on Kubernetes, or connects it to another system: the tools made for it, each released on its own.

[The documentation set](../README.md)

## Contents

- [The Kubernetes Operator](01-kubernetes-operator.md): databases, their
  volumes, certificates, services and backups, made and kept by
  `Cluster`, `Backup` and `ScheduledBackup` resources.
- [The Helm Charts](02-helm-charts.md): a chart that installs the
  operator, and one that runs a single server as a StatefulSet.

Each tool has versions and releases of its own, apart from Turborel's
([Versions](../appendix/f-versions.md)), and its own documentation says
what it was tested with. Its images and the database's are pinned to
builds tested together.
