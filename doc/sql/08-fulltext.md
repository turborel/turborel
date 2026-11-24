# Full-Text Search

[Contents](README.md)

Turborel has the full-text search of SQL/MM Full-Text, ISO/IEC
13249-2:2002 (CD, SC32 N0640), built in. A FULLTEXT value is a
document's text and its language. Its Contains method tells whether the
document matches a search pattern, and its Rank method scores how well.
The pattern language has words, phrases, wild cards, stems, sound-alike
and misspelt words, proximity, sentences and paragraphs, and thesauri,
each on [Full-Text Patterns](09-fulltext-patterns.md). An index of the
method `fulltext` *(extension)* finds the rows a search matches, and the best ranked
of them, without reading the table.

TEXT_MATCHES *(extension)* finds the rows whose text holds every word
of a search. For an introduction with complete
examples, read the [Full-Text Guide](../data/01-fulltext.md).

The examples on this page are run in order, by the user ANN, on a table
of articles:

```sql
CREATE SCHEMA AUTHORIZATION ANN;
CREATE TABLE ARTICLES (ID INTEGER PRIMARY KEY, TITLE VARCHAR(40), BODY FULLTEXT);
INSERT INTO ARTICLES VALUES
    (1, 'Rivers', FULLTEXT('Salmon swim up the river to spawn. The river runs fast in spring.')),
    (2, 'Ponds', NEW FullText('A pond holds still water. Frogs and newts live in ponds.')),
    (3, 'Hatcheries', CAST('Running water carries the salmon eggs downstream.' AS FULLTEXT)),
    (4, 'Glossary', FULLTEXT('Swimming, swimmers, swam.', 'NONE'));
COMMIT;
```

```
OK
OK
OK
OK
```

## The FULLTEXT type

FULLTEXT holds a document of at most 32,767 characters and the name of
its language. The value is kept inline in the row, so a longer text is
refused with SQLSTATE 22001. Two values are equal if their language and
text are, and values order by language, then by text in code point
order *(extension)*, so a FULLTEXT column can be sorted, grouped and keyed.

A value is made in one of three ways.

- `FULLTEXT(text [, language])`, the function form of the type's
  FullText method.
- `NEW FullText(text [, language])`, the method as the standard writes
  it, which is bound as the function.
- `CAST(text AS FULLTEXT)`, in the default language. A CLOB is read a
  character at a time up to the limit.

The language is ENGLISH by default, or another of the 43
[languages](#languages). ENGLISH stems words by Snowball's English
stemmer and has Snowball's 174 stop words. NONE stems nothing and has no
stop words. A language name is read in any case, its trailing spaces
not counting. A null, empty or unknown name raises SQLSTATE 2FF02
before any value is made. A null text gives the null value.

A character string is not stored in a FULLTEXT column by assignment.
Write FULLTEXT(...) or CAST, so the language is chosen on purpose.

`FULLTEXT_TO_CHARACTER(doc)` gives the text, as a CHARACTER VARYING(32767),
and so does `CAST(doc AS VARCHAR(n))`. The LANGUAGE method gives the
language's name. LANGUAGE is a reserved word, so the method is written
with a delimited identifier, `BODY."LANGUAGE"`:

```sql
SELECT ID, BODY."LANGUAGE" AS LANG, FULLTEXT_TO_CHARACTER(BODY) AS TEXT FROM ARTICLES ORDER BY ID;
```

```
           ID | LANG                             | TEXT
--------------+----------------------------------+--------------------------------------------------------------
            1 | ENGLISH                          | Salmon swim up the river to spawn. The river runs fast in spring.
            2 | ENGLISH                          | A pond holds still water. Frogs and newts live in ponds.
            3 | ENGLISH                          | Running water carries the salmon eggs downstream.
            4 | NONE                             | Swimming, swimmers, swam.
(4 rows)
```

A plain string, and a language the tokenizer does not know, are refused:

```sql
INSERT INTO ARTICLES VALUES (5, 'Lakes', 'Lakes are deep.');
INSERT INTO ARTICLES VALUES (5, 'Lakes', FULLTEXT('Lakes are deep.', 'KLINGON'));
ROLLBACK;
```

```
turborel: SQLCODE -103 (SQLSTATE 42000): cannot assign a CHARACTER(15) value to column BODY (FULLTEXT) [ISO/IEC 9075:1992 §13.8 SR6; ISO/IEC 9075:1992 §9.2 SR2] (at character 41)
turborel: SQLCODE -301 (SQLSTATE 2FF02): [ISO/IEC 13249-2:2002 §5.1.9] no language is named KLINGON
OK
```

## The FT_PATTERN type

FT_PATTERN holds a search pattern of at most 32,767 characters. A
character string is stored in it by assignment, and it casts to and from
one. Its values compare only with each other, by their text. Contains
and Rank take an FT_PATTERN or a character string, which they cast to
one, so a pattern kept in a table is used as it is:

```sql
CREATE TABLE ALERTS (NAME VARCHAR(20) PRIMARY KEY, PATTERN FT_PATTERN);
INSERT INTO ALERTS VALUES ('fish', '"salmon" | "trout"'), ('amphibians', '"frog%" | "newt%"');
COMMIT;
SELECT A.NAME, R.ID FROM ALERTS A, ARTICLES R WHERE R.BODY.Contains(A.PATTERN) = 1 ORDER BY A.NAME, R.ID;
```

```
OK
OK
OK
 NAME                 |           ID
----------------------+--------------
 amphibians           |            2
 fish                 |            1
 fish                 |            3
(3 rows)
```

A pattern is checked when it is searched with, not when it is stored.

## Routines

| Routine | Result |
|---|---|
| `FULLTEXT(text [, language])`, `NEW FullText(text [, language])` | A FULLTEXT value |
| `FULLTEXT_TO_CHARACTER(doc)` | Its text, CHARACTER VARYING(32767) |
| `doc."LANGUAGE"` | Its language's name, CHARACTER VARYING(32) |
| `doc.Contains(pattern)`, `CONTAINS(doc, pattern)` | INTEGER 1 if `doc` matches `pattern`, else 0 |
| `doc.Rank(pattern)`, `RANK(doc, pattern)` | DOUBLE PRECISION, 0 or more, higher for a better match |
| `doc.Tokenize()`, `TOKENIZE(doc)` | Its words, VARCHAR(1024) ARRAY[65535] |
| `doc.TokenizeAndStem()`, `TOKENIZEANDSTEM(doc)` | Their stems, VARCHAR(1024) ARRAY[65535] |
| `doc.TokenizePosition(unit)`, `TOKENIZEPOSITION(doc, unit)` | Its words and their places, an ARRAY of ROW(TOKEN, POSITION, CORRVAL) |
| `doc.TokenizePositionAndStem()`, `TOKENIZEPOSITIONANDSTEM(doc)` | Their stems and their places in words, an ARRAY of ROW(TOKEN, POSITION, CORRVAL) |
| `doc.Segmentize(unit)`, `SEGMENTIZE(doc, unit)` | Its sentences or paragraphs, a FULLTEXT ARRAY |
| `TEXT_MATCHES(text, words)` | BOOLEAN, TRUE if `text` holds every word of `words` |

FULLTEXT and FT_PATTERN are types beyond the standard, registered as the
database starts, and their routines are scalar functions. A method call,
`doc.Contains(p)`, is bound as the call of the function of its name
whose first argument is the value, `CONTAINS(doc, p)`, whatever
SQL.FUNCTION_CALLS is. The function forms are called where
SQL.FUNCTION_CALLS is ON, its default.

## Searching

Contains gives an INTEGER, 1 or 0, so a query writes
`BODY.Contains(pattern) = 1`. A pattern is read in the document's
language unless a word names another. STEMMED FORM OF stems in English,
so `"swim"` finds the article that says `swim` but not the glossary,
whose language is NONE:

```sql
SELECT ID, TITLE FROM ARTICLES WHERE BODY.Contains('"salmon"') = 1 ORDER BY ID;
SELECT ID, TITLE FROM ARTICLES WHERE CONTAINS(BODY, 'STEMMED FORM OF "swim"') = 1 ORDER BY ID;
```

```
           ID | TITLE
--------------+------------------------------------------
            1 | Rivers
            3 | Hatcheries
(2 rows)
           ID | TITLE
--------------+------------------------------------------
            1 | Rivers
(1 row)
```

Contains and Rank of a null document are null, and the pattern is not
read. Of a null pattern they are null too. A document of no characters
matches no pattern, NOT patterns included, and ranks 0.

A pattern that is not of the [pattern language](09-fulltext-patterns.md)
raises SQLSTATE 2FF01. One that names a language the tokenizer does not
know raises 2FF02, and one every word of which is a stop word raises
2FF03:

```sql
SELECT ID FROM ARTICLES WHERE BODY.Contains('"river" &') = 1;
SELECT ID FROM ARTICLES WHERE BODY.Contains('"the" | "a"') = 1;
SELECT ID FROM ARTICLES WHERE BODY.Contains('KLINGON "river"') = 1;
```

```
turborel: SQLCODE -301 (SQLSTATE 2FF01): [ISO/IEC 13249-2:2002 §5.3.1 Description 3] expected a word or phrase in double quotes, at character 10 of the search pattern
turborel: SQLCODE -301 (SQLSTATE 2FF03): [ISO/IEC 13249-2:2002 §6.4.2 Description 2a i] every word of the search pattern is a stop word
turborel: SQLCODE -301 (SQLSTATE 2FF02): [ISO/IEC 13249-2:2002 §5.3.1 Description 3c] no language is named KLINGON, at character 1 of the search pattern
```

### Character string documents *(extension)*

The document may also be a character string or a CLOB, which is read a
piece at a time and so may be longer than a FULLTEXT value. It is read
in the FULLTEXT.LANGUAGE of the full-text index of its column, if it has
one, else in ENGLISH:

```sql
SELECT ID, TITLE FROM ARTICLES WHERE CONTAINS(TITLE, '"pond%" | "lake%"') = 1;
VALUES CONTAINS(CAST('A long report, kept as a large object.' AS CHARACTER LARGE OBJECT(1M)), '"report"');
```

```
           ID | TITLE
--------------+------------------------------------------
            2 | Ponds
(1 row)

--------------
            1
(1 row)
```

## Tokenizing

Five methods show how a document is read, as arrays a query reads by
CARDINALITY, by element, or as a table with UNNEST. Each takes a
FULLTEXT value, gives null of a null one and an empty array of an empty
one. A word is normalized as Contains compares it, and the methods that
stem give its stem in the document's language. Stop words are left
out, as `up`, `the` and `to` are here:

```sql
SELECT BODY.Tokenize() AS WORDS, CARDINALITY(BODY.Tokenize()) AS N FROM ARTICLES WHERE ID = 1;
SELECT BODY.TokenizeAndStem() FROM ARTICLES WHERE ID = 1;
```

```
 WORDS                                                        |            N
--------------------------------------------------------------+--------------
 ARRAY['salmon', 'swim', 'river', 'spawn', 'river', 'runs', 'fast', 'spring'] |            8
(1 row)

--------------------------------------------------------------
 ARRAY['salmon', 'swim', 'river', 'spawn', 'river', 'run', 'fast', 'spring']
(1 row)
```

TokenizePosition(unit) gives each word with its place in the unit,
WORDS, SENTENCES or PARAGRAPHS, or CHARACTERS, the singular SENTENCE and
PARAGRAPH too. A word's place counts the stop words before it, so
distances between words stay right. Under CHARACTERS, POSITION is the
word's first character, from 1, and CORRVAL its length, so
`t2.POSITION - t1.POSITION - t1.CORRVAL` is the characters between two
words. Under the other units CORRVAL is 0. The row's fields TOKEN and
POSITION are key words, so they are written quoted:

```sql
SELECT U."TOKEN", U."POSITION", U.CORRVAL FROM ARTICLES, UNNEST(ARTICLES.BODY.TokenizePosition('WORDS')) AS U WHERE ID = 1;
SELECT U."TOKEN", U."POSITION", U.CORRVAL FROM ARTICLES, UNNEST(ARTICLES.BODY.TokenizePosition('CHARACTERS')) AS U WHERE ID = 1;
```

```
 TOKEN                                                        |     POSITION |      CORRVAL
--------------------------------------------------------------+--------------+--------------
 salmon                                                       |            1 |            0
 swim                                                         |            2 |            0
 river                                                        |            5 |            0
 spawn                                                        |            7 |            0
 river                                                        |            9 |            0
 runs                                                         |           10 |            0
 fast                                                         |           11 |            0
 spring                                                       |           13 |            0
(8 rows)
 TOKEN                                                        |     POSITION |      CORRVAL
--------------------------------------------------------------+--------------+--------------
 salmon                                                       |            1 |            6
 swim                                                         |            8 |            4
 river                                                        |           20 |            5
 spawn                                                        |           29 |            5
 river                                                        |           40 |            5
 runs                                                         |           46 |            4
 fast                                                         |           51 |            4
 spring                                                       |           59 |            6
(8 rows)
```

TokenizePositionAndStem() gives the stems at their places in words.
Segmentize(unit) cuts the document into its sentences, of SENTENCE, or
its paragraphs, of PARAGRAPH, each a FULLTEXT value in the document's
language with its closing punctuation:

```sql
SELECT FULLTEXT_TO_CHARACTER(S) AS SENTENCE, S."LANGUAGE" AS LANG
    FROM ARTICLES, UNNEST(ARTICLES.BODY.Segmentize('SENTENCE')) WITH ORDINALITY AS U (S, N)
    WHERE ID = 1 ORDER BY N;
```

```
 SENTENCE                                                     | LANG
--------------------------------------------------------------+----------------------------------
 Salmon swim up the river to spawn.                           | ENGLISH
 The river runs fast in spring.                               | ENGLISH
(2 rows)
```

A unit is read in any case, its trailing spaces not counting. Any other
unit gives null, as WORD does of TokenizePosition and SENTENCES of
Segmentize. A call of a character string, or of a unit where the
method takes none, is refused:

```sql
SELECT BODY.TokenizePosition('WORDS')[2]."TOKEN" AS T2, CARDINALITY(BODY.TokenizePosition('WORD')) AS BY_WORD,
    CARDINALITY(BODY.Segmentize('SENTENCES')) AS BY_SENTENCES FROM ARTICLES WHERE ID = 1;
SELECT BODY.Tokenize('WORDS') FROM ARTICLES WHERE ID = 1;
SELECT TOKENIZE('plain') FROM ARTICLES WHERE ID = 1;
```

```
 T2                                                           |      BY_WORD | BY_SENTENCES
--------------------------------------------------------------+--------------+--------------
 swim                                                         |         NULL |         NULL
(1 row)
turborel: SQLCODE -103 (SQLSTATE 42000): [ISO/IEC 13249-2:2002 §5.1.1] invalid arguments to TOKENIZE (at character 7)
turborel: SQLCODE -103 (SQLSTATE 42000): argument 1 of TOKENIZE, a CHARACTER(5) value, cannot be assigned to its type FULLTEXT [ISO/IEC 9075:1992 §9.2 SR2] (at character 7)
```

An array holds at most 65,535 elements, and a document of more words
or segments raises SQLSTATE 2202F.

## Ranking

Rank scores a document by Okapi BM25. For each term the pattern's words
match in the document, words under NOT apart, each occurrence counting,
it adds

```
idf · tf · (k1 + 1) / (tf + k1 · (1 − b + b · dl / avgdl))
```

where `tf` is the term's frequency in the document, `dl` the document's
length and `avgdl` the average length, both in words that are not stop
words. The inverse document frequency is `idf = ln(1 + (N − df + 0.5) /
(df + 0.5))`, of `N` documents of which `df` hold the term. A document
that matches nothing ranks 0.

The statistics are those of the full-text index of the document's
column, with its options FULLTEXT.K1, 1.2 by default, and FULLTEXT.B,
0.75 by default. Of a document no index ranks, as an expression's or a
column's with no index, every `idf` is 1 and every document of average
length, so the score counts the matches with BM25's saturation:

```sql
SELECT ID, BODY.Rank('"river" | "water"') AS SCORE FROM ARTICLES ORDER BY SCORE DESC, ID;
```

```
           ID |                    SCORE
--------------+--------------------------
            1 |                  1.375E0
            2 |                      1E0
            3 |                      1E0
            4 |                      0E0
(4 rows)
```

A statement reads an index's statistics once, so a document's rank is
the same however often the statement asks for it, even while other
transactions change the table. Two statements may rank one document
differently, as the collection may have changed between them. FT_MinRankValue
is 0 and FT_MaxRankValue is the greatest DOUBLE PRECISION.

### Ranked by an index

This table of short texts has 2,000 rows of filler beside them, so the
words that matter are rare:

```sql
CREATE TABLE DOCS (ID INTEGER PRIMARY KEY, BODY VARCHAR(200));
INSERT INTO DOCS VALUES (1, 'fox fox fox'), (2, 'a fox and a dog'), (3, 'dog dog dog dog dog dog dog dog dog fox'), (4, 'a cat');
INSERT INTO DOCS
    WITH RECURSIVE N (K) AS (VALUES (CAST(100 AS INTEGER)) UNION ALL SELECT K + 1 FROM N WHERE K < 2099)
    SELECT K, 'filler row number ' || CAST(K AS VARCHAR(8)) FROM N;
COMMIT;
SELECT ID, RANK(BODY, '"fox"') AS SCORE FROM DOCS WHERE ID < 5 ORDER BY ID;
```

```
OK
OK
OK
OK
           ID |                    SCORE
--------------+--------------------------
            1 |     1.5714285714285714E0
            2 |                      1E0
            3 |                      1E0
            4 |                      0E0
(4 rows)
```

With an index, `fox` weighs by its rarity and each text by its length,
so the short text that mentions it once ranks above the long one:

```sql
COMMIT;
CREATE INDEX DOCS_BODY ON DOCS (BODY) USING FULLTEXT;
SELECT ID, RANK(BODY, '"fox"') AS SCORE FROM DOCS WHERE ID < 5 ORDER BY ID;
```

```
OK
OK
           ID |                    SCORE
--------------+--------------------------
            1 |     1.0544452842822388E1
            2 |      7.983657152422666E0
            3 |     3.9356056385182154E0
            4 |                      0E0
(4 rows)
```

### The best rows first

A query for the best few matches filters by Contains, orders by Rank
descending and fetches the first rows. The index then gives the rows
best first, bounding each by its terms' greatest weights, and stops once
no row left can rank higher, so only the few rows it gives are read and
sorted:

```sql
SELECT ID, RANK(BODY, '"fox"') AS SCORE FROM DOCS
    WHERE CONTAINS(BODY, '"fox"') = 1
    ORDER BY RANK(BODY, '"fox"') DESC FETCH FIRST 2 ROWS ONLY;
```

```
           ID |                    SCORE
--------------+--------------------------
            1 |     1.0544452842822388E1
            2 |      7.983657152422666E0
(2 rows)
```

The plan shows a NearestScan of the index:

```sql
EXPLAIN SELECT ID FROM DOCS
    WHERE CONTAINS(BODY, '"fox"') = 1
    ORDER BY RANK(BODY, '"fox"') DESC FETCH FIRST 2 ROWS ONLY;
```

```
 PLAN
--------------------------------------------------------------
 -- bound:
 Project DOCS.ID
   Limit fetch 2
     Sort RANK(DOCS.BODY, ASSIGN('"fox"'), 'DOCS_BODY') DESC
       Project DOCS.ID, RANK(DOCS.BODY, ASSIGN('"fox"'), 'DOCS_BODY')
         Filter CONTAINS(DOCS.BODY, ASSIGN('"fox"'), 'DOCS_BODY') = 1
           Scan ANN.DOCS
 -- rules: constant_folding x2
 -- optimized:
 Project DOCS.ID
   Limit fetch 2
     Sort RANK(DOCS.BODY, CAST('"fox"' AS FT_PATTERN), 'DOCS_BODY') DESC
       Project DOCS.ID, RANK(DOCS.BODY, CAST('"fox"' AS FT_PATTERN), 'DOCS_BODY')
         Filter CONTAINS(DOCS.BODY, CAST('"fox"' AS FT_PATTERN), 'DOCS_BODY') = 1
           Scan ANN.DOCS
 -- physical:
 Project DOCS.ID
   Limit fetch 2
     Sort RANK(DOCS.BODY, CAST('"fox"' AS FT_PATTERN), 'DOCS_BODY') DESC
       Project DOCS.ID, RANK(DOCS.BODY, CAST('"fox"' AS FT_PATTERN), 'DOCS_BODY')
         NearestScan ANN.DOCS using DOCS_BODY [fulltext]: RANK(DOCS.BODY, CAST('"fox"' AS FT_PATTERN), 'DOCS_BODY') where CONTAINS(DOCS.BODY, CAST('"fox"' AS FT_PATTERN), 'DOCS_BODY') = 1  (first=2, cost=1785.8; certified bounds or scan)
(21 rows)
```

The Rank and Contains calls must search one pattern, of the indexed
column, and the pattern must be a constant or a parameter. Where a
pattern is read differently in documents of different languages, the
index's order is a bound and each row's exact rank is computed again,
so the rows are the ones a full sort gives.

### Choosing the ranking

FULLTEXT.K1 sets how fast repeated occurrences stop adding weight, and
FULLTEXT.B how much a long document is held back, from 0 for not at all
to 1. Both change in place, without building the index again. With B 0,
the two texts that mention `fox` once rank alike, whatever their length.
FULLTEXT.RANKING *(extension)* names the ranking, BM25 or one a program
registered:

```sql
COMMIT;
ALTER INDEX DOCS_BODY SET (FULLTEXT.K1 = 2, FULLTEXT.B = 0);
SELECT ID, RANK(BODY, '"fox"') AS SCORE FROM DOCS WHERE ID < 5 ORDER BY ID;
COMMIT;
ALTER INDEX DOCS_BODY SET (FULLTEXT.RANKING = TFIDF);
ALTER INDEX DOCS_BODY SET (FULLTEXT.K1 = DEFAULT, FULLTEXT.B = DEFAULT);
SELECT OPTION_NAME, OPTION_VALUE, VALUE_SOURCE FROM INFORMATION_SCHEMA.INDEX_OPTIONS
    WHERE INDEX_NAME = 'DOCS_BODY' ORDER BY OPTION_NAME;
COMMIT;
```

```
OK
OK
           ID |                    SCORE
--------------+--------------------------
            1 |     1.1431145468241544E1
            2 |      6.350636371245302E0
            3 |      6.350636371245302E0
            4 |                      0E0
(4 rows)
OK
turborel: SQLCODE -103 (SQLSTATE 42000): FULLTEXT.RANKING is one of BM25, not TFIDF
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

A program embedding Turborel adds a ranking with
`Database::register_ranking`, an implementation of the `Ranking` trait
([The Embedded Library](../client/03-embedding.md#full-text-rankings-extension)).
A ranking whose weights break its contract, by passing its own bound,
fails the statement with SQLCODE -910 rather than give a wrong top-k.
An index naming a ranking that is not registered when the database is
next opened fails Rank with -910 too.

## The fulltext index method

```
CREATE INDEX <index name> ON <table name> ( <column name> ) USING FULLTEXT
    [ WITH ( <index option> [ { , <index option> }... ] ) ]
```

The index *(extension)* is of one column, a FULLTEXT column or a character string
column with no collation. It holds each distinct text's words and their
places in blocks, so it stays small and a search of a rare and a common
word skips most of the common one's entries. A FULLTEXT document is read
in its own language, a character string in FULLTEXT.LANGUAGE. The index
notes each language its FullText documents are in, and reads a pattern
in each of them.

The index finds the rows of `CONTAINS(column, pattern) = 1`, of
`column.Contains(pattern) = 1` and of `TEXT_MATCHES(column, words)`,
where the pattern or words are a constant or a parameter. It gives the
best rows of a Contains filter ordered by Rank, and the statistics Rank
scores by. The executor checks each row it finds, so a pattern the
index cannot narrow, as a thesaurus expansion, finds every row it holds
and the check keeps the matches. A malformed pattern raises its error at
the first row, as a scan would. It can index a temporary table. A table
whose rows are encrypted can have one only where the database's files
are encrypted too, as the index keeps the table's words
(STORAGE.ENCODINGS).

| Option | Default | Values | A change |
|---|---|---|---|
| FULLTEXT.LANGUAGE | ENGLISH | One of the [languages](#languages), the one a character string column is read in | builds the index again |
| FULLTEXT.STOP_WORDS | DEFAULT | DEFAULT for the language's list, NONE, or words written with commas | builds the index again |
| FULLTEXT.K1 | 1.2 | 0 to 1000 | takes effect at once |
| FULLTEXT.B | 0.75 | 0 to 1 | takes effect at once |
| FULLTEXT.RANKING | BM25 | BM25 or the name of a registered ranking | takes effect at once |

A word on the index's own stop list has no entries, so a search of it
reads the table.

## TEXT_MATCHES

```
TEXT_MATCHES ( <document> , <words> )
```

TEXT_MATCHES *(extension)* takes two character strings, CHARACTER
LARGE OBJECT values too, read a piece at a time. The result is BOOLEAN.

- TRUE if the document holds every word of `<words>` that is not a stop
  word, FALSE if it lacks one.
- TRUE if `<words>` holds no word that is not a stop word, as every
  document holds them all.
- Null if either argument is null.

Words are matched as written, after normalization. TEXT_MATCHES does
not stem, so `invoice` does not find `invoices`. It reads both arguments
in the FULLTEXT.LANGUAGE of the full-text index of the document's
column, by the index and by a scan alike, and in ENGLISH when the
column has no index. A FULLTEXT document names its own language and is
searched by CONTAINS. In FRENCH an elided article is taken off, so
`école` finds `l'école`, which in ENGLISH is one word:

```sql
CREATE TABLE FR (ID INTEGER NOT NULL UNIQUE, BODY VARCHAR(100));
INSERT INTO FR VALUES (1, 'Les enfants de l''école'), (2, 'Le village');
COMMIT;
SELECT ID FROM FR WHERE TEXT_MATCHES(BODY, 'ÉCOLE enfants');
COMMIT;
CREATE INDEX FR_BODY ON FR (BODY) USING fulltext WITH (FULLTEXT.LANGUAGE = FRENCH);
COMMIT;
SELECT ID FROM FR WHERE TEXT_MATCHES(BODY, 'ÉCOLE enfants');
```

```
OK
OK
OK
           ID
--------------
(0 rows)
OK
OK
OK
           ID
--------------
            1
(1 row)
```

## How text is read

A document is cut into words, sentences and paragraphs. The standard
leaves each rule to the implementation, and these are Turborel's.

| Unit | Rule |
|---|---|
| Word | A run of letters, numbers and marks. An underscore joins two runs, and an apostrophe between letters joins them, so `can't` is one word. Each Han, Hiragana or Katakana character is a word by itself, but in the languages read as bigrams |
| Elision | In French, Italian, Catalan and Irish an elided article or pronoun is taken off its word, so `l'école` is `école`. In Catalan a middle dot joins letters, so `col·lecció` is one word |
| Bigrams | In Chinese, Japanese, Korean, Thai, Lao, Khmer and Burmese a run of their letters is read as its overlapping pairs, each a word, a letter with its marks. One letter alone is a word |
| Sentence | Ends after `.`, `!` or `?` when a blank, a line end or the end of the text follows, so `3.14` ends none. The full-width terminators end one by themselves |
| Paragraph | Ends at two line ends in a row, or at U+2029. It ends the sentence too |
| Normalized word | In ENGLISH and NONE, case folded, accents taken off, the right single quotation mark made an apostrophe, so `Résumé` is `resume`. In a stemmed language, case folded with accents kept, so `Élève` is `élève`; Turkish folds `I` to `ı` and `İ` to `i`. In a bigram language, full-width and half-width forms made one |
| Stem | The word's stem by the language's Snowball stemmer, so in English `searching` and `searched` are `search`. In NONE and the bigram languages, the word |
| Stop word | One of the language's Snowball list, as English `the` and `can't`. A stop word keeps its place, so distances stay right, but is never looked up |

A word is kept to its first 256 characters, and a term in the index to
its first 255 bytes. A longer word is found by that prefix and checked
against the row.

## Languages

A FULLTEXT value names its language, and a document is read by that
language's rules. These are the 43 languages the tokenizer knows. A
stemmed language stems by Snowball's own stemmer for it, run as
Snowball's program, and has Snowball's stop words where Snowball has a
list. Chinese, Japanese, Korean, Thai, Lao, Khmer and Burmese are
written without spaces between words, so a run of their letters is read
as its overlapping pairs, each a word. No language is guessed from a
text; a value without one is in ENGLISH.

| Language | Stemmer | Stop words | Words |
|---|---|---|---|
| NONE | None | None | Words, accents taken off |
| ARABIC | Snowball's Arabic | None | Words, accents kept |
| ARMENIAN | Snowball's Armenian | None | Words, accents kept |
| BASQUE | Snowball's Basque | None | Words, accents kept |
| CATALAN | Snowball's Catalan | None | Words, accents kept, elisions taken off, `l·l` joined |
| CZECH | Snowball's Czech | None | Words, accents kept |
| DANISH | Snowball's Danish | Snowball's list | Words, accents kept |
| DUTCH | Snowball's Dutch | Snowball's list | Words, accents kept |
| ENGLISH | Snowball's English | Snowball's list | Words, accents taken off |
| ESPERANTO | Snowball's Esperanto | None | Words, accents kept |
| ESTONIAN | Snowball's Estonian | None | Words, accents kept |
| FINNISH | Snowball's Finnish | Snowball's list | Words, accents kept |
| FRENCH | Snowball's French | Snowball's list | Words, accents kept, elisions taken off |
| GERMAN | Snowball's German | Snowball's list | Words, accents kept |
| GREEK | Snowball's Greek | None | Words, accents kept |
| HINDI | Snowball's Hindi | None | Words, accents kept |
| HUNGARIAN | Snowball's Hungarian | Snowball's list | Words, accents kept |
| INDONESIAN | Snowball's Indonesian | Snowball's list | Words, accents kept |
| IRISH | Snowball's Irish | Snowball's list | Words, accents kept, elisions taken off |
| ITALIAN | Snowball's Italian | Snowball's list | Words, accents kept, elisions taken off |
| LITHUANIAN | Snowball's Lithuanian | None | Words, accents kept |
| NEPALI | Snowball's Nepali | None | Words, accents kept |
| NORWEGIAN | Snowball's Norwegian | Snowball's list | Words, accents kept |
| PERSIAN | Snowball's Persian | None | Words, accents kept |
| POLISH | Snowball's Polish | None | Words, accents kept |
| PORTUGUESE | Snowball's Portuguese | Snowball's list | Words, accents kept |
| ROMANIAN | Snowball's Romanian | None | Words, accents kept |
| RUSSIAN | Snowball's Russian | Snowball's list | Words, accents kept |
| SERBIAN | Snowball's Serbian | None | Words, accents kept |
| SESOTHO | Snowball's Sesotho | Snowball's list | Words, accents kept |
| SPANISH | Snowball's Spanish | Snowball's list | Words, accents kept |
| SWEDISH | Snowball's Swedish | Snowball's list | Words, accents kept |
| TAMIL | Snowball's Tamil | None | Words, accents kept |
| TURKISH | Snowball's Turkish | None | Words, accents kept, `I` folded to `ı`, `İ` to `i` |
| UKRAINIAN | Snowball's Ukrainian | Snowball's list | Words, accents kept |
| YIDDISH | Snowball's Yiddish | None | Words, accents kept |
| CHINESE | None | None | Character bigrams, full-width forms made one |
| JAPANESE | None | None | Character bigrams, full-width forms made one |
| KOREAN | None | None | Character bigrams, full-width forms made one |
| THAI | None | None | Character bigrams, full-width forms made one |
| LAO | None | None | Character bigrams, full-width forms made one |
| KHMER | None | None | Character bigrams, full-width forms made one |
| BURMESE | None | None | Character bigrams, full-width forms made one |

Each value is read in its own language, and a pattern's word naming no
language is read in its document's. French takes `l'` off `l'école`,
German stems `Kinder` to `kind`, and Japanese is read as pairs:

```sql
CREATE TABLE NOTES (ID INTEGER PRIMARY KEY, BODY FULLTEXT);
INSERT INTO NOTES VALUES
    (1, FULLTEXT('Les enfants jouaient dans l''école.', 'FRENCH')),
    (2, FULLTEXT('Die Kinder spielten im Garten.', 'GERMAN')),
    (3, FULLTEXT('東京都庁は新宿にあります。', 'JAPANESE')),
    (4, FULLTEXT('İstanbul ılık bir şehir.', 'TURKISH'));
COMMIT;
SELECT ID, BODY.TokenizeAndStem() AS STEMS FROM NOTES ORDER BY ID;
```

```
OK
OK
OK
           ID | STEMS
--------------+--------------------------------------------------------------
            1 | ARRAY['enfant', 'jou', 'écol']
            2 | ARRAY['kind', 'spielt', 'gart']
            3 | ARRAY['東京', '京都', '都庁', '庁は', 'は新', '新宿', '宿に', 'にあ', 'あり', 'りま', 'ます']
            4 | ARRAY['istanbul', 'ılık', 'bir', 'şehir']
(4 rows)
```

A stemmed word matches in each document's language. A word of several
Japanese letters is the phrase of its pairs, and a single letter matches
every pair holding it. In Turkish `İ` is the capital of `i` and `I` of
`ı`, so `"İSTANBUL"` finds the city and `"ISTANBUL"` does not:

```sql
SELECT ID FROM NOTES WHERE BODY.Contains('FORM OF "école" | FORM OF "Kind"') = 1 ORDER BY ID;
SELECT ID FROM NOTES WHERE BODY.Contains('"京都"') = 1;
SELECT ID FROM NOTES WHERE BODY.Contains('"新"') = 1;
SELECT ID FROM NOTES WHERE BODY.Contains('"İSTANBUL" & "ILIK"') = 1;
SELECT ID FROM NOTES WHERE BODY.Contains('"ISTANBUL"') = 1;
```

```
           ID
--------------
            1
            2
(2 rows)
           ID
--------------
            3
(1 row)
           ID
--------------
            3
(1 row)
           ID
--------------
            4
(1 row)
           ID
--------------
(0 rows)
```

A full-text index reads each FullText document in its language too, and
notes the languages its documents are in, so a search reads its pattern
in each of them.

### Languages plug-ins serve *(extension)*

An extension may serve a language of its own under a name no built-in
language has, by a LANGUAGE object ([Kinds of
Plug-in](../client/04-plugins-kinds.md)). It is named where a built-in
language is, in FULLTEXT.LANGUAGE, `FULLTEXT(text, language)` and a
pattern's language specification. The program may cut text into words,
stem words and give stop words, and leave any of them to a built-in
language. Cutting Chinese or Japanese into dictionary words is served
this way, and the database ships no dictionary. A plug-in's author may
use one under a permissive license, as jieba's dictionary (MIT) for
Chinese or IPADIC for Japanese.

A document in such a language costs one call to cut and, if the
program stems, one call more for the words not stemmed before, and a
pattern's words are cut by the same program. A failed call fails the
statement with an error of class 38, and the next call starts the
program again.

### Snowball's license

The stemmers' programs and the stop-word lists are Snowball's
(snowballstem.org), kept unchanged in the source tree under
`src/fulltext/text`. They are distributed under this notice:

```
Copyright (c) 2001, Dr Martin Porter
Copyright (c) 2002, Richard Boulton
Copyright (c) 2004,2005, Richard Boulton
Copyright (c) 2013, Yoshiki Shibukawa
Copyright (c) 2006-2025, Olly Betts
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions
are met:

  1. Redistributions of source code must retain the above copyright notice,
     this list of conditions and the following disclaimer.
  2. Redistributions in binary form must reproduce the above copyright notice,
     this list of conditions and the following disclaimer in the documentation
     and/or other materials provided with the distribution.
  3. Neither the name of the Snowball project nor the names of its contributors
     may be used to endorse or promote products derived from this software
     without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR
ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

## The Full-Text Information Schema

FT_INFORMATION_SCHEMA has two views, which any user may read. FT_FEATURES
gives the limits and the units of proximity and context supported, and
FT_SCHEMATA the schemata in which the full-text types and routines are
found. They are built in, in no schema, so every schema has them:

```sql
SELECT FT_FEATURENAME, FT_FEATUREVALUE FROM FT_INFORMATION_SCHEMA.FT_FEATURES;
SELECT SCHEMA_NAME, SCHEMA_OWNER, DEFAULT_CHARACTER_SET_NAME FROM FT_INFORMATION_SCHEMA.FT_SCHEMATA ORDER BY SCHEMA_NAME;
```

```
 FT_FEATURENAME                                               | FT_FEATUREVALUE
--------------------------------------------------------------+--------------------------------------------------------------
 FT_MaxTextLength                                             | 32767
 FT_MaxLanguageLength                                         | 32
 FT_DefaultLanguage                                           | ENGLISH
 FT_ThesNameLength                                            | 128
 FT_ThesTermLength                                            | 1024
 FT_MinRankValue                                              | 0E0
 FT_MaxRankValue                                              | 1.7976931348623157E308
 FT_ProximityCharacters                                       | YES
 FT_ProximityWords                                            | YES
 FT_ProximitySentences                                        | YES
 FT_ProximityParagraphs                                       | YES
 FT_ContextSentence                                           | YES
 FT_ContextParagraph                                          | YES
(13 rows)
 SCHEMA_NAME                                                  | SCHEMA_OWNER                                                 | DEFAULT_CHARACTER_SET_NAME
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 ANN                                                          | ANN                                                          | SQL_TEXT
 DEFINITION_SCHEMA                                            | DEFINITION_SCHEMA                                            | SQL_TEXT
 FT_INFORMATION_SCHEMA                                        | FT_INFORMATION_SCHEMA                                        | SQL_TEXT
 INFORMATION_SCHEMA                                           | INFORMATION_SCHEMA                                           | SQL_TEXT
 SF_INFORMTN_SCHEMA                                           | SF_INFORMTN_SCHEMA                                           | SQL_TEXT
 ST_DEFINITION_SCHEMA                                         | ST_DEFINITION_SCHEMA                                         | SQL_TEXT
 ST_INFORMTN_SCHEMA                                           | ST_INFORMTN_SCHEMA                                           | SQL_TEXT
(7 rows)
```

The maximum length of a pattern, FT_MaxPatternLength, is 32,767 too.

## Errors

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2FF01 | -301 | A pattern is not of the pattern language, or breaks one of its rules |
| 2FF02 | -301 | A language is null, empty or unknown, given to FULLTEXT or named in a pattern |
| 2FF03 | -301 | Every word of a pattern is a stop word |
| 22001 | -301 | A text or pattern is longer than 32,767 characters |
| 2202F | -301 | A tokenizing method's array would hold more than 65,535 elements |
| 42000 | -103 | A character string is stored in a FULLTEXT column without FULLTEXT(...) or CAST |
| 42000 | -103 | An index option is not one of the method's, its value is out of range, or FULLTEXT.RANKING names no ranking |
| 42000 | -103 | An encrypted table is indexed in files that are not encrypted |
| 0A000 | -900 | The index is of more than one column, or of a column not of FULLTEXT or a character string type, or of a collated one |
| TB910 | -910 | A ranking breaks its contract, or the ranking an index names is not registered |

## Standard

ISO/IEC 13249-2:2002 (CD, SC32 N0640), SQL/MM Full-Text. Its FullText
type and methods [§5.1], the tokenizing methods among them
[§5.1.4-§5.1.8], FT_Pattern [§5.3], the pattern language
[§5.3.1], thesauri [§8] and the Full-Text Information Schema [§9]. The
choices the standard leaves to an implementation are listed in
[CONFORMANCE.md](../../CONFORMANCE.md).

TEXT_MATCHES, the `fulltext` index method and its options, BM25 and
registered rankings, character string and CLOB documents, and the
ordering of FULLTEXT values are extensions. FULLTEXT and FT_PATTERN
are registered types, not user-defined types, so they declare no
methods and AS names no supertype of them.

## See also

[Full-Text Patterns](09-fulltext-patterns.md),
[CREATE INDEX](statements/create-index.md),
[ALTER INDEX](statements/alter-index.md),
[Full-Text Guide](../data/01-fulltext.md),
[The Embedded Library](../client/03-embedding.md#full-text-rankings-extension)
