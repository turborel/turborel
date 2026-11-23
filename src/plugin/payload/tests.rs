//! Unit tests: the payloads of the kinds' calls, each written and read
//! back as written, and each cut short at any byte refused as a malformed
//! message received, never read as a shorter payload (RULES.md D167,
//! R-TEST-1, R-TEST-2).

use crate::bytes::Reader;
use crate::disk::buffer::{Policy, RANGES, Range, Report, Sampled};
use crate::error::{Result, SqlState};
use crate::fulltext::rank::{Corpus, InDocument, TermStats};
use crate::plugin::errors::VIOLATION;
use crate::value::collation::Pad;
use crate::value::{CompOp, Decimal, Value};

use super::index::{Near, Probed, Write, entries, near, neighbours, op_byte, op_of, probe, put_entries, put_near, put_neighbours};
use super::index::{put_probe, put_writes, writes};
use super::replication::consensus::{Input, Output, input, open, outputs, put_input, put_open, put_outputs};
use super::replication::{Held, Manifest, Served, members, positions, put_members, put_positions};
use super::replication::{arrived, given, held, manifest, mark, optional, put_arrived, put_given, put_held, put_manifest};
use super::replication::{put_mark, put_optional, put_served, served};
use super::{collation, engine, eviction, process, quantizer, ranking, reader, types};

/// What `read` reads of `bytes`, which must be all of them, after each
/// shorter prefix of them is refused, SQLSTATE 08006.
fn whole<T>(bytes: &[u8], read: impl Fn(&mut Reader<'_>) -> Result<T>) -> T {
    for end in 0..bytes.len() {
        let cut = read(&mut reader(&bytes[..end])).err().map(|e| e.sqlstate());
        assert_eq!(cut, Some(SqlState::CONNECTION_FAILURE), "cut at {end} of {}", bytes.len());
    }
    let mut r = reader(bytes);
    let got = read(&mut r).unwrap();
    assert!(r.at_end(), "all of it read");
    got
}

/// That `got` is `want`, as their values are shown: a value has no
/// equality of its own but SQL's.
fn shown<T: std::fmt::Debug>(got: &T, want: &T) {
    assert_eq!(format!("{got:?}"), format!("{want:?}"));
}

/// Bytes `write` writes.
fn written(write: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut out = Vec::new();
    write(&mut out);
    out
}

/// A key of two values.
fn key(n: i128) -> Vec<Value> {
    vec![Value::Exact(Decimal::new(n, 0)), Value::Char(format!("k{n}").into())]
}

/// Verifies: [DESIGN] RULES.md D226, D227: each operator, each probe of
/// SEARCH, WRITE's writes, entries, NEAREST's question and its answer are
/// read back as written, and cut short refused; an operator or a probe of
/// no byte is refused as malformed.
#[test]
fn index_payloads_are_read_as_written() {
    for op in [CompOp::Eq, CompOp::Ne, CompOp::Lt, CompOp::Gt, CompOp::Le, CompOp::Ge] {
        assert_eq!(whole(&[op_byte(op)], op_of), op);
    }
    assert_eq!(op_of(&mut reader(&[6])).unwrap_err().sqlstate(), SqlState::CONNECTION_FAILURE);
    let one = Value::Exact(Decimal::new(1, 0));
    let probes = [
        Probed::All,
        Probed::Compare(CompOp::Gt, one.clone()),
        Probed::Prefix(key(1), None),
        Probed::Prefix(key(2), Some((CompOp::Le, one.clone()))),
        Probed::Range(key(3), (CompOp::Ge, one.clone()), (CompOp::Lt, Value::Null)),
    ];
    for p in &probes {
        let bytes = written(|out| put_probe(out, p).unwrap());
        let back = whole(&bytes, probe);
        assert_eq!(written(|out| put_probe(out, &back).unwrap()), bytes, "{p:?}");
    }
    assert_eq!(probe(&mut reader(&[4])).unwrap_err().sqlstate(), SqlState::CONNECTION_FAILURE);
    let all = [Write { insert: true, id: 7, key: key(7) }, Write { insert: false, id: 8, key: key(8) }];
    let (locator, back) = whole(&written(|out| put_writes(out, b"loc", &all).unwrap()), writes);
    assert_eq!((locator.as_slice(), back.len(), back[1].insert, back[1].id), (&b"loc"[..], 2, false, 8));
    let rows = vec![(1, key(1)), (2, key(2))];
    shown(&whole(&written(|out| put_entries(out, &rows).unwrap()), entries), &rows);
    let asked = Near { column: 1, value: one, metric: "TAXICAB".into(), first: 3, descending: true };
    let (_, n) = whole(&written(|out| put_near(out, b"n", &asked).unwrap()), near);
    assert_eq!((n.column, n.metric.as_str(), n.first, n.descending), (1, "TAXICAB", 3, true));
    let nearest = vec![(0.5_f64, 1, key(1)), (1.5_f64, 2, key(2))];
    shown(&whole(&written(|out| put_neighbours(out, &nearest).unwrap()), neighbours), &nearest);
}

/// Verifies: [DESIGN] RULES.md D224, D225: OPEN's answer, rows maybe found,
/// identified rows, SCAN's and FETCH's questions, rows of a fetch, and
/// the changes of a write are read back as written, and cut short refused.
#[test]
fn engine_payloads_are_read_as_written() {
    let o = engine::Opened { found: true, next: 9, rows: 8, durable: 7 };
    assert_eq!(whole(&written(|out| engine::put_opened(out, &o)), engine::opened), o);
    let row = key(1);
    shown(&whole(&written(|out| engine::put_maybe(out, Some(&row)).unwrap()), engine::maybe), &Some(row.clone()));
    assert!(whole(&written(|out| engine::put_maybe(out, None).unwrap()), engine::maybe).is_none());
    let rows = vec![(3, key(3)), (4, key(4))];
    shown(&whole(&written(|out| engine::put_identified(out, &rows).unwrap()), engine::identified), &rows);
    assert_eq!(whole(&written(|out| engine::put_scan(out, b"t", 5, 100)), engine::scan), (b"t".to_vec(), 5, 100));
    assert_eq!(whole(&written(|out| engine::put_ids(out, b"t", &[1, 2])), engine::ids), (b"t".to_vec(), vec![1, 2]));
    let maybes = vec![Some(key(5)), None];
    shown(&whole(&written(|out| engine::put_maybes(out, &maybes).unwrap()), engine::maybes), &maybes);
    let changes = vec![(6, Some(key(6))), (7, None)];
    shown(&whole(&written(|out| engine::put_changes(out, b"t", &changes).unwrap()), engine::changes), &(b"t".to_vec(), changes));
}

/// Verifies: [DESIGN] RULES.md D283, D284: a buffer pool's report and a
/// policy pushed, or none, are read back as written, and cut short
/// refused; a policy of a HISTORY not 1 or 2, of more than `RANGES`
/// ranges, or of a range ending before it begins is refused, SQLSTATE
/// 38T03.
#[test]
fn eviction_payloads_are_read_as_written() {
    let sampled = vec![Sampled { file: 1, page: 2, uses: 3, age: 4, weight: 5 }];
    let report = Report { interval: 10, frames: 64, evicted: vec![(1, 9), (2, 8)], sampled };
    assert_eq!(whole(&written(|out| eviction::put_report(out, &report)), eviction::report), report);
    let range = Range { file: 1, first: 0, last: 10, weight: 3 };
    let policy = Policy { history: 2, recency: 1, frequency: 2, once: 3, sample: 4, ranges: vec![range] };
    assert_eq!(whole(&written(|out| eviction::put_answer(out, Some(&policy))), eviction::answer), Some(policy.clone()));
    assert_eq!(whole(&written(|out| eviction::put_answer(out, None)), eviction::answer), None);
    let backwards = Range { first: 10, last: 0, ..range };
    let too_many = vec![range; RANGES + 1];
    for bad in [
        Policy { history: 3, ..policy.clone() },
        Policy { ranges: too_many, ..policy.clone() },
        Policy { ranges: vec![backwards], ..policy },
    ] {
        let bytes = written(|out| eviction::put_answer(out, Some(&bad)));
        assert_eq!(eviction::answer(&mut reader(&bytes)).unwrap_err().sqlstate(), VIOLATION);
    }
}

/// Verifies: [DESIGN] RULES.md D221: a ranking's corpus, a term's
/// statistics, WEIGHTS' and BOUNDS' questions are read back as written,
/// and cut short refused.
#[test]
fn ranking_payloads_are_read_as_written() {
    let corpus = Corpus { documents: 100, average_length: 12.5, k1: 1.2, b: 0.75 };
    let term = TermStats { documents: 7, most: 3 };
    assert_eq!(whole(&written(|out| ranking::put_corpus(out, &corpus)), ranking::corpus), corpus);
    assert_eq!(whole(&written(|out| ranking::put_term(out, &term)), ranking::term), term);
    let asked = vec![(term, InDocument { frequency: 2, length: 30 })];
    assert_eq!(whole(&written(|out| ranking::put_weights(out, &corpus, &asked)), ranking::weights), (corpus, asked));
    assert_eq!(whole(&written(|out| ranking::put_bounds(out, &corpus, &[term])), ranking::bounds), (corpus, vec![term]));
}

/// Verifies: [DESIGN] RULES.md D167, D221, D222, D223: a quantizer's training and what it
/// trained, a type's values made, texts and binaries, a collation's keys
/// asked and answered, a verifier asked and the events a process tells
/// are each read back as written, and cut short refused.
#[test]
fn other_payloads_are_read_as_written() {
    let training = quantizer::Training { dims: 2, code_bytes: 1, seed: 42, sample: vec![vec![1.0, 2.0], vec![3.0, 4.0]] };
    assert_eq!(whole(&written(|out| quantizer::put_training(out, &training)), quantizer::training), training);
    let trained = quantizer::Trained { k: 2, widths: vec![1, 1], centroids: vec![vec![0.5, 1.5], vec![2.5, 3.5]] };
    assert_eq!(whole(&written(|out| quantizer::put_trained(out, &trained)), quantizer::trained), trained);
    let made = types::Made { key: vec![1], binary: vec![2, 3], text: "t".into() };
    assert_eq!(whole(&written(|out| types::put_made(out, &made)), types::made), made);
    assert_eq!(whole(&written(|out| types::put_texts(out, &["a", "b"])), types::texts), ["a", "b"]);
    assert_eq!(whole(&written(|out| types::put_binaries(out, &[&[1][..], &[2, 3][..]])), types::binaries), [vec![1], vec![2, 3]]);
    assert_eq!(whole(&written(|out| types::put_answered(out, std::slice::from_ref(&made))), types::answered), [made]);
    for (pad, byte) in [(Pad::Space, 0_u8), (Pad::None, 1)] {
        let bytes = written(|out| collation::put_keys(out, pad, &["x"]));
        assert_eq!(bytes[0], byte);
        assert_eq!(whole(&bytes, collation::keys).1, ["x"]);
    }
    assert_eq!(whole(&written(|out| collation::put_answered(out, &[vec![9]])), collation::answered), [vec![9]]);
    assert_eq!(whole(&written(|out| process::put_verifier(out, "U", "p")), process::verifier), ("U".to_owned(), "p".to_owned()));
    let told = vec![process::Told { level: "INFO".into(), kind: "K".into(), text: "t".into() }];
    assert_eq!(whole(&written(|out| process::put_events(out, &told, 3)), process::events), (told, 3));
}

/// Verifies: [EXTENSION] RULES.md D270, D271, D272: a journal's held
/// state, an export's manifest, members, positions, frames arrived, bytes
/// maybe given, a mark and SERVE's outcomes, each cut short at any byte,
/// are refused as a malformed message received, and whole are read back
/// as written.
#[test]
fn replication_payloads_cut_short_are_refused() {
    let members_of = vec![(1, "a:1".to_owned()), (2, "b:2".to_owned())];
    let h =
        Held { durable: true, state: Some(vec![9]), first: 4, records: vec![vec![1], vec![2, 3]], marks: vec![(5, 2, members_of.clone())] };
    assert_eq!(whole(&written(|out| put_held(out, &h)), held), h);
    let m = Manifest { image: vec![7], files: vec![("f".to_owned(), 3, "d".to_owned())] };
    assert_eq!(whole(&written(|out| put_manifest(out, &m)), manifest).files, m.files);
    assert_eq!(whole(&written(|out| put_members(out, &[1, 2])), members), [1, 2]);
    assert_eq!(whole(&written(|out| put_positions(out, &[3, 4])), positions), [3, 4]);
    assert_eq!(whole(&written(|out| put_arrived(out, &[(2, vec![4, 5])])), arrived), [(2, vec![4, 5])]);
    assert_eq!(whole(&written(|out| put_optional(out, Some(&[1]))), optional), Some(vec![1]));
    assert_eq!(whole(&written(|out| put_mark(out, 6, &members_of)), mark), (6, members_of.clone()));
    assert_eq!(whole(&written(|out| put_given(out, Some((1, 8, &[6])))), given), Some((1, 8, vec![6])));
    for s in [Served::Nothing, Served::Answer(vec![1]), Served::Need(1, 2, 3)] {
        assert_eq!(whole(&written(|out| put_served(out, &s)), served), s);
    }
}

/// Verifies: [EXTENSION] RULES.md D273: a consensus's OPEN, each of its
/// inputs and each of its outputs, cut short at any byte, are refused as
/// a malformed message received, and whole are read back as written.
#[test]
fn consensus_payloads_cut_short_are_refused() {
    let members_of = vec![(1, "a:1".to_owned()), (2, "b:2".to_owned())];
    let h = Held { durable: false, state: None, first: 1, records: vec![vec![1]], marks: Vec::new() };
    let opened = written(|out| put_open(out, (3, &members_of), Some(&h), 77));
    assert_eq!(whole(&opened, open), (3, members_of.clone(), Some(h), 77));
    let inputs = [
        Input::Append(vec![1]),
        Input::Received(2, vec![3]),
        Input::Persisted(4),
        Input::Applied(5),
        Input::Installed(6, vec![vec![7]]),
        Input::Retain(8),
        Input::Reconfigure(vec![9], members_of.clone()),
        Input::Opened(10),
    ];
    for i in &inputs {
        assert_eq!(&whole(&written(|out| put_input(out, i)), input), i);
    }
    let all = [
        Output::Assigned(1),
        Output::Persist(2, vec![3]),
        Output::Truncate(4),
        Output::Retain(5),
        Output::PersistState(vec![6]),
        Output::Send(7, vec![8]),
        Output::Committed(9),
        Output::Deliver(10),
        Output::Outcome(11, Some(true)),
        Output::Role(Some(14)),
        Output::FetchState(15),
        Output::Mark(16, 17, members_of.clone()),
        Output::Reach(members_of.clone()),
        Output::Epoch(18, members_of),
    ];
    for o in all {
        assert_eq!(whole(&written(|out| put_outputs(out, std::slice::from_ref(&o))), outputs), [o]);
    }
}
