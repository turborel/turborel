# DROP INDEX

[Statements](README.md)

## NAME

DROP INDEX: drop an index *(extension)*

## SYNOPSIS

```
<drop index statement> ::= DROP INDEX <index name>
```

## DESCRIPTION

DROP INDEX drops an index made by [CREATE INDEX](create-index.md). No
row changes, nor any result; queries find their rows by other means.
It is made at once, apart from any transaction: no ROLLBACK undoes it.

## RULES

1. The session has no transaction open: COMMIT or ROLLBACK first.
2. The index exists, and its user owns the schema of its table.
3. The index is not a UNIQUE or PRIMARY KEY constraint's: that index goes
   when the constraint is dropped, by [ALTER TABLE](alter-table.md).

## PRIVILEGES

The schema of the index's table is its user's.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 25000 | -917 | The session has a transaction open |
| 2A000 | -103 | A rule above is broken |
| 2A000 | -104 | Its user does not own the schema of the index's table |

## EXAMPLES

Of the indexes of [CREATE INDEX](create-index.md):

```sql
DROP INDEX STAFF_DEPT;
DROP INDEX NOPE;
DROP INDEX "ANN.STAFF.UNIQUE1";
```

```
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): there is no index NOPE (at character 11)
turborel: SQLCODE -103 (SQLSTATE 2A000): index ANN.STAFF.UNIQUE1 is a UNIQUE constraint's: drop the constraint (at character 11)
```

## STANDARD

An extension: neither SQL-86 nor SQL-92 has indexes.

## SEE ALSO

[CREATE INDEX](create-index.md), [ALTER INDEX](alter-index.md)
