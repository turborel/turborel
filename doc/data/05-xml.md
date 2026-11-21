# XML Data

[Specialized Data](README.md)

A shop buys from suppliers who send their product catalogs as XML
documents, and it sends its orders back as XML. This guide stores the
catalogs, checks each against an XML Schema the shop registers, reads
values out of them with XMLQUERY and XMLCAST, finds catalogs with
XMLEXISTS, turns their products into rows with XMLTABLE, asks XQuery
for more, builds orders with XMLELEMENT and XMLAGG, and indexes the
documents by path and value. The [reference](../sql/19-xml.md) gives
every rule.

## The catalogs

The shop registers the catalogs' XML Schema with
[CREATE XML SCHEMA](../sql/statements/create-xml-schema.md). A catalog
names its supplier and holds products, each with a SKU, a name, a
price and any number of tags. The XML column holds one document a row,
and a CHECK of IS VALID keeps out a document the schema does not
accept. XMLPARSE reads the text, and STRIP WHITESPACE drops the white
space between elements:

```sql
CREATE XML SCHEMA CATALOG_XSD AS
  '<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="catalog"><xs:complexType><xs:sequence>'
  '<xs:element name="product" maxOccurs="unbounded"><xs:complexType><xs:sequence>'
  '<xs:element name="name" type="xs:string"/><xs:element name="price" type="xs:decimal"/>'
  '<xs:element name="tag" type="xs:string" minOccurs="0" maxOccurs="unbounded"/>'
  '</xs:sequence><xs:attribute name="sku" type="xs:string" use="required"/></xs:complexType></xs:element>'
  '</xs:sequence><xs:attribute name="supplier" type="xs:string" use="required"/></xs:complexType></xs:element></xs:schema>';
CREATE TABLE CATALOGS (ID INTEGER PRIMARY KEY, RECEIVED DATE,
    DOC XML(DOCUMENT(UNTYPED)) CHECK (DOC IS VALID DOCUMENT ACCORDING TO XMLSCHEMA ID CATALOG_XSD));
INSERT INTO CATALOGS VALUES
    (1, DATE '2026-10-01', XMLPARSE(DOCUMENT
        '<catalog supplier="Penhallow Paper">'
        '<product sku="PEN"><name>Fountain pen</name><price>12.50</price><tag>writing</tag></product>'
        '<product sku="INK"><name>Blue ink</name><price>4.20</price><tag>writing</tag><tag>refill</tag></product>'
        '</catalog>' STRIP WHITESPACE)),
    (2, DATE '2026-10-15', XMLPARSE(DOCUMENT
        '<catalog supplier="Atlas Office">'
        '<product sku="PAD"><name>Note pad</name><price>3.00</price></product>'
        '<product sku="CLIP"><name>Paper clips</name><price>1.10</price><tag>desk</tag></product>'
        '<product sku="LAMP"><name>Desk lamp</name><price>29.90</price><tag>desk</tag></product>'
        '</catalog>' STRIP WHITESPACE));
COMMIT;
INSERT INTO CATALOGS VALUES (3, DATE '2026-10-20', XMLPARSE(DOCUMENT '<catalog supplier="Quill & Co">' STRIP WHITESPACE));
INSERT INTO CATALOGS VALUES (3, DATE '2026-10-20', XMLPARSE(DOCUMENT
    '<catalog supplier="Quill and Co"><product sku="NIB"><name>Steel nib</name><price>cheap</price></product></catalog>' STRIP WHITESPACE));
```

```
OK
OK
OK
OK
turborel: SQLCODE -301 (SQLSTATE 2200M): the string is not a well-formed XML document: an entity reference [XML §4.1 [68]] requires a name here [XML §2.3 [5]] (at line 1, column 27) [ISO/IEC 9075-14:2006 draft §10.16 GR6a i]
turborel: SQLCODE -201 (SQLSTATE 23000): CHECK ANN.CATALOGS_CHECK1 of ANN.CATALOGS is false for a row [ISO/IEC 9075:1992 §11.6 GR3]
```

The first is no XML at all, as a bare `&` must be written `&amp;`. The
second is XML but breaks the schema, and the CHECK says only that. To
learn why, validate the document with XMLVALIDATE, which raises the
error IS VALID keeps to itself:

```sql
VALUES XMLVALIDATE(DOCUMENT XMLPARSE(DOCUMENT
    '<catalog supplier="Quill and Co"><product sku="NIB"><name>Steel nib</name><price>cheap</price></product></catalog>' STRIP WHITESPACE)
    ACCORDING TO XMLSCHEMA ID CATALOG_XSD);
```

```
turborel: SQLCODE -301 (SQLSTATE 2201R): validation failure: [Schema1 §3.3.4 cvc-type.3.1.3] 'cheap' is no value of the element's type [ISO/IEC 9075-14:2006 draft §6.20 GR6b iv]
```

XML values are not comparable, so an XML column is never a key, nor
sorted or grouped. The shop keys its catalogs by ID.

## Reading values

XMLQUERY runs an XQuery expression with the document as its context
item, and XMLCAST turns what it finds into an SQL value. RETURNING
SEQUENCE gives the items found as they are, which an attribute needs.
A path that finds nothing gives the null value with NULL ON EMPTY.
XMLSERIALIZE writes a part of a document back as text:

```sql
SELECT ID, XMLCAST(XMLQUERY('/catalog/@supplier' PASSING BY REF DOC RETURNING SEQUENCE NULL ON EMPTY) AS VARCHAR(20)) AS SUPPLIER,
       XMLCAST(XMLQUERY('count(/catalog/product)' PASSING BY REF DOC RETURNING SEQUENCE NULL ON EMPTY) AS INTEGER) AS PRODUCTS,
       XMLCAST(XMLQUERY('/catalog/product[@sku = "PEN"]/price' PASSING BY REF DOC NULL ON EMPTY) AS DECIMAL(6,2)) AS PEN
  FROM CATALOGS ORDER BY ID;
SELECT ID, XMLSERIALIZE(XMLQUERY('/catalog/product[1]' PASSING BY REF DOC NULL ON EMPTY) AS VARCHAR(100)) AS FIRST_PRODUCT
  FROM CATALOGS ORDER BY ID;
```

```
           ID | SUPPLIER             |     PRODUCTS |      PEN
--------------+----------------------+--------------+----------
            1 | Penhallow Paper      |            2 |    12.50
            2 | Atlas Office         |            3 |     NULL
(2 rows)
           ID | FIRST_PRODUCT
--------------+--------------------------------------------------------------
            1 | <product sku="PEN"><name>Fountain pen</name><price>12.50</price><tag>writing</tag></product>
            2 | <product sku="PAD"><name>Note pad</name><price>3.00</price></product>
(2 rows)
```

## Finding catalogs

XMLEXISTS is true of a document in which the query finds something.
PASSING gives the query SQL values as variables, here the tag asked
for:

```sql
SELECT ID FROM CATALOGS WHERE XMLEXISTS('/catalog/product[tag = $TAG]' PASSING BY REF DOC, 'desk' AS TAG);
SELECT ID FROM CATALOGS WHERE XMLEXISTS('/catalog/product[price > 10]' PASSING BY REF DOC) ORDER BY ID;
```

```
           ID
--------------
            2
(1 row)
           ID
--------------
            1
            2
(2 rows)
```

## Products as rows

XMLTABLE makes a row of each item its row pattern finds, and each
column's path is read from that item. FOR ORDINALITY numbers the rows.
An attribute's column takes `data(@sku)`, its value, not the attribute
itself. The rows then join, group and sum as any table's do:

```sql
SELECT C.ID, P.N, P.SKU, P.NAME, P.PRICE, P.TAGS
  FROM CATALOGS AS C, XMLTABLE('/catalog/product' PASSING BY REF C.DOC
       COLUMNS N FOR ORDINALITY,
               SKU VARCHAR(5) PATH 'data(@sku)',
               NAME VARCHAR(20) PATH 'name',
               PRICE DECIMAL(6,2) PATH 'price',
               TAGS INTEGER PATH 'count(tag)') AS P
  ORDER BY C.ID, P.N;
SELECT T.TAG, COUNT(*) AS PRODUCTS, MIN(T.PRICE) AS CHEAPEST
  FROM CATALOGS AS C, XMLTABLE('/catalog/product/tag' PASSING BY REF C.DOC
       COLUMNS TAG VARCHAR(10) PATH '.', PRICE DECIMAL(6,2) PATH '../price') AS T
  GROUP BY T.TAG ORDER BY T.TAG;
```

```
           ID |            N | SKU   | NAME                 |    PRICE |         TAGS
--------------+--------------+-------+----------------------+----------+--------------
            1 |            1 | PEN   | Fountain pen         |    12.50 |            1
            1 |            2 | INK   | Blue ink             |     4.20 |            2
            2 |            1 | PAD   | Note pad             |     3.00 |            0
            2 |            2 | CLIP  | Paper clips          |     1.10 |            1
            2 |            3 | LAMP  | Desk lamp            |    29.90 |            1
(5 rows)
 TAG        |             PRODUCTS | CHEAPEST
------------+----------------------+----------
 desk       |                    2 |     1.10
 refill     |                    1 |     4.20
 writing    |                    2 |     4.20
(3 rows)
```

## Typed values

The column is UNTYPED, so XQuery reads a price as untyped text and
does arithmetic on it as a double, with a double's rounding. Validated,
the document's elements carry the schema's types, and the prices are
decimals. XMLVALIDATE validates a value in SQL, and XQuery's validate
expression does it in a query. Here each works out the cost of three
of every product:

```sql
SELECT ID, XMLSERIALIZE(XMLQUERY('sum(/catalog/product/price) * 3' PASSING BY REF DOC RETURNING SEQUENCE NULL ON EMPTY) AS VARCHAR(30)) AS UNTYPED,
       XMLSERIALIZE(XMLQUERY('sum(/catalog/product/price) * 3' PASSING BY REF XMLVALIDATE(DOCUMENT DOC ACCORDING TO XMLSCHEMA ID CATALOG_XSD) RETURNING SEQUENCE NULL ON EMPTY) AS VARCHAR(30)) AS VALIDATED,
       XMLSERIALIZE(XMLQUERY('sum((validate { . })/catalog/product/price) * 3' PASSING BY REF DOC RETURNING SEQUENCE NULL ON EMPTY) AS VARCHAR(30)) AS IN_QUERY
  FROM CATALOGS ORDER BY ID;
```

```
           ID | UNTYPED                        | VALIDATED                      | IN_QUERY
--------------+--------------------------------+--------------------------------+--------------------------------
            1 | 50.099999999999994             | 50.1                           | 50.1
            2 | 102                            | 102                            | 102
(2 rows)
```

A column of XML(DOCUMENT(XMLSCHEMA ID CATALOG_XSD)) would keep the
documents validated, so every query read them typed
([The XMLSCHEMA types](../sql/19-xml.md#the-xmlschema-types)).

## Asking XQuery

A query may be any XQuery 1.0 expression. A FLWOR expression picks,
sorts and reshapes the products, and constructs new elements of them.
The functions of XQuery work on strings, sequences and numbers, and a
quantified expression asks whether every product holds a condition:

```sql
SELECT ID, XMLSERIALIZE(XMLQUERY(
    'for $p in /catalog/product where $p/price < 20 '
    'order by xs:decimal($p/price) descending '
    'return <offer sku="{$p/@sku}">{concat(upper-case($p/name), " at ", $p/price)}</offer>'
    PASSING BY REF DOC NULL ON EMPTY) AS VARCHAR(200)) AS OFFERS
  FROM CATALOGS ORDER BY ID;
SELECT ID, XMLCAST(XMLQUERY(
    'string-join(for $t in distinct-values(//tag) order by $t return $t, ", ")'
    PASSING BY REF DOC RETURNING SEQUENCE NULL ON EMPTY) AS VARCHAR(40)) AS TAGS,
    XMLCAST(XMLQUERY('every $p in /catalog/product satisfies exists($p/tag)'
    PASSING BY REF DOC RETURNING SEQUENCE NULL ON EMPTY) AS BOOLEAN) AS ALL_TAGGED
  FROM CATALOGS ORDER BY ID;
```

```
           ID | OFFERS
--------------+--------------------------------------------------------------
            1 | <offer sku="PEN">FOUNTAIN PEN at 12.50</offer><offer sku="INK">BLUE INK at 4.20</offer>
            2 | <offer sku="PAD">NOTE PAD at 3.00</offer><offer sku="CLIP">PAPER CLIPS at 1.10</offer>
(2 rows)
           ID | TAGS                                     | ALL_TAGGED
--------------+------------------------------------------+------------
            1 | refill, writing                          | TRUE
            2 | desk                                     | FALSE
(2 rows)
```

A function many queries share goes in an XQuery library module,
registered by [CREATE XQUERY MODULE](../sql/statements/create-xquery-module.md)
and imported by a query's prolog. This one gives a product's price
with 20% tax:

```sql
CREATE XQUERY MODULE SHOP_LIB AS
  'module namespace s = "urn:shop";'
  'declare function s:gross($p as element(product)) as xs:decimal { round-half-to-even(xs:decimal($p/price) * 1.2, 2) };';
SELECT C.ID, P.SKU, P.GROSS
  FROM CATALOGS AS C, XMLTABLE('import module namespace s = "urn:shop"; '
       'for $p in /catalog/product return <p sku="{$p/@sku}" gross="{s:gross($p)}"/>' PASSING BY REF C.DOC
       COLUMNS SKU VARCHAR(5) PATH 'data(@sku)', GROSS DECIMAL(6,2) PATH 'data(@gross)') AS P
  WHERE C.ID = 1;
```

```
OK
           ID | SKU   |    GROSS
--------------+-------+----------
            1 | PEN   |    15.00
            1 | INK   |     5.04
(2 rows)
```

## Building XML

The shop keeps its orders as rows and sends each as XML. XMLELEMENT
makes an element, XMLATTRIBUTES its attributes and XMLFOREST an element
of each value. A null value makes no attribute, and no element of
XMLFOREST. XMLAGG puts a group's elements one after another, in the
order its ORDER BY gives. Text is escaped as XML needs:

```sql
CREATE TABLE ORDER_LINES (ORDER_NO INTEGER, SKU VARCHAR(5), QTY INTEGER, NOTE VARCHAR(40));
INSERT INTO ORDER_LINES VALUES (501, 'PEN', 10, NULL), (501, 'INK', 30, 'blue & black'), (502, 'LAMP', 2, NULL);
COMMIT;
SELECT ORDER_NO, XMLSERIALIZE(XMLELEMENT(NAME "order", XMLATTRIBUTES(ORDER_NO AS "no"),
           XMLAGG(XMLELEMENT(NAME "line", XMLATTRIBUTES(SKU AS "sku"), XMLFOREST(QTY AS "qty", NOTE AS "note")) ORDER BY SKU))
       AS VARCHAR(200)) AS DOC
  FROM ORDER_LINES GROUP BY ORDER_NO ORDER BY ORDER_NO;
```

```
OK
OK
OK
     ORDER_NO | DOC
--------------+--------------------------------------------------------------
          501 | <order no="501"><line sku="INK"><qty>30</qty><note>blue &amp; black</note></line><line sku="PEN"><qty>10</qty></line></order>
          502 | <order no="502"><line sku="LAMP"><qty>2</qty></line></order>
(2 rows)
```

A whole document to send joins the order's lines to the catalog's
products for their names and supplier. XMLPI makes a processing
instruction and XMLCOMMENT a comment, XMLTEXT makes text, and
XMLCONCAT puts values side by side. XMLDOCUMENT makes them one
document, which XMLSERIALIZE writes with an XML declaration:

```sql
SELECT XMLSERIALIZE(DOCUMENT XMLDOCUMENT(XMLCONCAT(
           XMLPI(NAME "xml-stylesheet", 'type="text/xsl" href="order.xsl"'),
           XMLCOMMENT('sent by the shop'),
           XMLELEMENT(NAME "order", XMLATTRIBUTES(L.ORDER_NO AS "no", P.SUPPLIER AS "to"),
               XMLAGG(XMLELEMENT(NAME "line", XMLTEXT(P.NAME), XMLTEXT(' x '), L.QTY) ORDER BY P.NAME))))
       AS VARCHAR(300) INCLUDING XMLDECLARATION) AS DOC
  FROM ORDER_LINES AS L, CATALOGS AS C, XMLTABLE('/catalog/product' PASSING BY REF C.DOC
       COLUMNS SKU VARCHAR(5) PATH 'data(@sku)', NAME VARCHAR(20) PATH 'name', SUPPLIER VARCHAR(20) PATH 'data(../@supplier)') AS P
  WHERE P.SKU = L.SKU AND L.ORDER_NO = 501
  GROUP BY L.ORDER_NO, P.SUPPLIER;
```

```
 DOC
--------------------------------------------------------------
 <?xml version="1.0" encoding="UTF-8"?><?xml-stylesheet type="text/xsl" href="order.xsl"?><!--sent by the shop--><order no="501" to="Penhallow Paper"><line>Blue ink x 30</line><line>Fountain pen x 10</line></order>
(1 row)
```

## Indexes

An XML index *(extension)* keys the paths and values of a column's
documents, so a query of a path reads the rows whose documents hold
it, not every row ([XML indexes](../sql/19-xml.md#xml-indexes)). Of a
few catalogs reading them all costs less, so the shop first receives
many more. XML_PATH keys each path and the value at it. An index
statement runs outside a transaction, so COMMIT ends the one the
insert began:

```sql
INSERT INTO CATALOGS WITH RECURSIVE N (I) AS (VALUES (CAST(3 AS INTEGER)) UNION ALL SELECT I + 1 FROM N WHERE I < 400)
  SELECT I, DATE '2026-11-01', XMLPARSE(DOCUMENT
    '<catalog supplier="Supplier ' || CAST(I AS VARCHAR(3)) || '"><product sku="S' || CAST(I AS VARCHAR(3)) || '">'
    || '<name>Item ' || CAST(I AS VARCHAR(3)) || '</name><price>' || CAST(I AS VARCHAR(3)) || '.00</price>'
    || CASE WHEN I > 397 THEN '<tag>clearance</tag>' ELSE '' END || '</product></catalog>' STRIP WHITESPACE)
  FROM N;
COMMIT;
CREATE INDEX CATALOG_PATHS ON CATALOGS (DOC) USING XML_PATH;
SELECT ID FROM CATALOGS WHERE XMLEXISTS('/catalog[@supplier = "Atlas Office"]' PASSING BY REF DOC);
EXPLAIN SELECT ID FROM CATALOGS WHERE XMLEXISTS('/catalog[@supplier = "Atlas Office"]' PASSING BY REF DOC);
```

```
OK
OK
OK
           ID
--------------
            2
(1 row)
 PLAN
--------------------------------------------------------------
 -- bound:
 Project CATALOGS.ID
   Filter XMLEXISTS(CATALOGS.DOC)
     Scan ANN.CATALOGS
 -- rules: none
 -- optimized:
 Project CATALOGS.ID
   Filter XMLEXISTS(CATALOGS.DOC)
     Scan ANN.CATALOGS
 -- physical:
 Project CATALOGS.ID
   Filter XMLEXISTS(CATALOGS.DOC)  (est. rows=1)
     IndexScan ANN.CATALOGS using CATALOG_PATHS [xml_path]: CATALOGS.DOC has /catalog/@supplier = 'Atlas Office'  (est. rows=1, cost=60.5)
(13 rows)
```

The planner reads the index for an XMLEXISTS of one path from the
document's root, its predicates compared with literals, and for an
XMLTABLE joined to its rows. Each row the index gives is checked by the
whole query, so the index changes no result:

```sql
SELECT C.ID, P.SKU FROM CATALOGS AS C, XMLTABLE('/catalog/product[tag = "clearance"]' PASSING BY REF C.DOC
       COLUMNS SKU VARCHAR(5) PATH 'data(@sku)') AS P ORDER BY C.ID;
EXPLAIN SELECT C.ID, P.SKU FROM CATALOGS AS C, XMLTABLE('/catalog/product[tag = "clearance"]' PASSING BY REF C.DOC
       COLUMNS SKU VARCHAR(5) PATH 'data(@sku)') AS P;
```

```
           ID | SKU
--------------+-------
          398 | S398
          399 | S399
          400 | S400
(3 rows)
 PLAN
--------------------------------------------------------------
 -- bound:
 Project C.ID, P.SKU
   Unnest XMLTABLE(C.DOC, NULL)
     Scan ANN.CATALOGS
 -- rules: none
 -- optimized:
 Project C.ID, P.SKU
   Unnest XMLTABLE(C.DOC, NULL)
     Scan ANN.CATALOGS
 -- physical:
 Project C.ID, P.SKU
   Unnest of 1 collections
     IndexScan ANN.CATALOGS using CATALOG_PATHS [xml_path]: C.DOC has /catalog/product/tag = 'clearance'  (est. rows=3, cost=77.7)
(13 rows)
```

XML_VALUE keys a value first, for a value sought at a path given in
part. XML_PRIMARY keeps each node whole, and answers a query of a path
and its steps' predicates from the index alone, without reading the
documents. It suits large documents queried for a few of their nodes,
and takes about twice the space:

```sql
COMMIT;
DROP INDEX CATALOG_PATHS;
CREATE INDEX CATALOG_NODES ON CATALOGS (DOC) USING XML_PRIMARY;
SELECT ID FROM CATALOGS WHERE XMLEXISTS('/catalog/product[@sku = "S399"]/tag' PASSING BY REF DOC);
EXPLAIN SELECT ID FROM CATALOGS WHERE XMLEXISTS('/catalog/product[@sku = "S399"]/tag' PASSING BY REF DOC);
```

```
OK
OK
OK
           ID
--------------
          399
(1 row)
 PLAN
--------------------------------------------------------------
 -- bound:
 Project CATALOGS.ID
   Filter XMLEXISTS(CATALOGS.DOC)
     Scan ANN.CATALOGS
 -- rules: none
 -- optimized:
 Project CATALOGS.ID
   Filter XMLEXISTS(CATALOGS.DOC)
     Scan ANN.CATALOGS
 -- physical:
 Project CATALOGS.ID
   IndexScan ANN.CATALOGS using CATALOG_NODES [xml_primary]: CATALOGS.DOC has /catalog/product[@sku = 'S399']/tag, answered by the index  (est. rows=9, cost=129.3)
(12 rows)
```
