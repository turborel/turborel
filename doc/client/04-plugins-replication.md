# Replication and Archive Plug-ins

[Contents](README.md)

A node's replication is made of four parts: the transport that carries
its group's frames, the journal that keeps its records durable, the
state transfer that brings a replica far behind up to date, and the
consensus, the protocol itself. Each may be served by a host plug-in in
place of the built-in one, and so may a backup's archive. The built-in
parts stay: TCP, the journal in the database's files, the differences of
pages, and Viewstamped Replication serve wherever no plug-in is declared.

These are host plug-ins, not extensions. They are declared by the host,
started before any database opens, and never installed by SQL, because
a replica's parts and a restore's archive are needed before there is a
catalog to install anything in.

## Declaring them

A host plug-in's manifest names each object it serves with HOST and the
words of its kind.

```
HOST REPLICA TRANSPORT <identifier>
HOST JOURNAL <identifier>
HOST STATE TRANSFER <identifier>
HOST CONSENSUS <identifier>
HOST ARCHIVE <identifier>
```

The example plug-in's manifest, as the tests write it, serves a
transport, a journal and a state transfer.

```
NAME REPL; VERSION '1.0.0'; PROTOCOL 1; EXECUTABLE 'plugin_example';
HOST REPLICA TRANSPORT LOOP;
HOST JOURNAL KEPT;
HOST STATE TRANSFER SHIPPED;
```

The host declares the plug-in in PLUGINS.HOST, as `NAME=directory`, the
directory under one of PLUGINS.DIRECTORIES. A node takes at most one
object of each replication kind, and each object keeps a process of its
own for as long as the replica runs, so PLUGINS.PROCESSES must be at
least the number of objects the plug-in serves. An archive's name is
the scheme of the locations it serves, case aside, so `HOST ARCHIVE
vault` serves `vault:...`. As every host plug-in, they are started by
the host before it opens a database (`Database::start_host_plugins`),
and a restore starts them itself.

## Failing stop

Every replication plug-in is fail-stop. Any failure of a call stops the
part, the replica's driver halts, and the node stops: every statement
after fails with SQLSTATE 38T11, and the database recovers from its
files and journal as it next starts. A failure is an ERROR answered, a
process that ended, a call that timed out, or an answer too long or not
of the protocol. A replica that went on after a part that may have half
acted could diverge from its group, and one stopped cannot. The
deterministic simulator kills each kind of plug-in at moments its seed
chooses, before and after the call was carried out, and checks that no
replica diverges or acknowledges a record that was not durable.

The driver calls its parts under its own lock, as it calls the built-in
ones. A host call runs no SQL, so it cannot re-enter the database, and
PLUGINS.CALL_TIMEOUT_MS bounds how long the lock is held.

The archive is the exception. A program's own ERROR fails only the call,
so a backup, an archive round or a restore fails as it would with a
built-in target out of reach. A process that fails still stops the node.

## Replica transports

A transport links a replica to its group's members and carries frames
between them.

```text
TRANSPORT    1 open         me u32 | configuration              -> members
             2 send         to u32 | frame bytes                -> sent u8 (0 to no member)
             3 receive      (nothing)                           -> count u32 | (from u32 | frame bytes)...
             4 reconfigure  configuration                       -> members
configuration: count u32 | (id u32 | address str)...
members: count u32 | u32...
```

A frame sent costs one call. The frames arrived are taken by one
RECEIVE at the start of each turn of the driver, so a transport that
receives nothing still costs a call a turn. The members are kept as
OPEN and RECONFIGURE answered them, and asking for them costs nothing.
A frame is a field of a whole message of the protocol, which the host
reads whole or refuses, so a program that ends while writing one gives
no part of a frame. A frame the program loses is as one the network
loses, which the consensus recovers.

## Journals

A journal keeps a replica's records, the marks of its reconfigurations
and the consensus's own state.

```text
JOURNAL      1 open         (nothing)                           -> held
             2 persist      at u64 | record bytes               -> (nothing)
             3 truncate     after u64                           -> (nothing)
             4 retain       through u64                         -> (nothing)
             5 mark         at u64 | mark                       -> (nothing)
             6 state        state bytes                         -> (nothing), durable with every write before
             7 sync         (nothing)                           -> positions
             8 flush        (nothing)                           -> positions, every write durable
mark: epoch u64 | configuration
held: durable u8 | state (u8 0, or 1 and bytes) | first u64 | count u32 | record bytes... | count u32 | (at u64 | mark)...
positions: count u32 | u64...
```

Each write costs one call. A record is acknowledged only once SYNC or
FLUSH names it. They answer the positions made durable since either last
answered, each once, in the order persisted, those truncated since among
them. The host keeps the positions persisted and not yet named, and a
position named out of that order, or never given, stops the node. A
program that ends while appending acknowledges nothing more, and the
journal opened again must hold only what it named.

## State transfers

A state transfer's program makes and reads the transfer's frames, and
the host keeps what is served and what is fetched.

```text
TRANSFER     1 begin        from u32                            -> fetch u64 | request
             2 take         fetch u64 | frame bytes             -> done u8 | request
             3 fetched      fetch u64                           -> manifest
             4 chunk        fetch u64 | file u32 | at u64 | max u32   -> bytes | digest u64
             5 serve        to u32 | export u64 | frame bytes | given -> served
             6 export       export u64 | manifest               -> (nothing)
request: u8 0, or 1 and frame bytes
manifest: image bytes | count u32 | (within str | length u64 | owner str)...
given: u8 0, or 1 and file u32 | at u64 | bytes
served: u8 0 (no answer), 1 and answer bytes, or 2 and file u32 | at u64 | length u32 (a piece wanted)
```

Serving, the database's export is told once (EXPORT), and each request
answered by SERVE, which may ask for a piece of one of the export's
files, given back with the request again, at most 64 pieces an answer.
A request served costs one call and one more a piece. Fetching, the
requests BEGIN and TAKE give are sent, and the answers taken, until TAKE
says the fetch is done. Its files are then copied out by CHUNK, one call
a chunk. Each chunk carries a digest, XXH3-64 of its bytes, which the
host checks before it writes the chunk to
the database's `transfer/` directory. Nothing is installed until every
file is staged and made durable, and a program that ends mid-chunk stops
the node. The replica started again clears `transfer/` and fetches
anew, so nothing half fetched is ever installed.

The example's SHIPPED keeps each fetch in a directory of its own,
`shipped/` of its working directory, cleared as the fetch begins. Its
request names a file and an offset, or asks for the manifest, and its
answer carries the manifest or the piece, which it appends to its file.
CHUNK reads the files back, 64 KiB at most a call, so a file of 200,000
bytes is staged in four chunks. The tests kill it after the first chunk
of a file and check that the node stopped, and that the node started
again clears what was staged and fetches every file whole.

## Consensus

A consensus is a deterministic machine of inputs and outputs, so it can
cross a pipe. One STEP gives every output of an input in one answer, and
the driver never acts on part of a step.

```text
CONSENSUS    1 open         me u32 | configuration | held (u8 0, or 1 and held) | nonce u64   -> (nothing)
             2 step         input                               -> count u32 | output...
             3 describe     (nothing)                           -> str
input:  1 append record | 2 received from u32, frame | 3 tick | 4 persisted u64 | 5 applied u64
        | 6 installed holds u64, records | 7 retain u64 | 8 reconfigure record, configuration | 9 opened u64
output: 1 assigned u64 | 2 persist at u64, record | 3 truncate u64 | 4 retain u64 | 5 state bytes
        | 6 send to u32, frame | 7 committed u64 | 8 deliver u64 | 9 outcome at u64, kept u8 (0 no, 1 yes, 2 unknown)
        | 10 role u8 (0 backup, 1 primary) and a primary's view u64 | 11 fetch from u32 | 12 mark at u64, mark
        | 13 reach configuration | 14 epoch u64, configuration | 15 retire | 16 event
event:  1 view changed view u64, primary u32 | 2 frame refused from u32, why str | 3 append refused
        | 4 recovering | 5 recovered view u64, from u32 | 6 reconfigured epoch u64, configuration | 7 removed
```

Each tag stands for one of the replication crate's `Input`, `Output` or
`ReplicaEvent` variants. OPEN gives a replica made again what its journal held and its
recovery's nonce. Each input costs one call. A step that fails gives no
outputs and the node stops before it acts on anything. The consensus
made again is opened from the journal, which holds everything it made
durable, so one killed while it changes views recovers as a replica
that crashed does.

The example plug-in serves no consensus, as one would need a whole
protocol of its own. The simulator's stand-ins serve one through the
real bridges, killed at moments each seed chooses and in the middle of
view changes, and the group stays safe and converges.

## Archives

An archive serves the backup targets of one scheme, with the calls a
target of an extension has ([Plug-ins](04-plugins.md)), its kind's code
36 in place of 8. An archive is an object of its own, apart from a
kind 8 target of the same name that the same program serves, and each
keeps its own stores and uploads. A restore starts the host plug-ins before it does
anything, so a location of an archive's scheme is reached with no
database open. A target the host registered serves its scheme first,
then a host plug-in's archive, then an extension's target.

## Serving them from the SDK

The SDK gives a trait for each kind, `ReplicaTransport`, `Journal`,
`StateTransfer` and `Consensus`, and an archive is a `Target`. Each is
registered by a method of `Plugin` naming it. The SDK computes each
chunk's digest itself.

```rust
use turborel_plugin::{Answer, Member, Plugin, ReplicaTransport, serve};

#[derive(Default)]
struct Loop(Vec<u32>, Vec<(u32, Vec<u8>)>);

impl ReplicaTransport for Loop {
    fn open(&mut self, _me: u32, configuration: &[Member]) -> Answer<Vec<u32>> {
        self.0 = configuration.iter().map(|m| m.0).collect();
        Ok(self.0.clone())
    }
    fn send(&mut self, to: u32, frame: &[u8]) -> Answer<bool> {
        self.1.push((to, frame.to_vec()));
        Ok(self.0.contains(&to))
    }
    fn receive(&mut self) -> Answer<Vec<(u32, Vec<u8>)>> {
        Ok(std::mem::take(&mut self.1))
    }
    fn reconfigure(&mut self, configuration: &[Member]) -> Answer<Vec<u32>> {
        self.open(0, configuration)
    }
}

fn main() {
    let _ = serve(Plugin::new("1.0.0").replica_transport("LOOP", Loop::default()));
}
```
