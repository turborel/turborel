# Network transport

`cargo xtask bench network` measures the server's transport over loopback
TCP, a Unix socket and TLS (RULES.md D236). It is an example of the
server's crate, `server/examples/network/`, on the shared harness, since
the core crate's benchmark targets depend on nothing.
`TURBOREL_NETWORK_ONLY=stream,small` runs those workloads alone.

Two servers run in the benchmark's process, each on a runtime of its own
with two I/O threads, as the tests' servers are, over in-memory
databases. One is in the clear, at TCP and a Unix socket; the other
requires TLS 1.3. Both hold the same tables and the user ANN, whose
password is proved by SCRAM-SHA-256 at 10,000 iterations, the least
SERVER.PASSWORD_ITERATIONS takes.

| Operation | What a unit is |
|---|---|
| round trip | `SELECT K FROM ONE` as its own transaction, through the client library; p50 and p99 of 25,000 samples |
| small queries serial | the same query, by hand, each reply read before the next is sent |
| small queries pipelined | the same, 64 sent before their replies are read |
| stream 100000 rows | a row of `SELECT K, V FROM BIG`, an integer and 64 characters, 94 bytes on the wire |
| insert prepared serial | a row inserted by Execute of a prepared INSERT, through the client library, one transaction of 20,000 |
| insert prepared pipelined | the same, by hand, 64 Executes sent before their replies are read |
| connect | a connection made, logged in and ended; trusted, or by SCRAM-SHA-256 |
| concurrent small queries | the trivial query over 256 connections at once, each on a thread of its own, 200 each |

The hand-made client counts replies by their frames and reads no values,
so those figures are the server's and the transport's.

## Before and after

Before is the server at 4f727f44, after is the branch with RULES.md
D230 to D232; both built optimized from the same benchmark source. Each
figure is the median of three runs, the two builds' runs taken in turn
on a machine shared with other builds, so a figure the change did not
touch can move by about 15% between runs. Rates are per second; the
round trips and connects also give their p50 and p99 in microseconds.

| Operation | Unit | Before | After | Change | p50 / p99 µs before | after |
|---|---|---:|---:|---:|---:|---:|
| round trip tcp | query/s | 24,392 | 24,122 | -1% | 39 / 67 | 40 / 64 |
| round trip unix | query/s | 31,955 | 32,353 | +1% | 30 / 67 | 29 / 54 |
| round trip tls | query/s | 16,621 | 22,602 | +36% | 58 / 83 | 43 / 72 |
| small queries serial tcp | query/s | 25,115 | 26,098 | +4% |  |  |
| small queries pipelined tcp | query/s | 35,187 | 53,662 | +53% |  |  |
| small queries serial unix | query/s | 30,791 | 31,585 | +3% |  |  |
| small queries pipelined unix | query/s | 41,836 | 52,085 | +24% |  |  |
| small queries serial tls | query/s | 16,432 | 18,790 | +14% |  |  |
| small queries pipelined tls | query/s | 18,758 | 43,733 | +133% |  |  |
| stream 100000 rows tcp | row/s | 3,044,078 | 2,619,819 | -14% |  |  |
| stream 100000 rows unix | row/s | 3,120,097 | 2,669,013 | -14% |  |  |
| stream 100000 rows tls | row/s | 2,783,177 | 2,424,705 | -13% |  |  |
| insert prepared serial tcp | row/s | 33,001 | 33,037 | +0% |  |  |
| insert prepared pipelined tcp | row/s | 53,189 | 71,846 | +35% |  |  |
| insert prepared serial unix | row/s | 45,606 | 45,696 | +0% |  |  |
| insert prepared pipelined unix | row/s | 65,772 | 75,888 | +15% |  |  |
| insert prepared serial tls | row/s | 23,187 | 28,319 | +22% |  |  |
| insert prepared pipelined tls | row/s | 22,351 | 68,595 | +207% |  |  |
| connect trust tcp | connection/s | 11,199 | 11,683 | +4% | 54 / 77 | 48 / 95 |
| connect trust unix | connection/s | 17,517 | 19,968 | +14% | 33 / 47 | 30 / 45 |
| connect trust tls | connection/s | 2,554 | 2,342 | -8% | 344 / 458 | 368 / 520 |
| connect scram tcp | connection/s | 649 | 644 | -1% | 1455 / 2310 | 1400 / 1970 |
| connect scram tls | connection/s | 507 | 554 | +9% | 1798 / 2878 | 1723 / 2624 |
| concurrent small queries tcp | query/s | 137,985 | 147,325 | +7% |  |  |
| concurrent small queries tls | query/s | 106,845 | 138,997 | +30% |  |  |

Pipelined requests gain the most, since their replies now share a write
(D231), and TLS more again, since the worker reads and writes the
session itself rather than handing each request to the connection's
task (D230). Serial requests over TCP and a Unix socket were already
taken by the worker, so they stay level. The streaming figures above
were taken with the old build always first; taken again in turn with
each build first, three runs each, they came out level, after ahead in
two rounds of three, so the stream neither gains nor loses beyond the
noise. Connects are level too, a login being the same work as before;
SCRAM's cost is the 10,000 iterations of its key, on both sides.

Streaming is bounded by the server making its rows, not by the
transport: a profile of the stream shows allocation, copying and the
statement's own work well ahead of any of the transport's functions.
