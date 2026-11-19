//! Searches of an adjacency index: equalities of its key's first columns,
//! every entry, estimates, a key's rows, and values not of a column's
//! type, against the edges held (RULES.md D34).

use super::*;
use crate::index::searched;
use crate::value::CompOp;

/// The (key, row) pairs of the model a check of `probe` selects, sorted.
fn expected(model: &Model, probe: &Probe) -> Vec<(Key, RowId)> {
    let mut all: Vec<(Key, RowId)> =
        model.0.keys().map(|&(s, d, a, row)| (edge(s, d, a), row)).filter(|(k, _)| probe.answers(&k.0)).collect();
    all.sort();
    all
}

/// What `index` finds of `probe`, sorted.
fn found(index: &Arc<AdjacencyIndex>, probe: &Probe) -> Vec<(Key, RowId)> {
    let mut all = searched(index, probe).unwrap();
    all.sort();
    all
}

/// The probes each search test asks of vertex `v`.
fn probes(v: i128) -> Vec<Probe> {
    vec![
        Probe::All,
        Probe::Compare { op: CompOp::Eq, value: n(v) },
        Probe::Prefix { equal: vec![n(v)], then: None },
        Probe::Prefix { equal: vec![n(v)], then: Some((CompOp::Eq, n(v.saturating_add(1)))) },
        Probe::Prefix { equal: vec![n(v), n(v.saturating_add(1))], then: None },
        Probe::Prefix { equal: vec![n(v), n(v.saturating_add(1)), n(1)], then: None },
    ]
}

/// Verifies: [DESIGN] RULES.md D34: of every way of keeping edges, read
/// of the tree and of the compacted adjacency, a search of every entry,
/// or of an equality of the source, the source and destination, or an
/// included column too, finds exactly the entries whose key answers it,
/// each once, with its key; an estimate of a source's equality is exact;
/// a key's rows, read whole or as taken, are its entries'.
#[test]
fn searches_find_the_entries_answering() {
    for kept in [Kept::Both, Kept::Out, Kept::In] {
        let method = method(Some(1_usize << 30_u32));
        let ix = index(&method, kept);
        let (mut model, mut state) = (Model::default(), 13_u64);
        for _ in 0_u32..1_500 {
            model.insert(&ix, random_edge(&mut state, 20, 60));
        }
        for built in [false, true] {
            if built {
                assert!(ix.compact().unwrap());
                model.insert(&ix, (3, 4, 1, 61));
            }
            for v in 0..21 {
                for probe in probes(v) {
                    assert_eq!(found(&ix, &probe), expected(&model, &probe), "{kept:?} {built} {probe:?}");
                }
                let probe = Probe::Compare { op: CompOp::Eq, value: n(v) };
                let estimate = ix.estimate(&probe);
                assert!(estimate.is_none_or(|e| e == expected(&model, &probe).len()), "{kept:?} {v}: {estimate:?}");
            }
            for (&(s, d, a, row), _) in model.0.iter().take(50) {
                let rows = ix.rows(&edge(s, d, a)).unwrap();
                let taken: Vec<RowId> = Arc::clone(&ix).key_rows(&edge(s, d, a)).collect::<Result<_>>().unwrap();
                assert!(rows.contains(&row) && rows == taken, "{kept:?} {s} {d} {a}");
            }
        }
    }
}

/// Verifies: [DESIGN] RULES.md D34: a probe of a comparison other than
/// equality, of a range or of a function is refused; one of a null finds
/// nothing [X3.135-1986 §5.11 GR2]; of a value not of the column's type,
/// the entries whose key compares equal with it, by the comparison rules:
/// 1.5 none of an INTEGER column, 2.0E0 those of 2, as do steps of them;
/// a key of the wrong type is an error.
#[test]
fn probes_of_other_values_compare_as_sql_does() {
    let method = method(None);
    let ix = index(&method, Kept::Both);
    let mut model = Model::default();
    for (s, d) in [(1, 2), (2, 3), (2, 1)] {
        model.insert(&ix, (s, d, 0, u64::try_from(s * 10 + d).unwrap()));
    }
    for refused in [
        Probe::Compare { op: CompOp::Lt, value: n(2) },
        Probe::Prefix { equal: vec![n(1)], then: Some((CompOp::Gt, n(1))) },
        Probe::Range { equal: Vec::new(), low: Box::new((CompOp::Ge, n(0))), high: Box::new((CompOp::Le, n(1))) },
    ] {
        assert!(searched(&ix, &refused).is_err(), "{refused:?}");
        assert_eq!(ix.estimate(&refused), None);
    }
    let half = Value::Exact(Decimal::new(15, 1));
    assert_eq!(found(&ix, &Probe::Compare { op: CompOp::Eq, value: half.clone() }), []);
    assert_eq!(found(&ix, &Probe::Compare { op: CompOp::Eq, value: Value::Null }), []);
    let two = Probe::Compare { op: CompOp::Eq, value: Value::Approx(2.0) };
    assert_eq!(found(&ix, &two), expected(&model, &two));
    assert_eq!(found(&ix, &two).len(), 2);
    let mut out = Vec::new();
    ix.steps(&Key(vec![Value::Approx(2.0)]), Direction::Out, &mut out).unwrap();
    ix.steps(&Key(vec![half]), Direction::Out, &mut out).unwrap();
    ix.steps(&Key(vec![Value::Null]), Direction::In, &mut out).unwrap();
    assert_eq!(out.len(), 2, "{out:?}");
    assert_eq!(ix.degree(&Key(vec![Value::Approx(2.0)]), Direction::Out).unwrap(), 2);
    assert!(ix.steps(&Key(vec![n(1), n(2)]), Direction::Out, &mut out).is_err(), "two values for one column");
    assert!(ix.rows(&Key(vec![Value::Char("x".into()), n(1), n(1)])).is_err());
}
