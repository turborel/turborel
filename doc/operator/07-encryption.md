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
