# HOLD LOCATOR

[Statements](README.md)

## NAME

HOLD LOCATOR: keep large object locators past the end of their transaction

## SYNOPSIS

```
<hold locator statement> ::= HOLD LOCATOR <locator reference> [ { , <locator reference> }... ]

<locator reference> ::= <dynamic parameter specification>
```

## DESCRIPTION

A locator is valid until its transaction ends. HOLD LOCATOR makes each
locator its parameters give holdable, so it stays valid past a commit,
until it is freed or a rollback ends it, or the session ends. A held
locator of a value a table keeps holds a copy of it in the database's
temporary store, so a later change to the row or its collection leaves
the value as it was. Each parameter is an INTEGER.

The client library holds a locator by `hold_lob`, and an embedded
program by `Session::hold_locator`.

## RULES

1. Each locator reference is a dynamic parameter.
2. Each locator is valid.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 37000 | -103 | A locator reference is not a dynamic parameter, as PREPARE finds (rule 1) |
| 0F001 | -301 | A locator is not valid: freed, of a transaction that ended, or never given (rule 2) |

## EXAMPLES

In a program, of a locator `l` a session gave for a value it read:

```rust
let holding = s.prepare("HOLD LOCATOR ?")?;
s.execute_prepared(&holding, &[Value::Exact(Decimal::new(l.into(), 0))])?;
s.run("DELETE FROM NOTES")?;
s.commit()?;
s.read_lob(l, (0, 100), true)?;   // the value's first 100 characters, the row gone
```

## STANDARD

ISO/IEC 9075-2:2023 §14.18, Feature T561. A locator reference is an
embedded or host parameter there; turborel has no host language, so it
is a dynamic parameter.

## SEE ALSO

[FREE LOCATOR](free-locator.md), [COMMIT](commit.md), [Large objects](../02-data-types.md#large-objects)
