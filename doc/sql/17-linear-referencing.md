# Linear Referencing

[Contents](README.md)

Linear referencing places things by a distance along a line, such as a
post 25 metres along a road, rather than by coordinates. SQL/MM Spatial
§15 gives the types that do it. A linear referencing method, an ST_LRM,
says how distances are measured. A linear element, such as an
ST_LRCurve, is the line they are measured along. A position expression,
an ST_PositionExp, names an element, a method and a distance, and
locates a point.

The examples on this page are run in order, by the user ANN, who has a
table of roads:

```sql
CREATE SCHEMA AUTHORIZATION ANN;
CREATE TABLE ROADS (ID INTEGER, ROAD ST_LRCURVE, LRM ST_LRM);
```

```
OK
OK
```

## Methods and measures

An ST_LRM is made by its constructor, or read from its well-known text.
An ST_LRMeasure is a measure and, if given, its unit. Their observers are
methods, and a mutator returns a changed copy:

```sql
INSERT INTO ROADS VALUES (0, NULL, NEW ST_LRM(1, 'km', 'absolute', 'metre', ARRAY['posts']));
SELECT LRM.ST_LRMNAME() AS NAME, LRM.ST_LRMTYPE() AS KIND, LRM.ST_UNITOFMEASURE() AS UNIT,
    LRM.ST_LRMNAME('chainage').ST_LRMNAME() AS RENAMED
    FROM ROADS;
SELECT ST_LRMEASURE(2.5, 'foot').ST_UNITOFMEASURE() AS UNIT FROM ROADS;
```

```
OK
 NAME                                                         | KIND                                                         | UNIT                                                         | RENAMED
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 km                                                           | absolute                                                     | metre                                                        | chainage
(1 row)
 UNIT
--------------------------------------------------------------
 foot
(1 row)
```

## Locating a point

A road is an ST_LRCurve of a geometry. Here road 1 runs 100 metres east,
measured by LRM 1 from 0. A position expression of the road, the method
and a distance locates the point 25 metres along, and ST_LRPosition finds
the distance of a point back:

```sql
INSERT INTO ROADS VALUES (1, NEW ST_LRCURVE(1, 1, ST_LRMEASURE(0), CAST(NULL AS VARCHAR(8)),
    CAST(NULL AS ST_STARTVALUE ARRAY[2]), ST_GEOMFROMTEXT('LINESTRING (0 0, 100 0)')), NULL);
SELECT ST_ASTEXT(ROAD.ST_POINT(ST_POSITIONEXP(ROAD, ST_LRMFROMTEXT('LRM LRMID 1, LRMNAME "km", LRMTYPE "absolute", LRMUOM "metre"'),
    ST_DISTANCEEXP(ST_LRMEASURE(25, 'metre'))))) AS AT_25
    FROM ROADS WHERE ID = 1;
```

```
OK
 AT_25
--------------------------------------------------------------
 POINT (25 0)
(1 row)
```

## Methods and columns in the catalog

A method defined by ST_DEFINELRM is a row of
ST_INFORMTN_SCHEMA.ST_LRMS, and a column registered by
ST_REGISTERLRCOLUMN says which method its elements use. With both, a
position expression may name its element and method by identifier, and
ST_LRPosition measures by the element's default method:

```sql
CALL ST_DEFINELRM('KM', 1, 'ACME', 7, 'LRM LRMID 1, LRMNAME "km", LRMTYPE "absolute", LRMUOM "metre"', 'posts');
CALL ST_REGISTERLRCOLUMN('ANN', 'ROADS', 'ROAD', 1);
SELECT LRM_NAME, LRMID, DESCRIPTION FROM ST_INFORMTN_SCHEMA.ST_LRMS;
SELECT ST_ASTEXT(ROAD.ST_POINT(ST_POSITIONEXP(1, 1, ST_DISTANCEEXP(ST_LRMEASURE(25, 'metre'))))) AS AT_25
    FROM ROADS WHERE ID = 1;
SELECT ROAD.ST_LRPOSITION(ST_POINT(40, 0)).ST_DISTANCEEXP().ST_DISTANCEALONG().ST_MEASURE() AS ALONG
    FROM ROADS WHERE ID = 1;
```

```
OK
OK
 LRM_NAME                                                     |        LRMID | DESCRIPTION
--------------------------------------------------------------+--------------+--------------------------------------------------------------
 KM                                                           |            1 | posts
(1 row)
 AT_25
--------------------------------------------------------------
 POINT (25 0)
(1 row)
                    ALONG
--------------------------
                      4E1
(1 row)
```

ST_UNREGISTERLRCOLUMN and ST_DROPLRM take them away again. A method still
used by a registered column cannot be dropped:

```sql
CALL ST_DROPLRM('KM');
CALL ST_UNREGISTERLRCOLUMN('ANN', 'ROADS', 'ROAD');
CALL ST_DROPLRM('KM');
```

```
turborel: SQLCODE -201 (SQLSTATE 23000): [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §21.2] ST_LR_COLUMNS: LRM_SUPPORTED: column ROAD of ANN.ROADS is registered with KM
OK
OK
```
