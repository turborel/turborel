# Debugging Routines

[Contents](README.md)

*(extension)* A procedure, a function or a trigger written in SQL can be
run under a debugger. The debugger is another connection to the same
server. It attaches to the session that runs the routine, sets
breakpoints at lines of the routine's text, and, each time that session
pauses, reads its call stack, each frame's parameters, variables and
cursors, and tells it how to go on. Studio drives it this way, and so
can the `turborel` command and any program on the client library.

## Who may debug what

A connection attaches to the session of another connection of its own
user. The database's owner, and a user granted the role MONITOR, attach
to any connection's session, as they see every connection in
INFORMATION_SCHEMA.CONNECTIONS.

Attaching shows nothing by itself. The debugger sets breakpoints in,
pauses in and sees the variables of only the routines its user owns or
holds DEBUG on ([GRANT DEBUG](../sql/statements/grant-debug.md)). A
frame of any other routine is shown by its name alone.

## Two sessions

The session to debug tells its connection's ID with `\debug id`. Here
ANN's first session defines a procedure and learns its ID.

```sql
CREATE TABLE STAFF (ID INTEGER PRIMARY KEY, SALARY INTEGER);
INSERT INTO STAFF VALUES (1, 50000), (2, 60000);
CREATE PROCEDURE RAISE_PAY(IN PCT INTEGER) MODIFIES SQL DATA
BEGIN
  DECLARE N INTEGER DEFAULT 0;
  UPDATE STAFF SET SALARY = SALARY * (100 + PCT) / 100;
  SET N = (SELECT COUNT(*) FROM STAFF);
END;
COMMIT;
\debug id
```

```
OK
OK
OK
OK
This is connection 2.
```

ANN's second session attaches to it and sets a breakpoint at line 4 of
the procedure's text, its UPDATE. The breakpoint names the procedure by
its specific name, RAISE_PAY_1 here, as a routine's name may be
overloaded.

```sql
\debug attach 2
\debug break procedure RAISE_PAY 4
```

```
Attached to connection 2.
Breakpoint 1 at ANN.RAISE_PAY_1 line 4.
```

The first session calls the procedure. The session pauses as it comes
to line 4, and its CALL completes only once the debugger lets it go
on.

```sql
CALL RAISE_PAY(10);
```

```
OK
```

The debugger waits for the pause, at most 10 seconds here, and is shown
the call stack, the innermost frame first.

```sql
\debug wait 10
```

```
Paused at breakpoint 1.
#1 PROCEDURE ANN.RAISE_PAY line 4, column 3
    PCT INTEGER = 10
    N INTEGER = 0
```

`\debug step over` runs the UPDATE and pauses at the next statement.

```sql
\debug step over
\debug wait 10
```

```
Going on.
Paused.
#1 PROCEDURE ANN.RAISE_PAY line 5, column 3
    PCT INTEGER = 10
    N INTEGER = 0
```

`\debug continue` lets the session run to its next breakpoint, or here
to the end of its CALL, which completes in the first session. The
debugger detaches.

```sql
\debug continue
\debug detach
```

```
Going on.
Detached.
```

## Stepping

`\debug step` or `\debug step into` pauses at the next statement,
within a routine the statement invokes too. `\debug step over` pauses
at the next statement of the same frame, or of the one it returns to.
`\debug step out` pauses at the next statement of the frame this one
returns to. `\debug continue` runs to a breakpoint, and `\debug
interrupt` pauses a running session at its next statement.

A breakpoint may carry a condition over the frame's parameters and
variables, the rest of its line, and pauses only where it is true.

```
\debug break procedure RAISE_PAY 5 PCT > 5
```

`\debug breakpoints` lists the breakpoints, `\debug clear N` clears one,
and `\debug stack` shows the paused session's frames again.

## When a pause ends by itself

A paused session holds what it holds, its locks among them. It waits
for its debugger at most DEBUGGER.PAUSE_TIMEOUT_MS, five minutes unless
set ([Settings](../appendix/c-settings.md)), and then goes on by itself,
its debugger told it timed out. A cancel of its statement ends a pause
too. A debugger that detaches, or whose connection ends, lets its
session go on as if never debugged. A session that ends detaches its
debugger, which is told so.

## In a program

The client library's connection has a call for each command:
`debug_id`, `debug_attach`, `debug_break`, `debug_clear`,
`debug_breakpoints`, `debug_resume`, `debug_interrupt`, `debug_stack`,
`debug_wait` and `debug_detach`
([The Client Library](../client/02-library.md)). The messages they send
are in [The Native Protocol](../client/05-protocol.md).
