//! The network benchmark's workloads, each over every transport.

use std::time::{Duration, Instant};

use turborel::value::{Decimal, Value};
use turborel_client::connection::Outcome;
use turborel_protocol::Request;

use crate::harness::{Bench, Case, note};
use crate::raw::{Ended, Raw};
use crate::served::{BIG_ROWS, Kind, Served};
use crate::with;

/// Round trips sampled in each run of the latency case.
const ROUND_TRIPS: usize = 5_000;
/// Small queries each run of the throughput cases sends.
const SMALL: usize = 20_000;
/// Requests sent before their replies are read, pipelined.
const DEPTH: usize = 64;
/// Rows each run of the insert cases inserts.
const INSERTS: usize = 20_000;
/// Connections made and ended in each run of the setup cases.
const SETUPS: usize = 300;
/// Connections at once in the concurrent case.
const CONCURRENT: usize = 256;
/// Queries each of them runs, each run.
const EACH: usize = 200;

/// The trivial query.
const TRIVIAL: &str = "SELECT K FROM ONE";

/// The trivial query, as a request that is its own transaction.
fn trivial() -> Request {
    Request::Query { sql: TRIVIAL.into(), ends: true, attributes: Vec::new(), multi: false }
}

/// The round trip of the trivial query, through the client library.
pub fn latency(bench: &Bench, served: &Served) {
    for kind in Kind::ALL {
        let mut any = served.connect(kind);
        with!(&mut any, c => c.set_autocommit(true));
        bench.measure_sampled(Case::new(format!("round trip {}", kind.name()), "query"), || {
            let mut samples = Vec::with_capacity(ROUND_TRIPS);
            for _ in 0..ROUND_TRIPS {
                let began = Instant::now();
                let outcome = with!(&mut any, c => c.query(TRIVIAL)).unwrap();
                samples.push(began.elapsed());
                assert!(matches!(outcome, Outcome::Rows { .. }));
            }
            samples
        });
    }
}

/// `count` requests `request` makes, sent `depth` at a time before their
/// replies are read: how many were answered.
fn pipelined(raw: &mut Raw, count: usize, depth: usize, request: &dyn Fn(usize) -> Request) -> u64 {
    let mut n = 0;
    while n < count {
        let batch = depth.min(count - n);
        for i in 0..batch {
            raw.queue(&request(n + i));
        }
        raw.flush();
        for _ in 0..batch {
            assert_eq!(raw.answered(), Ended::Complete);
        }
        n += batch;
    }
    count as u64
}

/// Many small queries, one at a time and pipelined.
pub fn small(bench: &Bench, served: &Served) {
    for kind in Kind::ALL {
        let mut raw = Raw::connect(served, kind);
        for (depth, name) in [(1, "serial"), (DEPTH, "pipelined")] {
            bench.measure(Case::new(format!("small queries {name} {}", kind.name()), "query"), || {
                pipelined(&mut raw, SMALL, depth, &|_| trivial())
            });
        }
        raw.terminate();
    }
}

/// A large result streamed: rows a second, and its bytes.
pub fn streaming(bench: &Bench, served: &Served) {
    for kind in Kind::ALL {
        let mut raw = Raw::connect(served, kind);
        raw.queue(&Request::Query { sql: "SELECT K, V FROM BIG".into(), ends: true, attributes: Vec::new(), multi: false });
        raw.flush();
        assert_eq!(raw.answered(), Ended::Complete);
        let per_row = raw.bytes as usize / BIG_ROWS;
        bench.measure(Case::new(format!("stream {BIG_ROWS} rows {}", kind.name()), "row").bytes(per_row), || {
            raw.rows = 0;
            raw.queue(&Request::Query { sql: "SELECT K, V FROM BIG".into(), ends: true, attributes: Vec::new(), multi: false });
            raw.flush();
            assert_eq!(raw.answered(), Ended::Complete);
            raw.rows
        });
        raw.terminate();
    }
}

/// The values of the `n`th row inserted.
fn row(n: usize) -> Vec<Value> {
    vec![Value::Exact(Decimal::new(n as i128, 0)), Value::Char(format!("row {n} of the bulk insert").into())]
}

/// Bulk inserts through a prepared statement, in one transaction: each
/// row a request answered before the next, through the client library;
/// and pipelined, by hand.
pub fn inserts(bench: &Bench, served: &Served) {
    for kind in Kind::ALL {
        let any = std::cell::RefCell::new(served.connect(kind));
        let emptied = || with!(&mut *any.borrow_mut(), c => c.query("DELETE FROM B").and_then(|_| c.query("COMMIT"))).unwrap();
        let statement = with!(&mut *any.borrow_mut(), c => c.prepare("INSERT INTO B VALUES (?, ?)")).unwrap();
        bench.measure_fresh(Case::new(format!("insert prepared serial {}", kind.name()), "row"), emptied, |_| {
            let mut any = any.borrow_mut();
            for n in 0..INSERTS {
                with!(&mut *any, c => c.execute(&statement, &row(n))).unwrap();
            }
            with!(&mut *any, c => c.query("COMMIT")).unwrap();
            INSERTS as u64
        });
        let mut raw = Raw::connect(served, kind);
        raw.queue(&Request::Prepare { statement: 1, sql: "INSERT INTO B VALUES (?, ?)".into() });
        raw.flush();
        assert_eq!(raw.answered(), Ended::Prepared);
        let execute = |n: usize| Request::Execute { statement: 1, values: row(n), cursor: 0, rows: 0, ends: false, attributes: Vec::new() };
        bench.measure_fresh(Case::new(format!("insert prepared pipelined {}", kind.name()), "row"), emptied, |_| {
            pipelined(&mut raw, INSERTS, DEPTH, &execute);
            pipelined(&mut raw, 1, 1, &|_| Request::Query { sql: "COMMIT".into(), ends: false, attributes: Vec::new(), multi: false });
            INSERTS as u64
        });
        raw.terminate();
        with!(any.into_inner(), c => c.terminate()).unwrap();
    }
}

/// Connections made, logged in and ended: trusted over each transport,
/// and by SCRAM-SHA-256 over TCP and TLS.
pub fn setup(bench: &Bench, served: &Served) {
    let cases: [(Kind, bool); 5] = [(Kind::Tcp, false), (Kind::Unix, false), (Kind::Tls, false), (Kind::Tcp, true), (Kind::Tls, true)];
    for (kind, scram) in cases {
        let how = if scram { "scram" } else { "trust" };
        bench.measure_sampled(Case::new(format!("connect {how} {}", kind.name()), "connection"), || {
            let mut samples = Vec::with_capacity(SETUPS);
            for _ in 0..SETUPS {
                let began = Instant::now();
                let any = if scram { served.connect_scram(kind) } else { served.connect(kind) };
                samples.push(began.elapsed());
                with!(any, c => c.terminate()).unwrap();
            }
            samples
        });
    }
}

/// Hundreds of connections at once, each running small queries, one at a
/// time.
pub fn concurrent(bench: &Bench, served: &Served) {
    for kind in [Kind::Tcp, Kind::Tls] {
        let case = Case::new(format!("concurrent small queries {}", kind.name()), "query").threads(CONCURRENT);
        let mut slowest = Duration::ZERO;
        bench.measure_fresh(
            case,
            || (0..CONCURRENT).map(|_| Raw::connect(served, kind)).collect::<Vec<_>>(),
            |raws| {
                let took = at_once(raws);
                slowest = slowest.max(took.iter().copied().max().unwrap_or_default());
                (CONCURRENT * EACH) as u64
            },
        );
        note(&format!("slowest connection's {EACH} queries took {:.1} ms at most", slowest.as_secs_f64() * 1e3_f64));
    }
}

/// Runs `EACH` small queries on each of `raws` at once, each on a thread
/// of its own: how long each took.
fn at_once(raws: Vec<Raw>) -> Vec<Duration> {
    let barrier = std::sync::Barrier::new(raws.len());
    let one = |mut raw: Raw| {
        barrier.wait();
        let began = Instant::now();
        pipelined(&mut raw, EACH, 1, &|_| trivial());
        let took = began.elapsed();
        raw.terminate();
        took
    };
    std::thread::scope(|s| {
        let handles: Vec<_> = raws.into_iter().map(|raw| s.spawn(|| one(raw))).collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    })
}
