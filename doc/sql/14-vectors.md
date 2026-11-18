# Vectors *(extension)*

[Contents](README.md)

A VECTOR value is a list of numbers, an embedding a model made of a text
or an image, say. Two vectors are near when the things they stand for
are alike, so a query finds the rows whose vectors are nearest a
vector it is given. Turborel has the VECTOR type built in, with its
distances, operators, arithmetic, aggregates and quantization, and
distance functions a user defines in SQL. None of it is in the SQL
standards. For an introduction with complete examples, read the
[Vector Guide](../data/04-vectors.md).

The examples on this page are run in order, by the user ANN, on a table
of three-dimensional embeddings:

```sql
CREATE TABLE ITEMS (ID INTEGER PRIMARY KEY, NAME VARCHAR(20), EMBEDDING VECTOR(3));
INSERT INTO ITEMS VALUES
    (1, 'apple', '[1, 0, 0]'),
    (2, 'pear', '[0.9, 0.1, 0]'),
    (3, 'carrot', '[0, 1, 0.2]'),
    (4, 'stone', NULL);
COMMIT;
```

```
OK
OK
OK
```

## The VECTOR type

```
<vector type> ::= VECTOR [ ( <dimensions> [ , <format> [ , <layout> ] ] ) ]

<dimensions> ::= <unsigned integer> | *
<format> ::= FLOAT32 | FLOAT16 | FLOAT64 | INT8 | BINARY | *
<layout> ::= DENSE | SPARSE
```

A VECTOR type fixes a vector's dimension count, the format of its
elements and its layout, or leaves them open. `*` leaves the dimension
count or the format open, and VECTOR alone leaves all three open, so
such a column holds vectors of any shape. The format is FLOAT32 and the
layout DENSE unless written. The words are read in any case.

| Format | Each element |
|---|---|
| FLOAT32 | a single precision number, the default |
| FLOAT16 | a half precision number |
| FLOAT64 | a double precision number |
| INT8 | an integer from -128 to 127 |
| BINARY | one bit; the dimension count is a multiple of 8 |

A dense vector keeps every element. A sparse vector keeps only the
elements that are not zero, with their positions, as the vectors a
model of words makes are. BINARY is never sparse.

A dimension count is from 1 to 65,535, of a sparse vector to
2,147,483,647, and of BINARY from 8 to 65,528, a multiple of 8.
A type out of these bounds, or of an unknown format, is refused with
SQLSTATE 2A000 (SQLCODE -103). An element is never null, NaN or
infinite.

VECTOR_DIMENSION_COUNT, VECTOR_DIMENSION_FORMAT, VECTOR_LAYOUT and
VECTOR_NNZ, the elements that are not zero, say what a value is:

```sql
SELECT ID, VECTOR_DIMENSION_COUNT(EMBEDDING) AS DIMS, VECTOR_DIMENSION_FORMAT(EMBEDDING) AS FORMAT,
    VECTOR_LAYOUT(EMBEDDING) AS LAYOUT, VECTOR_NNZ(EMBEDDING) AS NNZ FROM ITEMS ORDER BY ID;
```

```
           ID |         DIMS | FORMAT           | LAYOUT           |          NNZ
--------------+--------------+------------------+------------------+--------------
            1 |            3 | FLOAT32          | DENSE            |            1
            2 |            3 | FLOAT32          | DENSE            |            2
            3 |            3 | FLOAT32          | DENSE            |            2
            4 |         NULL | NULL             | NULL             |         NULL
(4 rows)
```

## Writing vectors

A vector is written as text. A dense vector is a list of its elements
in brackets. A BINARY vector is a list of bytes, from 0 to 255, each
eight elements, the first the highest bit. A sparse vector is its
dimension count, the positions of its elements that are not zero,
counted from 0, and those elements:

```
'[0.12, -0.5, 3.0E-2]'
'[7, 200]'
'[30522, [17, 2048], [0.4, 1.2]]'
```

A sparse vector may be written as a JSON object too,
`'{"dims":30522,"indices":[17,2048],"values":[0.4,1.2]}'`.

Text is made a vector by CAST, by `TO_VECTOR(text [, dims [, format [,
layout]]])`, or by VECTOR, its other name. Text stored in a VECTOR
column by INSERT or UPDATE is made one of the column's type, and
arithmetic and the distance functions take text for one operand. A
vector is made text by FROM_VECTOR or by CAST to a character string
type. FROM_VECTOR writes each number in the fewest digits that read
back as it, and may name the layout to write and a CLOB to return:

```
FROM_VECTOR ( <value expression> [ RETURNING { VARCHAR | <character string type> } ] [ FORMAT { DENSE | SPARSE } ] )
```

```sql
SELECT FROM_VECTOR(EMBEDDING) AS DENSE, FROM_VECTOR(EMBEDDING FORMAT SPARSE) AS SPARSE,
    CAST(EMBEDDING AS VARCHAR(40)) AS TEXT_FORM FROM ITEMS WHERE ID = 3;
SELECT FROM_VECTOR(TO_VECTOR('[1.5, 2]', 2, FLOAT16)) AS HALF,
    FROM_VECTOR(CAST('[4, [0, 3], [1, -1]]' AS VECTOR(4, FLOAT32, SPARSE)) RETURNING CLOB FORMAT DENSE) AS DENSE
    FROM ITEMS WHERE ID = 1;
```

```
 DENSE                                                        | SPARSE                                                       | TEXT_FORM
--------------------------------------------------------------+--------------------------------------------------------------+------------------------------------------
 [0,1,0.2]                                                    | [3,[1,2],[1,0.2]]                                            | [0,1,0.2]
(1 row)
 HALF                                                         | DENSE
--------------------------------------------------------------+--------------------------------------------------------------
 [1.5,2]                                                      | [1,0,0,-1]
(1 row)
```

CAST takes a vector to another format, rounding to its precision, and
to another layout. A vector of another dimension count than the type's
raises SQLSTATE 22000, text that is no vector 22018, and an element out
of the format's range 22003, each SQLCODE -301:

```sql
INSERT INTO ITEMS VALUES (5, 'leaf', '[1, 2]');
SELECT CAST('[1, x, 2]' AS VECTOR(3)) FROM ITEMS WHERE ID = 1;
SELECT CAST('[1, 200, 2]' AS VECTOR(3, INT8)) FROM ITEMS WHERE ID = 1;
```

```
turborel: SQLCODE -301 (SQLSTATE 22000): dimension mismatch: expected 3, got 2
turborel: SQLCODE -301 (SQLSTATE 22018): invalid vector literal: '' is not a number
turborel: SQLCODE -301 (SQLSTATE 22003): 200 is not an INT8 element, an integer from -128 to 127
```

## No order and no equality

Vectors have no order, and two vectors near each other are not equal
in any useful sense. So a vector is refused, with SQLSTATE 2A000
(SQLCODE -103), as an operand of a comparison, `=` and `<>` among
them, of BETWEEN, IN, ORDER BY, GROUP BY, DISTINCT, UNION, INTERSECT
and EXCEPT, MAX and MIN, and as a column of a PRIMARY KEY, UNIQUE or
FOREIGN KEY constraint or of a `btree` or `hash` index. ORDER BY a
distance between vectors is the way to sort by them. VECTOR_EQUALS
tells whether two vectors are of one shape and the same elements:

```sql
SELECT ID FROM ITEMS ORDER BY EMBEDDING;
SELECT ID FROM ITEMS WHERE VECTOR_EQUALS(EMBEDDING, '[1, 0, 0]');
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): ORDER BY orders, groups, compares or keys values of VECTOR(3), which have no order and no equality; VECTOR_EQUALS compares vectors
           ID
--------------
            1
(1 row)
```

## Distances

```
VECTOR_DISTANCE ( <vector> , <vector> [ , <metric> ] )
VECTOR_DISTANCE ( '<metric>' , <vector> , <vector> )
```

VECTOR_DISTANCE gives the distance between two vectors as a DOUBLE
PRECISION, smaller for vectors more alike, by a metric written as a key
word or as a string, in any case. A string first names the metric
before the vectors. Without a metric, it is the metric of an index of
the first vector's column that has one, or COSINE.

| Metric | Distance of a and b | Formats |
|---|---|---|
| COSINE | 1 - a·b / (‖a‖ ‖b‖), null if either is zero | all but BINARY |
| DOT | -(a·b) | all but BINARY |
| EUCLIDEAN | √Σ(aᵢ - bᵢ)² | all but BINARY |
| EUCLIDEAN_SQUARED, L2_SQUARED | Σ(aᵢ - bᵢ)² | all but BINARY |
| MANHATTAN | Σ\|aᵢ - bᵢ\| | all but BINARY |
| HAMMING | the elements that differ | all |
| JACCARD | 1 - \|a ∩ b\| / \|a ∪ b\| of the bits set | BINARY |

The vectors are of one dimension count and format, one dense and one
sparse among them. A null vector gives null. Vectors of other dimension
counts, or a metric of none of their formats, raise SQLSTATE 22000
(SQLCODE -301).

Each metric has an operator and most a function of their own, with the
two vectors as operands:

| Operator | Function | Is |
|---|---|---|
| `<->` | L2_DISTANCE | EUCLIDEAN |
| `<=>` | COSINE_DISTANCE | COSINE |
| `<#>` | | DOT |
| `<+>` | L1_DISTANCE | MANHATTAN |
| `<~>` | HAMMING_DISTANCE | HAMMING |
| `<%>` | JACCARD_DISTANCE | JACCARD |
| | INNER_PRODUCT | a·b, DOT's negation |

An operator binds less tightly than `+` and `-` and more tightly than a
comparison, so `A <-> B + C < 2` is `(A <-> (B + C)) < 2`.

```sql
SELECT NAME, VECTOR_DISTANCE(EMBEDDING, '[1, 0, 0]', EUCLIDEAN) AS EUCLIDEAN,
    VECTOR_DISTANCE('cosine', EMBEDDING, '[1, 0, 0]') AS COSINE, EMBEDDING <#> '[1, 0, 0]' AS DOT
    FROM ITEMS WHERE ID < 5 ORDER BY ID;
```

```
 NAME                 |                EUCLIDEAN |                   COSINE |                      DOT
----------------------+--------------------------+--------------------------+--------------------------
 apple                |                      0E0 |                      0E0 |                     -1E0
 pear                 |    1.4142137441313676E-1 |    6.1162757863052475E-3 |    -8.999999761581421E-1
 carrot               |     1.4282856723544584E0 |                      1E0 |                     -0E0
 stone                |                     NULL |                     NULL |                     NULL
(4 rows)
```

## Finding the nearest

ORDER BY a distance and FETCH FIRST give the rows nearest a vector:

```sql
SELECT NAME FROM ITEMS WHERE EMBEDDING IS NOT NULL ORDER BY EMBEDDING <-> '[0.8, 0.2, 0]', ID FETCH FIRST 2 ROWS ONLY;
```

```
 NAME
----------------------
 pear
 apple
(2 rows)
```

Such a query reads every row. A vector index finds the nearest rows
without reading them all, and FETCH APPROX lets a query take the rows
it finds ([FETCH APPROX and FETCH EXACT](06-queries.md#fetch-approx-and-fetch-exact-extension),
[CREATE VECTOR INDEX](statements/create-vector-index.md)).

## Arithmetic

`+`, `-` and `*` of two vectors are of each pair of elements, `*` of a
number and a vector scales it, and `/` of a vector and a number divides
each element. The result is FLOAT32 of INT8, FLOAT16 or BINARY operands,
FLOAT64 if either is FLOAT64. They are VECTOR_ADD, VECTOR_SUBTRACT,
VECTOR_MULTIPLY, VECTOR_SCALE and VECTOR_DIVIDE as functions. `/` of two
vectors is refused with SQLSTATE 2A000 (SQLCODE -103), and division by
zero raises SQLSTATE 22012 (SQLCODE -301).

```sql
SELECT FROM_VECTOR(EMBEDDING + '[1, 1, 1]') AS PLUS, FROM_VECTOR(2 * EMBEDDING) AS TWICE,
    FROM_VECTOR(EMBEDDING / 4) AS QUARTER FROM ITEMS WHERE ID = 2;
```

```
 PLUS                                                         | TWICE                                                        | QUARTER
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 [1.9,1.1,1]                                                  | [1.8,0.2,0]                                                  | [0.225,0.025,0]
(1 row)
```

## Functions

| Function | Gives |
|---|---|
| VECTOR_NORM(v [, norm]) | the length of v: NORM2 by default, NORM1 or NORMINF |
| VECTOR_NORMALIZE(v) | v divided by its length |
| VECTOR_ELEMENT(v, i) | element i, counted from 1 |
| VECTOR_SLICE(v, from, count) | count elements from element from |
| VECTOR_CONCAT(a, b) | a's elements, then b's |
| VECTOR_EQUALS(a, b) | whether a and b are alike in shape and every element |
| VECTORPROPERTY(v, property) | its Dimensions or its BaseType, the format in lower case |
| VECTOR_TO_ARRAY(v), ARRAY_TO_VECTOR(a) | the elements as an ARRAY, and back |
| VECTOR_QUANTIZE(v, format [, scale [, zero]]) | v in INT8, by scale and zero point; in BINARY, a bit set of each element above the threshold scale, 0 by default; or in FLOAT16 |
| VECTOR_DEQUANTIZE(v, scale, zero) | an INT8 vector back in FLOAT32 |

```sql
SELECT VECTOR_NORM(EMBEDDING) AS NORM, VECTOR_NORM(EMBEDDING, NORM1) AS NORM1,
    FROM_VECTOR(VECTOR_NORMALIZE(EMBEDDING)) AS UNIT, FROM_VECTOR(VECTOR_QUANTIZE(EMBEDDING, INT8, 0.01, 0)) AS QUANTIZED
    FROM ITEMS WHERE ID = 3;
```

```
                     NORM |                    NORM1 | UNIT                                                         | QUANTIZED
--------------------------+--------------------------+--------------------------------------------------------------+--------------------------------------------------------------
     1.0198038816452026E0 |     1.2000000029802322E0 | [0,0.9805807,0.19611615]                                     | [0,100,20]
(1 row)
```

## Aggregates

SUM and AVG of vectors, or VECTOR_SUM and VECTOR_AVG, are of each
element; VECTOR_MIN and VECTOR_MAX give each element's least and
greatest. Null vectors are left out, and of no vector the result is
null. A sum of sparse vectors is sparse.

```sql
SELECT FROM_VECTOR(AVG(EMBEDDING)) AS CENTRE, FROM_VECTOR(VECTOR_MAX(EMBEDDING)) AS MOST FROM ITEMS;
```

```
 CENTRE                                                       | MOST
--------------------------------------------------------------+--------------------------------------------------------------
 [0.6333333,0.36666667,0.06666667]                            | [1,1,0.2]
(1 row, with a warning: SQLSTATE 01003)
```

## Distance functions

A user defines a metric of its own as a distance function of SQL,
[CREATE VECTOR DISTANCE FUNCTION](statements/create-vector-distance-function.md),
and VECTOR_DISTANCE names it as it names a metric. A program embedding
Turborel registers one written in Rust with `Database::register_metric`
([The Embedded Library](../client/03-embedding.md)), and a vector index
may measure by a metric so registered.

## The catalog

INFORMATION_SCHEMA.VECTOR_COLUMNS has a row for each VECTOR column of a
table the user holds a privilege on, with the dimension count, format
and layout its type fixes, null of one it leaves open.
INFORMATION_SCHEMA.VECTOR_DISTANCE_FUNCTIONS has a row for each
distance function the user holds EXECUTE on
([The Information Schema](07-information-schema.md)).

```sql
SELECT TABLE_NAME, COLUMN_NAME, VECTOR_DIMENSIONS, VECTOR_FORMAT, VECTOR_LAYOUT FROM INFORMATION_SCHEMA.VECTOR_COLUMNS;
```

```
 TABLE_NAME                                                   | COLUMN_NAME                                                  | VECTOR_DIMENSIONS | VECTOR_FORMAT    | VECTOR_LAYOUT
--------------------------------------------------------------+--------------------------------------------------------------+-------------------+------------------+------------------
 ITEMS                                                        | EMBEDDING                                                    |                 3 | FLOAT32          | DENSE
(1 row)
```

## Errors

| SQLSTATE | SQLCODE | Raised when |
|---|---|---|
| 2A000 | -103 | a VECTOR type out of its bounds; a vector ordered, compared or keyed; `/` of two vectors |
| 22000 | -301 | vectors of other dimension counts or formats; a vector of another dimension count than its type's; a metric of none of the vectors' formats, or of no name |
| 22001 | -301 | FROM_VECTOR of a vector whose text is longer than a character string |
| 22003 | -301 | an element out of its format's range |
| 22012 | -301 | a vector divided by zero |
| 22018 | -301 | text that is no vector |
| 2202E | -301 | VECTOR_ELEMENT or VECTOR_SLICE outside the vector; VECTOR_TO_ARRAY of more elements than an array holds |
| 2A000 | -104 | a distance function used by a user without EXECUTE on it |
| TB910 | -910 | a registered metric that breaks its contract, as a negative distance of one declared NON_NEGATIVE |

## See also

[CREATE VECTOR DISTANCE FUNCTION](statements/create-vector-distance-function.md),
[DROP VECTOR DISTANCE FUNCTION](statements/drop-vector-distance-function.md),
[CREATE VECTOR INDEX](statements/create-vector-index.md),
[Hybrid Search](15-hybrid-search.md),
[Query Expressions](06-queries.md#fetch-approx-and-fetch-exact-extension),
[Vector Guide](../data/04-vectors.md)
