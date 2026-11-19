# Full-Text Search

[Specialized Data](README.md)

A support desk keeps its tickets' text and needs the tickets that
mention a problem, whatever their wording's case or accents. This guide
searches them with TEXT_MATCHES *(extension)*, indexes them so a search
reads only the tickets it finds, and then searches them by pattern and
ranks them with CONTAINS and RANK of SQL/MM Full-Text. The
[reference](../sql/08-fulltext.md) gives every rule.

## The tickets

```sql
CREATE SCHEMA AUTHORIZATION ANN;
CREATE TABLE TICKETS (ID INTEGER PRIMARY KEY, SUBJECT VARCHAR(80), BODY VARCHAR(2000));
INSERT INTO TICKETS VALUES
    (1, 'Login fails', 'I can''t log in since the update. The password reset email never arrives.'),
    (2, 'Invoice wrong', 'The March invoice charges twice for the same order.'),
    (3, 'Résumé upload', 'Uploading a résumé as PDF fails with an error.'),
    (4, 'Password rules', 'Which characters may a password hold? The reset page does not say.'),
    (5, 'Refund', 'Please refund the second charge on my invoice.');
COMMIT;
```

```
OK
OK
OK
OK
```

## Searching

TEXT_MATCHES is true of a text that holds every word of the search, in
any order and anywhere in it:

```sql
SELECT ID, SUBJECT FROM TICKETS WHERE TEXT_MATCHES(BODY, 'password reset') ORDER BY ID;
```

```
           ID | SUBJECT
--------------+--------------------------------------------------------------
            1 | Login fails
            4 | Password rules
(2 rows)
```

Words are compared normalized, case folded and accents taken off, so
`RESUME` finds `résumé`:

```sql
SELECT ID FROM TICKETS WHERE TEXT_MATCHES(BODY, 'RESUME pdf');
```

```
           ID
--------------
            3
(1 row)
```

A word is matched as written, not by its stem, so `invoices` finds no
ticket that says `invoice`:

```sql
SELECT ID FROM TICKETS WHERE TEXT_MATCHES(BODY, 'invoice') ORDER BY ID;
SELECT ID FROM TICKETS WHERE TEXT_MATCHES(BODY, 'invoices');
```

```
           ID
--------------
            2
            5
(2 rows)
           ID
--------------
(0 rows)
```

Stop words, as `the` and `can't`, are left out of a search. A search of
stop words alone is true of every text:

```sql
SELECT ID, TEXT_MATCHES(BODY, 'can''t') AS MATCHED FROM TICKETS WHERE ID < 3 ORDER BY ID;
```

```
           ID | MATCHED
--------------+---------
            1 | TRUE
            2 | TRUE
(2 rows)
```

## Indexing

Without an index, a search reads every ticket. An index of the method
`fulltext` *(extension)* keeps each word's tickets, so a search reads those alone. It
is kept up to date as tickets change, and the planner uses it when it
costs less than reading the table, which these few tickets may not. An
index is made outside a transaction, so the searches' is ended first:

```sql
COMMIT;
CREATE INDEX TICKETS_BODY ON TICKETS (BODY) USING FULLTEXT;
SELECT OPTION_NAME, OPTION_VALUE, VALUE_SOURCE FROM INFORMATION_SCHEMA.INDEX_OPTIONS
    WHERE INDEX_NAME = 'TICKETS_BODY' ORDER BY OPTION_NAME;
COMMIT;
```

```
OK
OK
 OPTION_NAME                                                  | OPTION_VALUE                                                 | VALUE_SOURCE
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 FULLTEXT.B                                                   | 7.5E-1                                                       | DEFAULT
 FULLTEXT.K1                                                  | 1.2E0                                                        | DEFAULT
 FULLTEXT.LANGUAGE                                            | ENGLISH                                                      | DEFAULT
 FULLTEXT.RANKING                                             | BM25                                                         | DEFAULT
 FULLTEXT.STOP_WORDS                                          | DEFAULT                                                      | DEFAULT
(5 rows)
OK
```

The index finds the rows a search without it finds. Its options choose
how it reads text. A desk whose tickets often say `please` can leave it
out of the index with its own stop words. That builds the index again,
while the table stays open to others. A language the tokenizer does not
know is refused:

```sql
ALTER INDEX TICKETS_BODY SET (FULLTEXT.STOP_WORDS = 'please, the');
ALTER INDEX TICKETS_BODY SET (FULLTEXT.LANGUAGE = KLINGON);
```

```
OK
turborel: SQLCODE -103 (SQLSTATE 2A000): FULLTEXT.LANGUAGE is one of ENGLISH, NONE, not KLINGON
```

An index is of one character string or FULLTEXT column:

```sql
CREATE INDEX TICKETS_SUBJECT ON TICKETS (SUBJECT, BODY) USING FULLTEXT;
CREATE INDEX TICKETS_ID ON TICKETS (ID) USING FULLTEXT;
```

```
turborel: SQLCODE -900 (SQLSTATE 0A000): the fulltext index method indexes one character string or FULLTEXT column
turborel: SQLCODE -900 (SQLSTATE 0A000): the fulltext index method indexes one character string or FULLTEXT column
```

## Searching by pattern

TEXT_MATCHES asks for every word. CONTAINS reads a search pattern of
ISO/IEC 13249-2, in which words are written in double quotes and may be
stemmed, joined by `&`, `|` and NOT, or asked to stand near each other.
It gives 1 or 0. A stemmed word finds `invoice` and `invoices` alike,
and NEAR finds `password` a few words from `reset`, in either order:

```sql
SELECT ID, SUBJECT FROM TICKETS WHERE CONTAINS(BODY, 'STEMMED FORM OF "invoices"') = 1 ORDER BY ID;
SELECT ID FROM TICKETS WHERE CONTAINS(BODY, '"password reset" | "refund"') = 1 ORDER BY ID;
SELECT ID FROM TICKETS WHERE CONTAINS(BODY, '"password" NEAR "reset" WITHIN 3 WORDS ANY ORDER') = 1 ORDER BY ID;
```

```
           ID | SUBJECT
--------------+--------------------------------------------------------------
            2 | Invoice wrong
            5 | Refund
(2 rows)
           ID
--------------
            1
            5
(2 rows)
           ID
--------------
            1
            4
(2 rows)
```

RANK scores how well a ticket matches, by BM25 over the index's
statistics, so the desk can show the best few first. With the index,
such a query reads the tickets in rank order and stops after the first
rows:

```sql
SELECT ID, SUBJECT, RANK(BODY, '"invoice" | "charge%"') AS SCORE FROM TICKETS
    WHERE CONTAINS(BODY, '"invoice" | "charge%"') = 1
    ORDER BY RANK(BODY, '"invoice" | "charge%"') DESC FETCH FIRST 2 ROWS ONLY;
```

```
           ID | SUBJECT                                                      |                    SCORE
--------------+--------------------------------------------------------------+--------------------------
            5 | Refund                                                       |     2.6002217094092983E0
            2 | Invoice wrong                                                |      2.468305162708714E0
(2 rows)
```

The [pattern reference](../sql/09-fulltext-patterns.md) has every form
of a pattern, phrases and wild cards, sentences and paragraphs,
sound-alike and misspelt words, and thesauri. The
[reference](../sql/08-fulltext.md) has the FULLTEXT type, which keeps a
document's language with its text, and how ranking can be tuned.
