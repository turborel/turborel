# Writing a Plug-in

[Contents](README.md)

This chapter walks through a plug-in from nothing to a call in SQL. It
starts a project with `turborel extension new`, builds and tests it with
cargo, packs it into a package file with `turborel extension pack`, and
installs that file into a server's plug-in directory. The plug-in is
written in Rust on the plug-in SDK, the crate `turborel-plugin`
([Plug-ins](04-plugins.md)). turborel has no registry, so the package
file is what the author hands to operators, by whatever means they
share files.

## Starting a project

`turborel extension new NAME` writes a starter project into a new
directory, named NAME in lower case unless `--dir` names another. It
refuses a directory that is there already.

```
$ turborel extension new hello
made the project of HELLO in hello, serving scalar
  Cargo.toml src/main.rs turborel-plugin.manifest LICENSE README.md .gitignore
```

The project is a crate of its own that builds as it is written.

| File | Holds |
|---|---|
| `Cargo.toml` | the crate, which depends on `turborel` and `turborel-plugin` of the release that made it |
| `src/main.rs` | the program, which serves each object by its name, and tests of them on the SDK's harness |
| `turborel-plugin.manifest` | the manifest, which declares the package and its objects |
| `LICENSE` | a placeholder of the license's text |
| `README.md` | how to build, test, pack and install it |

The manifest of `hello` names the program cargo builds and declares a
license, with the file of its text, beside the one scalar function.

```
NAME HELLO;
VERSION '0.1.0';
PROTOCOL 1;
EXECUTABLE 'hello';
LICENSE 'LicenseRef-Proprietary';
LICENSE FILE 'LICENSE';
FUNCTION TWICE(INTEGER) RETURNS INTEGER DETERMINISTIC;
```

On Windows the program is `hello.exe`. A project starts with a scalar
function. `--kind` asks for others, once each, and may be given more
than once, as `--kind aggregate --kind metric`. It takes `scalar`,
`aggregate`, `table`, `procedure`, `metric`, `embedding`,
`authenticator`, `target`, `partition`, `ranking`, `quantizer`,
`collation`, `system-table`, `xquery`, `type`, `engine`,
`index-method`, `credentials` and `diagnostics`, and the project then
serves a small object of each ([Kinds of Plug-in](04-plugins-kinds.md)).
`--sdk DIR` makes the project depend on a turborel source tree at DIR
by path instead of on the release's crates.

A host plug-in's project asks for host kinds alone, as a manifest
declares host objects or an extension's, never both ([Host
Plug-ins](04-plugins-host.md)). `--kind` takes `encoding`,
`hash-function`, `clock`, `log-storage`, `catalog-storage`, `spill`,
`file-system`, `served-connections`, `listener`, `package-crypto`,
`keyring`, `background-job`, `replica-transport`, `journal`,
`state-transfer`, `consensus` and `archive`. A kind that keeps what the
database relies on starts refusing each call, which stops the node,
until its author writes it. Its README tells how the node names it in
PLUGINS.HOST.

```
$ turborel extension new vault --kind clock --kind log-storage
made the project of VAULT in vault, serving clock, log-storage
  Cargo.toml src/main.rs turborel-plugin.manifest LICENSE README.md .gitignore
```

## Building

The program is built as any crate is.

```
cargo build --release
```

`turborel extension pack` looks for the program under `target/release`
of the project, or of `CARGO_TARGET_DIR` if that is set.

## Testing

The project's tests run its objects on `Harness`, the SDK's stand-in
for the database. Each call is made as the database makes it, its
payload written and its answer read, in the test's own process, so a
test needs no server. The starter tests that the plug-in serves every
kind its manifest declares, and that TWICE doubles 21.

```
cargo test
```

## Packing

A package file holds the manifest, the program, the files the manifest
names and those under `doc/` and `data/`, each with its SHA-256 digest.
The sources and anything else in the project are left out. Choose the
license before packing. Put its text in `LICENSE` and its SPDX
expression in the manifest's LICENSE item. Packing warns of a manifest
that declares no license, and refuses a LICENSE FILE that is not in the
project.

`--key` signs the package with a key `turborel extension keygen --out
author.key` wrote. Give operators the public key it printed.

```
$ turborel extension pack hello --key author.key --out hello-0.1.0.tbpkg
packed HELLO 0.1.0 into hello-0.1.0.tbpkg, signed by ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c, license LicenseRef-Proprietary (text in LICENSE)
```

## Installing

The operator installs the file into the server's plug-in directory,
trusting the author's key. Install tells the package's license, and
never refuses one for its license.

```
$ turborel extension install hello-0.1.0.tbpkg --set PLUGINS.DIRECTORIES=plugins --set PLUGINS.TRUSTED_KEYS=ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c --set PLUGINS.REQUIRE_SIGNED=ON
installed HELLO 0.1.0 from hello-0.1.0.tbpkg, license LicenseRef-Proprietary (text in LICENSE)
```

A user then makes the extension in a database and calls its function.

```sql
CREATE EXTENSION hello;
VALUES (TWICE(21));
```

```
OK

--------------
           42
(1 row)
```

The database's lock of packages shows the license each declares.

```sql
SELECT PACKAGE_NAME, LICENSE, LICENSE_FILE FROM INFORMATION_SCHEMA.EXTENSION_PACKAGES;
```

```
 PACKAGE_NAME                                                 | LICENSE                                                      | LICENSE_FILE
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 HELLO                                                        | LicenseRef-Proprietary                                       | LICENSE
(1 row)
```

[Packages](04-plugins-packages.md) tells how packages require one
another, how a set is locked, and every command of `turborel extension`.
