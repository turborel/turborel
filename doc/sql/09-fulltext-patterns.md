# Full-Text Patterns

[Contents](README.md)

A search pattern says what a document must hold for Contains to match
it and Rank to score it ([Full-Text Search](08-fulltext.md)). It is a
character string or an FT_PATTERN value of at most 32,767 characters,
written in the pattern language of ISO/IEC 13249-2:2002 (CD, SC32
N0640) §5.3.1. Key words are read in any case, and white space between
items does not matter.

The examples on this page are run in order, by the user ANN, on a table
of notes:

```sql
CREATE SCHEMA AUTHORIZATION ANN;
CREATE TABLE NOTES (ID INTEGER PRIMARY KEY, BODY FULLTEXT);
INSERT INTO NOTES VALUES
    (1, FULLTEXT('The International Standard defines full-text search. Each standard has several parts.')),
    (2, FULLTEXT('Searching documents quickly needs an index. The index is searched by words.')),
    (3, FULLTEXT('Smith wrote the report on colour printers. Smyth reviewed it, and wrote color.')),
    (4, FULLTEXT('The cat sat on the mat. A dog barked at the cat.')),
    (5, FULLTEXT('Send an e-mail with the 50% discount code to every customer.')),
    (6, FULLTEXT('A kitten chased a puppy around the garden.'));
COMMIT;
```

```
OK
OK
OK
OK
```

## Grammar

```
<search expression> ::= <search term> [ { | <search term> }... ]
<search term>       ::= <search factor> [ { & <search factor> }... ]
<search factor>     ::= [ NOT ] <search primary>
<search primary>    ::= ( <search expression> )
                      | <text literal>
                      | IS ABOUT <word or phrase>
                      | <expansion>
                      | <argument> NEAR <argument> WITHIN <unsigned integer> <unit> { IN | ANY } ORDER
                      | <argument> IN SAME { SENTENCE | PARAGRAPH } AS <argument> [ { AND <argument> }... ]

<text literal>      ::= [ STEMMED FORM OF | FORM OF ] [ <language> ] "<word or phrase>" [ ESCAPE "<character>" ]
<argument>          ::= <text literal> | ( <text literal> [ { , <text literal> }... ] ) | <expansion>
<expansion>         ::= SOUNDS LIKE "<word>"
                      | FUZZY FORM OF "<word>"
                      | THESAURUS "<name>" EXPAND <relation> TERM OF <text literal> [ FOR <unsigned integer> { LEVEL | LEVELS } ]
<relation>          ::= BROADER | NARROWER | SYNONYM | PREFERRED | RELATED | TOP
<unit>              ::= CHARACTERS | WORDS | SENTENCE | SENTENCES | PARAGRAPH | PARAGRAPHS
```

`&` binds tighter than `|`, and NOT tightest. FOR ... LEVELS follows
only BROADER and NARROWER. A pattern nests at most 100 deep.

## Words and phrases

A word or phrase is written in double quotes, a `""` inside standing for
one quote. Words are separated by white space, so `"international
standard"` is a phrase of two words, which matches where the document
has them in a row. Each word is normalized as a document's words are,
case folded and accents taken off, so `"STANDARD"` finds `Standard`.
A word the tokenizer reads as several, as `"e-mail"`, is a phrase of
them:

```sql
SELECT ID FROM NOTES WHERE BODY.Contains('"STANDARD"') = 1 ORDER BY ID;
SELECT ID FROM NOTES WHERE BODY.Contains('"international standard"') = 1 ORDER BY ID;
SELECT ID FROM NOTES WHERE BODY.Contains('"e-mail"') = 1 ORDER BY ID;
```

```
           ID
--------------
            1
(1 row)
           ID
--------------
            1
(1 row)
           ID
--------------
            5
(1 row)
```

A phrase may run across the end of a sentence.

### Wild cards and escapes

`%` in a word stands for any characters, none too, and `_` for one
character. A word of `%` alone in a phrase is an optional word, which
matches one word or none, so three of them let up to three words stand
between `cat` and `mat`. `ESCAPE "c"` makes `c` before `%`, `_` or
itself stand for that character:

```sql
SELECT ID FROM NOTES WHERE BODY.Contains('"search%"') = 1 ORDER BY ID;
SELECT ID FROM NOTES WHERE BODY.Contains('"sm_th"') = 1 ORDER BY ID;
SELECT ID FROM NOTES WHERE BODY.Contains('"cat % % % mat"') = 1 ORDER BY ID;
SELECT ID FROM NOTES WHERE BODY.Contains('"50!%" ESCAPE "!"') = 1 ORDER BY ID;
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
            4
(1 row)
           ID
--------------
            5
(1 row)
```

A word of wild cards alone, a phrase of optional words alone, an escape
of more than one character and an escape before any other character are
each SQLSTATE 2FF01.

### Languages

A word or phrase is read in the document's language, unless a language
name stands just before its opening quote, as `NONE "searching"`. Words
in another language than the document's are compared by their
normalized text, each side's stop words and stems its own. An unknown
name is SQLSTATE 2FF02. The examples under
[Stems and topics](#stems-and-topics) search in NONE. The
[languages](08-fulltext.md#languages) are listed with the full-text type.

In Chinese, Japanese, Korean, Thai, Lao, Khmer and Burmese a word is
read as the overlapping pairs of its letters, so `"東京都"` is the
phrase of `東京` and `京都`. A word of one letter in these languages
matches every word of the document holding that letter.

### Stop words

Stop words are those of the word's language, Snowball's list where it
has one. A stop word may stand anywhere a word may, and is never 2FF01. A
pattern every word of which is a stop word is 2FF03. A stop word by
itself matches nothing, so `"the" | "cat"` finds what `"cat"` finds.
Inside a phrase, a stop word matches any stop word of the document,
and a word that is not a stop word never matches one.

## And, or and not

`&` matches a document that both sides match, `|` one that either does,
and `NOT` one that its operand does not. A document of no characters
matches no pattern, NOT included, while one of characters but no words
matches NOT.

```sql
SELECT ID FROM NOTES WHERE BODY.Contains('"cat" & "dog"') = 1 ORDER BY ID;
SELECT ID FROM NOTES WHERE BODY.Contains('"cat" | "index"') = 1 ORDER BY ID;
SELECT ID FROM NOTES WHERE BODY.Contains('("cat" | "kitten") & NOT "dog"') = 1 ORDER BY ID;
SELECT ID FROM NOTES WHERE BODY.Contains('"the" | "cat"') = 1 ORDER BY ID;
```

```
           ID
--------------
            4
(1 row)
           ID
--------------
            2
            4
(2 rows)
           ID
--------------
            6
(1 row)
           ID
--------------
            4
(1 row)
```

## Stems and topics

`STEMMED FORM OF "word"`, or `FORM OF "word"`, matches every word of the
document with the same stem, by the Snowball stemmer of the document's
[language](08-fulltext.md#languages). In English it is Snowball's English stem, so
`search` finds `search`, `searching` and `searched`. A stemmed word or
phrase takes no wild card or escape. A word in NONE is not stemmed, so
`NONE "searching"` finds only the note that says `searching`.

`IS ABOUT "words"` matches a document holding any word of the phrase
that is not a stop word.

```sql
SELECT ID FROM NOTES WHERE BODY.Contains('STEMMED FORM OF "search"') = 1 ORDER BY ID;
SELECT ID FROM NOTES WHERE BODY.Contains('NONE "searching"') = 1 ORDER BY ID;
SELECT ID FROM NOTES WHERE BODY.Contains('IS ABOUT "cat food"') = 1 ORDER BY ID;
```

```
           ID
--------------
            1
            2
(2 rows)
           ID
--------------
            2
(1 row)
           ID
--------------
            4
(1 row)
```

## Sound-alike and misspelt words

`SOUNDS LIKE "word"` matches the words of the same American Soundex
code, taken of the ASCII letters of the normalized word. `smith` and
`smyth` are S530, and so is `send`. A word with no letters matches only
itself.

`FUZZY FORM OF "word"` matches the words at most one edit away, an
insertion, deletion, substitution or swap of two neighbours, or two
edits for a word of eight characters or more.

Neither matches a stop word, and each takes one word, with no wild card
or escape.

```sql
SELECT ID FROM NOTES WHERE BODY.Contains('SOUNDS LIKE "smith"') = 1 ORDER BY ID;
SELECT ID FROM NOTES WHERE BODY.Contains('FUZZY FORM OF "standerd"') = 1 ORDER BY ID;
```

```
           ID
--------------
            3
            5
(2 rows)
           ID
--------------
            1
(1 row)
```

## Proximity

`a NEAR b WITHIN n unit IN ORDER` matches where a word of `a` and a word
of `b` are at most `n` units apart, the word of `a` first. `ANY ORDER`
takes either first. Each side is a word, a list of words in parentheses
of which any may match, or an expansion. A phrase is 2FF01 there.

The distance is the difference of the two words' places in the unit.

| Unit | Distance |
|---|---|
| WORDS | Their ordinals, stop words counting. WITHIN 0 WORDS is one word matching both sides |
| CHARACTERS | The characters between the end of the first and the start of the second |
| SENTENCES, SENTENCE | Their sentences' numbers. WITHIN 0 is the same sentence, which is in order either way |
| PARAGRAPHS, PARAGRAPH | Their paragraphs' numbers |

```sql
SELECT ID FROM NOTES WHERE BODY.Contains('"cat" NEAR "dog" WITHIN 5 WORDS ANY ORDER') = 1 ORDER BY ID;
SELECT ID FROM NOTES WHERE BODY.Contains('"cat" NEAR "dog" WITHIN 5 WORDS IN ORDER') = 1 ORDER BY ID;
SELECT ID FROM NOTES WHERE BODY.Contains('("smith", "smyth") NEAR "report" WITHIN 0 SENTENCES ANY ORDER') = 1 ORDER BY ID;
SELECT ID FROM NOTES WHERE BODY.Contains('"discount" NEAR "code" WITHIN 1 CHARACTERS IN ORDER') = 1 ORDER BY ID;
```

```
           ID
--------------
            4
(1 row)
           ID
--------------
(0 rows)
           ID
--------------
            3
(1 row)
           ID
--------------
            5
(1 row)
```

The first `cat` of note 4 is six words before `dog`, and the second
comes four words after it, so only ANY ORDER finds it within five.

## Sentences and paragraphs

`a IN SAME SENTENCE AS b` matches where a match of `a` and one of `b`,
and of each further argument after AND, lie in one sentence, each match
wholly inside it. `IN SAME PARAGRAPH` is alike for paragraphs. An
argument is a word or phrase, a list of them in parentheses, or an
expansion. CHARACTERS and WORDS make no context, and are 2FF01.

A paragraph ends at a blank line. An SQL literal cannot hold a line end,
so the minutes below, two paragraphs, are imported from a file and then
made a FULLTEXT value:

```
$ cat minutes.csv
ID,BODY
7,"Minutes of the board meeting.

The budget was approved. The printer budget was cut."
```

```sql
CREATE TABLE MINUTES_IN (ID INTEGER, BODY VARCHAR(1000));
COMMIT;
```

```
OK
OK
```

```
$ turborel -U ANN --import minutes.csv --table MINUTES_IN --header
OK, 1 row imported
```

```sql
INSERT INTO NOTES SELECT ID, FULLTEXT(BODY) FROM MINUTES_IN;
COMMIT;
SELECT ID FROM NOTES WHERE BODY.Contains('"printer" IN SAME SENTENCE AS "approved"') = 1 ORDER BY ID;
SELECT ID FROM NOTES WHERE BODY.Contains('"printer" IN SAME PARAGRAPH AS "approved"') = 1 ORDER BY ID;
SELECT ID FROM NOTES WHERE BODY.Contains('"printer" IN SAME PARAGRAPH AS "meeting"') = 1 ORDER BY ID;
SELECT ID FROM NOTES WHERE BODY.Contains('"printer%" IN SAME SENTENCE AS "smith" AND "colour"') = 1 ORDER BY ID;
```

```
OK
OK
           ID
--------------
(0 rows)
           ID
--------------
            7
(1 row)
           ID
--------------
(0 rows)
           ID
--------------
            3
(1 row)
```

## Thesauri

`THESAURUS "name" EXPAND relation TERM OF "term"` matches any of the
terms the thesaurus `name` relates to `term`.

| Relation | Terms |
|---|---|
| NARROWER | The term and those below it, every level down, or `FOR n LEVELS` down |
| BROADER | The term and those above it, every level up, or `FOR n LEVELS` up |
| SYNONYM | The term and its synonyms, and theirs in turn |
| RELATED | The term and its related terms |
| PREFERRED | The preferred terms of its synonyms, or the term if it has none |
| TOP | The topmost terms above it, or the term if it has none |

`FOR n LEVELS` follows `n` steps of the hierarchy, so `FOR 1 LEVEL` of
NARROWER is the term and the terms just below it. Terms are compared by
their words, normalized, and a thesaurus's name is compared with its
trailing and leading spaces taken off. A thesaurus that does not exist,
or a term it does not hold, expands to the term alone. The term is read
in its own language, and its expansion is matched word for word, with
no stemming.

### Making a thesaurus

A thesaurus is the rows of four tables of the schema FT_THESAURUS, which
a user makes with the definitions of clause 8 of the standard. Turborel
reads them by these names and columns, and the user decides who may
change them. EXPR, a term, is up to 1,024 characters, and a thesaurus's
name up to 128. The standard writes DEFAULT after NOT NULL, which SQL's
grammar does not allow, so it comes first here.

```sql
CREATE SCHEMA FT_THESAURUS AUTHORIZATION ANN
    CREATE TABLE TERM_DICTIONARY (
        TERMID INTEGER DEFAULT 0 NOT NULL,
        EXPR CHARACTER VARYING(1024),
        THNAME_DIC CHARACTER VARYING(128) NOT NULL,
        PRIMARY KEY (TERMID, THNAME_DIC))
    CREATE TABLE TERM_HIERARCHY (
        TERMID INTEGER NOT NULL,
        NARROWER_TERMID INTEGER NOT NULL,
        THNAME_HRR CHARACTER VARYING(128) NOT NULL,
        PRIMARY KEY (TERMID, NARROWER_TERMID, THNAME_HRR),
        FOREIGN KEY (NARROWER_TERMID, THNAME_HRR) REFERENCES TERM_DICTIONARY,
        FOREIGN KEY (TERMID, THNAME_HRR) REFERENCES TERM_DICTIONARY)
    CREATE TABLE TERM_SYNONYM (
        TERMID INTEGER NOT NULL,
        SYNONYM_TERMID INTEGER NOT NULL,
        PREFERRED_TERMID INTEGER,
        THNAME_SYN CHARACTER VARYING(128) NOT NULL,
        PRIMARY KEY (TERMID, SYNONYM_TERMID, THNAME_SYN),
        FOREIGN KEY (SYNONYM_TERMID, THNAME_SYN) REFERENCES TERM_DICTIONARY,
        FOREIGN KEY (PREFERRED_TERMID, THNAME_SYN) REFERENCES TERM_DICTIONARY,
        FOREIGN KEY (TERMID, THNAME_SYN) REFERENCES TERM_DICTIONARY)
    CREATE TABLE TERM_RELATED (
        TERMID INTEGER NOT NULL,
        RELATED_TERMID INTEGER NOT NULL,
        THNAME_REL CHARACTER VARYING(128) NOT NULL,
        PRIMARY KEY (TERMID, RELATED_TERMID, THNAME_REL),
        FOREIGN KEY (RELATED_TERMID, THNAME_REL) REFERENCES TERM_DICTIONARY,
        FOREIGN KEY (TERMID, THNAME_REL) REFERENCES TERM_DICTIONARY);
COMMIT;
```

```
OK
OK
```

| Table | A row |
|---|---|
| TERM_DICTIONARY | A term, EXPR, numbered TERMID in the thesaurus THNAME_DIC |
| TERM_HIERARCHY | TERMID is broader than NARROWER_TERMID |
| TERM_SYNONYM | TERMID and SYNONYM_TERMID are synonyms, PREFERRED_TERMID the one to prefer |
| TERM_RELATED | TERMID is related to RELATED_TERMID |

A thesaurus of animals, where a pet is an animal, a cat and a dog are
pets, a kitten is a cat and a puppy a dog, a feline is a cat, and a
mouse is related to a cat:

```sql
INSERT INTO FT_THESAURUS.TERM_DICTIONARY VALUES
    (1, 'animal', 'ANIMALS'), (2, 'pet', 'ANIMALS'), (3, 'cat', 'ANIMALS'), (4, 'dog', 'ANIMALS'),
    (5, 'kitten', 'ANIMALS'), (6, 'puppy', 'ANIMALS'), (7, 'feline', 'ANIMALS'), (8, 'mouse', 'ANIMALS');
INSERT INTO FT_THESAURUS.TERM_HIERARCHY VALUES
    (1, 2, 'ANIMALS'), (2, 3, 'ANIMALS'), (2, 4, 'ANIMALS'), (3, 5, 'ANIMALS'), (4, 6, 'ANIMALS');
INSERT INTO FT_THESAURUS.TERM_SYNONYM VALUES (7, 3, 3, 'ANIMALS');
INSERT INTO FT_THESAURUS.TERM_RELATED VALUES (8, 3, 'ANIMALS');
COMMIT;
```

```
OK
OK
OK
OK
OK
```

### Searching with a thesaurus

Every pet, at any depth, is in notes 4 and 6. Two levels below `animal`
reach `cat` and `dog`, not `kitten`. One level above `kitten` is `cat`.
The top of `puppy` is `animal`, which no note holds. An expansion can
stand beside NEAR too:

```sql
SELECT ID FROM NOTES WHERE BODY.Contains('THESAURUS "ANIMALS" EXPAND NARROWER TERM OF "pet"') = 1 ORDER BY ID;
SELECT ID FROM NOTES WHERE BODY.Contains('THESAURUS "ANIMALS" EXPAND NARROWER TERM OF "animal" FOR 2 LEVELS') = 1 ORDER BY ID;
SELECT ID FROM NOTES WHERE BODY.Contains('THESAURUS "ANIMALS" EXPAND BROADER TERM OF "kitten" FOR 1 LEVEL') = 1 ORDER BY ID;
SELECT ID FROM NOTES WHERE BODY.Contains('THESAURUS "ANIMALS" EXPAND SYNONYM TERM OF "feline"') = 1 ORDER BY ID;
SELECT ID FROM NOTES WHERE BODY.Contains('THESAURUS "ANIMALS" EXPAND RELATED TERM OF "mouse"') = 1 ORDER BY ID;
SELECT ID FROM NOTES WHERE BODY.Contains('THESAURUS "ANIMALS" EXPAND TOP TERM OF "puppy"') = 1 ORDER BY ID;
SELECT ID FROM NOTES WHERE BODY.Contains('THESAURUS "ANIMALS" EXPAND NARROWER TERM OF "cat" NEAR "garden" WITHIN 6 WORDS ANY ORDER') = 1 ORDER BY ID;
```

```
           ID
--------------
            4
            6
(2 rows)
           ID
--------------
            4
(1 row)
           ID
--------------
            4
            6
(2 rows)
           ID
--------------
            4
(1 row)
           ID
--------------
            4
(1 row)
           ID
--------------
(0 rows)
           ID
--------------
            6
(1 row)
```

A statement reads the thesaurus tables once, in its snapshot and with
its user's privileges, so a change a transaction makes is seen by its
own statements and by no other's until it commits. A statement's
thesauri hold at most 65,535 relations. A full-text index cannot narrow
a thesaurus expansion, so it gives every row it holds and each is
checked.

## Errors

Every error of a pattern names the character of the pattern it is
about, counted from 1:

```sql
SELECT ID FROM NOTES WHERE BODY.Contains('"cat" NEAR "the mat" WITHIN 3 WORDS ANY ORDER') = 1;
SELECT ID FROM NOTES WHERE BODY.Contains('"cat" IN SAME WORDS AS "dog"') = 1;
SELECT ID FROM NOTES WHERE BODY.Contains('SOUNDS LIKE "sm%"') = 1;
SELECT ID FROM NOTES WHERE BODY.Contains('THESAURUS "ANIMALS" EXPAND SYNONYM TERM OF "cat" FOR 2 LEVELS') = 1;
SELECT ID FROM NOTES WHERE BODY.Contains('"%"') = 1;
```

```
turborel: SQLCODE -301 (SQLSTATE 2FF01): [ISO/IEC 13249-2:2002 §5.3.1 Description 3] a token list holds words, not phrases, at character 12 of the search pattern
turborel: SQLCODE -301 (SQLSTATE 2FF01): [ISO/IEC 13249-2:2002 §5.3.1 Description 3] a context is a SENTENCE or a PARAGRAPH, at character 15 of the search pattern
turborel: SQLCODE -301 (SQLSTATE 2FF01): [ISO/IEC 13249-2:2002 §5.3.1 Description 3d, 3e] a stemmed or expanded word takes no wild card or escape, at character 13 of the search pattern
turborel: SQLCODE -301 (SQLSTATE 2FF01): [ISO/IEC 13249-2:2002 §5.3.1 Description 3] the search expression ends before this, at character 50 of the search pattern
turborel: SQLCODE -301 (SQLSTATE 2FF01): [ISO/IEC 13249-2:2002 §5.3.1 Description 3a] a word must hold a character that is not a wild card, at character 1 of the search pattern
```

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2FF01 | -301 | The pattern is not of the grammar, or breaks one of its rules |
| 2FF02 | -301 | It names a language the tokenizer does not know |
| 2FF03 | -301 | Every word of it is a stop word of its language |

## Standard

ISO/IEC 13249-2:2002 (CD, SC32 N0640) §5.3.1, the `<search expression>`,
and §6, the semantics of each of its parts. Where the draft disagrees
with itself, Turborel follows the definitions of §6. A proximity's
distance is the difference of places of §6.8.2, `FOR n LEVELS` is `n`
steps, and a bare word beside NEAR is taken as in the example of
§4.2.5.1. Clause 8's thesaurus tables are a model in the standard, and
are tables a user makes here. The choices left to an implementation are
listed in [CONFORMANCE.md](../../CONFORMANCE.md).

## See also

[Full-Text Search](08-fulltext.md),
[Full-Text Guide](../data/01-fulltext.md),
[The turborel Command](../client/01-turborel.md)
