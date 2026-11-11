# CREATE COLLATION

[Statements](README.md)

## NAME

CREATE COLLATION: define a collation

## SYNOPSIS

```
<collation definition> ::=
    CREATE COLLATION <collation name> FOR SQL_TEXT
    FROM <existing collation name> [ <pad characteristic> ]

<pad characteristic> ::= NO PAD | PAD SPACE

<collate clause> ::= COLLATE <collation name>
```

## DESCRIPTION

A collation says how character strings compare. A column takes one by its
`<collate clause>`, a domain likewise, and an expression, ORDER BY or GROUP
BY names one for itself. These are built in.

| Collation | Order | Pad |
|---|---|---|
| SQL_TEXT, UCS_BASIC | Unicode scalar value, so `Z` before `a` | PAD SPACE |
| UNICODE | Language-neutral, by base letter, then accent, then case | NO PAD |
| UNICODE_SV, UNICODE_DE_PHONEBOOK, ... | UNICODE as a language orders it | NO PAD |
| CASE_INSENSITIVE | Scalar value of each character's case folding | PAD SPACE |

A language's collation is UNICODE with that language's letters where its
dictionaries put them. UNICODE_SV sorts `å`, `ä` and `ö` after `z`, UNICODE_ES
puts `ñ` after `n`, and UNICODE_DE_PHONEBOOK sorts `ü` as `ue`. The order comes
from CLDR 38, the Unicode Consortium's locale data. There is one for each of
these, by its CLDR locale.

| Collations | Languages |
|---|---|
| UNICODE_DA, UNICODE_FI, UNICODE_FO, UNICODE_IS, UNICODE_NB, UNICODE_NN, UNICODE_SE, UNICODE_SMN, UNICODE_SV | Danish, Finnish, Faroese, Icelandic, Norwegian Bokmål and Nynorsk, Northern and Inari Sami, Swedish |
| UNICODE_DE_PHONEBOOK, UNICODE_ES, UNICODE_ES_TRADITIONAL, UNICODE_FR_CA, UNICODE_GL | German phone book, Spanish, traditional Spanish (`ch` and `ll` as letters), Canadian French (accents compared from the end), Galician |
| UNICODE_CS, UNICODE_DSB, UNICODE_HSB, UNICODE_HU, UNICODE_LT, UNICODE_LV, UNICODE_PL, UNICODE_RO, UNICODE_SK, UNICODE_SL, UNICODE_SQ | Czech, Lower and Upper Sorbian, Hungarian, Lithuanian, Latvian, Polish, Romanian, Slovak, Slovenian, Albanian |
| UNICODE_AF, UNICODE_BR, UNICODE_CY, UNICODE_EO, UNICODE_MT, UNICODE_TR, UNICODE_UZ, UNICODE_TK, UNICODE_WAE | Afrikaans, Breton, Welsh, Esperanto, Maltese, Turkish, Uzbek, Turkmen, Walser |
| UNICODE_CEB, UNICODE_FIL, UNICODE_HAW, UNICODE_KL, UNICODE_LKT, UNICODE_TO, UNICODE_VI | Cebuano, Filipino, Hawaiian, Greenlandic, Lakota, Tongan, Vietnamese |
| UNICODE_EE, UNICODE_HA, UNICODE_IG, UNICODE_LN, UNICODE_OM, UNICODE_WO, UNICODE_YO | Ewe, Hausa, Igbo, Lingala, Oromo, Wolof, Yoruba |

A language not listed sorts by UNICODE. For some, such as Russian and Greek,
that is already their order. Others, such as Croatian, Hindi and Chinese, have
letters UNICODE puts elsewhere.

CREATE COLLATION gives one of them another name and pad characteristic. Under
PAD SPACE a shorter string compares as if extended with spaces. Under NO PAD
it compares as a prefix, so `'a'` is less than `'a '`.

A literal, parameter or CAST takes the collation of the column it is compared
with. Two columns of different collations need COLLATE. A foreign key's
columns have the collation of those they reference.

UNIQUE and indexes on a column hold it by its collation, so `Bob` and `bob`
are duplicates under CASE_INSENSITIVE. An index of a collation serves
comparisons by that collation. Its keys are longer than the text, about 16
bytes for each character under UNICODE, which limits an indexed column's
length ([Appendix D](../../appendix/d-limits.md)).

## RULES

1. No collation of its name exists in its schema.
2. The collation it comes from exists. A name of no schema is a built-in.
3. It is for SQL_TEXT, the one character set.

## PRIVILEGES

Its schema's owner, holding USAGE on the collation it comes from. Built-ins
are everyone's. COLLATE needs USAGE, which GRANT gives.

## ERRORS

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | One of its name exists, or no collation of the name it comes from |
| 2A000 | -104 | Its user does not own the schema, or lacks USAGE |
| 0A000 | -900 | It is for another character set, or comes from SQL-92's EXTERNAL, DESC, DEFAULT or TRANSLATION |

## EXAMPLES

```sql
CREATE TABLE PEOPLE (NAME VARCHAR(20) COLLATE CASE_INSENSITIVE UNIQUE);
INSERT INTO PEOPLE VALUES ('Bob'), ('Ann');
INSERT INTO PEOPLE VALUES ('bob');
SELECT NAME FROM PEOPLE WHERE NAME = 'BOB';
CREATE COLLATION WITH_PAD FOR SQL_TEXT FROM UNICODE PAD SPACE;
CREATE TABLE WORDS (W VARCHAR(20) COLLATE ANN.WITH_PAD);
INSERT INTO WORDS VALUES ('résumé  '), ('Zebra'), ('apple');
SELECT W FROM WORDS ORDER BY W;
SELECT COUNT(*) FROM WORDS WHERE W = 'apple   ';
CREATE COLLATION WITH_PAD FOR SQL_TEXT FROM UNICODE;
CREATE COLLATION OTHER FOR SQL_TEXT FROM NOPE;
```

```
OK
OK
turborel: SQLCODE -201 (SQLSTATE 23000): UNIQUE ANN.PEOPLE_UNIQUE1 (NAME) of ANN.PEOPLE would be violated: a row of these values is there [X3.135-1986 §6.4 GR2]
 NAME
----------------------
 Bob
(1 row)
OK
OK
OK
 W
----------------------
 apple
 résumé
 Zebra
(3 rows)

----------------------
                    1
(1 row)
turborel: SQLCODE -103 (SQLSTATE 2A000): collation ANN.WITH_PAD exists [ISO/IEC 9075-2:2023 §11.43 SR3] (at character 17)
turborel: SQLCODE -103 (SQLSTATE 2A000): no collation INFORMATION_SCHEMA.NOPE [ISO/IEC 9075-2:2023 §10.7 SR1] (at character 41)
```

## STANDARD

ISO/IEC 9075-2:2023 §4.3.6, §10.7, §11.43, Features F690 and F692. See
[CONFORMANCE.md](../../../CONFORMANCE.md#collations).

## SEE ALSO

[DROP COLLATION](drop-collation.md), [SET COLLATION](set-collation.md),
[CREATE TABLE](create-table.md), [GRANT](grant.md)
