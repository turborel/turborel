# FREE LOCATOR

[Statements](README.md)

## NAME

FREE LOCATOR: end large object locators before their transaction does

## SYNOPSIS

```
<free locator statement> ::= FREE LOCATOR <locator reference> [ { , <locator reference> }... ]

<locator reference> ::= <dynamic parameter specification>
```

## DESCRIPTION

A locator is the four-octet integer a session gives a client for a
large object value, sent in a result's row or made by the client a piece
at a time. It is valid until its transaction ends. FREE LOCATOR ends
each locator its parameters give at once, so the session no longer
keeps its value. Each parameter is an INTEGER.

The client library frees a locator by `free_lob`, and an embedded
program by `Session::free_locator`.

## RULES

1. Each locator reference is a dynamic parameter.
2. Each locator is valid.

## PRIVILEGES

None.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 42000 | -103 | A locator reference is not a dynamic parameter, as PREPARE finds (rule 1) |
| 0F001 | -301 | A locator is not valid: freed, of a transaction that ended, or never given (rule 2) |

## EXAMPLES

In a program, of a locator `l` a session gave for a value it read:

```rust
let freeing = s.prepare("FREE LOCATOR ?")?;
s.execute_prepared(&freeing, &[Value::Exact(Decimal::new(l.into(), 0))])?;
s.read_lob(l, (0, 100), true)?;   // SQLSTATE 0F001: the locator is freed
s.execute_prepared(&freeing, &[Value::Exact(Decimal::new(l.into(), 0))])?;   // 0F001 too
```

## STANDARD

ISO/IEC 9075-2:2023 §14.17, Features T039 and T049. A locator reference
is an embedded or host parameter there; turborel has no host language,
so it is a dynamic parameter.

## SEE ALSO

[HOLD LOCATOR](hold-locator.md), [Large objects](../02-data-types.md#large-objects)
