# Appendix F: Versions

[Contents](README.md)

## Releases and codenames

A release is numbered MAJOR.MINOR.PATCH, one number for the library,
the server, the client and the command-line client, which work together
when of one release. Each major release has a codename, as may a minor
or patch release that marks a large step; any other carries its
predecessor's.

| Release | Codename |
|---|---|
| 0.y.z | alpha |
| 1.0.0 | Sprout |

turborel is in alpha, 0.0.0: while its version is 0.y.z, its SQL, API,
protocol and files may change in any release, and a database made by
one may not open with another. From 1.0.0 a patch fixes, a minor adds,
and only a major release takes away. [VERSIONING.md](../../VERSIONING.md)
says what each may change.

The programs print their release:

```
$ turborel --version
turborel 0.0.0 (alpha)
$ turboreld --version
turborel 0.0.0 (alpha)
```

The client prints it with the server's as it begins, typed at a
terminal.

## Format and protocol versions

The files a database is kept in, and the protocol, have version numbers
of their own, changed only when they change. A database's files are
opened only by a release that knows their versions.

| Format | Version |
|---|---|
| The log's segments | 1 |
| The data file of tables and indexes | 1 |
| The catalog | 1 |
| A log record | 1 |
| A row store table | 1 |
| A B-link tree | 1 |
| A hash index | 1 |
| The native protocol | 1 |
