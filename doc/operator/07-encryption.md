# Encryption

[Contents](README.md)

## Keys

An encryption key is 32 bytes, written as 64 hexadecimal digits in a
file of the key directory `--keys` names, the file's name the key's:

```
$ mkdir keys && openssl rand -hex 32 > keys/MASTER
$ turboreld --dir db --keys keys --set STORAGE.ENCODINGS=LZ4,AES-256-GCM:MASTER
```

STORAGE.ENCODINGS, set as a database is made, passes every file through
its encodings in order: here LZ4 compression, then AES-256-GCM
encryption by the key MASTER. The database's data, log and catalog hold
no plaintext. A database encrypted is opened only with its keys:

```
turboreld: cannot open db: SQLCODE -910 (SQLSTATE TB910): the keyring (none: give turboreld --keys DIR) has no key MASTER
```

Keep the keys apart from the database, and safe: without them, the
database cannot be read by anyone, its operator included.

While anything of the database is encrypted, its files or a table, the
diagnostics tell no statement's text: a slow statement is told by who
ran it and its time alone ([Diagnostics](08-diagnostics.md)).

## Ciphers

Two encryptions are served, each named with its key, wherever an
encoding is named:

| Encoding | Where it is fast |
|---|---|
| `AES-256-GCM:KEY` | on processors with AES instructions, as most servers' are |
| `ChaCha20-Poly1305:KEY` | in software, on processors without them |

Each authenticates what it encrypts, and where it is kept: data
changed, moved or encrypted by another key is refused as damaged
(SQLCODE -916). One key may serve both: each derives keys of its own
from it.

## Table encodings

A table's rows may pass through encodings of their own, compressed or
encrypted, set as the table is made or altered:

```sql
CREATE TABLE SECRET (S VARCHAR(40)) WITH (ENCODINGS = 'AES-256-GCM:MASTER');
CREATE TABLE PACKED (S VARCHAR(40)) WITH (ENCODINGS = 'LZ4');
ALTER TABLE PACKED SET (ENCODINGS = 'LZ4,AES-256-GCM:MASTER');
```

A key not in the key directory is refused. An encrypted table is indexed
only where STORAGE.ENCODINGS encrypts the files too, as an index's keys
are its values.

## Retiring a key

A change of a table's ENCODINGS is made in place: rows written after
pass through the new encodings, while rows written before keep theirs,
and their key. To retire a key, name another key in the table's
ENCODINGS:

```sql
ALTER TABLE S.SECRET SET (ENCODINGS = 'ChaCha20-Poly1305:SPARE');
COMMIT;
```

then, the server stopped, seal the table's rows again:

```
$ turboreld --dir db --keys keys reseal S.SECRET
S.SECRET sealed again
```

`reseal` copies the table's rows anew through the encodings it names
now, makes its indexes anew, commits, and closes the database, which
checkpoints it. Once each table that named the old key is sealed again,
its file may leave the key directory. A key STORAGE.ENCODINGS names
encrypts every file, and is kept for the database's life.

## Checksums

Every block and page of the database's files carries a checksum, of the
hash function STORAGE.CHECKSUM names as the database is made: by
default CRC-32/ISCSI, which finds damage, not a forger. Two
cryptographic ones are served:

| STORAGE.CHECKSUM | What it stops |
|---|---|
| `SHA-256` | damage, and changes made without the checksums written again |
| `HMAC-SHA-256` | a forger without its key, `--checksum-key NAME` of `--keys` |

```
$ turboreld --dir db --keys keys --checksum-key MASTER --set STORAGE.CHECKSUM=HMAC-SHA-256
```

A database checksummed with HMAC-SHA-256 is opened only with
`--checksum-key` naming the same key. Without one, it is refused, its
hash function not given (SQLCODE -910, `checksummed with hash function
12, which is not given`); with another key, its files fail their
checksums, and it is refused as damaged (SQLCODE -916). An encryption
authenticates what it encrypts already: the checksum matters most to
files not encrypted.
