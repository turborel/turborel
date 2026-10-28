# The turborel Documentation Set

turborel is a relational database system: a library a program embeds, a
server, `turboreld`, that serves its databases over a network, and a
command-line client, `turborel`. This set of manuals describes them for
those who use, run and connect to them.

turborel speaks the SQL of the international standards. Today that is
ANSI X3.135-1986 ("SQL-86") and ISO/IEC 9075:1992 ("SQL-92"), read as one
language, the newer prevailing where they differ. Later editions of the
standard are added to the same language as they are implemented, through
SQL:2023. What is implemented, clause by clause, is in
[CONFORMANCE.md](../CONFORMANCE.md).

turborel is in alpha. These manuals describe what is settled; a chapter
not yet written says so.

## The volumes

| Volume | For | |
|---|---|---|
| [I. User's Guide](user/README.md) | Anyone who writes SQL against turborel | Concepts, and how to use them, chapter by chapter |
| [II. SQL Reference Manual](sql/README.md) | The same, looking something up | The language element by element; each statement on a page of its own |
| [III. Operator's Guide](operator/README.md) | Whoever installs and runs a server | Installing, running `turboreld`, databases, settings, users, connections |
| [IV. Client Reference](client/README.md) | Users of the client, and programs that connect | The `turborel` command, and the client library |
| [Appendices](appendix/README.md) | Everyone | SQLSTATEs, key words, settings, limits, implementation-defined choices, versions, a glossary |

Read the User's Guide from the start to learn turborel; turn to the
Reference Manual and the appendices to look a thing up.

## Conventions

**Syntax** is written in the notation of the SQL standard:

| Notation | Means |
|---|---|
| `<name>` | A syntactic element, defined elsewhere in the syntax |
| `::=` | Is defined as |
| `[ x ]` | `x` may be written, or left out |
| `{ x }` | `x`, grouped |
| `x \| y` | Either `x` or `y` |
| `x...` | `x`, once or more |
| `KEY WORD` | Written as shown, in any case |

**Examples** of SQL are set in a fixed-width font. Key words are in upper
case, names in upper case unless delimited, as the standard folds them:

```sql
SELECT NAME, SALARY FROM STAFF WHERE SALARY > 50000;
```

**Citations** name the clause of a standard a rule comes from:
[ISO/IEC 9075:1992 §11.3] is clause 11.3 of SQL-92; [X3.135-1986 §8.7]
is clause 8.7 of SQL-86. A rule is cited further as Syntax Rule (SR),
General Rule (GR) or Access Rule (AR) and its number: §11.9 AR1 is Access
Rule 1 of clause 11.9. turborel's own error messages cite rules the same
way, so a message can be looked up.

**Extensions**, facilities the standards do not define, are marked
*(extension)* where they are described.

**Errors** are given by SQLSTATE, the code the standard defines, and
SQLCODE, the older number; [Appendix A](appendix/a-sqlstate.md) lists
every one.

## Pages of reference

A statement's reference page has these sections, in this order:

- **NAME**: the statement, and what it does in a line.
- **SYNOPSIS**: its syntax.
- **DESCRIPTION**: what it does.
- **RULES**: what must hold for it to run, and what it does in each case.
- **PRIVILEGES**: what its user must hold.
- **ERRORS**: each way it fails, by SQLSTATE.
- **EXAMPLES**: statements, and what they give.
- **STANDARD**: the clauses it is defined by, and what is an extension.
- **SEE ALSO**: related pages.
