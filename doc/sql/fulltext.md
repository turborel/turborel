# Full-Text Search *(extension)*

[Contents](README.md)

TEXT_MATCHES finds the rows whose text holds every word of a search, and
an index of the method `fulltext` finds them without reading the table.
Words are compared as the tokenizer reads them, so case, accents and
the place of a word in the text do not matter. The full-text search of
ISO/IEC 13249-2, its FULLTEXT type and CONTAINS method, is not yet
implemented. For an introduction with complete examples, read the
[Full-Text Guide](../data/fulltext/README.md).

## TEXT_MATCHES

```
TEXT_MATCHES ( <document> , <words> )
```

Both arguments are character strings, CHARACTER LARGE OBJECT values too,
read a piece at a time. The result is BOOLEAN.

- TRUE if the document holds every word of `<words>` that is not a stop
  word, FALSE if it lacks one.
- TRUE if `<words>` holds no word that is not a stop word, as every
  document holds them all.
- Null if either argument is null.

Words are matched as written, after normalization. TEXT_MATCHES does
not stem, so `invoice` does not find `invoices`. It reads both arguments
in English, whatever an index's FULLTEXT.LANGUAGE.

## How text is read

A document is cut into words, sentences and paragraphs. The standard
leaves each rule to the implementation, and these are turborel's.

| Unit | Rule |
|---|---|
| Word | A run of letters, numbers and marks. An underscore joins two runs, and an apostrophe between letters joins them, so `can't` is one word. Each Han, Hiragana or Katakana character is a word by itself |
| Sentence | Ends after `.`, `!` or `?` when a blank, a line end or the end of the text follows, so `3.14` ends none. The full-width terminators end one by themselves |
| Paragraph | Ends at two line ends in a row, or at U+2029 |
| Normalized word | Case folded, accents taken off, the right single quotation mark made an apostrophe, so `Résumé` is `resume` |
| Stop word | In English, one of Snowball's 174, as `the` and `can't`. A stop word keeps its place, so distances stay right, but is never looked up |

A word is kept to its first 256 characters, and a term in the index to
its first 255 bytes. A longer word is found by that prefix and checked
against the row.

## The fulltext index method

```
CREATE INDEX <index name> ON <table name> ( <column name> ) USING FULLTEXT
    [ WITH ( <index option> [ { , <index option> }... ] ) ]
```

The index is of one character string column with no collation. It holds
each distinct text's words and their places in blocks, so it stays small
and a search of a rare and a common word skips most of the common one's
entries. It answers `TEXT_MATCHES(column, value)` where the value is a
constant or a parameter, and the executor checks each row it finds. It
can index a temporary table. A table whose rows are encrypted can have
one only where the database's files are encrypted too, as the index
keeps the table's words (STORAGE.ENCODINGS).

| Option | Default | Values | A change |
|---|---|---|---|
| FULLTEXT.LANGUAGE | ENGLISH | ENGLISH or NONE, which stems nothing and has no stop words | builds the index again |
| FULLTEXT.STOP_WORDS | DEFAULT | DEFAULT for the language's list, NONE, or words written with commas | builds the index again |
| FULLTEXT.K1 | 1.2 | 0 to 1000 | takes effect at once |
| FULLTEXT.B | 0.75 | 0 to 1 | takes effect at once |

FULLTEXT.K1 and FULLTEXT.B are the weights of BM25 ranking, for the
ranking of ISO/IEC 13249-2 search when it is implemented. A word on the
index's own stop list has no entries, so a search of it reads the table.

## Errors

| SQLSTATE | SQLCODE | When |
|---|---|---|
| 2A000 | -103 | An option is not one of the method's, or its value is out of range |
| 2A000 | -103 | An encrypted table is indexed in files that are not encrypted |
| 0A000 | -900 | The index is of more than one column, or of a column not of a character string type, or of a collated one |

## Standard

An extension. ISO/IEC 13249-2 searches by the CONTAINS method of its
FULLTEXT type and a search language of its own.

## See also

[CREATE INDEX](statements/create-index.md),
[ALTER INDEX](statements/alter-index.md),
[Full-Text Guide](../data/fulltext/README.md)
