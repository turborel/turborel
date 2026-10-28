# ALTER INDEX

[Statements](README.md)

## NAME

ALTER INDEX: change an index's options *(extension)*

## SYNOPSIS

```
<alter index statement> ::=
    ALTER INDEX <index name>
    SET ( <index option> [ { , <index option> }... ] )

<index option> ::= <option name> = { <option value> | DEFAULT }
```

## DESCRIPTION

ALTER INDEX sets options of an index, DEFAULT an option's default. An
option its method takes in place changes the index at once; any other
builds the index again with it, while other transactions go on reading
and writing the table, and then puts it in the old one's place. It is
made at once, apart from any transaction: no ROLLBACK undoes it.

The options are those the index's method declares and those plug-ins
declare for every index; the built-in methods, `btree` and `hash`,
declare none. INFORMATION_SCHEMA.INDEX_OPTIONS shows an index's options.

## RULES

1. The session has no transaction open: COMMIT or ROLLBACK first.
2. The index exists, and its user owns the schema of its table.
3. Each option is declared, given once, of a value it accepts.

## PRIVILEGES

The schema of the index's table is its user's.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 25000 | -917 | The session has a transaction open |
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the schema of the index's table |

## EXAMPLES

Of the index STAFF_NAME of [CREATE INDEX](create-index.md), of the
method `hash`, which declares no options:

```sql
ALTER INDEX STAFF_NAME SET (FILL = 50);
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): hash takes no option FILL (at character 28)
```

## STANDARD

An extension: neither SQL-86 nor SQL-92 has indexes.

## SEE ALSO

[CREATE INDEX](create-index.md), [DROP INDEX](drop-index.md)
