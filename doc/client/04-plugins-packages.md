# Packages and Registries

[Contents](README.md)

An extension is distributed as a package, one file that holds its
manifest, its executable for each platform, its documents and any data
files. Each file in it carries its SHA-256 digest, and the package may be
signed with Ed25519. A registry is a directory of packages and a signed
index of them, served over HTTPS. `turborel extension` installs packages
from files and registries into a server's plug-in directory, keeps a
lock of exactly what it installed, and upgrades and removes them.
`CREATE EXTENSION` then loads an installed package by its name.

This chapter is for two readers. An author packs and signs a plug-in and
publishes it. An operator chooses the registries and keys a server
trusts, and installs, upgrades and checks packages.

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
([Plug-ins](04-plugins.md)) with three more items, each ended by a
semicolon.

| Item | Says |
|---|---|
| `REQUIRES name 'range'` | the package needs another package, of a version in the range |
| `TURBOREL 'range'` | the releases of turborel it runs on |
| `UPGRADE FROM 'range' SCRIPT 'path'` | the SQL script `ALTER EXTENSION ... UPDATE` runs when it upgrades from a version in the range |

A package's name is an SQL identifier, upper case unless quoted, and
`turborel extension` takes it in any case.

```
NAME geo_tools;
VERSION '1.4.0';
PROTOCOL 1;
EXECUTABLE FOR 'linux' 'bin/geo-tools';
EXECUTABLE FOR 'windows' 'bin/geo-tools.exe';
EXECUTABLE FOR 'macos' 'bin/geo-tools-macos';
REQUIRES UNITS '^2.0.0';
TURBOREL '>=0.0.0';
UPGRADE FROM '>=1.0.0 <1.4.0' SCRIPT 'upgrade/1.4.sql';
FUNCTION AREA_KM2(DOUBLE PRECISION, DOUBLE PRECISION) RETURNS DOUBLE PRECISION DETERMINISTIC;
```

The upgrade scripts that apply are run in the order of the lowest
version each range names.

### A signing key

`turborel extension keygen` writes a new Ed25519 signing key, and its
public key beside it with `.pub` after the name. It never writes over a
file. Keep the key file private, as the command writes it with the
permissions a new file gets, and give operators the public key.

```
$ turborel extension keygen --output author.key
wrote the key author.key and its public key 3b6a27bcceb6a42d62a3a8d02a6f0d73653215771de243a63ac048a18b59da29
```

### Packing

`turborel extension pack DIR` builds the package of the plug-in in DIR.
It takes every file under DIR but those whose names begin with `.`. The
manifest, the executables and upgrade scripts it names, and the files
under `doc/` are told apart in the package. With `--key` the package is
signed. The signature covers the manifest and every file's path and
digest, so no file can be changed or added without breaking it.

```
$ turborel extension pack geo_tools --key author.key
packed GEO_TOOLS 1.4.0 into geo_tools-1.4.0.tbpkg, signed by 3b6a27bc...9da29, sha256 0d5c...
```

### Publishing to a registry

`turborel extension publish DIR --registry-dir REG --key KEY` packs and
signs DIR, writes the package into the registry directory REG, adds it
to REG's index, and signs the index again with the same key. The index
is trusted for 30 days unless `--expires-days` says otherwise. Publish
again before it expires, even with no new package, so clients never
see an index that expired.

```
$ turborel extension publish geo_tools --registry-dir /srv/registry --key author.key
published GEO_TOOLS 1.4.0 as geo_tools-1.4.0.tbpkg
signed the index of /srv/registry, trusted until 2026-12-19T09:00:00Z
```

## Hosting a registry

A registry is a static directory, so any web server that serves files
over HTTPS hosts one. No registry is operated for turborel, so there is
no default, and each organization runs its own or uses a third party's.
The directory holds three kinds of file.

| File | Holds |
|---|---|
| `index` | the packages, one `package` line each, with their requirements, and when the index expires |
| `index.sig` | the public key that signed the index and its Ed25519 signature, in hexadecimal |
| `*.tbpkg` | the package files |

An index, format 1, reads as follows.

```
turborel-registry 1
expires 2026-12-19T09:00:00Z
package GEO_TOOLS 1.4.0 geo_tools-1.4.0.tbpkg 0d5c...e1 3b6a...29
requires UNITS ^2.0.0
turborel >=0.0.0
package UNITS 2.1.0 units-2.1.0.tbpkg 9f02...4a 3b6a...29
```

A client trusts an index only after its signature verifies by a key it
trusts, whether or not it requires signed packages, and only before it
expires. A stale index, or an old one replayed, is refused. Only then
does the client trust the digests the index lists, and a package fetched
whose digest differs is refused. Each package still carries its own
signature, which the server checks as it installs the package and every
time its program starts.

A mirror is a copy of the directory. A directory of the local machine
also serves as a registry, named by its path or a `file://` URL.

## For operators

### Settings

`turborel extension` reads the server's settings, given with `--set
NAME=VALUE` or in the environment as `TURBOREL_` and the name with `.`
written `_`.

| Setting | Meaning |
|---|---|
| `PLUGINS.DIRECTORIES` | the plug-in directories; packages are installed into the first, as `<name>/<version>/` |
| `PLUGINS.REGISTRIES` | the registries' URLs, separated by `;`, the first tried first |
| `PLUGINS.PINS` | `NAME=URL` entries separated by `;`, each taking a package from that registry alone |
| `PLUGINS.TRUSTED_KEYS` | the Ed25519 public keys trusted, in hexadecimal, separated by `;`, or `@` and a file of them, one a line |
| `PLUGINS.REQUIRE_SIGNED` | `ON` refuses a package no trusted key signed, and a plug-in directory no package installed |

Downloaded packages are kept in a cache, `.cache` in the plug-in
directory unless `--cache` names another, by their digest, so a package
installed again is not fetched again. `--tls FILE` trusts the
registries' certificates of a PEM file instead of the platform's
authorities.

### Installing

```
$ turborel extension install geo_tools@^1.0.0 --set PLUGINS.DIRECTORIES=/var/lib/turborel/plugins \
    --set PLUGINS.REGISTRIES=https://plugins.example.com/ --set PLUGINS.TRUSTED_KEYS=3b6a...29
installed UNITS 2.1.0 from https://plugins.example.com/
installed GEO_TOOLS 1.4.0 from https://plugins.example.com/
```

Install resolves the whole set again. It takes every package asked for
by name, with its range, and every package they require, choosing the
newest versions that meet every requirement and trying older ones where
a newer one leads to a conflict. One version of each package is
installed in a set. A dependency cycle, or requirements no versions
meet, is refused with SQLSTATE 38T09, naming the requirement. A package
file may be named instead of a name, and is installed with what it
requires.

A package installed is never changed after. Its files are checked
against their digests as it is installed and each time its program
starts, so a changed file is refused with SQLSTATE 38T07.

### The lock

The plug-in directory keeps a lock, `turborel-extensions.lock`, of the
exact set installed, each package's name, version, the SHA-256 digest of
its package file, its registry, and the range it was asked for by name.

```
turborel-lock 1
GEO_TOOLS 1.4.0 0d5c...e1 https://plugins.example.com/ explicit ^1.0.0
UNITS 2.1.0 9f02...4a https://plugins.example.com/ dependency
```

`install` with no package installs exactly what the lock names, from
the cache or the registry, each file's digest the one locked. `lock
export FILE` writes the lock to a file, and `lock import FILE` installs
exactly what it names on another server and keeps it as that server's
lock, so two servers run the same packages to the byte. Each database
records the version of each extension it created, as
`INFORMATION_SCHEMA.EXTENSIONS` shows, and a version stays installed
while a database runs it.

### Keeping packages current

| Command | Does |
|---|---|
| `outdated` | lists each package with a newer version, the newest its range allows and the newest of all |
| `upgrade [NAME]...` | upgrades to the newest versions the ranges allow |
| `upgrade NAME --latest` | upgrades beyond the range, and keeps the new version's caret range |
| `why NAME` | tells which packages required NAME, back to one asked for by name |
| `remove NAME` | removes a package no installed package requires |
| `autoremove` | removes packages installed only as dependencies that nothing requires now, and versions the lock no longer names |
| `verify` | checks every installed file against its digest and the package's signature |
| `clean` | empties the cache |
| `search WORDS...` | lists the registries' packages whose names hold a word |
| `info NAME` | lists a package's versions, what each requires, and its signer |

```
$ turborel extension outdated --set ...
name current wanted latest
GEO_TOOLS 1.4.0 1.5.2 2.0.0
$ turborel extension why units --set ...
UNITS is required by GEO_TOOLS 1.4.0 (^2.0.0)
  GEO_TOOLS 1.4.0 was installed by name
```

A database moves to a new version with `ALTER EXTENSION name UPDATE`,
which runs the package's upgrade scripts in the statement's transaction.
`CREATE EXTENSION name` with no `FROM` loads the newest installed version
that meets every requirement, and `VERSION` names a version or range.

### Conditions

| SQLSTATE | Condition |
|---|---|
| 38T07 | a package or installed file is not what its digest says, or a lock's digest differs from the registry's |
| 38T08 | a package is unsigned or signed by no trusted key where a signature is required, a signature does not verify, or a registry's index is unsigned, untrusted or expired |
| 38T09 | no versions meet every requirement, or the requirements form a cycle |
