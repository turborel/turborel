# Store and Exchange Geometries

[Spatial Data](README.md) · [Previous: coordinates and geometry](01-model.md) ·
[Next: R-tree queries](03-indexing.md)

This chapter assumes geometry functions are enabled as described in the
[guide introduction](README.md), and that a schema is selected. Numeric R-tree queries in the
next chapter work without that extension.

## Create and populate a geometry column

Use a geometry column when the shape itself is part of the data:

```sql
CREATE TABLE places (
    id INTEGER PRIMARY KEY,
    name VARCHAR(30),
    shape GEOMETRY(4326)
);
INSERT INTO places VALUES
    (1, 'Shop', ST_POINT(4.9, 52.37, 4326)),
    (2, 'Unlocated', ST_GEOMFROMTEXT('POINT EMPTY', 4326)),
    (3, 'Unknown', NULL);
COMMIT;
SELECT id, ST_ASTEXT(shape), ST_SRID(shape), ST_ISEMPTY(shape)
FROM places ORDER BY id;
```

Result rows:

```text
1,POINT (4.9 52.37),4326,0
2,POINT EMPTY,4326,1
3,NULL,NULL,NULL
```

The empty point retains its SRID; the NULL row has no geometry metadata.
`GEOMETRY(4326)` accepts any of the seven supported kinds but requires
SRID 4326 on every non-NULL value, including empty geometries. An INSERT
or UPDATE with another SRID fails atomically with SQLCODE -301. The
constraint survives recovery and applies to prepared statements too.
Plain `GEOMETRY` accepts any signed 32-bit SRID. The optional column
parameter accepts 0 through 2147483647.

Checking the identifier does not validate coordinate ranges or look up a
CRS. A geometry kind constraint is not yet available, and registered
functions cannot currently be used in stored CHECK conditions.
Applications requiring only points must validate the kind before writing.

`ST_POINT` constructs a point directly from numbers or numeric columns.
Read its coordinates back with `ST_X` and `ST_Y`:

```sql
SELECT ST_X(shape), ST_Y(shape) FROM places WHERE id = 1;
```

```text
4.9E0,5.237E1
```

Both observers return NULL for POINT EMPTY and reject non-point kinds.
They do not interpret the coordinates as longitude, latitude or metres.

## Construct windows and inspect bounds

Build a rectangular polygon from numbers with `ST_MAKEENVELOPE`:

```sql
VALUES ST_ASTEXT(ST_MAKEENVELOPE(0, 0, 10, 20));
```

```text
POLYGON ((0 0, 0 20, 10 20, 10 0, 0 0))
```

The arguments are minimum X, minimum Y, maximum X and maximum Y, followed
by an optional SRID. Each minimum must be strictly below its maximum;
reversed or zero-area rectangles fail. The default SRID is 0. Pass the
same SRID as your stored data when using a rectangle as a search window.

Read coordinate bounds without constructing another geometry:

```sql
SELECT id, ST_MINX(shape), ST_MINY(shape), ST_MAXX(shape), ST_MAXY(shape)
FROM places ORDER BY id;
```

```text
1,4.9E0,5.237E1,4.9E0,5.237E1
2,NULL,NULL,NULL,NULL
3,NULL,NULL,NULL,NULL
```

These observers work for lines, polygons and collections too. An empty
geometry has no bounds. A point has equal minima and maxima, so use
`ST_POINT` directly when a point is the intended search region.

## Text interchange

WKT is useful for inspection and small examples. Its keywords are
case-insensitive, and coordinates accept decimal and exponent notation.
The output preserves the coordinate values but may normalize their
spelling and whitespace.

WKT contains no SRID. Pass it separately when rebuilding a value:

```sql
SELECT ST_ASTEXT(ST_GEOMFROMTEXT(ST_ASTEXT(shape), ST_SRID(shape)))
FROM places WHERE id = 1;
```

```text
POINT (4.9 52.37)
```

Leaving off the SRID gives the new value SRID 0. `CAST(text AS GEOMETRY)`
also uses SRID 0. `CAST(text AS GEOMETRY(4326))` supplies SRID 4326 while
reading the text. Casting an existing geometry to a constrained type
checks its SRID and never changes it. A geometry's printed WKT alone is not a
complete backup of the value and its reference identifier.

Use `ST_SETSRID` when a correctly sourced coordinate sequence needs its
reference label attached. It changes only the identifier:

```sql
VALUES ST_ASTEXT(ST_SETSRID(ST_POINT(100, 200), 28992));
```

```text
POINT (100 200)
```

The coordinates are still `(100, 200)`. This function cannot convert
longitude/latitude to projected coordinates; transformation support is
separate work.

## Binary interchange

WKB encodes the kind, counts and coordinates in binary form. Each
geometry component carries its own byte-order marker. Turborel accepts
both orders and writes little-endian output. EWKB flags, including an
embedded SRID, are not accepted by these constructors.

This standard XY WKB value represents the point `(1, 2)`:

```sql
VALUES ST_ASTEXT(ST_GEOMFROMWKB(
    X'0101000000000000000000F03F0000000000000040', 4326));
```

```text
POINT (1 2)
```

As with WKT, retain the SRID separately:

```sql
SELECT ST_ASTEXT(ST_GEOMFROMWKB(ST_ASBINARY(shape), ST_SRID(shape)))
FROM places WHERE id = 1;
```

```text
POINT (4.9 52.37)
```

POINT EMPTY uses the interoperable pair-of-NaNs WKB convention. A NaN
in only one ordinate, an infinite ordinate or a NaN in a line is rejected.
Empty lines, polygons and collections use zero component counts.

## Inspect and preserve values

`ST_GEOMETRYTYPE` reports a SQL/MM-style name for the outer kind, even
for an empty value:

```sql
SELECT id, ST_GEOMETRYTYPE(shape) FROM places ORDER BY id;
```

```text
1,ST_Point
2,ST_Point
3,NULL
```

Geometry values participate in the database's ordinary transactions,
rollback, checkpoints and recovery. The stored payload retains the SRID
and coordinates. An application reopening the files must enable the same
type before opening them. This is separate from exchanging WKT or WKB.

The native client can carry an extension value without interpreting its
geometry. That does not provide automatic GIS discovery: GEOMETRY_COLUMNS,
SPATIAL_REF_SYS and dedicated GIS drivers are not implemented yet.

## Errors and limits

Malformed text or binary input fails with SQLCODE -301. Unsupported
dimensions, geometry kinds and representation limits fail with -900.
A failed multirow insertion does not leave its earlier rows inserted.

SQL text and binary values currently have limits of 32,767 characters
and octets respectively. A geometry observer that would produce a larger
result fails with -900. Geometry nesting and component counts have
additional [limits](../../appendix/d-limits.md). Large-object SQL types
are needed before arbitrary large shapes can use these SQL interchange
functions.

See the [function reference](../../sql/spatial.md) for signatures, NULL
handling and the exact supported representation subset.
