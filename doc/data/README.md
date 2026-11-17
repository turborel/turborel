# Volume V: Specialized Data

SQL guides for data whose meaning goes beyond a number or a string.
Each guide explains the data model, shows complete queries and describes
when indexes can help. Function signatures and error details live in the
[SQL Reference](../sql/README.md). Rust setup and APIs live in the
[Client Reference](../client/03-embedding.md).

[The documentation set](../README.md)

## Guides

- [Spatial data](spatial/README.md): import delivery locations, search a
  map window, assign service areas, check route intersections and find
  nearby depots. A shared dataset makes the queries runnable.

Full-text search, vectors and JSON will have their own guides here when
implemented. Their planned work is tracked in [TODO.md](../../TODO.md);
this volume documents available features.
