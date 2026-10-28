# Errors and Diagnostics

[Contents](README.md)

## SQLCODE and SQLSTATE

Every statement completes with a SQLCODE and an SQLSTATE. The client
shows a warning or an error with both, what is wrong, and, in brackets,
the rule of the standard that says so:

```
turborel: SQLCODE -103 (SQLSTATE 2A000): table ANN.NOPE is not defined [X3.135-1986 §5.4 SR5] (at character 14)
```

The SQLCODE says the kind: -101 to -104 a statement not written right,
-201 a constraint broken, -301 a value that does not fit, -911 a
conflict with another transaction, -900 valid SQL turborel does not
process. The SQLSTATE is the standard's code of the condition.
[SQLSTATE and SQLCODE](../appendix/a-sqlstate.md) lists them all.

A statement that fails is undone; the transaction goes on, unless the
failure rolls it back, as a COMMIT refused does (SQLSTATE class 40).

## Where an error is placed

An error of a statement's text gives where in it the error is: `(at
character 14)` counts from 0 at the statement's first character, so
the 15th is the one meant: NOPE, here, of `SELECT * FROM NOPE`.

## GET DIAGNOSTICS

A program reads a statement's outcome, its SQLSTATE, the message, the
constraint broken, by [GET DIAGNOSTICS](../sql/statements/get-diagnostics.md).
