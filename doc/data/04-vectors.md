# Vectors

[Specialized Data](README.md)

A recipe site keeps an embedding of each recipe, a vector a model made
of its text, so recipes alike in kind have vectors near each other. It
needs the recipes nearest one a reader likes, and the nearest a query's
vector. This guide stores the embeddings in a VECTOR column
*(extension)*, finds the nearest by distance, compares the metrics,
works with the vectors' arithmetic, and defines a distance of its own.
The [reference](../sql/14-vectors.md) gives every rule.

## The recipes

Real embeddings have hundreds of dimensions. These have four, so the
numbers can be read:

```sql
CREATE SCHEMA AUTHORIZATION ANN;
CREATE TABLE RECIPES (ID INTEGER PRIMARY KEY, TITLE VARCHAR(40), EMBEDDING VECTOR(4));
INSERT INTO RECIPES VALUES
    (1, 'Tomato soup', '[0.9, 0.1, 0.0, 0.2]'),
    (2, 'Minestrone', '[0.8, 0.3, 0.1, 0.1]'),
    (3, 'Chocolate cake', '[0.0, 0.1, 0.9, 0.3]'),
    (4, 'Brownies', '[0.1, 0.0, 0.8, 0.4]'),
    (5, 'Green salad', '[0.4, 0.9, 0.0, 0.0]');
COMMIT;
```

```
OK
OK
OK
OK
```

The text is made a vector of the column's type as it is stored. Text
of another dimension count is refused, never cut or padded:

```sql
INSERT INTO RECIPES VALUES (6, 'Bread', '[0.3, 0.3, 0.3]');
```

```
turborel: SQLCODE -301 (SQLSTATE 22000): dimension mismatch: expected 4, got 3
```

## Finding the nearest

A reader liked the tomato soup. The recipes nearest it, by the
Euclidean distance the `<->` operator gives, are the ones to show next.
ORDER BY the distance and FETCH FIRST read every recipe and keep the
nearest:

```sql
SELECT TITLE, EMBEDDING <-> (SELECT EMBEDDING FROM RECIPES WHERE ID = 1) AS DISTANCE FROM RECIPES
    WHERE ID <> 1 ORDER BY DISTANCE FETCH FIRST 3 ROWS ONLY;
```

```
 TITLE                                    |                 DISTANCE
------------------------------------------+--------------------------
 Minestrone                               |       2.6457513166967E-1
 Green salad                              |     9.643650180005846E-1
 Brownies                                 |     1.1532562263894588E0
(3 rows)
```

A search box's text is embedded by the model too, and its vector is
given as a parameter or written as text:

```sql
SELECT TITLE FROM RECIPES ORDER BY EMBEDDING <=> '[0.1, 0.1, 0.9, 0.2]' FETCH FIRST 2 ROWS ONLY;
```

```
 TITLE
------------------------------------------
 Chocolate cake
 Brownies
(2 rows)
```

## Indexing

A site of millions of recipes cannot read each one for every query. A
vector index finds the nearest without reading them all. It is
approximate, and may miss a near row to answer sooner, so a query asks
for its answer with FETCH APPROX, and ORDER BY the distance the index
measures by:

```sql
COMMIT;
CREATE VECTOR INDEX RECIPES_V ON RECIPES (EMBEDDING) USING hnsw WITH (METRIC = 'COSINE');
SELECT TITLE FROM RECIPES ORDER BY VECTOR_DISTANCE(EMBEDDING, '[0.1, 0.1, 0.9, 0.2]', COSINE) FETCH APPROX FIRST 2 ROWS ONLY;
```

```
OK
OK
 TITLE
------------------------------------------
 Chocolate cake
 Brownies
(2 rows)
```

FETCH FIRST without APPROX is always exact, and reads every row as
before. [CREATE VECTOR INDEX](../sql/statements/create-vector-index.md)
has the methods and how they are tuned.

## Choosing a metric

VECTOR_DISTANCE names its metric. COSINE, the default, compares the
vectors' directions alone, so it suits a model whose vectors' lengths
mean nothing. EUCLIDEAN compares their positions, and DOT, the negated
dot product, suits vectors made of length 1. A smaller distance is
always the nearer:

```sql
SELECT TITLE, VECTOR_DISTANCE(EMBEDDING, '[0.9, 0.2, 0.0, 0.1]', COSINE) AS COSINE,
    VECTOR_DISTANCE(EMBEDDING, '[0.9, 0.2, 0.0, 0.1]', EUCLIDEAN) AS EUCLIDEAN,
    VECTOR_DISTANCE(EMBEDDING, '[0.9, 0.2, 0.0, 0.1]', DOT) AS DOT
    FROM RECIPES WHERE ID IN (1, 2, 5) ORDER BY ID;
```

```
 TITLE                                    |                   COSINE |                EUCLIDEAN |                      DOT
------------------------------------------+--------------------------+--------------------------+--------------------------
 Tomato soup                              |     1.162801023930149E-2 |     1.414213612422477E-1 |    -8.499999046325684E-1
 Minestrone                               |    1.6335059691790832E-2 |    1.7320507344417657E-1 |    -7.899999618530273E-1
 Green salad                              |      4.08767021850181E-1 |     8.660253693716803E-1 |    -5.399999618530273E-1
(3 rows)
```

Vectors have no order and no equality, so a query cannot sort, group
or compare them directly:

```sql
SELECT TITLE FROM RECIPES ORDER BY EMBEDDING;
```

```
turborel: SQLCODE -103 (SQLSTATE 2A000): ORDER BY orders, groups, compares or keys values of VECTOR(4), which have no order and no equality; VECTOR_EQUALS compares vectors
```

## Working with vectors

The centre of a group of vectors is their average. The site's "soups"
are the recipes near the centre of the two soups' vectors:

```sql
SELECT FROM_VECTOR(AVG(EMBEDDING)) AS SOUPS FROM RECIPES WHERE ID IN (1, 2);
SELECT R.TITLE FROM RECIPES AS R, (SELECT AVG(EMBEDDING) FROM RECIPES WHERE ID IN (1, 2)) AS S (CENTRE)
    ORDER BY R.EMBEDDING <-> S.CENTRE FETCH FIRST 3 ROWS ONLY;
```

```
 SOUPS
--------------------------------------------------------------
 [0.85,0.2,0.05,0.15]
(1 row)
 TITLE
------------------------------------------
 Tomato soup
 Minestrone
 Green salad
(3 rows)
```

Arithmetic moves a vector: the tomato soup, less what makes it a soup
and more what makes a dessert, is nearest the desserts. A distance
operator binds less tightly than `+` and `-`, so the sum is measured:

```sql
SELECT R.TITLE FROM RECIPES AS R, RECIPES AS SOUP WHERE SOUP.ID = 1
    ORDER BY R.EMBEDDING <-> SOUP.EMBEDDING - '[0.8, 0, 0, 0]' + '[0, 0, 0.8, 0]' FETCH FIRST 1 ROW ONLY;
```

```
 TITLE
------------------------------------------
 Chocolate cake
(1 row)
```

A vector of length 1 keeps its direction alone. VECTOR_NORMALIZE makes
one, and VECTOR_QUANTIZE keeps a vector in a smaller format, here a
byte of each element:

```sql
SELECT FROM_VECTOR(VECTOR_NORMALIZE(EMBEDDING)) AS UNIT,
    FROM_VECTOR(VECTOR_QUANTIZE(EMBEDDING, INT8, 0.01, 0)) AS BYTES FROM RECIPES WHERE ID = 3;
```

```
 UNIT                                                         | BYTES
--------------------------------------------------------------+--------------------------------------------------------------
 [0,0.104828484,0.94345635,0.31448546]                        | [0,10,90,30]
(1 row)
```

## A distance of one's own

The site weighs the first dimension, savoury, twice the others. A
distance function of SQL measures that way, and VECTOR_DISTANCE names
it as it names a metric:

```sql
CREATE VECTOR DISTANCE FUNCTION SAVOURY (A VECTOR, B VECTOR) RETURNS FLOAT64 DETERMINISTIC
    PROPERTIES (SYMMETRIC, NON_NEGATIVE) LANGUAGE SQL
    AS 'VECTOR_NORM((A - B) * ''[2, 1, 1, 1]'')';
SELECT TITLE, VECTOR_DISTANCE(EMBEDDING, '[0.9, 0.1, 0.0, 0.2]', SAVOURY) AS D FROM RECIPES ORDER BY D, ID;
```

```
OK
 TITLE                                    |                        D
------------------------------------------+--------------------------
 Tomato soup                              |                      0E0
 Minestrone                               |     3.162277340888977E-1
 Green salad                              |     1.2961480617523193E0
 Brownies                                 |      1.802775502204895E0
 Chocolate cake                           |      2.014944076538086E0
(5 rows)
```

[CREATE VECTOR DISTANCE FUNCTION](../sql/statements/create-vector-distance-function.md)
has every rule. The [reference](../sql/14-vectors.md) has sparse
vectors, the other formats, every function and the catalog's views.
