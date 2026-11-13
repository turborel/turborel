//! Unit tests of LISTAGG's accumulator: what it holds against a whole sort,
//! each case of GR11g iii, and DISTINCT's copy kept (RULES.md R-TEST-1,
//! R-TEST-8).

use super::*;
use crate::value::Decimal;

/// MLRT as a count.
const MOST: usize = 32_767;

/// The spec of a LISTAGG of `separator` and `truncation`, DISTINCT and
/// its one key DESC as `(distinct, descending)` say.
fn spec(separator: &str, truncation: Option<Truncation>, (distinct, descending): (bool, bool)) -> Arc<ListaggSpec> {
    Arc::new(ListaggSpec {
        separator: separator.to_owned(),
        truncation,
        distinct,
        collation: None,
        descending: Arc::from(vec![descending]),
        collations: vec![None],
    })
}

/// The row of `text` and its key `key`, as LISTAGG's argument is.
fn row(text: &str, key: i128) -> Value {
    Value::Row(Arc::from(vec![Value::Char(text.to_owned()), Value::Exact(Decimal::new(key, 0))]))
}

/// LISTAGG of `spec` given `values` in turn, each with its key.
fn listagg(spec: &Arc<ListaggSpec>, values: &[(String, i128)]) -> Result<Value> {
    let mut acc = Listagg(Arc::clone(spec)).accumulator(DataType::varying(MAX_LISTAGG_LENGTH));
    for (text, key) in values {
        acc.update(&row(text, *key))?;
    }
    acc.finish()
}

/// The string `v` is.
fn text(v: Result<Value>) -> String {
    let Value::Char(s) = v.unwrap() else { panic!("not a string") };
    s
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR11a, GR11g iii];
/// [DESIGN] the values let go past MLRT change nothing: of values of
/// lengths about MLRT's thirds, given in an order apart from their keys,
/// LR is that of all of them sorted whole, by either direction, WITH and
/// WITHOUT COUNT; under MLRT, LIR.
#[test]
fn what_it_lets_go_changes_nothing() {
    let lengths = [9_000_usize, 12_000, 300, 15_000, 7, 11_000, 0, 20_000];
    let values: Vec<(String, i128)> = lengths
        .iter()
        .zip([5_i128, 2, 7, 1, 8, 3, 6, 4])
        .enumerate()
        .map(|(i, (&n, key))| (format!("{}{i}", "v".repeat(n)), key))
        .collect();
    for descending in [false, true] {
        let mut whole = values.clone();
        whole.sort_by_key(|(_, k)| if descending { -k } else { *k });
        let sorted: Vec<&str> = whole.iter().map(|(t, _)| t.as_str()).collect();
        for count in [false, true] {
            let t = Truncation { filler: "~~".to_owned(), count };
            let got = text(listagg(&spec(";", Some(t.clone()), (false, descending)), &values));
            assert_eq!(got, truncated(&sorted, sorted.len(), ";", &t), "descending {descending}, count {count}");
            assert!(got.chars().count() <= MOST);
        }
    }
    let short: Vec<(String, i128)> = values.iter().filter(|(t, _)| t.len() < 400).cloned().collect();
    let expected = format!("6,{}2,{}4", "v".repeat(300), "v".repeat(7));
    assert_eq!(
        text(listagg(&spec(",", Some(Truncation { filler: "~".to_owned(), count: true }), (false, false)), &short)),
        expected,
        "keys 6, 7 and 8"
    );
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR11g i, ii] of ON
/// OVERFLOW ERROR, values joined of exactly MLRT characters are LIR; one
/// character more is SQLSTATE 22001, raised as the value that passes it is
/// given, of no order but its length. Of TRUNCATE, values joined of
/// exactly MLRT characters are LIR too, none let go.
#[test]
fn error_is_raised_once_past_mlrt() {
    let half = 16_383_usize;
    let exact = [("a".repeat(half), 1_i128), ("b".repeat(MOST - half - 1), 2)];
    let s = spec(",", None, (false, false));
    assert_eq!(text(listagg(&s, &exact.clone().map(|(t, k)| (t, k)))).chars().count(), MOST);
    let truncating = spec(",", Some(Truncation { filler: "~".to_owned(), count: true }), (false, false));
    assert_eq!(text(listagg(&truncating, &exact.map(|(t, k)| (t, k)))).chars().count(), MOST, "of TRUNCATE too, LIR");
    let over = [("a".repeat(half), 1_i128), ("b".repeat(MOST - half), 2)];
    let mut acc = Listagg(Arc::clone(&s)).accumulator(DataType::varying(MAX_LISTAGG_LENGTH));
    acc.update(&row(&over[0].0, over[0].1)).unwrap();
    let raised = acc.update(&row(&over[1].0, over[1].1)).unwrap_err();
    assert_eq!(raised.sqlstate().as_str(), "22001");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR11g iii 2)] each
/// case of LR: WITH COUNT some LIRPi (A II), none (A I) with LIR2 or, past
/// MLRT, periods; WITHOUT COUNT some (B II), none (B I) with LTFP1 or, past
/// MLRT, periods. The count's digits narrow the room WITH COUNT leaves: a
/// value that fits WITHOUT COUNT may not WITH it.
#[test]
fn each_case_of_truncation() {
    let big = "b".repeat(MOST - 4);
    let values = [big.as_str(), "c", "d"];
    // LIRP1 is MOST - 4 long: room WITHOUT COUNT of ",..." is MOST - 4.
    assert_eq!(truncated(&values, 3, ",", &Truncation { filler: "...".into(), count: false }), format!("{big},..."));
    // WITH COUNT the room is MOST - 4 - 1 - 2: LIRP1 does not fit.
    assert_eq!(truncated(&values, 3, ",", &Truncation { filler: "...".into(), count: true }), ",...(3)");
    let small = ["x", "y", "z"];
    assert_eq!(truncated(&small, 3, ",", &Truncation { filler: "...".into(), count: true }), "x,y,...(1)");
    assert_eq!(truncated(&small, 3, ",", &Truncation { filler: "...".into(), count: false }), "x,y,...");
    let filler = "f".repeat(MOST);
    assert_eq!(truncated(&values, 3, ",", &Truncation { filler: filler.clone(), count: true }), "...(3)");
    assert_eq!(truncated(&values, 3, ",", &Truncation { filler: filler.clone(), count: false }), "...");
    let fits = "f".repeat(MOST - 1);
    assert_eq!(truncated(&values, 3, ",", &Truncation { filler: fits.clone(), count: false }), format!(",{fits}"));
    // LIR2 of MOST characters exactly is kept.
    let lir2 = "f".repeat(MOST - 4);
    assert_eq!(truncated(&values, 3, ",", &Truncation { filler: lir2.clone(), count: true }), format!(",{lir2}(3)"));
    // i < N: of one value, none is taken, however short.
    assert_eq!(truncated(&["x"], 1, ",", &Truncation { filler: "...".into(), count: false }), ",...");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR11c] under DISTINCT
/// a value given again is counted once, and its copy first in order is
/// kept, a later one taking an earlier one's place if before it; without
/// DISTINCT each is kept.
#[test]
fn distinct_keeps_the_first_copy() {
    let values = [("p".to_owned(), 3_i128), ("q".to_owned(), 2), ("p".to_owned(), 1), ("p".to_owned(), 4)];
    assert_eq!(text(listagg(&spec(",", None, (true, false)), &values)), "p,q");
    assert_eq!(text(listagg(&spec(",", None, (true, true)), &values)), "p,q");
    assert_eq!(text(listagg(&spec(",", None, (false, false)), &values)), "p,q,p,p");
    let none: [(String, i128); 0] = [];
    assert!(listagg(&spec(",", None, (true, false)), &none).unwrap().is_null());
}
