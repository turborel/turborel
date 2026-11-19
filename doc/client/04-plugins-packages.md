# Packages

[Contents](README.md)

An extension is distributed as a package, one file that holds its
manifest, its executable for each platform, its documents and any data
files. Each file in it carries its SHA-256 digest, and the package may be
signed with Ed25519. turborel has no registry and fetches nothing. A
package is a file, and its author hands it to operators by whatever means
they share files. `turborel extension` installs package files into a
server's plug-in directory, resolving what they require among the files
given and the packages installed, keeps a lock of exactly what it
installed, and removes packages. `CREATE EXTENSION` then loads an
installed package by its name.

This chapter is for two readers. An author packs, signs and licenses a
plug-in ([Writing a Plug-in](04-plugins-sdk.md) starts one). An operator
chooses the keys a server trusts, and installs and checks packages.

## Versions and ranges

A package's `VERSION` is a semantic version, `MAJOR.MINOR.PATCH`, with
an optional prerelease after `-`, as `2.1.0-beta.1`. Build metadata after
`+` is allowed and passed over. Versions are ordered as semantic
versioning orders them, so `1.0.0-rc.1` comes before `1.0.0`.

A range says which versions a requirement accepts.

| Range | Accepts |
|---|---|
| `1.2.3` or `=1.2.3` | exactly 1.2.3 |
| `>1.2.3`, `>=1.2.3`, `<2.0.0`, `<=1.4` | the versions above, from, below or up to the one written |
| `^1.2.3` | 1.2.3 up to but not including 2.0.0; `^0.2.3` stops before 0.3.0 and `^0.0.3` before 0.0.4 |
| `~1.2.3` | 1.2.3 up to but not including 1.3.0; `~1` is any 1.x |
| `1.x`, `1.2.x`, `1`, `1.2` | any version of the parts written |
| `*`, `x` or nothing | any version |
| `1.2.3 - 2.0.0` | from the first to the second, both included |
| `>=1.2.0 <2.0.0` or `>=1.2.0, <2.0.0` | the versions that meet every comparator |
| `^1.0.0 \|\| ^3.0.0` | the versions of either |

A prerelease is taken only when asked for. It meets a range only if a
comparator of that range names a prerelease of the same
`MAJOR.MINOR.PATCH`, so `^1.0.0` never takes `1.1.0-beta.1`, and
`>=1.1.0-beta.1` takes `1.1.0-beta.2` but not `1.2.0-beta.1`.

## For authors

### The manifest of a package

A package's manifest is the plug-in's manifest
([Plug-ins](04-plugins.md)) with five more items, each ended by a
semicolon.

| Item | Says |
|---|---|
| `REQUIRES name 'range'` | the package needs another package, of a version in the range |
| `TURBOREL 'range'` | the releases of turborel it runs on |
| `UPGRADE FROM 'range' SCRIPT 'path'` | the SQL script `ALTER EXTENSION ... UPDATE` runs when it upgrades from a version in the range |
| `LICENSE 'expression'` | the license the package is offered under, an SPDX license expression |
| `LICENSE FILE 'path'` | the file in the package that holds the license's text |

A package's name is an SQL identifier, upper case unless quoted, and
`turborel extension` takes it in any case. Each item but REQUIRES,
EXECUTABLE FOR a platform and UPGRADE is written once at most.

```
$ cat geo_tools/turborel-plugin.manifest
NAME geo_tools;
VERSION '1.4.0';
PROTOCOL 1;
EXECUTABLE FOR 'linux' 'bin/geo-tools';
EXECUTABLE FOR 'windows' 'bin/geo-tools.exe';
EXECUTABLE FOR 'macos' 'bin/geo-tools-macos';
REQUIRES UNITS '^2.0.0';
TURBOREL '>=0.0.0';
UPGRADE FROM '>=1.0.0 <1.4.0' SCRIPT 'upgrade/1.4.sql';
LICENSE 'MIT';
LICENSE FILE 'LICENSE';
FUNCTION AREA_KM2(DOUBLE PRECISION, DOUBLE PRECISION) RETURNS DOUBLE PRECISION DETERMINISTIC;
$ cat geo_tools/LICENSE
GEO_TOOLS is offered under the MIT license.
```

The upgrade scripts that apply are run in the order of the lowest
version each range names.

### Licenses

LICENSE is checked against the grammar of SPDX license expressions. It
is a license identifier, as `MIT` or `Apache-2.0`, or a reference of the
author's own, `LicenseRef-` and letters, digits, `.` and `-`. A `+`
after an identifier takes that version or later. Expressions are joined
by `AND` and `OR`, an exception follows `WITH`, and parentheses group
them, as `(MIT OR Apache-2.0) AND LicenseRef-Fonts`. A list of
identifiers separated by commas is not an expression and is refused with
SQLSTATE 38T06, as is an expression left unfinished. The identifiers
themselves are not looked up, so a new one of SPDX's list is taken.

LICENSE FILE names a file of the project, which pack puts in the
package. Its digest is among the package's, so the signature covers it
as it covers the program. Both items are optional. A package that
declares no license packs with a warning, and installs as any other. A
license is told, never enforced. Install tells it, and `turborel
extension license NAME`, `\dx+`, the lock and the column LICENSE of
INFORMATION_SCHEMA.EXTENSION_PACKAGES show it. A plug-in of the host's
own reads an installed package's license with the function
`turborel::plugin::package::package_license`.

### Signing and packing

`turborel extension keygen --out FILE` writes a new Ed25519 signing key,
and its public key beside it with `.pub` after the name. It never writes
over a file. Keep the key file private, as the command writes it with
the permissions a new file gets, and give operators the public key.

```
turborel extension keygen --out author.key
```

`turborel extension pack DIR` builds the package of the plug-in project
in DIR. It takes the manifest, the executables and upgrade scripts it
names, the LICENSE FILE, and the files under `doc/` and `data/`, and
nothing else. An executable is found at its path in DIR, or else under
`target/release`, of DIR or of `CARGO_TARGET_DIR`, by its file's name,
where `cargo build --release` puts it. A file the manifest names that is
in neither place is refused. With `--key` the package is signed. The
signature covers the manifest and every file's path and digest, so no
file can be changed or added without breaking it. The package is
written to `--out`, or into DIR as its name in lower case, `-`, its
version and `.tbpkg`.

```
$ turborel extension pack geo_tools --key author.key --out packages/geo_tools-1.4.0.tbpkg
packed GEO_TOOLS 1.4.0 into packages/geo_tools-1.4.0.tbpkg, signed by ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c, license MIT (text in LICENSE)
$ cat units/turborel-plugin.manifest
NAME units; VERSION '2.1.0'; PROTOCOL 1; EXECUTABLE 'bin/units';
FUNCTION KM(DOUBLE PRECISION) RETURNS DOUBLE PRECISION DETERMINISTIC;
$ turborel extension pack units --out packages/units-2.1.0.tbpkg
warning: the manifest declares no LICENSE; operators will be told the package has none
packed UNITS 2.1.0 into packages/units-2.1.0.tbpkg, unsigned, no license declared
```

## For operators

### Settings

`turborel extension` reads the server's settings, given with `--set
NAME=VALUE` or in the environment as `TURBOREL_` and the name with `.`
written `_`.

| Setting | Meaning |
|---|---|
| `PLUGINS.DIRECTORIES` | the plug-in directories; packages are installed into the first, as `<name>/<version>/` |
| `PLUGINS.TRUSTED_KEYS` | the Ed25519 public keys trusted, in hexadecimal, separated by `;`, or `@` and a file of them, one a line |
| `PLUGINS.REQUIRE_SIGNED` | `ON` refuses a package no trusted key signed, and a plug-in directory no package installed |

### Installing

`install FILE...` takes package files, or directories whose `.tbpkg`
files it takes. It resolves the whole set again. The packages of the
files given that no other file given requires are asked for by name,
with a caret range of their version, as are the packages asked for by
name before. Every package they require is chosen among the files given
and the packages installed, the newest versions that meet every
requirement, older ones tried where a newer one leads to a conflict. One
version of each package is installed in a set, and a file nothing chosen
needs is passed over. A requirement no file given and no package
installed meets is refused with SQLSTATE 38T09, naming it and the
package file to add.

```
$ turborel extension install packages/geo_tools-1.4.0.tbpkg --set PLUGINS.DIRECTORIES=plugins
turborel extension: SQLCODE -301 (SQLSTATE 38T09): the packages cannot be resolved: GEO_TOOLS 1.4.0 requires UNITS ^2.0.0, and no version of it found meets that; add a package file of UNITS of a version in ^2.0.0
$ turborel extension install packages/geo_tools-1.4.0.tbpkg packages/units-2.1.0.tbpkg --set PLUGINS.DIRECTORIES=plugins
installed UNITS 2.1.0 from packages/units-2.1.0.tbpkg, no license declared
installed GEO_TOOLS 1.4.0 from packages/geo_tools-1.4.0.tbpkg, license MIT (text in LICENSE)
```

A newer version moves on in the same way, its file given. A dependency
cycle among the files is refused with 38T09 too. A package installed is
never changed after. Its files are checked against their digests as it
is installed and each time its program starts, so a changed file is
refused with SQLSTATE 38T07.

### The lock

The plug-in directory keeps a lock, `turborel-extensions.lock`, of the
exact set installed. Each package has a line of its name, version, the
SHA-256 digest of its package file, and the range it was asked for by
name or `dependency`, and lines of its license after it, if it declares
one.

```
turborel-lock 1
GEO_TOOLS 1.4.0 0d5c...e1 explicit ^1.4.0
license MIT
license file LICENSE
UNITS 2.1.0 9f02...4a dependency
```

`lock export FILE` writes the lock to a file. `install --lock FILE`,
with the package files, installs exactly what that lock names on another
server and keeps it as that server's lock, so two servers run the same
packages to the byte. A file of a version the lock names whose digest is
another is refused with 38T07, and a package the lock names that is
neither installed nor given is refused with 38T09.

```
$ turborel extension lock export servers.lock --set PLUGINS.DIRECTORIES=plugins
wrote the lock to servers.lock
$ turborel extension install --lock servers.lock packages/geo_tools-1.4.0.tbpkg packages/units-2.1.0.tbpkg --set PLUGINS.DIRECTORIES=replica
installed GEO_TOOLS 1.4.0 from packages/geo_tools-1.4.0.tbpkg, license MIT (text in LICENSE)
installed UNITS 2.1.0 from packages/units-2.1.0.tbpkg, no license declared
```

Each database records the version of each extension it created, as
`INFORMATION_SCHEMA.EXTENSIONS` shows, and a version stays installed
while a database runs it.

### Looking after packages

| Command | Does |
|---|---|
| `list` | lists the packages locked, and those installed and not locked |
| `info NAME` | lists the versions of a package installed, what each requires, its signer and its license |
| `license NAME` | writes the text of a package's license, or its expression if it holds no text |
| `why NAME` | tells which packages required NAME, back to one asked for by name |
| `remove NAME` | removes a package no installed package requires |
| `autoremove` | removes packages installed only as dependencies that nothing requires now, and versions the lock no longer names |
| `verify` | checks every installed file against its digest and the package's signature |

```
$ turborel extension list --set PLUGINS.DIRECTORIES=plugins
GEO_TOOLS 1.4.0 explicit ^1.4.0
UNITS 2.1.0 dependency
$ turborel extension why units --set PLUGINS.DIRECTORIES=plugins
UNITS is required by GEO_TOOLS 1.4.0 (^2.0.0)
  GEO_TOOLS 1.4.0 was installed by name
$ turborel extension license geo_tools --set PLUGINS.DIRECTORIES=plugins
GEO_TOOLS is offered under the MIT license.
```

A database moves to a new version with `ALTER EXTENSION name UPDATE`,
which runs the package's upgrade scripts in the statement's transaction.
`CREATE EXTENSION name` with no `FROM` loads the newest installed version
that meets every requirement, and `VERSION` names a version or range.

### Conditions

| SQLSTATE | Condition |
|---|---|
| 38T06 | a manifest is malformed, its LICENSE no SPDX expression among them, or a file it names is not in the project |
| 38T07 | a package or installed file is not what its digest says, or a package file's digest differs from the lock's |
| 38T08 | a package is unsigned or signed by no trusted key where a signature is required, or a signature does not verify |
| 38T09 | no versions meet every requirement, a requirement names a package of which no file is given and none installed, or the requirements form a cycle |
