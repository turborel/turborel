# Installing

[Contents](README.md)

## Platforms

Turborel runs on Linux, macOS and Windows: each is built and tested on
every change. What only one platform has is refused, saying so,
elsewhere: Unix sockets are of Linux and macOS alone, named pipes of
Windows.

## The latest alpha

Each change on main that passes every check is built for Linux
(x86-64), macOS (Apple silicon) and Windows (x86-64), and published as
the pre-release [nightly](https://github.com/turborel/turborel/releases/tag/nightly).
Each build replaces the one before, so its files keep their addresses:

| Platform | File |
|---|---|
| Linux, x86-64 | [turborel-nightly-x86_64-unknown-linux-gnu.tar.gz](https://github.com/turborel/turborel/releases/download/nightly/turborel-nightly-x86_64-unknown-linux-gnu.tar.gz) |
| macOS, Apple silicon | [turborel-nightly-aarch64-apple-darwin.tar.gz](https://github.com/turborel/turborel/releases/download/nightly/turborel-nightly-aarch64-apple-darwin.tar.gz) |
| Windows, x86-64 | [turborel-nightly-x86_64-pc-windows-msvc.zip](https://github.com/turborel/turborel/releases/download/nightly/turborel-nightly-x86_64-pc-windows-msvc.zip) |

Each archive holds the two programs [What is installed](#what-is-installed)
describes. As an alpha, a build may not open a database another made
([VERSIONING.md](../../VERSIONING.md#alpha)).

## Building from source

Turborel is built with Rust's cargo, of Rust 1.89 or later:

```
git clone https://github.com/turborel/turborel.git
cd turborel
cargo build --release -p turboreld -p turborel-cli
```

The tests run with `cargo test --workspace`; `cargo xtask verify` runs
every check a change must pass.

## What is installed

Two programs, in `target/release`:

| Program | What it is |
|---|---|
| `turboreld` | the server: serves a database to many clients at once, and manages its users |
| `turborel` | the command-line client |

Each is one file, with nothing else to install: copy them where
programs are kept, as `/usr/local/bin`. Each prints its release:

```
$ turboreld --version
turborel 0.0.0 (alpha)
```

A program may instead embed the database: the crate `turborel` is the
database as a library, `turborel-client` the client library.
