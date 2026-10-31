# DEALLOCATE DESCRIPTOR

[Statements](README.md)

## NAME

DEALLOCATE DESCRIPTOR: deallocate an SQL descriptor area

## SYNOPSIS

```
<deallocate descriptor statement> ::= DEALLOCATE DESCRIPTOR <descriptor name>
```

The `<descriptor name>` is [ALLOCATE DESCRIPTOR](allocate-descriptor.md)'s.

## DESCRIPTION

DEALLOCATE DESCRIPTOR deallocates an SQL descriptor area: what it held
is let go, and its name may be allocated again. The session's areas are
deallocated as it ends.

DEALLOCATE DESCRIPTOR runs in direct SQL as well as in a program. It
begins a transaction if none is active.

## RULES

1. An area of the name and scope is allocated.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 33000 | -921 | No area of the name is allocated, or the name is not an identifier (rule 1) |

## EXAMPLES

Of the areas [ALLOCATE DESCRIPTOR](allocate-descriptor.md) allocates:

```sql
DEALLOCATE DESCRIPTOR 'STAFF_ROW';
DEALLOCATE DESCRIPTOR 'STAFF_ROW';
DEALLOCATE DESCRIPTOR GLOBAL 'STAFF_ROW';
COMMIT;
```

```
OK
turborel: SQLCODE -921 (SQLSTATE 33000): no SQL descriptor area 'STAFF_ROW' is allocated [ISO/IEC 9075:1992 §17.3 GR1]
OK
OK
```

## STANDARD

ISO/IEC 9075:1992 §17.3. SQL-92 makes it no `<directly executable
statement>` [§20.1]: direct SQL takes it as a statement of this
implementation's. SQL-86 has no dynamic SQL.

## SEE ALSO

[ALLOCATE DESCRIPTOR](allocate-descriptor.md)
