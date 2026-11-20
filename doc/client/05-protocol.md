# The Native Protocol

[Contents](README.md)

The native protocol is how a client talks to `turboreld`. The client
library speaks it ([The Client Library](02-library.md)), and so may any
program that follows this page. A server listens on TCP port 7422 unless
told another, and on a Unix socket or a named pipe where the operator
asks. A client may begin TLS on the same port. A TLS handshake begins
with the byte 22, and no message of the protocol does, so the server
tells the two apart by the first byte.

This page describes version 1, the only version. The protocol grows by
capabilities, never by a new version. A client names the capabilities it
wants as it connects, and the server grants those it has. Each behaviour
added since the first form of version 1 has a capability of its own, so
a client that asks for none sees the first form alone, byte for byte.

## Frames

Every message, either way, is a frame.

```text
frame
  kind       u8
  length     u32, the bytes of the body, the header not counted
  body       length bytes
```

The header is five bytes. The kind says which message it is, and is read
in the direction it travels, so `C` from a client is Close and `C` from
the server is Challenge. A kind the reader does not know, a body cut
short, or bytes left over after the last field all fail the connection
(SQLCODE -919, SQLSTATE 08006).

The server takes a frame of at most SERVER.MAX_MESSAGE_BYTES of body,
16 MiB by default. Before the client is authenticated it takes at most
16 KiB, or the setting if that is less. A longer frame fails the
connection. The client library takes replies of up to 16 MiB unless told
otherwise.

### Compressed frames

Once the capability `compression` is granted, either side may send any
frame compressed, as a frame of kind `z`.

```text
Compressed 'z'
  kind       u8, the kind of the frame it holds
  length     u32, the length of that frame's body
  stream     the rest, a zlib stream of the body
```

The reader inflates at most `length` bytes, and `length` is at most the
longest body it takes, so a stream cannot make it hold more. A `z` frame
that holds another `z` frame, one that inflates to another length, or
one received when compression was not granted fails the connection.

The server compresses each frame whose whole length, header included, is
at least SERVER.COMPRESSION_MIN_BYTES, 1024 by default, and only where
the compressed frame comes out shorter. It sends the others as they are.
Where that setting is 0, the server does not grant `compression` at all.
A client chooses its own threshold.

## Values and fields

Every integer is little-endian. A field written `i32 as u32` or `i64 as
u64` is the signed number's bits in an unsigned field of the same size.

| Field | Written as |
|---|---|
| `u8`, `u32`, `u64` | 1, 4 or 8 bytes, least significant first |
| `i128` | 16 bytes, two's complement, least significant first |
| flag | `u8`, 0 or 1; any other byte fails the connection |
| string | its length in bytes, `u32`, then its UTF-8 |
| bytes | its length, `u32`, then the bytes |
| list | its count, `u32`, then each item |
| strings | a list of strings |
| key | 16 bytes, with no length before them |

A reader holds what it reads as it goes. A count larger than what
follows fails as the body runs out, so it cannot make the reader reserve
memory.

A field of a value is not a frame's. A value whose body its reader
cannot take, as a truth value of 2 or a JSON document damaged, leaves
the frame read whole by its length and the stream in step. The client
library fails the request with the value's error, not of class 08, and
reads past the rest of its replies up to the last
([The Client Library](02-library.md)).

### Data types

A data type is a tag, then what its kind writes after it. Columns,
parameters and the types inside other types are written this way. A
distinct type is written as its source type.

| Tag | Type | After the tag |
|---|---|---|
| 1 | CHARACTER | length `u32`, units `u8` (0 characters, 1 octets) |
| 2 | exact numeric | precision `u32`, scale `u32` |
| 3 | approximate numeric | precision `u32` |
| 4 | a plug-in's type | its name, string; its parameters, a list of `u32` |
| 5 | CHARACTER VARYING | length `u32`, units `u8` |
| 6 | BINARY | length `u32` |
| 7 | BINARY VARYING | length `u32` |
| 8 | DATE | precision `u32` |
| 9 | TIME | precision `u32` |
| 10 | TIME WITH TIME ZONE | precision `u32` |
| 11 | TIMESTAMP | precision `u32` |
| 12 | TIMESTAMP WITH TIME ZONE | precision `u32` |
| 13 | INTERVAL | start field, end field, leading precision, fractional precision, each `u32` |
| 14 | BOOLEAN | nothing |
| 15 | DECFLOAT | precision `u32` |
| 16 | array | maximum cardinality `u32`, then the element type |
| 17 | row | its fields, a list of each field's name, string, then its type |
| 18 | CHARACTER LARGE OBJECT | length `u64`, units `u8` |
| 19 | BINARY LARGE OBJECT | length `u64`, units `u8` |
| 22 | multiset | the element type |
| 23 | structured type | its schema and name, strings; its proper supertypes, a list of each schema and name |
| 24 | reference type | the referenced type's names, as a structured type's; then scoped `u8`, and if 1 the scope's schema and name |
| 25 | JSON | nothing |
| 26 | XML | primary modifier `u8` (0 DOCUMENT, 1 CONTENT, 2 SEQUENCE), secondary `u8` (0 UNTYPED, 1 ANY, 2 XMLSCHEMA); of XMLSCHEMA, the schema's schema and name, then its namespace and its element, each `u8` 0 for none or 1 and a string |
| 27 | UUID | nothing |

An interval's fields are numbered 0 YEAR, 1 MONTH, 2 DAY, 3 HOUR, 4
MINUTE and 5 SECOND. A plug-in's type a client does not have is read by
its name and shown by its values' text.

### Values

A value is a tag, then its body. A value's tag says how it is written,
which is not always the tag of its column's type. A CHARACTER VARYING
value is written with tag 1, as a CHARACTER value is.

| Tag | Value | Body |
|---|---|---|
| 0 | null | nothing |
| 1 | character string | string |
| 2 | exact number | units `i128`, then scale `u32`; the number is units divided by ten to the scale |
| 3 | approximate number | the bits of an IEEE 754 double, `u64` |
| 4 | a plug-in's value | its type's name, string; its text, string; its bytes, bytes |
| 6 | binary string | bytes |
| 8 | date | as a timestamp, the instant its midnight's |
| 9 | time | as a timestamp, the instant microseconds from midnight UTC, within a day |
| 11 | timestamp | zoned `u8`; precision `u8`; the zone's displacement in minutes, the `i16`'s bits in the low half of a `u32`; the instant, microseconds in UTC from 0001-01-01 00:00:00, `i64 as u64` |
| 13 | interval | its qualifier, as the INTERVAL type writes it; then units `i128`, months of a year-month interval, microseconds of a day-time one |
| 14 | truth value | `u8`, 0 false, 1 true; unknown is null |
| 15 | decimal floating point | coefficient `i128`, then exponent `i32 as u32` |
| 16 | array | a list of values |
| 17 | row | a list of values, one for each field |
| 20 | large object, whole | kind `u8` (0 character, 1 binary), then its octets, bytes |
| 21 | large object, by locator | kind `u8`; locator `u32`; its octets `u64`; its characters `u64` |
| 22 | multiset | a list of values, in no set order |
| 23 | structured value | its most specific type's schema and name, strings; then its attributes, a list of values |
| 24 | reference | bytes |
| 25 | JSON | its stored form, bytes, as [JSON values](#json-values) says |
| 26 | XML | its serialization as CONTENT, string |
| 27 | UUID | its 16 octets, bytes |

The zoned byte of a datetime is 1 for a type WITH TIME ZONE. Without a
zone, the displacement is the one the value is shown in. A large object
of at most SERVER.LOB_INLINE_BYTES, 64 KiB by default, comes whole. A
longer one comes by a locator, which the client reads with Lob requests,
and which lasts until its transaction ends. A client always sends a
large object whole, or as a locator it made with Lob requests.

### JSON values

A JSON value is sent as the database stores it, a document of a typed
binary tree, never as JSON text. A document is a version octet, 1, then
its root node. A node is a tag octet, then its body. Integers are
little-endian, as everywhere in the protocol.

| Tag | Node | Body |
|---|---|---|
| 0 | null | nothing |
| 1 | false | nothing |
| 2 | true | nothing |
| 3 | integer | the number, `i64` |
| 4 | decimal | units `i64`, then scale `u8`; the number is units divided by ten to the scale |
| 5 | wide decimal | units `i128`, then scale `u32`, read as a decimal's |
| 6 | decimal floating point | coefficient `i128`, then exponent `i32`; the number is the coefficient times ten to the exponent |
| 7 | approximate number | the bits of an IEEE 754 double, `u64` |
| 8 | string | its length in octets, `u32`, then its UTF-8 |
| 9 | datetime | moment `u8` (0 date, 1 time, 2 timestamp); the instant, microseconds in UTC, `i64`; the zone's displacement in minutes, `i16`; zoned `u8`; precision `u8` |
| 10 | array | count `u32`; size `u32`; an offset `u32` of each element; the elements |
| 11 | object | count `u32`; size `u32`; an offset `u32` of each member, in key order; the members, in the order written |

An exact number is written in the narrowest of its three nodes. One of
scale 0 whose units an `i64` holds is an integer, one of another scale
a `u8` holds whose units an `i64` holds is a decimal, and any other a
wide decimal. A decimal keeps the scale it was written with, so 1.50 is
units 150 and scale 2. A datetime is the instant of a date's midnight,
of a time within its day, or of a timestamp, counted as the Values
table says, and its zoned octet is 1 for one WITH TIME ZONE.

An array's or an object's count is its elements or members. Its size is
the octets of its elements or members, after its offsets, so a reader
skips a node it does not need without reading inside it. Each offset
counts octets from the first element or member, so the first offset of
an array is 0.

A member is its key's length in octets, `u32`, the key's UTF-8, then
its value's node. An object keeps its members in the order they were
written, those of one key too, and JSON text written from it keeps
that order. Its offsets are a directory sorted by key, comparing the
keys' UTF-8 octet by octet, which is the order of Unicode code points,
and members of one key in the order written. A reader finds a key by a
binary search of the directory, and the first member of a key written
is the one a member accessor takes. A reader that only lists the
members reads them in place after the offsets, in their written order.

The value `{"a":[1,"x"]}` is these 51 octets, in hexadecimal:

```text
01                  version 1
0B                  object
  01 00 00 00         count 1
  25 00 00 00         size 37
  00 00 00 00         offset of key "a", 0
  01 00 00 00 61      key "a"
  0A                  array
    02 00 00 00         count 2
    0F 00 00 00         size 15
    00 00 00 00         offset of element 1, 0
    09 00 00 00         offset of element 2, 9
    03                  integer
      01 00 00 00 00 00 00 00   1
    08                  string
      01 00 00 00 78            "x"
```

In `{"b":1,"a":2}` the members stay in that order, and the directory
puts `a` first. Its offsets are `0E 00 00 00`, member `a` at 14, then
`00 00 00 00`, member `b` at 0.

A server checks a document it receives whole before it uses it, every
node within its container, every string UTF-8 and every offset at an
element or member. A document of another version or a damaged one is
refused with SQLCODE -916. Arrays and objects nest at most 128 deep,
and a document is at most 256 MiB.

### Composite fields

```text
row          a list of values
column       named flag; its name, string, if named; data type; not null flag
parameter    varying flag; data type
failure      sqlcode i32 as u32
             sqlstate, string of 5
             message, string
             cited flag; the rule, string, if cited
             placed flag; the position, u64, if placed
             active flag
             tail 7 to 15
```

A parameter's varying flag is 1 where the server gives the parameter
CHARACTER VARYING of the longest length, as it does of one whose type
the statement does not decide. Its type is written all the same.

A failure's active flag says whether the session's transaction is
active after it. Its position counts characters into the statement. Its
rule is the clause of a standard it cites, as messages do.

## The tail

Version 1 grew after its first form. The fields a message of the first
form gained come at the end of its body, as its tail.

```text
tail
  { tag u8, then the field of that tag }...  to the end of the body
```

Each field is written only where the capability that adds it was
granted and it has something to say. The tags come in increasing order.
A tag the message does not take, or one not past the last, fails the
connection. A message with an empty tail is its first form exactly.

| Tag | Field | Written as | In |
|---|---|---|---|
| 1 | attributes | a count `u32`, then each name and value, strings | Hello, Query, Execute |
| 2 | multi | nothing | Query |
| 3 | cursor | `u32` | Execute, Fetch, Close |
| 4 | position | `u64` | Complete |
| 5 | more | nothing | Complete |
| 6 | rows | `u64` | Complete |
| 7 | severity | string | a failure, Notice |
| 8 | detail | string | a failure, Notice |
| 9 | hint | string | a failure, Notice |
| 10 | context | string | a failure, Notice |
| 11 | schema | string | a failure, Notice |
| 12 | table | string | a failure, Notice |
| 13 | column | string | a failure, Notice |
| 14 | data type | string | a failure, Notice |
| 15 | constraint | string | a failure, Notice |
| 16 | generated | a list of strings | Complete |
| 17 | conditions | a list, each a failure as bytes | Complete |
| 18 | time | total, queued, locks, each `u64` | Complete |
| 19 | declared | a count `u32`, then of each column its type's name and its type as SQL writes it, strings | Prepared, Description |

A message takes at most 32 attributes. A name is 1 to 64 bytes and a
value at most 1024. More fails the connection.

## Opening a connection

A connection's first message is Hello, or Cancel. Hello names the
protocol's version, the user, a mechanism of authentication and its
first bytes, the capabilities the client asks for, and the connection's
attributes.

The server answers each step of the exchange with a Challenge, and the
client each Challenge with an Answer, until the server accepts or
refuses. Refused, it sends an Error and closes. Accepted, it sends a
Status for each of the session's settings, if `status` was granted, and
then Ready.

```text
client  Hello
server  Challenge          one or more steps, as the mechanism needs
client  Answer
server  Status...          with status
server  Ready
```

Ready carries the session's authorization identifier, the key that
cancels its statements, the capabilities granted and the server's
product. The capabilities granted are those asked for that the server
has, in the order of the table below. Ready at login is sent before
compression begins, so it is never compressed.

The server refuses a version other than 1 (SQLSTATE 08004). It refuses a
user name that is not an identifier, or a mechanism it does not serve,
before any exchange (SQLSTATE 28000). A connection has
SERVER.LOGIN_TIMEOUT_MS to authenticate.

### Mechanisms

| Mechanism | First bytes | Exchange |
|---|---|---|
| `trust` | anything | accepted at once, from a client on the server's own machine, where SERVER.TRUST_LOCAL is ON |
| `password` | the password's UTF-8 | accepted or refused at once; only over TLS, a Unix socket or a named pipe |
| `certificate` | anything | accepted if the TLS client certificate's common name is the user |
| `scram-sha-256` | client-first | two Challenges, as below |
| `scram-sha-256-plus` | client-first | the same, bound to the TLS channel |

A SCRAM exchange runs four messages after Hello.

```text
client  Hello              mechanism scram-sha-256, first bytes client-first
server  Challenge          server-first
client  Answer             client-final
server  Challenge          server-final, once the proof is right
client  Answer             empty, once the server's signature is right
server  Ready
```

The messages are those of RFC 5802 and RFC 7677. The user is the one
Hello names, and the `n=` of client-first is left aside. Over TLS the
`-plus` mechanism binds the channel by `tls-exporter` or
`tls-server-end-point`, as its GS2 header names. A plug-in may add a
mechanism of its own ([Plug-ins](04-plugins.md)).

### Capabilities

| Capability | Grants |
|---|---|
| `notices` | a Notice before a statement's completion, for each condition of class 01 it raised and each notice of the server's own |
| `status` | a Status of each session setting as the session begins, and of each one a statement changes |
| `diagnostics` | an Error's failure carries tags 7 to 15 |
| `describe` | the request Describe |
| `cursors` | several cursors of one prepared statement, numbered by tag 3 |
| `ping` | the request Ping, answered by Pong |
| `reset` | the request Reset |
| `change-user` | the request ChangeUser |
| `query-attributes` | attributes, tag 1, on Query and Execute |
| `connection-attributes` | says Hello carries attributes; every server reads them, granted or not |
| `multi-statement` | a Query of several statements, tag 2, and tag 5 on its Completes |
| `compression` | compressed frames, both ways |
| `commit-position` | a Complete that committed carries the commit's log position, tag 4 |
| `copy` | the requests CopyIn, CopyData and CopyEnd |
| `row-counts` | a Complete carries the rows its statement counted, tag 6, and the server's time, tag 18 |
| `generated-values` | a Complete of an INSERT carries the identity values it generated, tag 16 |
| `fetch-ends` | an Execute that ends the transaction ends it with its cursor, as the last row is fetched or the cursor closed |
| `conditions` | a Complete carries its statement's conditions, tag 17, in place of Notices |
| `declared-types` | a Prepared and a Description carry each column's declared type, tag 19 |

The server ignores a name it does not know. A client never sends a
request, or a tail field, of a capability it was not granted. One that
does fails the connection.

## Requests

A client may send a request before the last one is answered. The server
runs them one at a time and answers each in turn, in order. Every request
below except Hello, Answer and Cancel comes after Ready.

### Hello 'H'

```text
Hello 'H'
  version        u32, 1
  user           string
  mechanism      string
  initial        bytes, the mechanism's first bytes
  capabilities   strings
  tail           1 attributes
```

The first message of a connection that logs in. The server reads its
attributes whether or not `connection-attributes` is granted, and shows
them in INFORMATION_SCHEMA.CONNECTION_ATTRIBUTES. A user name is read as
an identifier, so `ann` logs in as ANN.

### Answer 'A'

```text
Answer 'A'
  answer         bytes
```

The client's answer to a Challenge. The server answers it with another
Challenge, with Ready, or with an Error.

### Query 'Q'

```text
Query 'Q'
  sql            string
  ends           flag
  tail           1 attributes, 2 multi
```

A direct SQL statement. A query is answered with a Description, its rows
in Rows replies, and a Complete. Another statement is answered with a
Complete. A failure is answered with an Error. A query that returns no
row completes with SQLCODE 100 and SQLSTATE 02000.

Where `ends` is 1, the transaction ends with the statement. It is
committed once the statement succeeds and rolled back if it fails, so
the Complete or the Error says no transaction is active. A statement that
is its own transaction is then one request and one reply. A commit that
fails sends its Error in place of the Complete. A result that sent a
large object by its locator keeps the transaction, since the locator
lasts only as long as it does.

With tag 2 the text is several statements, split at each semicolon
outside a string, a comment or a compound statement. They run one by one,
each as a Query of its own, `ends` applying to each. Each Complete but
the last carries tag 5. The first statement that fails is the last run,
and its Error ends the replies. Text of no statement gets one Complete.

### Prepare 'P'

```text
Prepare 'P'
  statement      u32, the client's number for it
  sql            string
```

Prepares a statement under the client's number, replacing one it had of
that number. Answered with Prepared or an Error. A connection holds at
most SERVER.MAX_STATEMENTS prepared statements. A statement whose cursor
is open cannot be replaced (SQLSTATE 24000).

### Execute 'E'

```text
Execute 'E'
  statement      u32
  values         row, one value for each input parameter
  rows           u32, the most rows sent now; 0 for all
  ends           flag
  tail           1 attributes, 3 cursor
```

Executes a prepared statement. A statement other than a query is answered
with a Complete. A CALL with a `?` for an OUT or INOUT parameter first
sends one Rows reply of one row, of the columns Prepared gave, holding the
value each such parameter had once the procedure ran. A query opens its cursor and sends at most `rows` rows,
in Rows replies, of the columns Prepared gave, with no Description. If
the cursor has more, Suspended follows and the cursor stays open.
Otherwise a Complete follows and the cursor is closed.

`ends` works as a Query's. A cursor left open keeps the transaction.
With `fetch-ends` granted, the transaction then ends as the cursor does,
with the Fetch that reads its last row or the Close that closes it.
Without it, a cursor left open leaves the transaction to the client.

The cursor is number 0 unless tag 3 names another, with `cursors`
granted. A cursor already open of that number fails (SQLSTATE 24000).

### Fetch 'F'

```text
Fetch 'F'
  statement      u32
  rows           u32, the most rows sent; 0 for all
  tail           3 cursor
```

Sends more rows of an open cursor, then Suspended if it has more, or a
Complete once it has none. A cursor not open fails (SQLSTATE 24000).

### Close 'C'

```text
Close 'C'
  statement      u32
  tail           3 cursor
```

Closes an open cursor. Answered with a Complete, or an Error if it is
not open.

### Deallocate 'D'

```text
Deallocate 'D'
  statement      u32
```

Destroys a prepared statement. Answered with a Complete. There must be
such a statement (SQLSTATE 26000) and none of its cursors may be open
(SQLSTATE 24000).

### Lob 'L'

```text
Lob 'L'
  op             u8, then by op
  0 read         locator u32, from u64, count u64, characters flag
  1 create       kind u8, 0 character, 1 binary
  2 append       locator u32, bytes
  3 free         locator u32
  4 hold         locator u32
```

Works on a large object by its locator. Read gives at most `count`
octets from octet `from`, counting from 0, or characters where the flag
is 1, and is answered with a Lob reply of its data. Create makes an
empty large object and is answered with a Lob reply of its locator.
Append adds octets to one Create made. A character one may split a
character between two Appends. Free ends a locator, and Hold keeps it
valid past its transaction's commit. Append, Free and Hold are answered
with a Complete.

### Cancel 'Z'

```text
Cancel 'Z'
  key            key, 16 bytes
```

The first and only message of a connection that cancels the statement
another connection is running. The key is the one that connection's
Ready gave. The server closes the connection with no reply, and the
statement fails with SQLCODE -920 (SQLSTATE TB920).

### Terminate 'X'

```text
Terminate 'X'
```

Ends the session, its transaction as SESSION.COMMIT_AT_END says, and the
server closes the connection. A commit that fails is sent as an Error
before it closes.

### Describe 'Y'

```text
Describe 'Y'
  what           u8, 0 a statement, 1 a cursor
  statement      u32
  cursor         u32, 0 of a statement
```

With `describe`. Describes a prepared statement, answered with Prepared
as Prepare gave it, or an open cursor, answered with a Description of
its columns. A statement's Describe whose cursor is not 0 fails the
connection.

### Ping 'G'

```text
Ping 'G'
```

With `ping`. Answered with Pong by the connection's own task, with no
worker, so it answers even while every worker is busy. It may come at
any point after Ready, during a copy too.

### Reset 'W'

```text
Reset 'W'
```

With `reset`. Puts the session back as it was at login. The transaction
is rolled back, and prepared statements, cursors, a copy under way,
temporary tables and settings are gone. The authentication and the
cancel key are kept. A Notice of SQLSTATE 01T01 comes first, with
`notices`, if a transaction was active, then a Status of each setting,
with `status`, then a Complete that says no transaction is active.

### ChangeUser 'U'

```text
ChangeUser 'U'
  user           string
  mechanism      string
  initial        bytes
```

With `change-user`. Authenticates the connection again, as Hello does,
with the same Challenge and Answer steps. Accepted, it renews the
session as Reset does, under the new user and a new cancel key, and is
answered with the Notice and Statuses as Reset is, then Ready with the
same capabilities as before. Refused, it fails the connection.

### CopyIn 'I'

```text
CopyIn 'I'
  table          string, the table as SQL names it
  columns        strings, none for every column in order
  ends           flag
```

With `copy`. Begins a copy of rows into a table. It has no reply. From
here until CopyEnd the client sends only CopyData and CopyEnd. Another
request that a worker would run, or CopyData or CopyEnd with no copy
under way, fails the connection. Ping is answered as ever.

### CopyData 'J'

```text
CopyData 'J'
  rows           a list of rows
```

Rows of the copy, as many at a time as the client likes. It has no
reply. Each value is assigned to its column as INSERT assigns it, so a
string too long fails rather than being cut.

### CopyEnd 'N'

```text
CopyEnd 'N'
  abandon        flag
```

Ends the copy. Answered with a Complete whose tag 6 counts the rows
inserted, whatever was granted, or with the copy's Error. A copy is all
or nothing. A failure is kept until CopyEnd and the rows after it are
thrown away. The Error's detail names the row that failed, as `row 7 of
the copy`, counting from 1. Where `abandon` is 1 every row is undone and
the Complete counts none. A copy that does not end the transaction runs
under a savepoint of its own, and fails before it inserts anything if the
savepoint cannot be taken. One whose `ends` is 1 is committed once it
succeeds and rolled back otherwise. A Terminate during a copy undoes the
copy before the session ends.

## Replies

### Challenge 'C'

```text
Challenge 'C'
  challenge      bytes
```

The next step of an authentication exchange.

### Ready 'R'

```text
Ready 'R'
  authorization  string
  key            key, 16 bytes
  capabilities   strings
  server         string, as turborel 0.0.0 (alpha)
```

The client is authenticated. The key cancels the session's statements
from another connection and is the client's secret. The server's
product is told only to a client authenticated.

### Prepared 'P'

```text
Prepared 'P'
  statement      u32
  parameters     a list of parameter
  columns        a list of column
  tail           19 declared
```

A statement prepared, or described. The parameters are its inputs, in
the order of their `?` marks. The columns are a query's, or of a CALL
one for each `?` that is the argument of an OUT or INOUT parameter, in
the order of the procedure's parameters, named and typed as each; a
statement of neither has none. A `?` of an OUT parameter is no input, so
it has no parameter; one of an INOUT parameter is both, so it has one,
and a column.

### Description 'D'

```text
Description 'D'
  columns        a list of column
  tail           19 declared
```

The columns of the rows that follow a Query, or of an open cursor that
Describe asked of.

#### Declared types

A data type's tag can stand for several declared types. SMALLINT,
INTEGER, BIGINT, DECIMAL and NUMERIC are all an exact numeric type of
tag 2, and INTEGER is written as precision 10 and scale 0. REAL, DOUBLE
PRECISION and FLOAT are all an approximate numeric type of tag 3. A
distinct type is written as its source type. Every other tag is one
type. With `declared-types` granted, a Prepared or a Description of one
column or more ends with tag 19, which names the type each column was
declared with, in the columns' order.

```text
declared
  count          u32, the columns'
  name           string, as the Information Schema's DATA_TYPE gives it
  written        string, the whole type as SQL writes it
                 (name and written again for each column)
```

| Declared as | Tag | Name | Written |
|---|---|---|---|
| SMALLINT | 2 | SMALLINT | SMALLINT |
| INTEGER | 2 | INTEGER | INTEGER |
| BIGINT | 2 | BIGINT | BIGINT |
| DECIMAL(9,2) | 2 | DECIMAL | DECIMAL(9,2) |
| NUMERIC(5) | 2 | NUMERIC | NUMERIC(5,0) |
| REAL | 3 | REAL | REAL |
| DOUBLE PRECISION | 3 | DOUBLE PRECISION | DOUBLE PRECISION |
| FLOAT(20) | 3 | FLOAT | FLOAT(24) |
| S.MONEY, a distinct type | its source's | USER-DEFINED | S.MONEY |
| INTEGER ARRAY[3] | 16 | ARRAY | INTEGER ARRAY[3] |
| any other | its own | its name, as DATE | as SQL writes it, as TIME(3) |

FLOAT(20) holds the values of FLOAT(24), so that is its type. The
values are written as ever, by their own tags, whether or not the
capability was granted. A client not granted it is sent the columns
alone, byte for byte as before. The tag tells a client how to read a
column's values, and the declared type is what it shows of the column.

### Rows 'B'

```text
Rows 'B'
  rows           a list of rows
```

A batch of rows. The server sends a batch once its rows reach about
SERVER.BATCH_BYTES, 64 KiB by default, so a large result never sits in
memory whole.

### Suspended 'S'

```text
Suspended 'S'
```

The rows asked for were sent and the cursor has more. Fetch reads them.

### Complete 'K'

```text
Complete 'K'
  sqlcode        i32 as u32, 0 or 100
  sqlstate       string of 5
  active         flag
  tail           4 position, 5 more, 6 rows, 16 generated,
                 17 conditions, 18 time
```

A statement's completion, with SQLSTATE 00000, a warning's of class 01,
or 02000 of no data. `active` says whether a transaction is active after
it.

Tag 4 is the log position of the commit the statement made. A session
on a replica that sets REPLICATION.READ_AFTER to it has each statement
wait for the replica to apply that position first. A statement that
waits past STATEMENT.TIMEOUT_MS, or 30 seconds where that is 0, fails
with SQLSTATE TBR01. A Query of a SET statement never waits.

Tag 6 is the rows a statement inserted, updated, deleted or merged, as
GET DIAGNOSTICS ROW_COUNT gives them. Of a query it is the rows the
cursor returned in all, given by the Complete that ends it.

Tag 16 is each identity value an INSERT generated, as SQL writes it, in
the order of its rows.

Tag 17 is each condition the statement raised, in order, each a
failure written as bytes, so its own tail ends where its length does.
Its SQLCODE is 0, or 100 of class 02, and with `diagnostics` its tail
carries its severity, tag 7, and what else is known, as an Error's.

Tag 18, with `row-counts`, is the server's time for the statement, in
microseconds, three u64 fields, total, queued and locks. Total runs from
when a worker began the request until the completion was ready, the
commit of `ends` included. Queued is the time the request
waited for a worker before that, and is not part of total. Locks is the
part of total spent waiting for locks. A cursor's time sums its Execute
and every Fetch, and a copy's sums its requests. The Complete of a
Close, a Deallocate, a Lob request or a Reset carries no time.

### Error 'E'

```text
Error 'E'
  failure        failure
```

A statement's failure, or the connection's. Its tail, tags 7 to 15, is
written only with `diagnostics` granted. An Error of SQLSTATE class 08
but 08003, or of class 28 as the connection is made, is the connection's, and the
server closes the connection after it.

### Lob 'L'

```text
Lob 'L'
  what           u8, then by what
  0 data         bytes
  1 locator      u32
```

The data a Lob read asked for, or the locator a Lob create made.

### Notice 'N'

```text
Notice 'N'
  sqlstate       string of 5
  message        string
  tail           7 to 15
```

With `notices`. A condition a statement raised, or a notice of the
server's own, sent before the statement's completion. Its tail, as an
Error's, is written only with `diagnostics` granted.

### Status 'T'

```text
Status 'T'
  name           string
  value          string, as SQL writes it; empty for a null
```

With `status`. A setting of the session. At login, before Ready, and
after Reset or ChangeUser, before the reply that ends it, the server
sends one of each name below, in the table's order, then one of every
setting a session may set for itself, SESSION.TIME_ZONE and
TRANSACTION.ISOLATION among them.

| Name | Value | Sent again after a statement |
|---|---|---|
| SERVER_VERSION | the server's product, `turborel`, its version and, in parentheses, its codename | never |
| AUTHORIZATION | the session's current authorization identifier | if the statement changed it, as SET SESSION AUTHORIZATION does |
| SCHEMA | the session's default schema | if the statement changed it, as SET SCHEMA does |
| READ_ONLY | YES if the session's transaction is READ ONLY, else NO | if the statement changed it, as SET TRANSACTION or the end of a transaction does |
| SERVER_READ_ONLY | YES if the server takes no changes, as a replica following its group does, else NO | if it changed by the time the statement completed, as a promotion changes it |
| CATALOG | the name of the database's catalog, the setting CATALOG.NAME, an identifier of 1 to 128 characters; never empty, as the setting takes no null | never |
| OWNER | YES if the session's current authorization identifier is the database's owner, else NO | if the statement changed AUTHORIZATION and this changed with it |

After a statement the server sends a Status only of what the statement
changed, before the statement's completion, and none for a statement
that changed nothing. A setting the session set for itself is sent at
its new value, and one it reset is sent again at the database's value.
After Reset or ChangeUser every name is sent again, changed or not. A
client that keeps the last value of each name has the session's
settings with no query, and tells a primary from a replica by
SERVER_READ_ONLY.

### Pong 'G'

```text
Pong 'G'
```

With `ping`. The answer to Ping.

## Conversations

A statement's replies come in a fixed order. Rows come first, then any
Notices, then any Statuses, then the statement's last reply, which is a
Complete, an Error or Suspended. The listings show the messages a
client granted every capability sees. Those in brackets come only where
there is something to say.

A statement under autocommit is one Query with `ends` set.

```text
client  Query      sql "INSERT INTO T VALUES (1)", ends 1
server  [Status]   READ_ONLY and the like, if the statement changed them
server  Complete   0, 00000, active 0, rows 1, position 4711, time
```

A cursor read in parts.

```text
client  Prepare    statement 1, sql "SELECT ID FROM T"
server  Prepared   statement 1, no parameters, column ID
client  Execute    statement 1, no values, rows 2, ends 1
server  Rows       2 rows
server  Suspended
client  Fetch      statement 1, rows 2
server  Rows       1 row
server  Complete   0, 00000, active 0, rows 3, time
client  Deallocate statement 1
server  Complete   0, 00000, active 0, time
```

The transaction here ends with the Fetch because `fetch-ends` was
granted. Without it, the Complete would say the transaction is still
active.

A copy.

```text
client  CopyIn     table "T", columns ["ID", "NAME"], ends 1
client  CopyData   500 rows
client  CopyData   500 rows
client  CopyEnd    abandon 0
server  Complete   0, 00000, active 0, rows 1000, time
```

A copy whose 712th row breaks a constraint.

```text
client  CopyIn     table "T", columns [], ends 0
client  CopyData   1000 rows
client  CopyEnd    abandon 0
server  Error      23000, detail "row 712 of the copy", active 1
```

A reset of a session in a transaction.

```text
client  Query      sql "UPDATE T SET N = 2", ends 0
server  Complete   0, 00000, active 1, rows 3, time
client  Reset
server  Notice     01T01, "[DESIGN] Reset rolled back the transaction that was active"
server  Status...  every setting
server  Complete   0, 00000, active 0
```

## Errors and notices

A failure says what went wrong in its SQLCODE, SQLSTATE and message, the
rule it cites, where in the statement, and whether a transaction is
still active. With `diagnostics` its tail says more.

| Tag | Field | Holds |
|---|---|---|
| 7 | severity | ERROR, FATAL, WARNING or NOTICE |
| 8 | detail | more than the message says, as the key a constraint refused |
| 9 | hint | what may be done about it |
| 10 | context | the trigger or routine running, as `trigger S.T` |
| 11 | schema | the schema of what it is about |
| 12 | table | the table or view it is about |
| 13 | column | the column it is about |
| 14 | data type | the data type of the value it is about |
| 15 | constraint | the constraint broken |

ERROR is a statement's failure, and the session goes on. FATAL is a
failure that ends the connection. WARNING is a completion condition of
class 01, or a SIGNAL of class 01 or 02 that no handler handled. NOTICE
is a notice of the server's own.

A statement's conditions are one list, told one way. With `conditions`
they come in its Complete, tag 17. Otherwise, with `notices`, each comes
as a Notice before the statement's last reply. Otherwise they are not
sent. A statement that fails has no Complete, so its conditions come as
Notices, with `notices`. The server's own notices always come as a
Notice.

The server defines two SQLSTATEs of its own here.

| SQLSTATE | Means |
|---|---|
| 01T01 | Reset or ChangeUser rolled back the transaction that was active; a Notice, of severity NOTICE |
| TBR01 | the replica had not applied the position of REPLICATION.READ_AFTER in time; SQLCODE -920 |

A malformed message, a request not granted, a frame too long, or a
request out of turn fails the connection with SQLCODE -919 and SQLSTATE
08006. The server sends that Error and closes.
[Appendix A](../appendix/a-sqlstate.md) lists every SQLSTATE.

## Sample frames

The file [05-protocol-frames.tsv](05-protocol-frames.tsv) beside this
page holds sample frames for a decoder to test with. The protocol's own
tests make it from the encoders the server and the client use, and fail
if the file says otherwise, so it is always what the code sends. It has
every request and reply, every data type and value tag, each tail
field, each Status this page names, Notices and Errors with and without
their conditions, compressed frames, and the edges a decoder meets,
such as an empty string, a null, the longest attribute, the most
attributes, an empty batch and a Complete that says more follow.

The file is UTF-8 text, one frame a line. A line that begins with `#`
is a comment. Every other line has five fields apart by one tab each,
and no field holds a tab.

| Field | Holds |
|---|---|
| direction | `client` or `server`, the side that sends the frame |
| name | the sample's name, unique in the file, as `value-uuid` |
| needs | the capabilities that must be granted for the frame to be sent, apart by commas, or `-` for none |
| hex | the whole frame, its five-byte header too, two upper-case hexadecimal digits to a byte |
| means | what the frame decodes to, in words |

A line of the file reads as follows, its tabs shown here as spaces.

```text
server  pong  ping  4700000000  Pong, a header alone
```

A compressed frame's sample holds the `z` frame as sent. Its meaning
names the kind and length of the frame it holds, and the frame it
inflates to is that of the sample its meaning describes.

## A worked example

A client logs in by `trust` as ANN and asks for `ping`. Its Hello is 41
bytes.

```text
48                      kind 'H'
24 00 00 00             length 36
01 00 00 00             version 1
03 00 00 00 41 4E 4E    user "ANN"
05 00 00 00 74 72 75 73 74
                        mechanism "trust"
00 00 00 00             initial, no bytes
01 00 00 00             capabilities, one
04 00 00 00 70 69 6E 67 "ping"
```

There are no attributes, so there is no tail. After Ready, a Ping is a
header alone, and so is its Pong.

```text
47 00 00 00 00          Ping 'G', length 0
47 00 00 00 00          Pong 'G', length 0
```

Next the client sends `VALUES 1` with `ends` set.

```text
51                      kind 'Q'
0D 00 00 00             length 13
08 00 00 00             sql, 8 bytes
56 41 4C 55 45 53 20 31 "VALUES 1"
01                      ends 1
```

The server answers with a Description of the one column, then a Rows
reply and a Complete. The Description's bytes follow the column layout
above. The Rows reply holds one row of the exact number 1, units 1 and
scale 0.

```text
42                      kind 'B'
1D 00 00 00             length 29
01 00 00 00             rows, one
01 00 00 00             values, one
02                      tag 2, exact number
01 00 00 00 00 00 00 00
00 00 00 00 00 00 00 00 units 1, i128
00 00 00 00             scale 0
```

The Complete says the statement succeeded and, `ends` having committed
it, that no transaction is active. With no capability that adds to it,
its tail is empty.

```text
4B                      kind 'K'
0E 00 00 00             length 14
00 00 00 00             sqlcode 0
05 00 00 00 30 30 30 30 30
                        sqlstate "00000"
00                      active 0
```

Had the client been granted `row-counts`, the same Complete would end
with tag 6 and the row count, `06 01 00 00 00 00 00 00 00`, and then tag
18 and the server's three times.
