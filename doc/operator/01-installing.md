# Installing

[Contents](README.md)

## Platforms

turborel runs on Linux, macOS and Windows: each is built and tested on
every change. What only one platform has is refused, saying so,
elsewhere: Unix sockets are of Linux and macOS alone.

## Building from source

turborel is built with Rust's cargo, of Rust 1.89 or later:

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
