# Volume V: Specialized Data

For SQL users whose data is more than numbers and strings: its model, the queries asked of it, and the indexes that serve them, a guide each.

[The documentation set](../README.md)

## Contents

- [Full-Text Search](01-fulltext.md): search support tickets by their
  words, whatever their case and accents, index them, search them by
  pattern and show the best ranked first.
- [Spatial Data](02-spatial.md): import delivery addresses, search a
  map window, match orders to service areas, check routes against
  roadworks and find the nearest depots.
- [JSON Data](03-json.md): store orders as JSON documents, check
  them, read their values, find orders by their items, turn the items
  into rows and build JSON for a reply.
- [Vectors](04-vectors.md): store recipes' embeddings, find the nearest
  by distance, index them, compare the metrics, work with the vectors'
  arithmetic and define a distance of one's own.
- [Graphs](05-graphs.md): present a bank's accounts and transfers as a
  graph, follow money along paths, find the shortest and cheapest, and
  index the transfers for the search.

Each guide runs on tables it makes at its start. The
[SQL Reference Manual](../sql/README.md) gives each function's rules and
errors, and [The Embedded Library](../client/03-embedding.md) the Rust
interfaces.
