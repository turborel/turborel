# Lexical Elements

[Contents](README.md)

## Characters

SQL text is Unicode. Its characters are those of the one character set,
SQL_TEXT, which holds every Unicode character; a character string holds
any of them. A line ends with a newline, `\n` or `\r\n`.

Key words, operators and punctuation are written in the characters of
ASCII; identifiers and string literals may hold any character.

## Tokens and separators

SQL text is a sequence of tokens: key words, identifiers, literals,
operators and punctuation. A separator, one or more spaces, newlines or
comments, goes between two tokens that would otherwise run together:
a key word, an identifier or a number is followed by a separator or by
a delimiter, as `(`, `,`, `'` or an operator.

A tab is not a separator: SQL-92's separators are the space character,
newlines and comments alone.

```sql
VALUES	1;
VALUES 1abc;
VALUES 1 @;
```

```
turborel: SQLCODE -101 (SQLSTATE 2A000): "VALUES" must be followed by a <delimiter token> or a <separator>, not '\t' [X3.135-1986 §5.3 SR2] (at character 6)
turborel: SQLCODE -101 (SQLSTATE 2A000): "1" must be followed by a <delimiter token> or a <separator>, not 'a' [X3.135-1986 §5.3 SR2] (at character 8)
turborel: SQLCODE -101 (SQLSTATE 2A000): character '@' cannot start a <token> [X3.135-1986 §5.3 F] (at character 9)
```

The operators and delimiters are `( ) , . ; ' " + - * / = < > <= >= <>
|| ?` and `:`. Not equal is written `<>`: `!=` is not SQL.

## Identifiers

An identifier names a table, a column, a schema or another object.

A **regular identifier** is a letter, then letters, digits and
underscores, at most 128 of them. It is taken in upper case, whatever
case it is written in: `staff`, `Staff` and `STAFF` are one name,
STAFF. A key word is not a regular identifier.

A **delimited identifier** is written in double quotes, a double quote
in it written twice. It is taken as written, its case kept, and may be
a key word or hold any character: `"select"`,
`"Mixed Case"`. A delimited identifier of upper case letters, digits and
underscores is the regular one: `"STAFF"` is STAFF.

```sql
CREATE TABLE "select" ("Mixed Case" INTEGER, abc INTEGER);
SELECT COLUMN_NAME FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'select' ORDER BY 1;
CREATE TABLE value (K INTEGER);
```

```
OK
 COLUMN_NAME
--------------------------------------------------------------
 ABC
 Mixed Case
(2 rows)
turborel: SQLCODE -102 (SQLSTATE 2A000): expected an <identifier>, found VALUE (at character 13)
```

A name of an object in a schema may be qualified by the schema's,
`ANN.STAFF`, and a schema's by the catalog's; a column's by its
table's, or a correlation name's, `S.NAME`. See [Names](03-names.md).

## Literals

| Literal | Written | Of type |
|---|---|---|
| Character string | `'it''s'`, `''` | CHARACTER of its length |
| National character string | `N'text'` | CHARACTER of its length |
| Bit string | `B'1010'` | BIT of its length |
| Hexadecimal string | `X'1F'` | BIT of four bits a digit |
| Exact number | `42`, `-3.75`, `.5`, `5.` | NUMERIC of its digits |
| Approximate number | `1.5E3`, `-2E-1` | FLOAT(53), of DOUBLE PRECISION's precision |
| Date | `DATE '2026-10-28'` | DATE |
| Time | `TIME '13:45:30.25'`, `TIME '13:45:30+02:00'` | TIME of its fraction's digits |
| Timestamp | `TIMESTAMP '2026-10-28 13:45:30'` | TIMESTAMP of its fraction's digits |
| Interval | `INTERVAL '1-6' YEAR TO MONTH`, `INTERVAL -'3' DAY` | INTERVAL of its qualifier |

A quote in a character string is written twice. A string may be written
in parts, each quoted, separated by a separator holding a newline: the
parts are one string. A character string may begin with an introducer
naming its character set, `_SQL_TEXT'text'`, of SQL_TEXT alone. The
letters B, X, N and E may be written in either case.

An exact number holds at most 18 digits; an approximate number is
within the range of DOUBLE PRECISION. A datetime literal is a valid
date or time:

```sql
VALUES ('ab'
  'cd', 'it''s', '', N'n', B'101', X'1F', 1.5E3, .5, 5.);
VALUES 'ab' 'cd';
VALUES 123456789012345678901;
VALUES 1E400;
VALUES TIME '25:00:00';
```

```
      |      |  |   |     |          |                          |     |
------+------+--+---+-----+----------+--------------------------+-----+-----
 abcd | it's |  | n | 101 | 00011111 |                    1.5E3 | 0.5 |   5
(1 row)
turborel: SQLCODE -102 (SQLSTATE 2A000): expected end of statement, found 'cd' (at character 12)
turborel: SQLCODE -900 (SQLSTATE 0A000): <exact numeric literal> 123456789012345678901 has 21 digits; this implementation's maximum exact precision is 18 [X3.135-1986 §5.2 SR4] (at character 7)
turborel: SQLCODE -900 (SQLSTATE 0A000): 1E400 is outside the range of this implementation's approximate numbers [X3.135-1986 §5.2 SR5] (at character 7)
turborel: SQLCODE -103 (SQLSTATE 2A000): TIME '25:00:00' is no valid <datetime literal> [ISO/IEC 9075:1992 §5.3 SR21, SR22, SR25] (at character 8)
```

The parts of `'ab' 'cd'` on one line are two strings, not one.

## Comments

A comment begins with two minus signs and runs to the end of the line:

```sql
SELECT NAME   -- the name alone
  FROM STAFF;
```

A comment is a separator. It needs the newline that ends it; the client
ends one that the end of its input cuts short. SQL-92 has no `/* */`
comments.

## Key words

The key words are SQL-92's reserved words, in any case: `select` is
SELECT. None is an identifier, so a column named as one, VALUE or
POSITION, is written delimited, `"VALUE"`. A few words are key words
only where a statement writes them, and identifiers everywhere else:
SQL-92's non-reserved words that turborel reads, as SERIALIZABLE and
COMMITTED, and INDEX, PASSWORD and DATABASE, of turborel's extensions.
`SET DATABASE` followed by a setting's name is
[SET DATABASE](statements/set-database.md); otherwise, as in
`SET DATABASE = 1`, DATABASE is the name of the setting
[SET](statements/set-setting.md) sets.
[Key Words](../appendix/b-key-words.md) lists them all.

The standard: ISO/IEC 9075:1992 §5.1–§5.3; X3.135-1986 §5.1–§5.3.
