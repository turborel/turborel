# Appendix D: Limits

[Contents](README.md)

SQL sets few limits of its own. A statement past one of these is valid
SQL turborel does not process: it is refused with SQLCODE -900,
SQLSTATE 0A000.

## Lengths and precisions

| What | Limit |
|---|---|
| An identifier | 128 characters |
| CHARACTER and CHARACTER VARYING | 32,767 characters |
| BIT and BIT VARYING | 262,136 bits (32,767 octets) |
| NUMERIC, DECIMAL and an exact literal | 18 digits |
| FLOAT | 53 binary digits; an approximate literal within the range of DOUBLE PRECISION |
| A datetime's fraction of a second | 6 digits |
| An interval's leading field | 9 digits; its fraction of a second 6 |
| An unsigned integer, as a length | 4,294,967,295 |

## Nesting

A statement is at most 64 levels deep as it is read: about 30 nested
parentheses, 60 `AND`, `OR`, `+` or `UNION` in a row, or 60 tables in a
FROM clause. A deeper one is refused rather than run out of stack.

## Sizes

| What | Limit |
|---|---|
| An index's key | of the default 8 KiB page, 2,020 bytes in a B-link tree, 8,136 in a hash index; of a 4 KiB page, 996 in a B-link tree; of a 64 KiB page, 16,356 in a B-link tree, 65,480 in a hash index |
| A row | none: its long values are kept out of line |
| A message to the server | SERVER.MAX_MESSAGE_BYTES, 16 MiB by default |
| Connections at once | SERVER.MAX_CONNECTIONS, 256 by default |
| Prepared statements of a connection | SERVER.MAX_STATEMENTS, 1,024 by default |

An index's key is compared whole in its page, so it is limited where a
row is not; STORAGE.PAGE_BYTES, chosen as a database is made, raises the
limit.
